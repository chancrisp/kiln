//! The Make smaller and Resize dialogs, drawn as a ContentDialog over
//! smoke: the sub-line, the choices, Custom's box, the estimate card, More
//! options, and the accent button and Cancel; an open ComboBox's list over
//! it all.

use super::*;
use crate::controls::{ButtonKind, Lit};
use crate::layout::Rect;
use crate::theme::fluent::{radius, FluentPalette};
use crate::win::surfaces::{ToolPart, ToolStop, ToolView};
use crate::win::text::{Ramp, Style};

/// What an empty Custom size box shows.
const CUSTOM_HINT: &str = "e.g. 5";

impl Draw<'_> {
    /// The dialog over the smoke that quiets the window beneath it,
    /// coming in as a prompt does.
    pub(super) fn tool_dialog(
        &self,
        w: f32,
        h: f32,
        v: &ToolView,
        f: &FluentPalette,
    ) -> Result<()> {
        let (opacity, scale) = v.rise;
        self.smoke(w, h, opacity, f);
        let r = &v.layout.frame.rect;
        let base = self.base().get();
        self.set_base(compose(
            &scale_about(scale, r.x + r.w / 2.0, r.y + r.h / 2.0),
            &base,
        ));
        let drawn = self.with_opacity(opacity, || self.tool_body(v, f));
        self.set_base(base);
        drawn
    }

    fn tool_body(&self, v: &ToolView, f: &FluentPalette) -> Result<()> {
        let l = &v.layout;
        let ui = v.ui;
        let d = ui.model();
        let lit = |p: ToolPart| {
            if ui.part_disabled(p) {
                Lit::DISABLED
            } else if v.pressed == Some(p) {
                Lit::PRESSED
            } else if v.hover == Some(p) {
                Lit::HOVER
            } else {
                Lit::REST
            }
        };
        self.dialog_frame(&l.frame, d.title(), f)?;
        let caption = Style::new(Ramp::Caption);
        let mut rings: Vec<(Rect, f32)> = vec![];
        self.clipped(&l.frame.content, 0.0, || {
            self.styled(&d.sub_line(), caption, &l.sub, f.text_secondary)?;
            let (heading, strong) = d.heading();
            let style = Style::new(if strong { Ramp::BodyStrong } else { Ramp::Body });
            self.styled(heading, style, &l.heading, f.text)?;
            let labels = d.labels();
            for (i, parts) in l.radios.iter().enumerate() {
                let on = d.radios().selected == Some(i);
                self.radio(parts, labels[i], on, lit(ToolPart::Radio(i)), f)?;
                if let Some(tail) = d.tail(i) {
                    // A choice that would do nothing says so in the
                    // disabled colour too, so choice and hint read as one
                    let tone = if ui.part_disabled(ToolPart::Radio(i)) {
                        f.text_disabled
                    } else {
                        f.text_secondary
                    };
                    self.styled(tail, caption, &l.tails[i], tone)?;
                }
            }
            if ui.focus() == ToolStop::Radios {
                if let Some(p) = l.radios.get(d.radios().focus) {
                    rings.push((p.row, radius::CONTROL));
                }
            }
            if let Some((b, unit)) = l.custom {
                let focused = ui.focus() == ToolStop::Custom;
                self.text_box(
                    &b,
                    d.custom(),
                    Some(CUSTOM_HINT),
                    focused,
                    lit(ToolPart::Custom),
                    f,
                )?;
                self.styled(d.unit(), Style::new(Ramp::Body), &unit, f.text_secondary)?;
            }
            if let (Some(hint), Some(r)) = (d.hint(), l.hint) {
                self.styled(hint, caption, &r, f.text_secondary)?;
            }
            if let (Some((bold, detail)), Some((card, br, dr))) = (&v.estimate, l.card) {
                self.tint(&card, radius::CONTROL, f.card.mica);
                self.stroke(&card, radius::CONTROL, f.card_stroke);
                self.styled(bold, Style::new(Ramp::BodyStrong), &br, f.text)?;
                if !detail.is_empty() {
                    self.styled(detail, caption.wrapping(), &dr, f.text_secondary)?;
                }
            }
            if let (Some(e), Some(s)) = (l.expander, d.smaller()) {
                let deg = if s.more.open { 180.0 } else { 0.0 };
                self.expander(&e, "More options", deg, lit(ToolPart::Expander), f)?;
                if ui.focus() == ToolStop::Expander {
                    rings.push((e.header, radius::CONTROL));
                }
                for (k, p) in l.lines.iter().enumerate() {
                    self.styled(p.line.label(), Style::new(Ramp::Body), &p.label, f.text)?;
                    if let Some(note) = p.line.note() {
                        self.styled(note, caption.wrapping(), &p.note, f.text_secondary)?;
                    }
                    let lk = lit(ToolPart::Line(k));
                    if p.line.is_toggle() {
                        let on = s.line_on(p.line);
                        let cy = p.control.y + p.control.h / 2.0;
                        let at = if on { 1.0 } else { 0.0 };
                        self.toggle_switch(p.control.right(), cy, on, at, lk, f)?;
                    } else {
                        self.combo_box(&p.control, &s.line_value(p.line), lk, f)?;
                    }
                    if ui.focus() == ToolStop::Line(k) {
                        rings.push((p.control, radius::CONTROL));
                    }
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
        self.fl_button(
            ButtonKind::Accent,
            &b[0],
            d.title(),
            None,
            lit(ToolPart::Accent),
            f,
        )?;
        self.fl_button(
            ButtonKind::Standard,
            &b[1],
            "Cancel",
            None,
            lit(ToolPart::Cancel),
            f,
        )?;
        match ui.focus() {
            ToolStop::Accent => self.focus_visual(&b[0], radius::CONTROL, f),
            ToolStop::Cancel => self.focus_visual(&b[1], radius::CONTROL, f),
            _ => {}
        }
        if let (Some(list), Some(s)) = (&l.list, d.smaller()) {
            let open = d
                .more_lines()
                .iter()
                .find(|&&m| s.combo(m).is_some_and(|c| c.open));
            if let Some(&m) = open {
                let choices = s.line_choices(m);
                let refs: Vec<&str> = choices.iter().map(String::as_str).collect();
                if let Some(c) = s.combo(m) {
                    self.combo_list(list, &refs, &[], c.value, c.highlight, f)?;
                }
            }
        }
        Ok(())
    }
}
