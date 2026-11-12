//! Settings as one page of cards (mock screen 6): the content layer with
//! the page scrolling in it, the title, each section's heading and its
//! cards, each card's glyph, words and control, the helper apps'
//! Expanders, the scroll indicator, and an open ComboBox's list over it
//! all.

use super::*;
use crate::controls::{ButtonKind, Lit};
use crate::layout::{kit, CardLayout, Rect, CARD_GLYPH};
use crate::settings_page::{Body, CardSpec};
use crate::theme::fluent::{radius, FluentPalette};
use crate::win::text::{Ramp, Style};
use kiln_core::Preset;

/// `r` a hair wider, so the last letter of a run is never trimmed.
fn wider(r: Rect) -> Rect {
    Rect::new(r.x, r.y, r.w + 1.0, r.h)
}

impl Draw<'_> {
    /// The page, rising into place as it opens.
    pub(super) fn settings(&self, sv: &SettingsView, f: &FluentPalette) -> Result<()> {
        let l = &sv.layout;
        let view = l.view;
        self.tint(&view, radius::LAYER, f.layer.mica);
        self.stroke(&view, radius::LAYER, f.card_stroke);
        let a = sv.open.clamp(0.0, 1.0);
        let base = self.base().get();
        self.set_base(compose(&moved(0.0, sv.rise), &base));
        // The page scrolls inside the layer's own 1-DIP edge, so a card
        // scrolled to the top never covers it.
        let inside = view.inset(space::STROKE);
        let drawn = self.with_opacity(a, || {
            self.clipped(&inside, radius::LAYER - space::STROKE, || {
                self.settings_page(sv, f)
            })
        });
        self.set_base(base);
        drawn?;
        if l.max_scroll() > 0.0 {
            let track = Rect::new(view.x, view.y + 4.0, view.w - 2.0, view.h - 8.0);
            self.scroll_indicator(
                &track,
                view.h,
                l.content_h,
                l.scroll,
                0.0,
                sv.scroll_shown,
                f,
            );
        }
        // An open list, over everything on the page.
        if let Some((control, state, list)) = &sv.combo {
            if let Some(Body::Combo {
                choices, swatches, ..
            }) = sv
                .page
                .cards()
                .map(|c| &c.body)
                .find(|b| matches!(b, Body::Combo { control: k, .. } if k == control))
            {
                let choices: Vec<&str> = choices.iter().map(String::as_str).collect();
                self.combo_list(list, &choices, swatches, state.value, state.highlight, f)?;
            }
        }
        Ok(())
    }

    fn settings_page(&self, sv: &SettingsView, f: &FluentPalette) -> Result<()> {
        let l = &sv.layout;
        let shows = |r: &Rect| r.bottom() >= l.view.y && r.y <= l.view.bottom();
        if shows(&l.title) {
            self.styled(
                crate::settings_page::TITLE,
                Style::new(Ramp::Title),
                &l.title,
                f.text,
            )?;
        }
        for (section, r) in &l.sections {
            if shows(r) {
                self.styled(section.label(), Style::new(Ramp::BodyStrong), r, f.text)?;
            }
        }
        let mut rings: Vec<Rect> = vec![];
        for (card, spec) in l.cards.iter().zip(sv.page.cards()) {
            if !shows(&card.rect) {
                continue;
            }
            self.settings_card(sv, card, spec, &mut rings, f)?;
        }
        for r in &rings {
            self.focus_visual(r, radius::CONTROL, f);
        }
        Ok(())
    }

    /// How a control is lit: buttons ease their hover and press, the rest
    /// light at once.
    fn settings_lit(&self, sv: &SettingsView, c: Control) -> Lit {
        if let Some(id) = ButtonId::settings(c) {
            let (hover, pressed) = (sv.button)(id);
            return Lit {
                hover,
                pressed,
                disabled: false,
            };
        }
        if sv.pressed == Some(c) {
            Lit::PRESSED
        } else if sv.hover == Some(c) {
            Lit::HOVER
        } else {
            Lit::REST
        }
    }

    fn settings_card(
        &self,
        sv: &SettingsView,
        card: &CardLayout,
        spec: &CardSpec,
        rings: &mut Vec<Rect>,
        f: &FluentPalette,
    ) -> Result<()> {
        let l = &sv.layout;
        let rect_of = |c: Control| l.find(c).map(|i| l.controls[i].1);
        self.tint(&card.rect, radius::CONTROL, f.card.mica);
        // An Expander's header lights as a subtle button.
        if let Body::Expander(e) = &spec.body {
            let c = Control::Helper(e.helper);
            let look =
                crate::controls::button_look(ButtonKind::Subtle, self.settings_lit(sv, c), f);
            self.tint(
                &card.header.inset(space::STROKE),
                radius::CONTROL,
                look.fill,
            );
        }
        self.stroke(&card.rect, radius::CONTROL, f.card_stroke);
        match spec.glyph {
            Some(code) => self.glyph(code, &card.glyph, CARD_GLYPH, f.text),
            None => self.k_mark(&card.glyph, f),
        }
        let words = Style::new(Ramp::Body).wrapping();
        self.styled(&spec.label, words, &wider(card.label), f.text)?;
        if let (Some(note), Some(r)) = (&spec.note, card.note) {
            let ink = if spec.alert {
                f.critical
            } else {
                f.text_secondary
            };
            self.styled(note, Style::new(Ramp::Caption).wrapping(), &wider(r), ink)?;
        }
        let focused = |c: Control| sv.focus == Some(c);
        match &spec.body {
            Body::Combo {
                control,
                choices,
                value,
                ..
            } => {
                if let Some(r) = rect_of(*control) {
                    let v = choices.get(*value).map_or("", String::as_str);
                    self.combo_box(&r, v, self.settings_lit(sv, *control), f)?;
                    if focused(*control) {
                        rings.push(r);
                    }
                }
            }
            Body::Toggle { control, on } => {
                if let Some(r) = rect_of(*control) {
                    self.toggle_switch(
                        r.right(),
                        r.y + r.h / 2.0,
                        *on,
                        sv.knobs[0],
                        self.settings_lit(sv, *control),
                        f,
                    )?;
                    if focused(*control) {
                        rings.push(r);
                    }
                }
            }
            Body::Radios { setting, value } => {
                for p in Preset::ALL {
                    let c = Control::Preset(*setting, p);
                    let Some(r) = rect_of(c) else { continue };
                    let parts = kit::RadioParts {
                        row: r,
                        circle: Rect::new(
                            r.x,
                            r.y + (r.h - kit::RADIO) / 2.0,
                            kit::RADIO,
                            kit::RADIO,
                        ),
                        label: Rect::new(
                            r.x + kit::RADIO + kit::RADIO_GAP,
                            r.y,
                            (r.w - kit::RADIO - kit::RADIO_GAP + 1.0).max(0.0),
                            r.h,
                        ),
                    };
                    self.radio(
                        &parts,
                        p.label(),
                        *value == Some(p),
                        self.settings_lit(sv, c),
                        f,
                    )?;
                    if focused(c) {
                        rings.push(r);
                    }
                }
            }
            Body::Buttons(buttons) => {
                for (c, label) in buttons {
                    if let Some(r) = rect_of(*c) {
                        self.fl_button(
                            ButtonKind::Standard,
                            &r,
                            label,
                            None,
                            self.settings_lit(sv, *c),
                            f,
                        )?;
                        if focused(*c) {
                            rings.push(r);
                        }
                    }
                }
            }
            Body::Expander(e) => {
                if let (Some(b), Some(r)) = (e.badge, card.badge) {
                    let caption = Style::new(Ramp::Caption).centered();
                    if b.found && !f.high_contrast {
                        self.fill(&r, radius::CONTROL, f.success_fill, 1.0);
                        self.styled(b.text, caption, &r, f.success)?;
                    } else {
                        self.tint(&r, radius::CONTROL, f.control);
                        self.stroke(&r, radius::CONTROL, f.control_stroke);
                        let ink = if f.high_contrast {
                            f.text
                        } else {
                            f.text_secondary
                        };
                        self.styled(b.text, caption, &r, ink)?;
                    }
                }
                if let Some(ch) = card.chevron {
                    let deg = sv.chevrons[e.helper.index()] * 180.0;
                    let (code, turn) = super::controls::chevron_glyph(deg);
                    self.glyph_turned(code, &ch, kit::CHEVRON, turn, f.text);
                }
                let c = Control::Helper(e.helper);
                if focused(c) {
                    rings.push(card.header);
                }
                if let Some(content) = card.content {
                    let t = f.divider;
                    self.hline(
                        content.x + 1.0,
                        content.right() - 1.0,
                        content.y,
                        t.rgb,
                        t.alpha,
                    );
                    if let (Some(label), Some(r)) = (e.line_label, card.line_label) {
                        self.styled(
                            label,
                            Style::new(Ramp::Caption),
                            &wider(r),
                            f.text_secondary,
                        )?;
                    }
                    if let Some(r) = card.line {
                        self.styled(&e.line, words, &wider(r), f.text)?;
                    }
                    if let (Some(n), Some(r)) = (&e.line_note, card.line_note) {
                        self.styled(
                            n,
                            Style::new(Ramp::Caption).wrapping(),
                            &wider(r),
                            f.text_secondary,
                        )?;
                    }
                    for (c, label) in &e.buttons {
                        if let Some(r) = rect_of(*c) {
                            self.fl_button(
                                ButtonKind::Standard,
                                &r,
                                label,
                                None,
                                self.settings_lit(sv, *c),
                                f,
                            )?;
                            if focused(*c) {
                                rings.push(r);
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
