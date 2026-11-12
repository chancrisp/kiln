//! Phase 3's controls drawn in the Fluent tokens (`crate::controls` holds
//! their states and looks, `layout::kit` where their parts go). 15a: the
//! button kinds, ToggleSwitch, RadioButtons, ProgressBar, InfoBar, the
//! focus visual and the scroll indicator. 15b: the ComboBox and its list,
//! the Expander, the text box, the menu flyout and the ContentDialog's
//! frame with its smoke and shadow. Everything is in layout units; no
//! surface draws with these yet.

use super::*;
use crate::controls::{
    button_look, knob_size, radio_look, toggle_look, ButtonKind, Lit, Look, Severity, TextBoxState,
};
use crate::layout::kit;
use crate::theme::fluent::{glyph, radius, Edge, FluentPalette, Shadow, Tint};
use crate::win::text::{Ramp, Style};

/// The glyph an Expander's chevron is drawn with, and how far it is
/// turned, when it has turned `deg` from pointing down: at rest open or
/// shut it is an upright glyph (Chevron up, or down), which lands on
/// whole pixels; only while it turns is it turned.
pub(super) fn chevron_glyph(deg: f32) -> (char, f32) {
    let turn = crate::theme::fluent::motion::EXPANDER_TURN_DEG;
    if deg >= turn - 0.01 {
        (glyph::CHEVRON_UP, 0.0)
    } else {
        (glyph::CHEVRON_DOWN, deg.max(0.0))
    }
}

/// How many layers a soft shadow is drawn in.
const SHADOW_LAYERS: usize = 24;

/// A line of a menu flyout as it is drawn: an item's words, glyph and
/// shortcut, and whether it can be used; or a separator.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MenuLine<'a> {
    Item {
        label: &'a str,
        glyph: Option<char>,
        shortcut: Option<&'a str>,
        enabled: bool,
    },
    Separator,
}

impl Draw<'_> {
    /// `t` over `r`, if it shows at all.
    pub(super) fn tint(&self, r: &Rect, radius: f32, t: Tint) {
        if t.alpha > 0.0 {
            self.fill(r, radius, t.rgb, t.alpha);
        }
    }

    /// A 1-DIP stroke inside `r` in `t`, if it shows at all.
    pub(super) fn stroke(&self, r: &Rect, radius: f32, t: Tint) {
        if t.alpha > 0.0 {
            self.border(r, radius, t.rgb, t.alpha, space::STROKE);
        }
    }

    /// A control's face as `look` has it: its fill, its stroke, and its
    /// stronger edge along the top or bottom between the corners.
    pub(super) fn face(&self, r: &Rect, corner: f32, look: &Look) {
        self.tint(r, corner, look.fill);
        self.stroke(r, corner, look.stroke);
        if let Some((edge, t)) = look.edge {
            if t.alpha > 0.0 {
                let y = match edge {
                    Edge::Top => r.y,
                    Edge::Bottom => r.bottom() - space::STROKE,
                };
                let (x0, x1) = (r.x + corner, r.right() - corner);
                if x1 > x0 {
                    self.hline(x0, x1, y, t.rgb, t.alpha);
                }
            }
        }
    }

    /// How wide `s` runs in `style` on one line.
    pub(super) fn run(&self, s: &str, style: Style) -> Result<f32> {
        let line = style.ramp.style().line;
        Ok(self.text.styled(s, style, self.zoom, 4096.0, line)?.width)
    }

    /// Glyph `code` turned `deg` clockwise about the middle of `r`, as the
    /// Expander's chevron turns. Upright it lands on whole pixels.
    pub(super) fn glyph_turned(&self, code: char, r: &Rect, size: f32, deg: f32, c: Rgb) {
        if deg.abs() < 0.01 {
            return self.glyph(code, r, size, c);
        }
        let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        let base = self.base.get();
        let turn = compose(&moved(-cx, -cy), &turn_scale_at(deg, 1.0, cx, cy));
        self.set_base(compose(&turn, &base));
        self.glyph(code, r, size, c);
        self.set_base(base);
    }

    /// Glyph `code` `size` DIPs on screen whatever the Text size: laid out
    /// at its token size and scaled down by the zoom about the middle of
    /// `r`, so only token sizes make glyph formats. The title bar's glyphs
    /// don't grow with Text size.
    pub(super) fn glyph_dips(&self, code: char, r: &Rect, size: f32, c: Rgb) {
        let z = if self.zoom.is_finite() && self.zoom > 0.0 {
            self.zoom
        } else {
            1.0
        };
        if (z - 1.0).abs() < 1e-6 {
            return self.glyph(code, r, size, c);
        }
        let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        let base = self.base.get();
        let shrink = compose(&moved(-cx, -cy), &scale_at(1.0 / z, cx, cy));
        self.set_base(compose(&shrink, &base));
        self.glyph(code, r, size, c);
        self.set_base(base);
    }

    // ------------------------------------------------------------ 15a

    /// A button of `kind` in `r`: its face, then its glyph (if any) and
    /// label centred in the look's ink. A hyperlink is underlined while
    /// lit.
    pub(super) fn fl_button(
        &self,
        kind: ButtonKind,
        r: &Rect,
        label: &str,
        glyph: Option<char>,
        lit: Lit,
        p: &FluentPalette,
    ) -> Result<()> {
        let look = button_look(kind, lit, p);
        self.face(r, radius::CONTROL, &look);
        let body = Style::new(Ramp::Body);
        let w = self.run(label, body)?;
        let parts = kit::button_parts(*r, w.min(r.w), glyph.is_some());
        if let (Some(code), Some(g)) = (glyph, parts.glyph) {
            self.glyph(code, &g, kit::BUTTON_GLYPH, look.ink);
        }
        // A hair wider than the run, so the last letter is never trimmed.
        let label_r = Rect::new(parts.label.x, r.y, parts.label.w + 1.0, r.h);
        self.styled(label, body, &label_r, look.ink)?;
        if look.underline {
            let y = r.y + r.h / 2.0 + 7.0;
            self.hline(parts.label.x, parts.label.right(), y, look.ink, 1.0);
        }
        Ok(())
    }

    /// A subtle button holding only glyph `code`, `size` across.
    pub(super) fn icon_button(&self, r: &Rect, code: char, size: f32, lit: Lit, p: &FluentPalette) {
        let look = button_look(ButtonKind::Subtle, lit, p);
        self.face(r, radius::CONTROL, &look);
        self.glyph(code, r, size, look.ink);
    }

    /// A ToggleSwitch ending at `right`, centred on `cy`: "On" or "Off"
    /// before its track, its knob `knob_at` of the way across.
    pub(super) fn toggle_switch(
        &self,
        right: f32,
        cy: f32,
        on: bool,
        knob_at: f32,
        lit: Lit,
        p: &FluentPalette,
    ) -> Result<()> {
        let look = toggle_look(on, lit, p);
        let word = if on { "On" } else { "Off" };
        let body = Style::new(Ramp::Body);
        let w = self.run(word, body)?;
        let size = knob_size(lit);
        let parts = kit::toggle_parts(right, cy, w + 1.0, size, knob_at);
        let round = parts.track.h / 2.0;
        self.tint(&parts.track, round, look.track);
        self.stroke(&parts.track, round, look.rim);
        self.fill(&parts.knob, size / 2.0, look.knob, 1.0);
        self.styled(word, body.trailing(), &parts.word, look.word)?;
        Ok(())
    }

    /// One radio button: its circle, its dot when picked, its label.
    pub(super) fn radio(
        &self,
        parts: &kit::RadioParts,
        label: &str,
        on: bool,
        lit: Lit,
        p: &FluentPalette,
    ) -> Result<()> {
        let look = radio_look(on, lit, p);
        let c = &parts.circle;
        let round = c.w / 2.0;
        self.tint(c, round, look.fill);
        self.stroke(c, round, look.rim);
        if let Some(dot) = look.dot {
            let s = look.dot_size;
            let d = Rect::new(c.x + (c.w - s) / 2.0, c.y + (c.h - s) / 2.0, s, s);
            self.fill(&d, s / 2.0, dot, 1.0);
        }
        self.styled(label, Style::new(Ramp::Body), &parts.label, look.label)?;
        Ok(())
    }

    /// A ProgressBar in `r` (3 tall): a thin track in the strong stroke,
    /// so it holds 3:1 where the mock's faint band wouldn't, and the lit
    /// part from `span.0` to `span.1` in the accent.
    pub(super) fn progress_bar(&self, r: &Rect, span: (f32, f32), p: &FluentPalette) {
        let (line, lit) = kit::progress_parts(*r, span);
        self.tint(&line, 0.5, p.strong_stroke);
        if lit.w > 0.0 {
            self.fill(&lit, radius::PROGRESS, p.accent, 1.0);
        }
    }

    /// An InfoBar as `l` lays it out: its fill and edge, the severity's
    /// glyph, its title (Body Strong) and message, its action buttons
    /// (`actions`: label, kind, lit) and its Dismiss.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn info_bar(
        &self,
        l: &kit::InfoBarLayout,
        severity: Severity,
        title: &str,
        message: &str,
        actions: &[(&str, ButtonKind, Lit)],
        dismiss: Lit,
        p: &FluentPalette,
    ) -> Result<()> {
        let (fill, ink) = severity.colours(p);
        self.tint(&l.rect, radius::LAYER, fill);
        self.stroke(&l.rect, radius::LAYER, p.card_stroke);
        self.glyph(severity.glyph(), &l.glyph, kit::INFO_GLYPH, ink);
        if !title.is_empty() {
            let mut t = l.title;
            t.w += 1.0;
            self.styled(title, Style::new(Ramp::BodyStrong), &t, p.text)?;
        }
        let body = if l.inline {
            Style::new(Ramp::Body)
        } else {
            Style::new(Ramp::Body).wrapping()
        };
        self.styled(message, body, &l.message, p.text)?;
        for ((label, kind, lit), r) in actions.iter().zip(&l.actions) {
            self.fl_button(*kind, r, label, None, *lit, p)?;
        }
        if let Some(d) = l.dismiss {
            self.icon_button(&d, glyph::DISMISS, kit::CHEVRON, dismiss, p);
        }
        Ok(())
    }

    /// The two-tone focus visual round a control `r` with corners of
    /// `corner`: the outer ring in `focus_outer`, the inner in
    /// `focus_inner`.
    pub(super) fn focus_visual(&self, r: &Rect, corner: f32, p: &FluentPalette) {
        let [(outer, ro, wo), (inner, ri, wi)] = kit::focus_rings(*r, corner);
        self.border(&outer, ro, p.focus_outer, 1.0, wo);
        self.border(&inner, ri, p.focus_inner, 1.0, wi);
    }

    /// A dialog's focus `rings` (each a control's rect and corner), drawn
    /// after its content's clip has closed, clipped only to what
    /// `kit::ring_clip` allows round `content`: a control as wide as the
    /// content keeps the sides of its ring.
    pub(super) fn dialog_rings(
        &self,
        content: &Rect,
        rings: &[(Rect, f32)],
        p: &FluentPalette,
    ) -> Result<()> {
        if rings.is_empty() {
            return Ok(());
        }
        self.clipped(&kit::ring_clip(*content), 0.0, || {
            for (r, corner) in rings {
                self.focus_visual(r, *corner, p);
            }
            Ok(())
        })
    }

    /// The scroll indicator at the right of `track`: a 2-DIP thumb at
    /// rest, growing to 6 with a track behind it as `hover` eases in, all
    /// at `alpha` as it hides.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn scroll_indicator(
        &self,
        track: &Rect,
        view: f32,
        content: f32,
        scroll: f32,
        hover: f32,
        alpha: f32,
        p: &FluentPalette,
    ) {
        use crate::theme::fluent::space::{SCROLL_THUMB_HOVER, SCROLL_THUMB_REST};
        let hover = hover.clamp(0.0, 1.0);
        let width = SCROLL_THUMB_REST + (SCROLL_THUMB_HOVER - SCROLL_THUMB_REST) * hover;
        let Some(thumb) = kit::scroll_thumb(*track, view, content, scroll, width) else {
            return;
        };
        if alpha <= 0.0 {
            return;
        }
        if hover > 0.0 {
            let bar = SCROLL_THUMB_HOVER + 2.0 * kit::SCROLL_EDGE;
            let back = Rect::new(track.right() - bar, track.y, bar, track.h);
            let t = p.control_hover;
            self.fill(&back, bar / 2.0, t.rgb, t.alpha * hover * alpha);
        }
        let t = p.strong_stroke;
        self.fill(&thumb, width / 2.0, t.rgb, t.alpha * alpha);
    }

    // ------------------------------------------------------------ 15b

    /// A soft shadow under `r` with corners of `corner`, for each of
    /// `shadows` (CSS's `0 y blur`), at `alpha` of its strength: layers
    /// grown from `blur` inside to `blur` outside, each a share of the
    /// alpha, so the edge fades as a blur would.
    pub(super) fn fl_shadow(&self, r: &Rect, corner: f32, shadows: &[Shadow], alpha: f32) {
        for s in shadows {
            let a = 1.0 - (1.0 - s.alpha * alpha.clamp(0.0, 1.0)).powf(1.0 / SHADOW_LAYERS as f32);
            for j in 0..SHADOW_LAYERS {
                let grow = s.blur * (1.0 - 2.0 * (j as f32 + 0.5) / SHADOW_LAYERS as f32) / 2.0;
                let layer = Rect::new(
                    r.x - grow,
                    r.y + s.y - grow,
                    r.w + 2.0 * grow,
                    r.h + 2.0 * grow,
                );
                if layer.w <= 0.0 || layer.h <= 0.0 {
                    continue;
                }
                let c = (corner + grow).clamp(0.0, layer.w.min(layer.h) / 2.0);
                self.fill(&layer, c, s.rgb, a);
            }
        }
    }

    /// A flyout's frame: its shadow, the overlay and its edge.
    pub(super) fn flyout_frame(&self, r: &Rect, p: &FluentPalette) {
        self.fl_shadow(r, radius::LAYER, p.flyout_shadow, 1.0);
        self.fill(r, radius::LAYER, p.overlay, 1.0);
        self.stroke(r, radius::LAYER, p.overlay_stroke);
    }

    /// A ComboBox in `r` showing `value`: a standard button's face, the
    /// value at the left and the chevron at the right.
    pub(super) fn combo_box(
        &self,
        r: &Rect,
        value: &str,
        lit: Lit,
        p: &FluentPalette,
    ) -> Result<()> {
        let look = button_look(ButtonKind::Standard, lit, p);
        self.face(r, radius::CONTROL, &look);
        let (v, chevron) = kit::combo_parts(*r);
        self.styled(value, Style::new(Ramp::Body), &v, look.ink)?;
        let ink = if p.high_contrast || lit.disabled {
            look.ink
        } else {
            p.text_secondary
        };
        self.glyph(glyph::CHEVRON_DOWN, &chevron, kit::CHEVRON, ink);
        Ok(())
    }

    /// A lit row of a list or menu: the control's hover fill, or under a
    /// contrast theme Highlight. Returns the ink for the row's text.
    fn list_light(&self, r: &Rect, lit: bool, p: &FluentPalette) -> Rgb {
        if !lit {
            return p.text;
        }
        if p.high_contrast {
            self.fill(r, radius::CONTROL, p.accent, 1.0);
            p.on_accent
        } else {
            self.tint(r, radius::CONTROL, p.control_hover);
            p.text
        }
    }

    /// A ComboBox's open list as `l` lays it out: `choices`, the value
    /// marked by the selection pill, the highlight lit. `swatches`, when
    /// there are any, are each choice's three dots at the item's right.
    pub(super) fn combo_list(
        &self,
        l: &kit::FlyoutLayout,
        choices: &[&str],
        swatches: &[[crate::theme::Rgb; 3]],
        value: usize,
        highlight: usize,
        p: &FluentPalette,
    ) -> Result<()> {
        self.flyout_frame(&l.rect, p);
        for (k, item) in l.items.iter().enumerate() {
            let i = l.first + k;
            let Some(choice) = choices.get(i) else {
                break;
            };
            // Under a contrast theme only the highlight is Highlight; the
            // value is told by its pill, so the two never look alike.
            let lit = i == highlight || (i == value && !p.high_contrast);
            let ink = self.list_light(item, lit, p);
            if i == value {
                let (pw, ph) = crate::theme::fluent::space::SELECTION_PILL;
                let pill = Rect::new(item.x, item.y + (item.h - ph) / 2.0, pw, ph);
                let c = if p.high_contrast && i == highlight {
                    p.on_accent
                } else {
                    p.accent
                };
                self.fill(&pill, pw / 2.0, c, 1.0);
            }
            let room = if swatches.is_empty() {
                0.0
            } else {
                kit::swatch_room()
            };
            let text = Rect::new(
                item.x + kit::BUTTON_PAD,
                item.y,
                item.w - 2.0 * kit::BUTTON_PAD - room,
                item.h,
            );
            self.styled(choice, Style::new(Ramp::Body), &text, ink)?;
            if let Some(dots) = swatches.get(i) {
                // A dot the colour of the list under it would vanish, so
                // each has a rim.
                for (dot, rgb) in kit::swatch_dots(*item).iter().zip(dots) {
                    self.fill(dot, kit::SWATCH_DOT / 2.0, *rgb, 1.0);
                    self.stroke(dot, kit::SWATCH_DOT / 2.0, p.strong_stroke);
                }
            }
        }
        Ok(())
    }

    /// An Expander as `parts` lays it out: a card with its header (lit
    /// as a subtle button), its title, its chevron turned `deg`, and the
    /// divider over its content while it shows.
    pub(super) fn expander(
        &self,
        parts: &kit::ExpanderParts,
        title: &str,
        deg: f32,
        lit: Lit,
        p: &FluentPalette,
    ) -> Result<()> {
        let h = parts.header;
        let whole = match parts.content {
            Some(c) => Rect::new(h.x, h.y, h.w, c.bottom() - h.y),
            None => h,
        };
        self.tint(&whole, radius::CONTROL, p.card.mica);
        let look = button_look(ButtonKind::Subtle, lit, p);
        self.tint(&h.inset(space::STROKE), radius::CONTROL, look.fill);
        self.stroke(&whole, radius::CONTROL, p.card_stroke);
        self.styled(title, Style::new(Ramp::Body), &parts.title, look.ink)?;
        let (code, turn) = chevron_glyph(deg);
        self.glyph_turned(code, &parts.chevron, kit::CHEVRON, turn, look.ink);
        if parts.content.is_some() {
            let t = p.divider;
            self.hline(h.x + 1.0, h.right() - 1.0, h.bottom(), t.rgb, t.alpha);
        }
        Ok(())
    }

    /// A text box in `r` holding `t`: a control's face with its bottom
    /// edge in secondary text and its other edges in the field stroke, 2
    /// DIPs of accent along the bottom while it has the focus; the
    /// selection in the accent and the caret while focused. While it is
    /// empty it shows `placeholder` in secondary text.
    pub(super) fn text_box(
        &self,
        r: &Rect,
        t: &TextBoxState,
        placeholder: Option<&str>,
        focused: bool,
        lit: Lit,
        p: &FluentPalette,
    ) -> Result<()> {
        let look = button_look(
            ButtonKind::Standard,
            Lit {
                pressed: 0.0,
                ..lit
            },
            p,
        );
        // Under a contrast theme the box keeps its face under the pointer,
        // so its text stays on it, and the pointer shows in its edge.
        let (fill, stroke) = if p.high_contrast && !lit.disabled {
            let edge = if lit.lit() {
                p.control_hover_stroke
            } else {
                p.control_stroke
            };
            (p.control, edge)
        } else if lit.disabled {
            (look.fill, look.stroke)
        } else {
            (look.fill, p.field_stroke)
        };
        self.tint(r, radius::CONTROL, fill);
        self.stroke(r, radius::CONTROL, stroke);
        let edge = if focused && !lit.disabled { 2.0 } else { 1.0 };
        let edge_c = match (focused && !lit.disabled, p.high_contrast) {
            (true, _) => p.accent,
            (false, true) => stroke.rgb,
            (false, false) => p.text_secondary,
        };
        self.clipped(r, radius::CONTROL, || {
            let line = Rect::new(r.x, r.bottom() - edge, r.w, edge);
            self.fill(&line, 0.0, edge_c, 1.0);
            Ok(())
        })?;
        let box_r = Rect::new(
            r.x + kit::TEXT_PAD,
            r.y,
            (r.w - 2.0 * kit::TEXT_PAD).max(0.0),
            r.h,
        );
        let body = Style::new(Ramp::Body);
        let laid = self
            .text
            .styled(t.text(), body, self.zoom, box_r.w, box_r.h)?;
        let ink = if lit.disabled {
            p.text_disabled
        } else {
            p.text
        };
        if let (true, Some(hint)) = (t.text().is_empty(), placeholder) {
            let shown = if lit.disabled {
                p.text_disabled
            } else {
                p.text_secondary
            };
            self.styled(hint, body, &box_r, shown)?;
        }
        // SAFETY: plain draws on this thread's render target, in a
        // BeginDraw, of a layout that outlives the calls; the hit-test
        // metrics are locals.
        unsafe {
            self.rt.DrawTextLayout(
                pt(box_r.x, box_r.y),
                &laid.layout,
                self.paint(ink, 1.0),
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
            );
            if !focused || lit.disabled {
                return Ok(());
            }
            let at = |chars: usize| -> f32 {
                let units: usize = t.text().chars().take(chars).map(char::len_utf16).sum();
                let (mut x, mut y) = (0.0f32, 0.0f32);
                let mut m = DWRITE_HIT_TEST_METRICS::default();
                let _ =
                    laid.layout
                        .HitTestTextPosition(units as u32, false, &mut x, &mut y, &mut m);
                box_r.x + x
            };
            let line = body.ramp.style().line;
            let ty = r.y + (r.h - line) / 2.0;
            let sel = t.selection();
            if sel.is_empty() {
                let x = at(t.caret()).round();
                let caret = Rect::new(x, ty + 2.0, space::STROKE, line - 4.0);
                self.fill(&caret, 0.0, p.text, 1.0);
            } else {
                let (x0, x1) = (at(sel.start), at(sel.end));
                let band = Rect::new(x0, ty, x1 - x0, line);
                self.fill(&band, 0.0, p.accent, 1.0);
                self.rt
                    .PushAxisAlignedClip(&rectf(&band), D2D1_ANTIALIAS_MODE_ALIASED);
                self.rt.DrawTextLayout(
                    pt(box_r.x, box_r.y),
                    &laid.layout,
                    self.paint(p.on_accent, 1.0),
                    D2D1_DRAW_TEXT_OPTIONS_CLIP,
                );
                self.rt.PopAxisAlignedClip();
            }
        }
        Ok(())
    }

    /// A menu flyout as `l` lays it out: its `lines`, the `highlight`ed
    /// one (by index into `lines`) lit, glyphs in a column, labels after
    /// them, shortcuts at the right in Caption, separators as dividers.
    pub(super) fn menu_flyout(
        &self,
        l: &kit::MenuLayout,
        lines: &[MenuLine],
        highlight: Option<usize>,
        p: &FluentPalette,
    ) -> Result<()> {
        self.flyout_frame(&l.rect, p);
        let glyphs = lines
            .iter()
            .any(|m| matches!(m, MenuLine::Item { glyph: Some(_), .. }));
        for (i, (line, row)) in lines.iter().zip(&l.rows).enumerate() {
            match *line {
                MenuLine::Separator => {
                    let t = p.divider;
                    let y = row.y + (row.h - 1.0) / 2.0;
                    self.hline(l.rect.x, l.rect.right(), y, t.rgb, t.alpha);
                }
                MenuLine::Item {
                    label,
                    glyph,
                    shortcut,
                    enabled,
                } => {
                    let lit = enabled && highlight == Some(i);
                    let ink = if enabled {
                        self.list_light(row, lit, p)
                    } else {
                        p.text_disabled
                    };
                    let mut x = row.x + kit::BUTTON_PAD;
                    if let Some(code) = glyph {
                        let g = Rect::new(x, row.y, kit::BUTTON_GLYPH, row.h);
                        self.glyph(code, &g, kit::BUTTON_GLYPH, ink);
                    }
                    if glyphs {
                        x += kit::BUTTON_GLYPH + kit::BUTTON_PAD;
                    }
                    let right = row.right() - kit::BUTTON_PAD;
                    let text = Rect::new(x, row.y, (right - x).max(0.0), row.h);
                    self.styled(label, Style::new(Ramp::Body), &text, ink)?;
                    if let Some(s) = shortcut {
                        let secondary = if !enabled {
                            p.text_disabled
                        } else if lit && p.high_contrast {
                            ink
                        } else {
                            p.text_secondary
                        };
                        let cap = Rect::new(x, row.y, (right - x).max(0.0), row.h);
                        self.styled(s, Style::new(Ramp::Caption).trailing(), &cap, secondary)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// The right-click menu as a menu flyout: each item's label and its
    /// shortcut, the lit one lit.
    pub(super) fn row_menu(&self, mv: &MenuView, p: &FluentPalette) -> Result<()> {
        let parts: Vec<(&str, String)> = mv.items.iter().map(|i| i.parts()).collect();
        let lines: Vec<MenuLine> = mv
            .items
            .iter()
            .zip(&parts)
            .map(|(&item, (label, key))| {
                if item == crate::ways_in::MenuItem::Separator {
                    MenuLine::Separator
                } else {
                    MenuLine::Item {
                        label,
                        glyph: item.glyph(),
                        shortcut: (!key.is_empty()).then_some(key.as_str()),
                        enabled: true,
                    }
                }
            })
            .collect();
        self.menu_flyout(mv.layout, &lines, mv.lit, p)
    }

    /// The smoke over a window `w` × `h` behind a dialog, at `alpha` of
    /// its strength. A contrast theme has none.
    pub(super) fn smoke(&self, w: f32, h: f32, alpha: f32, p: &FluentPalette) {
        let t = p.smoke;
        if t.alpha > 0.0 && alpha > 0.0 {
            self.fill(&Rect::new(0.0, 0.0, w, h), 0.0, t.rgb, t.alpha * alpha);
        }
    }

    /// A ContentDialog's frame as `l` lays it out: its shadow, the
    /// overlay, the action band in the base colour under a divider, the
    /// edge, and its title in Subtitle. Its content and buttons are the
    /// surface's.
    pub(super) fn dialog_frame(
        &self,
        l: &kit::DialogLayout,
        title: &str,
        p: &FluentPalette,
    ) -> Result<()> {
        let r = &l.rect;
        self.fl_shadow(r, radius::LAYER, p.dialog_shadow, 1.0);
        self.fill(r, radius::LAYER, p.overlay, 1.0);
        self.clipped(r, radius::LAYER, || {
            self.fill(&l.band, 0.0, p.base, 1.0);
            let t = p.divider;
            self.hline(l.band.x, l.band.right(), l.band.y, t.rgb, t.alpha);
            Ok(())
        })?;
        self.stroke(r, radius::LAYER, p.overlay_stroke);
        self.styled(
            title,
            Style::new(Ramp::Subtitle).wrapping(),
            &l.title,
            p.text,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_expanders_chevron_is_upright_at_rest_and_turns_between() {
        assert_eq!(chevron_glyph(0.0), (glyph::CHEVRON_DOWN, 0.0));
        assert_eq!(chevron_glyph(180.0), (glyph::CHEVRON_UP, 0.0));
        assert_eq!(chevron_glyph(90.0), (glyph::CHEVRON_DOWN, 90.0));
        assert_eq!(chevron_glyph(179.0), (glyph::CHEVRON_DOWN, 179.0));
        assert_eq!(chevron_glyph(-1.0), (glyph::CHEVRON_DOWN, 0.0));
        assert_eq!(chevron_glyph(400.0), (glyph::CHEVRON_UP, 0.0));
    }
}
