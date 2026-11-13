use super::*;
use crate::controls::{ButtonKind, Lit};
use crate::model::RowMark;
use crate::theme::fluent::{glyph, radius, space, FileKind, FluentPalette, Tint};
use crate::win::text::{Ramp, Style};

/// A row's fill is this far in from its neighbours' at top and bottom
/// (the mock's 2-DIP margins).
const ROW_INSET: f32 = 2.0;

/// A just-finished row's accent flash at its strongest.
const ROW_COOL: f32 = 0.12;

/// A shell icon fills at most this share of its tile.
const ICON_SHARE: f32 = 0.8;

/// The kind glyph on a row's tile with no picture (the mock's 20), and
/// the checkmark and error glyphs at a row's right (16).
const ROW_GLYPH: f32 = 20.0;
const MARK_GLYPH: f32 = 16.0;

/// A running row's Cancel.
const CANCEL: &str = "Cancel";

/// The first-run card's step numbers.
const NUMBERS: [&str; 3] = ["1", "2", "3"];

/// The first-run card's Dismiss glyph, smaller than a button's (the mock's
/// 12).
const DISMISS_GLYPH: f32 = 12.0;

/// The drop outline: its dashed line's width, and the accent's wash under
/// it (the mock's 6%).
const DROP_LINE: f32 = 2.0;
const DROP_FILL: f32 = 0.06;

/// Where a row's name and line run: from `x`, both to the end
/// `layout::row_text_ends` gives, short of Cancel, Retry, the link or
/// Options and a mark's room; and the link if the row shows one. `saving`
/// says the row is being saved, and `links` that rows show their links
/// (not while the side pane does: then only Show in folder stays, as
/// `model::link_on_row` has it). Cancel is always on a running row.
pub(super) fn row_text_span(
    r: &Rect,
    e: &Entry,
    saving: bool,
    links: bool,
) -> (f32, (f32, f32), Option<&'static str>) {
    let x = layout::row_text_left(r);
    let link = e
        .link(false)
        .filter(|l| model::link_on_row(l, links) && !saving);
    let ends = layout::row_text_ends(
        r,
        matches!(e.job, JobState::Running { .. }),
        e.retryable() && !saving && links,
        link,
        e.options_for().is_some() && !saving && links,
        matches!(e.mark(), RowMark::Done | RowMark::Failed),
    );
    (x, ends, link)
}

/// How wide `line` runs in a row's second line (Caption) at Text size
/// zoom `zoom`.
pub(super) fn line_width(text: &Text, line: &str, zoom: f32) -> f32 {
    use crate::win::text::{Ramp, Style};
    text.styled(line, Style::new(Ramp::Caption), zoom, 10_000.0, 100.0)
        .map_or(0.0, |laid| laid.width)
}

/// Whether a failed row's line runs past the room it has, so the row cuts
/// it short (its tooltip then has it whole).
pub(super) fn line_cut(text: &Text, zoom: f32, r: &Rect, e: &Entry) -> bool {
    if !matches!(e.job, JobState::Failed { .. }) {
        return false;
    }
    let (x, (_, end), _) = row_text_span(r, e, false, true);
    line_width(text, &e.line().0, zoom) > (end - x).max(0.0)
}

/// The glyph a file with no picture shows: its kind's (mock `.ftype`).
pub(super) fn kind_glyph(kind: Option<Kind>) -> char {
    match kind {
        Some(Kind::Image) => '\u{E91B}',
        Some(Kind::Video) => '\u{E714}',
        Some(Kind::Audio) => '\u{E8D6}',
        Some(Kind::Document | Kind::Spreadsheet | Kind::Presentation) => '\u{E8A5}',
        _ => '\u{E7C3}',
    }
}

/// A row's subtle button in its slot `slot`, drawn as wide as its words
/// (and glyph) need at the slot's right. Where it starts.
fn slot_button(
    d: &Draw,
    slot: &Rect,
    label: &str,
    glyph: Option<char>,
    lit: Lit,
    f: &FluentPalette,
) -> Result<f32> {
    let w = d.run(label, Style::new(Ramp::Body))?;
    let bw = layout::kit::button_width(w, glyph.is_some()).min(slot.w);
    let r = Rect::new(slot.right() - bw, slot.y, bw, slot.h);
    d.fl_button(ButtonKind::Subtle, &r, label, glyph, lit, f)?;
    Ok(r.x)
}

/// `f` for what is drawn on a selected row under a contrast theme: its
/// words, marks and subtle buttons in HighlightText (`on_selection`) on
/// the Highlight fill. A hovered button's own Highlight fill keeps its
/// HighlightText.
fn on_highlight(f: &FluentPalette) -> FluentPalette {
    let ink = f.on_selection;
    FluentPalette {
        text: ink,
        text_secondary: ink,
        control_text: ink,
        critical: ink,
        success: ink,
        ..*f
    }
}

/// The colour a line of a row (or of the side pane) is said in: the
/// secondary text for everything that is so, a saved result included, since
/// the checkmark carries that state, and the critical colour for a problem
pub(super) fn line_ink(tone: &Tone, f: &FluentPalette) -> Rgb {
    match tone {
        Tone::Muted | Tone::Result => f.text_secondary,
        Tone::Crack => f.critical,
    }
}

/// A lit state from a hover that is on or off.
fn lit(on: bool) -> Lit {
    Lit {
        hover: if on { 1.0 } else { 0.0 },
        pressed: 0.0,
        disabled: false,
    }
}
impl Draw<'_> {
    /// The empty window (mock screen 1): the content layer, the K, the
    /// headline and body line, Add files, the status, the first-run card
    /// while it shows, and the dashed drop outline while files are dragged
    /// over it.
    pub(super) fn empty_state(&self, l: &Layout, v: &View) -> Result<()> {
        let f = v.title.palette;
        let e = v.empty_at.unwrap_or_else(|| {
            let card = v.first_run.map(|_| layout::CardText::ONE_LINE);
            layout::empty_state(&l.panel, card)
        });
        // The whole layer, with an InfoBar at its top when one shows; the
        // drop panel's group under it.
        self.tint(&l.body, radius::LAYER, f.layer.mica);
        self.stroke(&l.body, radius::LAYER, f.card_stroke);
        self.top_info_bar(v)?;
        let layer = e.layer;
        // Whatever runs past a short layer's bottom is cut at it.
        // SAFETY: a plain clip on this thread's render target, inside the
        // frame's BeginDraw, popped below.
        unsafe {
            self.rt
                .PushAxisAlignedClip(&rectf(&layer), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        }
        let drawn = self.empty_group(&e, v, f);
        // SAFETY: pops the clip pushed above.
        unsafe { self.rt.PopAxisAlignedClip() };
        drawn?;
        // A drag over the window: the dashed jade outline, 12 inside the
        // layer, over a faint wash of the accent (none under a contrast
        // theme, where the outline is Highlight).
        if v.drag > 0.0 {
            if !f.high_contrast {
                self.fill(&e.drop, radius::LAYER, f.accent, DROP_FILL * v.drag);
            }
            // SAFETY: a plain draw on this thread's render target, inside
            // the frame's BeginDraw.
            unsafe {
                self.rt.DrawRoundedRectangle(
                    &rounded(&e.drop.inset(DROP_LINE / 2.0), radius::LAYER),
                    self.paint(f.accent, v.drag),
                    DROP_LINE,
                    self.dash,
                );
            }
        }
        Ok(())
    }

    /// The empty window's centred group, as `empty_state` lays it out.
    fn empty_group(&self, e: &layout::EmptyState, v: &View, f: &FluentPalette) -> Result<()> {
        if let Some(m) = &e.mark {
            self.big_k(m, f);
        }
        let lines = v.empty;
        let centred = |r: Ramp| Style::new(r).centered();
        self.styled(lines.headline, centred(Ramp::Subtitle), &e.headline, f.text)?;
        self.styled(lines.body, centred(Ramp::Body), &e.body, f.text_secondary)?;
        let (hover, pressed) = (v.button)(ButtonId::Window(FocusButton::AddFiles));
        let lit = Lit {
            hover,
            pressed,
            disabled: false,
        };
        self.fl_button(
            ButtonKind::Accent,
            &e.button,
            lines.button,
            Some(glyph::ADD_FILES),
            lit,
            f,
        )?;
        self.styled(
            v.empty_status,
            centred(Ramp::Caption),
            &e.status,
            f.text_secondary,
        )?;
        if let (Some(fr), Some(c)) = (v.first_run, &e.card) {
            self.first_run_card(fr, c, v, f)?;
        }
        Ok(())
    }

    /// The first-run card: its heading, its three numbered steps, the
    /// right-click line and Dismiss.
    fn first_run_card(
        &self,
        fr: &ways_in::FirstRun,
        c: &layout::FirstRunCard,
        v: &View,
        f: &FluentPalette,
    ) -> Result<()> {
        self.tint(&c.card, radius::LAYER, f.card.mica);
        self.stroke(&c.card, radius::LAYER, f.card_stroke);
        self.styled(fr.title, Style::new(Ramp::BodyStrong), &c.title, f.text)?;
        let half = layout::CARD_NUMBER / 2.0;
        for (k, (step, (n, r))) in fr
            .steps
            .iter()
            .zip(c.numbers.iter().zip(&c.steps))
            .enumerate()
        {
            // The number in a circle of the selection's tint, in the
            // accent's text colour; a contrast theme rings it instead.
            let ink = if f.high_contrast {
                self.stroke(n, half, f.card_stroke);
                f.text
            } else {
                self.tint(n, half, f.selection);
                f.accent_text
            };
            let digit = NUMBERS[k];
            self.styled(digit, Style::new(Ramp::Caption).code().centered(), n, ink)?;
            let strong = ways_in::strong_runs(step, fr.strong);
            self.styled_runs(step, Style::new(Ramp::Body).wrapping(), &strong, r, f.text)?;
        }
        self.styled_runs(
            fr.more,
            Style::new(Ramp::Caption).wrapping(),
            &ways_in::strong_runs(fr.more, fr.strong),
            &c.more,
            f.text_secondary,
        )?;
        let (hover, pressed) = (v.button)(ButtonId::Window(FocusButton::Dismiss));
        let lit = Lit {
            hover,
            pressed,
            disabled: false,
        };
        self.icon_button(&c.dismiss, glyph::DISMISS, DISMISS_GLYPH, lit, f);
        Ok(())
    }

    /// The K at 64: the flame K on its accent tile, 2 DIPs in from each
    /// side as the mock draws it on a 64 x 64 grid.
    fn big_k(&self, r: &Rect, f: &FluentPalette) {
        let s = r.w / 64.0;
        let tile = Rect::new(r.x + 2.0 * s, r.y + 2.0 * s, 60.0 * s, 60.0 * s);
        self.k_mark(&tile, f);
    }

    /// The InfoBar at the top of the list or the empty window, while one
    /// shows.
    fn top_info_bar(&self, v: &View) -> Result<()> {
        let Some(t) = &v.top_info else {
            return Ok(());
        };
        let (hover, pressed) = (v.button)(ButtonId::Window(FocusButton::InfoDismiss));
        let lit = Lit {
            hover,
            pressed,
            disabled: false,
        };
        self.info_bar(
            &t.layout,
            crate::controls::Severity::Info,
            t.title,
            t.message,
            &[],
            lit,
            v.title.palette,
        )
    }

    /// The list with files (mock screen 2): the content layer, the rows on
    /// it, the side pane beside them and the rule over the footer; the
    /// dashed drop outline while files are dragged over it; and the scroll
    /// indicator when the rows overflow.
    pub(super) fn list(&self, l: &Layout, v: &View) -> Result<()> {
        let f = v.title.palette;
        let layer = l.body;
        self.tint(&layer, radius::LAYER, f.layer.mica);
        self.stroke(&layer, radius::LAYER, f.card_stroke);
        if let Some(foot) = l.footer {
            let rule = Rect::new(foot.x, foot.y, foot.w, layout::INSPECTOR_RULE);
            self.tint(&rule, 0.0, f.divider);
        }
        // The InfoBar at the top of the list, over the rows' start.
        self.top_info_bar(v)?;
        // Rows are cut at the list's edges as they scroll, and under the
        // InfoBar at its top.
        let top = l
            .top_info
            .map_or(l.panel.y, |r| r.bottom() + layout::TOP_INFO_GAP / 2.0);
        let clip = Rect::new(
            l.list.x - layout::LIST_PAD,
            top,
            l.list.w + 2.0 * layout::LIST_PAD,
            (l.panel.bottom() - top).max(0.0),
        );
        // SAFETY: a plain clip on this thread's render target, inside the
        // frame's BeginDraw, popped below.
        unsafe {
            self.rt
                .PushAxisAlignedClip(&rectf(&clip), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        }
        let first = (v.scroll / ROW_HEIGHT).floor().max(0.0) as usize;
        let visible = (l.list.h / ROW_HEIGHT).ceil() as usize + 1;
        // The one selected row's fill, under the rows, where it has glided
        // to; it pours in with its row.
        if let (Some(y), Some(&i)) = (v.selection_y, v.selected.first()) {
            let anim = (v.row_anim)(i);
            let at = l.list.y + y - v.scroll + anim.rise;
            let r = Rect::new(l.list.x, at, l.list.w, ROW_HEIGHT);
            self.selection_fill(&r, anim.appear, f);
        }
        let mut result = Ok(());
        for i in first..(first + visible).min(v.entries.len()) {
            let anim = (v.row_anim)(i);
            let mut r = l.rows[i];
            // Scrolled, and still rising as it pours in.
            r.y += anim.rise - v.scroll;
            result = self.row(&r, &v.entries[i], i, anim, v);
            if result.is_err() {
                break;
            }
        }
        // SAFETY: pops the clip pushed above.
        unsafe { self.rt.PopAxisAlignedClip() };
        result?;
        // The side pane beside the rows.
        if let Some(iv) = &v.inspector {
            self.inspector(l, iv, v)?;
        }
        // Files dragged over the window: the dashed jade outline inside the
        // list's part of the layer, over a faint wash of the accent (none
        // under a contrast theme).
        if v.drag > 0.0 {
            let drop = l.panel.inset(layout::DROP_INSET);
            if !f.high_contrast {
                self.fill(&drop, radius::LAYER, f.accent, DROP_FILL * v.drag);
            }
            // SAFETY: a plain draw on this thread's render target, inside
            // the frame's BeginDraw.
            unsafe {
                self.rt.DrawRoundedRectangle(
                    &rounded(&drop.inset(DROP_LINE / 2.0), radius::LAYER),
                    self.paint(f.accent, v.drag),
                    DROP_LINE,
                    self.dash,
                );
            }
        }
        // The scroll indicator at the list's right edge when it overflows.
        let content = v.entries.len() as f32 * ROW_HEIGHT;
        if content > l.list.h {
            let track = Rect::new(l.list.x, l.list.y, l.list.w + 2.0, l.list.h);
            self.scroll_indicator(&track, l.list.h, content, v.scroll, 0.0, 1.0, f);
        }
        Ok(())
    }

    /// Row `i` in `r` (mock screen 2): its hover or selection fill, 2 DIPs
    /// in from its neighbours, the accent pill at a selected row's left,
    /// what it shows, and the focus visual while the keyboard is on it.
    pub(super) fn row(&self, r: &Rect, e: &Entry, i: usize, anim: RowAnim, v: &View) -> Result<()> {
        let f = v.title.palette;
        let a = anim.appear;
        let hovered = v.hover_row == Some(i);
        let selected = v.selected.contains(&i);
        let fill = Rect::new(r.x, r.y + ROW_INSET, r.w, r.h - 2.0 * ROW_INSET);
        if selected {
            // One selected row's fill glides, and is drawn under the rows.
            if v.selection_y.is_none() {
                self.selection_fill(r, a, f);
            }
        } else if hovered {
            let t = f.control_hover;
            self.fill(&fill, radius::CONTROL, t.rgb, t.alpha * a);
            if f.high_contrast {
                self.border(&fill, radius::CONTROL, f.focus_outer, a, space::STROKE);
            }
        }
        // A row that just finished flashes faintly in the accent and cools
        // to rest. A contrast theme has no washes: the checkmark says it.
        if anim.cool > 0.0 && !f.high_contrast {
            self.fill(&fill, radius::CONTROL, f.accent, ROW_COOL * anim.cool * a);
        }
        // A row that just failed shakes its content, left then right, once.
        let shaken = anim.shake != 0.0;
        if shaken {
            self.set_base(moved(anim.shake, 0.0));
        }
        let drawn = self.row_content(r, e, i, anim, v);
        if shaken {
            self.set_base(Matrix3x2::identity());
        }
        drawn?;
        // The keyboard's row keeps its ring, selected or not.
        if v.focus == Some(i) {
            self.focus_visual(&fill.inset(space::FOCUS_GAP + 1.0), radius::CONTROL, f);
        }
        Ok(())
    }

    /// A selected row's fill in row `r`, at `a`: the selection tint 2 DIPs
    /// in from its neighbours, and the accent pill at its left, so the
    /// selection never rests on the tint alone.
    fn selection_fill(&self, r: &Rect, a: f32, f: &FluentPalette) {
        let fill = Rect::new(r.x, r.y + ROW_INSET, r.w, r.h - 2.0 * ROW_INSET);
        let t = f.selection;
        self.fill(&fill, radius::CONTROL, t.rgb, t.alpha * a);
        let (pw, ph) = space::SELECTION_PILL;
        let pill = Rect::new(fill.x, r.y + (r.h - ph) / 2.0, pw, ph);
        self.fill(&pill, pw / 2.0, f.accent, a);
    }

    /// A file's picture in `dst` (mock `.thumb`): Windows' thumbnail
    /// cropped to fill, fading in over the card-tinted tile with the
    /// file's kind glyph; the shell's own icon for its type centred on the
    /// tile where it has no thumbnail.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn fl_preview(
        &self,
        dst: &Rect,
        corner: f32,
        picture: Option<(Arc<Pixels>, u64, f32)>,
        kind: Option<Kind>,
        glyph_size: f32,
        a: f32,
        f: &FluentPalette,
    ) {
        let tile = |alpha: f32| {
            let t = f.card.mica;
            self.fill(dst, corner, t.rgb, t.alpha * alpha);
            let s = f.card_stroke;
            self.border(dst, corner, s.rgb, s.alpha * alpha, space::STROKE);
        };
        // A video shows its frame, or its kind's tile until the frame
        // arrives: not the icon of the program that opens it, which the
        // frame would then replace.
        let picture = picture.filter(|(px, ..)| !(px.icon && kind == Some(Kind::Video)));
        match picture {
            Some((px, id, fade)) if px.icon => {
                // The shell's icon, at most the size it was made, centred.
                tile(a);
                let side = (dst.w.min(dst.h) * ICON_SHARE).min(px.width as f32);
                let at = Rect::new(
                    dst.x + (dst.w - side) / 2.0,
                    dst.y + (dst.h - side) / 2.0,
                    side,
                    side,
                );
                if let Some(brush) = self.picture(&px, id) {
                    let k = side / px.width.max(1) as f32;
                    // SAFETY: plain calls on this thread's brush and target,
                    // inside the frame's BeginDraw.
                    unsafe {
                        brush.SetTransform(&Matrix3x2 {
                            M11: k,
                            M12: 0.0,
                            M21: 0.0,
                            M22: k,
                            M31: at.x,
                            M32: at.y,
                        });
                        brush.SetOpacity(a * fade.clamp(0.0, 1.0));
                        self.rt.FillRectangle(&rectf(&at), &brush);
                    }
                }
            }
            picture => {
                let fade = picture.as_ref().map_or(0.0, |(_, _, x)| x.clamp(0.0, 1.0));
                if fade < 1.0 {
                    tile(a * (1.0 - fade));
                    let ink = f.kind(FileKind::of(kind));
                    self.glyph(kind_glyph(kind), dst, glyph_size, ink);
                }
                if let Some((px, id, fade)) = picture {
                    if let Some(brush) = self.picture(&px, id) {
                        let (sx, sy, sw, sh) = cover_source((px.width, px.height), (dst.w, dst.h));
                        let (kx, ky) = (dst.w / sw, dst.h / sh);
                        // SAFETY: plain calls on this thread's brush and
                        // target, inside the frame's BeginDraw.
                        unsafe {
                            brush.SetTransform(&Matrix3x2 {
                                M11: kx,
                                M12: 0.0,
                                M21: 0.0,
                                M22: ky,
                                M31: dst.x - sx * kx,
                                M32: dst.y - sy * ky,
                            });
                            brush.SetOpacity(a * fade.clamp(0.0, 1.0));
                            self.rt.FillRoundedRectangle(&rounded(dst, corner), &brush);
                        }
                    }
                }
            }
        }
    }

    /// What row `i` shows (mock screens 2, 5 and 8b): its thumbnail, its
    /// name and its line, a running row's progress bar, and at its right
    /// end its percentage and Cancel, its checkmark or error glyph, and
    /// its buttons.
    pub(super) fn row_content(
        &self,
        r: &Rect,
        e: &Entry,
        i: usize,
        anim: RowAnim,
        v: &View,
    ) -> Result<()> {
        let base = v.title.palette;
        let a = anim.appear;
        let hovered = v.hover_row == Some(i);
        let selected = v.selected.contains(&i);
        let picture = (v.preview)(i, PreviewSize::Row);
        self.fl_preview(
            &layout::row_preview(r),
            layout::PREVIEW_RADIUS,
            picture,
            (v.kind)(i),
            ROW_GLYPH,
            a,
            base,
        );
        // Under a contrast theme a selected row is Highlight: everything on
        // it is HighlightText, as Windows draws a selected list item.
        let on_highlight = (base.high_contrast && selected).then(|| on_highlight(base));
        let f = on_highlight.as_ref().unwrap_or(base);

        let saving = v.saving.contains(&e.path);
        let running = matches!(e.job, JobState::Running { .. });
        let (x, (end, _), link) = row_text_span(r, e, saving, v.row_links);
        let bar = running && !saving;
        let name = layout::row_name(r, bar);
        let name = Rect::new(x, name.y, (end - x).max(0.0), name.h);
        // A selected row's name is in Body Strong (mock `.row.sel .nm`).
        let ramp = if selected {
            Ramp::BodyStrong
        } else {
            Ramp::Body
        };
        self.styled_at(&e.name, Style::new(ramp), &name, f.text, a)?;

        let note = v.notes.get(&e.path);
        let (mut line, tone) = if saving {
            let line = v.saving_lines.get(&e.path).map(String::as_str);
            (line.unwrap_or(model::SAVING).to_owned(), Tone::Muted)
        } else {
            note.cloned().unwrap_or_else(|| e.line())
        };
        let c = line_ink(&tone, f);
        // A result settles into place and a problem cracks over the settle
        // fade.
        let la = match tone {
            Tone::Muted => a,
            Tone::Result | Tone::Crack => a * anim.settle,
        };
        let at = layout::row_line_at(r, bar);
        let line_rect = Rect::new(x, at.y, (end - x).max(0.0), at.h);
        let caption = Style::new(Ramp::Caption);
        // What the row says for itself, when nothing stands in for it: a
        // saved result too long for a narrow list says itself shortly,
        // and a problem's size after it is not in the problem's colour.
        let whole = line.clone();
        let mut tail = None;
        if !saving && note.is_none() {
            match e.short_line() {
                Some(short) => {
                    if self.run(&line, caption)? > line_rect.w {
                        line = short;
                    }
                }
                None if tone == Tone::Crack => {
                    let (head, rest) = e.line_parts();
                    if rest.is_some() {
                        line = head;
                        tail = rest;
                    }
                }
                None => {}
            }
        }
        // When facts arrive, the line they replace fades out in place as
        // the new one fades in.
        let mut la = la;
        if anim.facts < 1.0 && matches!(e.job, JobState::Idle) {
            let before = e.plain_detail();
            if before != whole {
                let fade = a * (1.0 - anim.facts);
                self.styled_at(&before, caption, &line_rect, f.text_secondary, fade)?;
                la *= anim.facts;
            }
        }
        let said = self.styled_at(&line, caption, &line_rect, c, la)?;
        if let Some(rest) = tail {
            let room = Rect::new(
                line_rect.x + said,
                line_rect.y,
                (line_rect.w - said).max(0.0),
                line_rect.h,
            );
            self.styled_at(&rest, caption, &room, f.text_secondary, la)?;
        }

        // What sits at the right end, right to left: Cancel and the
        // percentage, or the buttons, then the checkmark or error glyph
        // just before the first of them (mock screens 5 and 8b).
        let options = e.options_for().is_some() && !saving && v.row_links;
        let retry = e.retryable() && !saving && v.row_links;
        let mut left = layout::row_right_start(r, running, retry, link, options);
        if let RowMark::Progress(fraction) = e.mark() {
            if bar {
                let span = match fraction {
                    Some(fr) => (0.0, fr.clamp(0.0, 1.0)),
                    None => anim::indeterminate(
                        v.progress_clock.unwrap_or(0.0),
                        v.progress_clock.is_some(),
                    ),
                };
                // On Highlight the bar's track and fill are HighlightText.
                let bar_ink = on_highlight.map(|p| FluentPalette {
                    accent: p.on_selection,
                    strong_stroke: Tint::solid(p.on_selection),
                    ..p
                });
                self.progress_bar(
                    &layout::row_bar(r, end),
                    span,
                    bar_ink.as_ref().unwrap_or(f),
                );
            }
            if let Some(fr) = fraction {
                let pct = layout::row_percent(r);
                let style = Style::new(Ramp::Caption).trailing();
                self.styled_at(&model::percent_text(fr), style, &pct, f.text_secondary, a)?;
            }
            let hot = hovered && v.hover_cancel;
            slot_button(self, &layout::row_cancel(r), CANCEL, None, lit(hot), f)?;
        }
        if options {
            let slot = layout::row_options(r);
            let hot = hovered && v.hover_options;
            left = slot_button(self, &slot, "Options", None, lit(hot), f)?;
        }
        if let Some(text) = link {
            let slot = layout::row_link_at(r, text, options);
            let g = (text == model::SHOW_IN_FOLDER).then_some(glyph::SHOW_IN_FOLDER);
            left = slot_button(self, &slot, text, g, lit(hovered && v.hover_link), f)?;
        }
        if retry {
            let slot = layout::row_retry(r, link);
            let hot = hovered && v.hover_retry;
            left = slot_button(self, &slot, "Retry", Some(glyph::RETRY), lit(hot), f)?;
        }
        let m = layout::row_mark(r, left);
        match e.mark() {
            RowMark::Done => {
                let ink = if f.high_contrast { f.text } else { f.success };
                self.glyph(glyph::CHECKMARK, &m, MARK_GLYPH, ink);
            }
            RowMark::Failed => self.glyph(glyph::ERROR, &m, MARK_GLYPH, f.critical),
            RowMark::Progress(_) | RowMark::None => {}
        }
        Ok(())
    }

    /// The credit at the window's bottom left, where `v` places it: its
    /// words quiet in Caption and the secondary text colour, the heart in
    /// colour, and the name a link that brightens and is underlined while
    /// pointed at or focused.
    pub(super) fn credit(&self, v: &View, f: &FluentPalette) -> Result<()> {
        use crate::credit;
        let Some(c) = v.credit else {
            return Ok(());
        };
        let caption = Style::new(Ramp::Caption);
        let quiet = f.text_secondary;
        self.styled(credit::MADE, caption, &c.made, quiet)?;
        self.heart(&c.heart, f)?;
        self.styled(credit::BY, caption, &c.by, quiet)?;
        let (hover, pressed) = (v.button)(ButtonId::Window(FocusButton::Credit));
        let focused = v.button_ring == Some(c.name);
        let lit = if focused { 1.0 } else { hover.clamp(0.0, 1.0) };
        let ink = if pressed > 0.0 {
            crate::theme::mix(f.text, quiet, pressed.clamp(0.0, 1.0) * 0.5)
        } else {
            crate::theme::mix(quiet, f.text, lit)
        };
        let w = self.styled(credit::NAME, caption, &c.name, ink)?;
        if lit > 0.0 || pressed > 0.0 {
            // Under the words' baseline, as a HyperlinkButton's.
            let y = c.name.y + c.name.h / 2.0 + 6.0;
            self.hline(c.name.x, c.name.x + w, y, ink, 1.0);
        }
        Ok(())
    }

    /// The credit's heart in `r`: the red heart emoji in colour, or where
    /// Windows can't draw it in colour, a plain heart in red. In a high
    /// contrast theme, the plain heart in the theme's text colour.
    fn heart(&self, r: &Rect, f: &FluentPalette) -> Result<()> {
        use windows::core::Interface;
        use windows::Win32::Graphics::Direct2D::ID2D1DeviceContext;
        let colour = self.rt.cast::<ID2D1DeviceContext>().is_ok();
        let laid = if f.high_contrast || !colour {
            None
        } else {
            self.text.heart(self.zoom)?
        };
        match laid {
            // SAFETY: a plain draw on this thread's render target, a
            // device context, in a BeginDraw, of a layout that outlives
            // the call.
            // Segoe UI Emoji's heart rides high on Caption's line: down a
            // little, to sit on the words' baseline as they do.
            Some(laid) => unsafe {
                self.rt.DrawTextLayout(
                    pt(r.x, r.y + HEART_DROP),
                    &laid.layout,
                    self.paint(f.text, 1.0),
                    D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                );
            },
            None => {
                let ink = if f.high_contrast { f.text } else { HEART_RED };
                // In the middle of the room the emoji would take.
                let caption = Style::new(Ramp::Caption).centered();
                self.styled(crate::credit::HEART_PLAIN, caption, r, ink)?;
            }
        }
        Ok(())
    }

    /// The footer (mock screen 2): the status in Caption at the left, with
    /// Undo after it while rows can come back, then Add files, Clear list,
    /// and Convert in the accent.
    pub(super) fn footer(&self, l: &Layout, v: &View) -> Result<()> {
        let f = v.title.palette;
        let Some(footer) = l.footer else {
            return Ok(());
        };
        let row = v
            .footer_row
            .unwrap_or_else(|| layout::footer_row(&footer, false));
        let status = row.status;
        // The idle hint comes in two lengths: the full one where it fits.
        let caption = Style::new(Ramp::Caption);
        let text = if v.status == ways_in::IDLE_HINT {
            let full = self.run(ways_in::IDLE_HINT, caption)?;
            ways_in::fit_hint(status.w, full)
        } else {
            v.status
        };
        let undo = v.undo.then(|| layout::undo_link(&status)).flatten();
        let words = match undo {
            Some(u) => Rect::new(status.x, status.y, (u.x - status.x).max(0.0), status.h),
            None => status,
        };
        self.styled(text, caption, &words, f.text_secondary)?;
        let lit_of = |b: FocusButton| {
            let (hover, pressed) = (v.button)(ButtonId::Window(b));
            Lit {
                hover,
                pressed,
                disabled: false,
            }
        };
        if let Some(u) = undo {
            self.fl_button(ButtonKind::Hyperlink, &u, "Undo", None, Lit::REST, f)?;
        }
        let add = lit_of(FocusButton::AddFiles);
        if row.add_glyph_only {
            // Its words are its name and tooltip.
            self.fl_button(
                ButtonKind::Standard,
                &row.add,
                "",
                Some(glyph::ADD_FILES),
                add,
                f,
            )?;
        } else {
            self.fl_button(
                ButtonKind::Standard,
                &row.add,
                v.empty.button,
                Some(glyph::ADD_FILES),
                add,
                f,
            )?;
        }

        let lit = lit_of(FocusButton::Clear);
        self.fl_button(
            ButtonKind::Standard,
            &row.clear,
            model::CLEAR_LIST,
            None,
            lit,
            f,
        )?;
        // The one accent on the surface; disabled while nothing selected
        // can be converted.
        let lit = Lit {
            disabled: !v.convert_enabled,
            ..lit_of(FocusButton::Convert)
        };
        self.fl_button(
            ButtonKind::Accent,
            &row.convert,
            v.convert_label,
            None,
            lit,
            f,
        )?;
        Ok(())
    }
}

/// The plain heart's red, where there is no colour emoji: the red of
/// Segoe UI Emoji's heart.
const HEART_RED: crate::theme::Rgb = crate::theme::Rgb(0xF4, 0x43, 0x36);

/// How far down the colour heart is drawn from its line's top, in DIPs.
const HEART_DROP: f32 = 2.0;
