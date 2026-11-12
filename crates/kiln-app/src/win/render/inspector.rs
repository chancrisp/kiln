use super::*;

use crate::controls::{ButtonKind, Lit};
use crate::theme::fluent::{glyph, radius, FluentPalette};
use crate::win::text::{Ramp, Style};

/// Nothing's mark, in the picture's place: this many DIPs across.
pub(super) const NOTHING_MARK: f32 = 48.0;

/// The kind glyph on a picture's tile while it has no picture.
pub(super) const HERO_GLYPH: f32 = 40.0;

/// Inside a format's row: the padding at either end, the gap after its
/// name, and Make smaller's glyph. A format's code has a column of its own
/// (`layout::OFFER_CODE`).
pub(super) const TARGET_PAD: f32 = layout::OFFER_PAD;
pub(super) const TARGET_GAP: f32 = layout::OFFER_GAP;
pub(super) const SMALLER_GLYPH: f32 = 16.0;

/// How lit an inspector control is: its eased hover and press.
fn lit_of(hot: f32, pressed: f32) -> Lit {
    Lit {
        hover: hot,
        pressed,
        disabled: false,
    }
}

impl Draw<'_> {
    /// The side pane beside the list (mock screens 2, 5 and 8b): the
    /// divider on its left edge, then what it says for the selection, with
    /// its controls. It is drawn where the layout puts it, as far as the
    /// fold has slid it in: its content, laid out on its whole place,
    /// slides in from the layer's right edge, which clips it. Folded all
    /// the way, there is nothing of it to draw.
    pub(super) fn inspector(&self, l: &Layout, iv: &InspectorView, v: &View) -> Result<()> {
        let f = v.title.palette;
        let Some(r) = l.inspector else {
            return Ok(());
        };
        let rule = Rect::new(r.x, r.y, layout::INSPECTOR_RULE, r.h);
        self.tint(&rule, 0.0, f.divider);
        self.clipped(&l.panel, radius::LAYER, || {
            // What it said before fades out under what it says now, which
            // rises in.
            for page in iv.before.iter().chain(std::iter::once(&iv.now)) {
                self.inspector_page(&r, page, v)?;
            }
            Ok(())
        })
    }

    /// One content of the side pane, slid across from where it was laid
    /// out to `r`, where the pane is drawn, at its alpha and rise.
    pub(super) fn inspector_page(&self, r: &Rect, page: &InspectorPage, v: &View) -> Result<()> {
        if page.alpha <= 0.0 {
            return Ok(());
        }
        let dx = r.x - page.laid.x;
        self.with_opacity(page.alpha, || {
            self.set_base(moved(dx, page.rise));
            let drawn = self
                .inspector_says(page, v)
                .and_then(|_| self.inspector_controls(page, v));
            self.set_base(Matrix3x2::identity());
            drawn
        })
    }

    /// What the side pane says for the selection: its picture, title,
    /// texts, facts, InfoBar and heading.
    pub(super) fn inspector_says(&self, iv: &InspectorPage, v: &View) -> Result<()> {
        let f = v.title.palette;
        let l = iv.layout;
        let quiet = f.text_secondary;
        let heading = |text: &str| -> Result<()> {
            if let Some(h) = l.label {
                self.styled(text, Style::new(Ramp::BodyStrong), &h, f.text)?;
            }
            Ok(())
        };
        // Each text under the title where the layout put it, while it has
        // room: see `InspectorLayout::lines`. The tree reads the same.
        let say = |k: usize, text: &str, style: Style, c: Rgb| -> Result<()> {
            if let Some(at) = l.line(k) {
                self.styled(text, style, &at, c)?;
            }
            Ok(())
        };
        let caption = Style::new(Ramp::Caption).wrapping();
        // A line in the tone the row says it in.
        let toned = |tone: &Tone| super::list::line_ink(tone, f);
        match iv.model {
            Inspector::Nothing => {
                if l.hero.h > 0.0 {
                    let (h, k) = (&l.hero, NOTHING_MARK / space::ICON_GRID);
                    let x = h.x + (h.w - NOTHING_MARK) / 2.0;
                    let y = h.y + (h.h - NOTHING_MARK) / 2.0;
                    self.mark_lined(x, y, k, MARK_LINE, quiet, 1.0);
                }
                let title = Style::new(Ramp::Subtitle).wrapping().centered();
                self.styled(insp::NOTHING_TITLE, title, &l.title, f.text)?;
                say(0, iv.nothing, caption.centered(), quiet)?;
            }
            Inspector::Waiting {
                row,
                facts,
                reason,
                targets,
                smaller,
                ..
            } => {
                self.one_file(*row, l, v)?;
                // Why it has nothing to offer, as its row says it. A healthy
                // file has no line here.
                if let Some((why, tone)) = reason {
                    say(0, why, caption, toned(tone))?;
                }
                for ((label, value), r) in facts.iter().zip(&l.facts) {
                    self.styled(label, Style::new(Ramp::Caption), r, quiet)?;
                    // The Folder's value is drawn as the link it is, with
                    // the controls.
                    if *label == "Folder" {
                        continue;
                    }
                    let column = layout::FACT_LABEL;
                    let at = Rect::new(r.x + column, r.y, (r.w - column).max(0.0), r.h);
                    self.styled(value, Style::new(Ramp::Caption), &at, f.text)?;
                }
                if !targets.is_empty() || *smaller {
                    heading(insp::TURN_INTO)?;
                }
            }
            Inspector::Converting {
                row,
                doing,
                percent,
                left,
            } => {
                self.one_file(*row, l, v)?;
                say(0, doing, Style::new(Ramp::Body), f.text)?;
                if let Some(pc) = percent {
                    say(1, &format!("{pc}%"), Style::new(Ramp::Title), f.text)?;
                }
                if let Some(bar) = l.line(2) {
                    let span = match percent {
                        Some(pc) => (0.0, *pc as f32 / 100.0),
                        None => anim::indeterminate(
                            v.progress_clock.unwrap_or(0.0),
                            v.progress_clock.is_some(),
                        ),
                    };
                    self.progress_bar(&bar, span, f);
                }
                if let Some(left) = left {
                    say(3, left, Style::new(Ramp::Caption), quiet)?;
                }
            }
            Inspector::Done {
                row,
                again,
                smaller,
                ..
            } => {
                self.one_file(*row, l, v)?;
                self.pane_bar(iv, v)?;
                if !again.is_empty() || *smaller {
                    heading(insp::CONVERT_AGAIN)?;
                }
            }
            Inspector::Failed { row, .. } => {
                self.one_file(*row, l, v)?;
                self.pane_bar(iv, v)?;
            }
            Inspector::Several {
                rows,
                count,
                kinds,
                targets,
                offered,
                smaller,
                ..
            } => {
                self.stack(&l.hero, rows, v);
                let title = insp::files_count(*count);
                self.styled(&title, Style::new(Ramp::Subtitle), &l.title, f.text)?;
                say(0, kinds, Style::new(Ramp::Caption), quiet)?;
                self.pane_bar(iv, v)?;
                if !targets.is_empty() || *smaller {
                    heading(insp::TURN_THEM_INTO)?;
                } else if !offered.is_empty() {
                    heading(insp::ONE_KIND)?;
                }
            }
        }
        Ok(())
    }

    /// The side pane's InfoBar where its layout put it, with its actions:
    /// the pane's buttons, as the controls list them.
    pub(super) fn pane_bar(&self, iv: &InspectorPage, v: &View) -> Result<()> {
        let f = v.title.palette;
        let (Some(at), Some(bar)) = (&iv.layout.info, iv.model.bar()) else {
            return Ok(());
        };
        let actions: Vec<(&str, ButtonKind, Lit)> = iv
            .controls
            .iter()
            .filter(|(_, r)| at.actions.contains(r))
            .filter_map(|&(c, _)| {
                let label = iv.model.label(c)?;
                let (hot, pressed) = self.lit_for(iv, c, v);
                let kind = if iv.model.accent(c) {
                    ButtonKind::Accent
                } else {
                    ButtonKind::Standard
                };
                Some((label, kind, lit_of(hot, pressed)))
            })
            .collect();
        self.info_bar(
            at,
            bar.severity,
            &bar.title,
            &bar.message,
            &actions,
            Lit::REST,
            f,
        )
    }

    /// How far control `c`'s hover and press have eased in: none for what
    /// it said before.
    fn lit_for(&self, iv: &InspectorPage, c: InspectorControl, v: &View) -> (f32, f32) {
        if iv.live {
            (v.button)(ButtonId::Window(FocusButton::Inspector(c)))
        } else {
            (0.0, 0.0)
        }
    }

    /// One file's picture and its name, in Subtitle.
    pub(super) fn one_file(&self, i: usize, l: &InspectorLayout, v: &View) -> Result<()> {
        let f = v.title.palette;
        let Some(e) = v.entries.get(i) else {
            return Ok(());
        };
        if l.hero.h > 0.0 {
            let picture = (v.preview)(i, PreviewSize::Hero);
            self.fl_preview(
                &l.hero,
                radius::LAYER,
                picture,
                (v.kind)(i),
                HERO_GLYPH,
                1.0,
                f,
            );
        }
        if l.title.h > 0.0 {
            // A long name wraps to a second line, and past that is cut with
            // an ellipsis.
            let title = Style::new(Ramp::Subtitle).wrapping();
            let name =
                self.text
                    .fit_lines(&e.name, title, self.zoom, l.title.w, layout::TITLE_LINES);
            self.styled(&name, title, &l.title, f.text)?;
        }
        Ok(())
    }

    /// Several files' pictures, stacked in the picture's place: up to
    /// three cards, each a step right and down from the one behind, the
    /// first selected file's at the front.
    pub(super) fn stack(&self, hero: &Rect, rows: &[usize], v: &View) {
        let f = v.title.palette;
        let n = rows.len().min(3);
        for (back, card) in layout::card_stack(hero, n).into_iter().enumerate() {
            let i = rows[n - 1 - back];
            // The layer parts each card from the one behind.
            let around = Rect::new(card.x - 1.0, card.y - 1.0, card.w + 2.0, card.h + 2.0);
            self.border(&around, radius::LAYER + 1.0, f.base, 1.0, space::STROKE);
            let picture = (v.preview)(i, PreviewSize::Hero);
            self.fl_preview(
                &card,
                radius::LAYER,
                picture,
                (v.kind)(i),
                HERO_GLYPH,
                1.0,
                f,
            );
        }
    }

    /// The side pane's controls where its layout placed them: Make
    /// smaller, each format or kind as a row, More as a link, and the rest
    /// as buttons (an InfoBar's are drawn with it). Each eases its hover
    /// and press; what it said before takes neither.
    pub(super) fn inspector_controls(&self, iv: &InspectorPage, v: &View) -> Result<()> {
        use InspectorControl as C;
        let f = v.title.palette;
        let targets = iv.model.targets();
        let offered = match iv.model {
            Inspector::Several { offered, .. } => &offered[..],
            _ => &[],
        };
        let in_bar = |r: &Rect| {
            iv.layout
                .info
                .as_ref()
                .is_some_and(|b| b.actions.contains(r))
        };
        for &(c, b) in iv.controls {
            let (hot, pressed) = self.lit_for(iv, c, v);
            let lit = lit_of(hot, pressed);
            match c {
                C::Target(k) => {
                    let Some(t) = targets.get(k) else {
                        continue;
                    };
                    let code = t.ext.to_ascii_uppercase();
                    let estimate = t.estimate_label();
                    let row = OfferRow {
                        glyph: None,
                        name: &code,
                        code: true,
                        why: t.why,
                        estimate: estimate.as_deref(),
                        suggested: t.suggested,
                    };
                    self.offer_row(&b, &row, lit, f)?;
                }
                C::Kind(k) => {
                    let Some((code, n)) = offered.get(k) else {
                        continue;
                    };
                    let files = insp::files_count(*n);
                    let row = OfferRow {
                        glyph: None,
                        name: code,
                        code: false,
                        why: Some(&files),
                        estimate: None,
                        suggested: false,
                    };
                    self.offer_row(&b, &row, lit, f)?;
                }
                C::Smaller => {
                    let row = OfferRow {
                        glyph: Some(glyph::MAKE_SMALLER),
                        name: insp::SMALLER,
                        code: false,
                        why: Some(insp::SMALLER_WHY),
                        estimate: None,
                        suggested: false,
                    };
                    self.offer_row(&b, &row, lit, f)?;
                }
                // A hyperlink whose words line up with the formats above
                // it, underlined under the pointer.
                C::More => {
                    let ink = f.accent_text;
                    let w = self.styled(insp::MORE_FORMATS, Style::new(Ramp::Body), &b, ink)?;
                    if lit.hover > 0.5 {
                        let y = b.y + b.h / 2.0 + 8.0;
                        self.hline(b.x, b.x + w, y, ink, 1.0);
                    }
                }
                // The folder's name, in the fact's own place and size.
                C::Folder => {
                    let ink = f.accent_text;
                    let name = match iv.model {
                        Inspector::Waiting { facts, .. } => {
                            facts.last().map(|(_, v)| v.as_str()).unwrap_or_default()
                        }
                        _ => "",
                    };
                    let w = self.styled(name, Style::new(Ramp::Caption), &b, ink)?;
                    if lit.hover > 0.5 {
                        let y = b.y + b.h / 2.0 + 7.0;
                        self.hline(b.x, b.x + w.min(b.w), y, ink, 1.0);
                    }
                }
                _ if in_bar(&b) => {}
                _ => {
                    if let Some(label) = iv.model.label(c) {
                        // Cancel and Get are the pane's one action.
                        self.fl_button(ButtonKind::Standard, &b, label, None, lit, f)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// A row the side pane offers: Make smaller, a format, or a kind of
    /// file to narrow the selection to (mock `.opt`). A control-filled, card-stroked row
    /// with its glyph, its name in BodyStrong, what it is for in Caption
    /// and its estimate at the right; the pointer lights it as a subtle
    /// control. The suggestion's stroke is the accent.
    pub(super) fn offer_row(
        &self,
        b: &Rect,
        row: &OfferRow,
        lit: Lit,
        f: &FluentPalette,
    ) -> Result<()> {
        let fill = if lit.pressed > 0.5 {
            f.control_pressed
        } else {
            f.control_hover
        };
        // The control fill at rest, so a row has a shape of its own in dark
        // as well as an outline; the pointer lights it over that.
        self.fill(b, radius::CONTROL, f.control.rgb, f.control.alpha);
        let shown = lit.hover.max(lit.pressed).clamp(0.0, 1.0);
        if shown > 0.0 {
            self.fill(b, radius::CONTROL, fill.rgb, fill.alpha * shown);
        }
        if row.suggested {
            self.border(b, radius::CONTROL, f.accent, 1.0, space::STROKE);
        } else {
            self.stroke(b, radius::CONTROL, f.card_stroke);
        }
        let mut x = b.x + TARGET_PAD;
        if let Some(g) = row.glyph {
            let at = Rect::new(
                x,
                b.y + (b.h - SMALLER_GLYPH) / 2.0,
                SMALLER_GLYPH,
                SMALLER_GLYPH,
            );
            self.glyph(g, &at, SMALLER_GLYPH, f.text);
            x = at.right() + TARGET_GAP;
        }
        let mut right = b.right() - TARGET_PAD;
        if let Some(estimate) = row.estimate {
            let at = Rect::new(x, b.y, (right - x).max(0.0), b.h);
            let w = self.styled(
                estimate,
                Style::new(Ramp::Caption).trailing(),
                &at,
                f.text_secondary,
            )?;
            right -= w + TARGET_GAP;
        }
        // A format's code is set as codes are everywhere, in a column of its
        // own, so what it is for starts in one place down the list.
        let (style, words) = if row.code {
            let column = layout::offer_code_column(b);
            self.styled(row.name, Style::new(Ramp::Body).code(), &column, f.text)?;
            (
                Style::new(Ramp::Caption).wrapping(),
                layout::offer_words_x(b),
            )
        } else {
            let at = Rect::new(x, b.y, (right - x).max(0.0), b.h);
            let w = self.styled(row.name, Style::new(Ramp::BodyStrong), &at, f.text)?;
            (Style::new(Ramp::Caption), x + w + TARGET_GAP)
        };
        if let Some(why) = row.why {
            let room = Rect::new(words, b.y, (right - words).max(0.0), b.h);
            let h = self.text.styled_height(why, style, self.zoom, room.w);
            let at = layout::centred_block(&room, h);
            self.styled(why, style, &at, f.text_secondary)?;
        }
        Ok(())
    }
}

/// A row the side pane offers, as [`Draw::offer_row`] draws it.
pub(super) struct OfferRow<'a> {
    pub(super) glyph: Option<char>,
    pub(super) name: &'a str,
    /// The name is a format's code: set in the code style, in its column.
    pub(super) code: bool,
    /// What it is for, or how many files a kind holds.
    pub(super) why: Option<&'a str>,
    pub(super) estimate: Option<&'a str>,
    pub(super) suggested: bool,
}
