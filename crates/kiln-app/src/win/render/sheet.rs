//! The options dialog (spec 2.2), drawn as a ContentDialog over smoke:
//! its sub-line and the notes, Quality as radio buttons or the slider, the
//! estimate card, More options with its drop-downs and spin boxes, and
//! Convert and Cancel in the band; an open drop-down's list over it all.

use super::*;
use crate::controls::{ButtonKind, Lit};
use crate::layout::{kit, Rect};
use crate::theme::fluent::{glyph, radius, FluentPalette};
use crate::win::text::{Ramp, Style};

/// The Quality radio buttons' words, Small first.
const QUALITY: [&str; 3] = ["Small", "High", "Best"];

impl Draw<'_> {
    /// A spin box in `r`: `value`, and the chevrons that step it back and
    /// forward, in a standard control's face.
    pub(super) fn spin_box(
        &self,
        r: &Rect,
        value: &str,
        lit: Lit,
        p: &FluentPalette,
    ) -> Result<()> {
        let look = crate::controls::button_look(ButtonKind::Standard, lit, p);
        self.face(r, radius::CONTROL, &look);
        let (v, back, next) = kit::spin_parts(*r);
        self.styled(value, Style::new(Ramp::Body), &v, look.ink)?;
        let ink = if p.high_contrast || lit.disabled {
            look.ink
        } else {
            p.text_secondary
        };
        self.glyph_turned(glyph::CHEVRON_RIGHT, &back, kit::CHEVRON, 180.0, ink);
        self.glyph(glyph::CHEVRON_RIGHT, &next, kit::CHEVRON, ink);
        Ok(())
    }

    /// The dialog over the smoke that quiets the window beneath it,
    /// coming in as a prompt does and fading as it goes.
    pub(super) fn sheet(&self, w: f32, h: f32, v: &SheetView, f: &FluentPalette) -> Result<()> {
        let (opacity, scale) = v.rise;
        let opacity = opacity * v.fade;
        self.smoke(w, h, opacity, f);
        let r = &v.layout.frame.rect;
        let base = self.base().get();
        self.set_base(compose(
            &scale_about(scale, r.x + r.w / 2.0, r.y + r.h / 2.0),
            &base,
        ));
        let drawn = self.with_opacity(opacity, || self.sheet_body(v, f));
        self.set_base(base);
        drawn
    }

    fn sheet_body(&self, v: &SheetView, f: &FluentPalette) -> Result<()> {
        let l = v.layout;
        let index = |c: SheetControl| l.controls.iter().position(|(k, _)| *k == c);
        let lit = |c: SheetControl, enabled: bool| {
            let i = index(c);
            if !enabled {
                Lit::DISABLED
            } else if i.is_some() && v.pressed == i {
                Lit::PRESSED
            } else if i.is_some() && v.hover == i {
                Lit::HOVER
            } else {
                Lit::REST
            }
        };
        let focused = v.focus.and_then(|i| l.controls.get(i)).map(|(c, _)| *c);
        self.dialog_frame(&l.frame, v.title, f)?;
        let caption = Style::new(Ramp::Caption);
        let mut rings: Vec<(Rect, f32)> = vec![];
        self.clipped(&l.frame.content, 0.0, || {
            self.styled(v.sub, caption, &l.sub, f.text_secondary)?;
            for (k, note) in v.notes.iter().enumerate() {
                self.styled(note, caption, &l.note_line(k), f.text)?;
            }
            if let Some(hd) = l.heading {
                self.styled("Quality", Style::new(Ramp::BodyStrong), &hd, f.text)?;
            }
            for (i, parts) in l.radios.iter().enumerate() {
                let c = l
                    .controls
                    .iter()
                    .map(|(c, _)| *c)
                    .filter(|c| c.is_radio())
                    .nth(i);
                let on = v.selected == Some(i);
                let shown = c.map_or(Lit::REST, |c| lit(c, true));
                let name = v.radios.get(i).copied().unwrap_or(QUALITY[i.min(2)]);
                self.radio(parts, name, on, shown, f)?;
                if c.is_some() && focused == c {
                    rings.push((parts.row, radius::CONTROL));
                }
            }
            if let Some(card) = l.card {
                self.tint(&card, radius::CONTROL, f.card.mica);
                self.stroke(&card, radius::CONTROL, f.card_stroke);
                let ink = if v.error { f.critical } else { f.text };
                self.styled(v.result, Style::new(Ramp::BodyStrong), &l.result, ink)?;
            }
            if let Some(e) = l.expander {
                let deg = if v.more_open { 180.0 } else { 0.0 };
                let c = SheetControl::Advanced;
                self.expander(&e, "More options", deg, lit(c, true), f)?;
                if focused == Some(c) {
                    rings.push((e.header, radius::CONTROL));
                }
            }
            for (k, &(c, label, control)) in l.lines.iter().enumerate() {
                let Some(line) = v.lines.get(k) else {
                    continue;
                };
                let ink = if line.enabled {
                    f.text
                } else {
                    f.text_disabled
                };
                self.styled(&line.label, Style::new(Ramp::Body), &label, ink)?;
                let shown = lit(c, line.enabled);
                if line.choices.is_some() {
                    self.combo_box(&control, &line.value, shown, f)?;
                } else {
                    self.spin_box(&control, &line.value, shown, f)?;
                }
                if focused == Some(c) {
                    rings.push((control, radius::CONTROL));
                }
            }
            Ok(())
        })?;
        // The rings after the content's clip closes: a full-width control's
        // ring has its sides.
        self.dialog_rings(&l.frame.content, &rings, f)?;
        // Content taller than the window lets it be scrolls; its indicator
        // says so, at the dialog's right edge.
        if l.scroll_max > 0.0 {
            let c = &l.frame.content;
            let track = Rect::new(c.x, c.y, l.frame.rect.right() - 4.0 - c.x, c.h);
            self.scroll_indicator(&track, c.h, c.h + l.scroll_max, l.scroll, 0.0, 1.0, f);
        }
        let b = &l.frame.buttons;
        let convert = SheetControl::Convert;
        self.fl_button(
            ButtonKind::Accent,
            &b[0],
            "Convert",
            None,
            lit(convert, v.convert_enabled),
            f,
        )?;
        let cancel = SheetControl::Cancel;
        self.fl_button(
            ButtonKind::Standard,
            &b[1],
            "Cancel",
            None,
            lit(cancel, true),
            f,
        )?;
        match focused {
            Some(SheetControl::Convert) => self.focus_visual(&b[0], radius::CONTROL, f),
            Some(SheetControl::Cancel) => self.focus_visual(&b[1], radius::CONTROL, f),
            _ => {}
        }
        // An open drop-down's list, over everything in the dialog.
        if let Some((c, state, list)) = &v.list {
            let k = l.lines.iter().position(|(m, ..)| m == c);
            if let Some(choices) = k.and_then(|k| v.lines.get(k)?.choices.as_ref()) {
                let refs: Vec<&str> = choices.iter().map(String::as_str).collect();
                self.combo_list(list, &refs, &[], state.value, state.highlight, f)?;
            }
        }
        Ok(())
    }
}
