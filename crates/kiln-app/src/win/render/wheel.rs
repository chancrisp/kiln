//! The format wheel's drawing (look spec, mock screens 3, 7 and 7b): one
//! flat ring of `overlay` segments with each format's code, the kind as a
//! 2-DIP arc inside the outer edge, the lit segment solid accent with
//! `on_accent` ink, a hub with the file's picture and two lines, and
//! More's list beside the ring. The same drawing serves Kiln's window and
//! the drag-wheel's floating one (`draw_wheel_on`).

use super::*;
use crate::theme::fluent::{radius, FileKind, FluentPalette, Shadow, Tint};
use crate::wheel::{self, FlyoutRow, SegmentShape};
use crate::win::text::{Ramp, Style};

/// How much of the dialog smoke shows behind a wheel over Kiln's window
/// (mock `.smoke.light20`).
pub(super) const WHEEL_SMOKE: f32 = 0.20 / 0.30;

/// The hub's picture, its corner, and where the hub's parts stack, about
/// the wheel's centre: the picture, then the pair and the hint under it.
///
/// The picture is 32 DIPs and the stack (picture, 4 DIPs, the 20-DIP pair,
/// the 16-DIP hint) is 72 tall, centred about the hub: it runs from -36 to
/// 36, where the 104-DIP disc is still about 75 across.
pub(super) const HUB_THUMB: f32 = 32.0;
pub(super) const HUB_TOP: f32 = -36.0;
pub(super) const HUB_PAIR: Rect = Rect {
    x: -50.0,
    y: 0.0,
    w: 100.0,
    h: 20.0,
};
pub(super) const HUB_HINT: Rect = Rect {
    x: -50.0,
    y: 20.0,
    w: 100.0,
    h: 16.0,
};

/// A glyph and its caption on a segment: the glyph this far above the
/// label point and the caption this far below.
pub(super) const GLYPH_RISE: f32 = 9.0;
pub(super) const GLYPH: f32 = 16.0;

/// More's list: the rows' padding at either end, as the side pane's rows
/// have it.
pub(super) const FLYOUT_INDENT: f32 = crate::layout::OFFER_PAD;

/// How many layers a soft shadow is drawn in.
const SHADOW_LAYERS: usize = 16;

/// The most segment shapes kept made at once: past this many the cache
/// starts afresh.
pub(super) const SEGMENT_CACHE: usize = 64;

/// The hub's picture, where the window has one for the file.
pub(super) fn hub_thumb_rect() -> Rect {
    Rect::new(-HUB_THUMB / 2.0, HUB_TOP, HUB_THUMB, HUB_THUMB)
}

/// One segment's shapes, made once and kept while the wheel is drawn: its
/// outline and its kind's arc. The opening's scale is a transform, so a
/// segment's shapes never change.
pub(super) struct SegmentGeometry {
    pub(super) body: ID2D1PathGeometry,
    pub(super) arc: ID2D1PathGeometry,
}

/// A segment's cache key: its angles and radii, bit for bit, and whether
/// it is a full ring.
pub(super) fn segment_key(t: &SegmentShape) -> [u32; 5] {
    [
        t.a0.to_bits(),
        t.a1.to_bits(),
        t.r0.to_bits(),
        t.r1.to_bits(),
        u32::from(t.full),
    ]
}

/// What the wheel draws with on one render target: its brush, its text,
/// and the segments' shapes, made with the target's own factory. Made
/// again with the target: when the device is lost, the DPI changes or the
/// theme does.
pub struct WheelResources {
    factory: ID2D1Factory,
    brush: ID2D1SolidColorBrush,
    text: Rc<Text>,
    round: ID2D1StrokeStyle,
    /// Each segment's shapes, by `segment_key`, made with `factory`.
    segments: RefCell<HashMap<[u32; 5], Rc<SegmentGeometry>>>,
}

impl WheelResources {
    pub fn new(rt: &ID2D1RenderTarget, dwrite: &IDWriteFactory) -> Result<Self> {
        // Geometries must come from the factory the target was made with.
        let factory = unsafe { rt.GetFactory()? };
        // Made on a new device or at a new scale: the text layouts its
        // wheel kept start again.
        let text = Text::shared(dwrite)?;
        text.clear_caches();
        Ok(Self {
            brush: unsafe { rt.CreateSolidColorBrush(&color(Rgb(0, 0, 0), 1.0), None)? },
            text,
            round: round_stroke(&factory)?,
            segments: RefCell::new(HashMap::new()),
            factory,
        })
    }

    /// How many segment shapes are kept made.
    #[cfg(test)]
    pub(super) fn segments_made(&self) -> usize {
        self.segments.borrow().len()
    }
}

/// Draw the wheel onto `rt`, which is between `BeginDraw` and `EndDraw`,
/// with `res` made for it. `dpi` is the target's: it is set on `rt` if
/// the target has another. `zoom` is the Text size's, for the type's
/// optical sizes. With no extras the wheel looks exactly as it does over
/// Kiln's window; the extras are the floating wheel's.
pub fn draw_wheel_on(
    rt: &ID2D1RenderTarget,
    res: &mut WheelResources,
    wv: &WheelView,
    extras: &WheelExtras,
    p: &FluentPalette,
    dpi: f32,
    zoom: f32,
) -> Result<()> {
    let (mut x, mut y) = (0.0, 0.0);
    unsafe {
        rt.GetDpi(&mut x, &mut y);
        if (x - dpi).abs() > 1e-3 || (y - dpi).abs() > 1e-3 {
            rt.SetDpi(dpi, dpi);
        }
    }
    let d = WheelDraw {
        rt,
        res,
        extras,
        base: Cell::new(Matrix3x2::identity()),
        dpi,
        zoom,
    };
    d.wheel(wv, p)
}

/// The wheel's drawing, onto any render target.
pub(super) struct WheelDraw<'a> {
    pub(super) rt: &'a ID2D1RenderTarget,
    pub(super) res: &'a WheelResources,
    pub(super) extras: &'a WheelExtras<'a>,
    pub(super) base: Cell<Matrix3x2>,
    pub(super) dpi: f32,
    pub(super) zoom: f32,
}

impl Ink for WheelDraw<'_> {
    fn rt(&self) -> &ID2D1RenderTarget {
        self.rt
    }
    fn brush(&self) -> &ID2D1SolidColorBrush {
        &self.res.brush
    }
    fn base(&self) -> &Cell<Matrix3x2> {
        &self.base
    }
}

/// Where the wheel's own drawing goes, about its centre: scaled by its
/// opening and by any shrink a small window asked for.
pub(super) fn wheel_base(wv: &WheelView) -> Matrix3x2 {
    let w = wv.wheel;
    scale_at(wv.grow * w.scale, w.center.0, w.center.1)
}

impl WheelDraw<'_> {
    /// The wheel, drawn in its own coordinates around its centre, then
    /// More's list in the window's.
    pub(super) fn wheel(&self, wv: &WheelView, p: &FluentPalette) -> Result<()> {
        let opacity = wv.opacity.clamp(0.0, 1.0);
        if opacity <= 0.0 {
            return Ok(());
        }
        let result = self.with_opacity(opacity, || {
            // The list alone has no ring.
            if !wv.wheel.list_only {
                self.set_base(wheel_base(wv));
                self.disc_shadow(R_OUTER, p.flyout_shadow);
                self.ring(wv, p)?;
                self.hub(wv, p)?;
                if let Some(s) = self.extras.caption {
                    self.caption_pill(s, p)?;
                }
            }
            self.set_base(Matrix3x2::identity());
            if let Some(f) = &wv.flyout {
                self.flyout(wv, f, p)?;
            }
            Ok(())
        });
        self.set_base(Matrix3x2::identity());
        result
    }

    /// Whether ring segment `i` is drawn lit: under the pointer or the
    /// keyboard, or More while its list is open.
    pub(super) fn lit(wv: &WheelView, i: usize) -> bool {
        wv.hover == Some(Spot::Inner(i)) || (wv.flyout.is_some() && wv.wheel.is_more(i))
    }

    /// The ring: each segment's face, its kind's arc, and its code or its
    /// glyph and caption.
    pub(super) fn ring(&self, wv: &WheelView, p: &FluentPalette) -> Result<()> {
        let w = wv.wheel;
        let n = w.segments();
        // The ground inside the ring is the base, out to the segments' inner
        // edge: the hub's gap is its colour, not whatever is behind the
        // wheel (a bright desktop made a heavy ring of it).
        self.circle(wheel::R_INNER + 1.0, p.base, 1.0);
        // The kinds' arcs say which segments are which kind: only when the
        // formats are of more than one kind, and never on the lit segment,
        // where an arc on a flat face looks like a flaw.
        let mixed = {
            let mut kinds = w.items.iter().filter_map(|it| it.kind);
            kinds.next().is_some_and(|first| kinds.any(|k| k != first))
        };
        // The rim: a fine edge round the whole ring, outside the segments.
        if !p.high_contrast {
            let t = p.overlay_stroke;
            self.ring_edge(R_OUTER + 0.5, t.rgb, t.alpha);
        }
        for i in 0..n {
            let shape = SegmentShape::of(i, n);
            let g = self.segment_shapes(&shape)?;
            let lit = Self::lit(wv, i);
            let (face, ink) = if lit {
                (p.accent, p.on_accent)
            } else {
                (p.overlay, p.text)
            };
            unsafe {
                self.rt.FillGeometry(&g.body, self.paint(face, 1.0), None);
                if p.high_contrast {
                    self.rt
                        .DrawGeometry(&g.body, self.paint(p.text, 1.0), space::STROKE, None);
                }
            }
            let item = w.items.get(i);
            // The kind's arc, for a format only, when the formats differ in
            // kind, and not on the lit face.
            if let Some(kind) = item.and_then(|it| it.kind).filter(|_| mixed && !lit) {
                unsafe {
                    self.rt.DrawGeometry(
                        &g.arc,
                        self.paint(p.kind(FileKind::of(Some(kind))), 1.0),
                        crate::theme::fluent::space::KIND_ARC,
                        &self.res.round,
                    );
                }
            }
            let (lx, ly) = wheel::label_point(i, n);
            match item {
                Some(it) if it.is_code => self.code_label(&it.label, lx, ly, ink)?,
                Some(it) => {
                    let (glyph, caption) = match it.action {
                        Action::Tool(kiln_core::Tool::Smaller, _) => (
                            crate::theme::fluent::glyph::MAKE_SMALLER,
                            wheel::MAKE_SMALLER_CAPTION,
                        ),
                        Action::Tool(t, _) => (wheel::glyph_for_tool(t), it.label.as_str()),
                        Action::Convert(_) => {
                            (crate::theme::fluent::glyph::MORE, it.label.as_str())
                        }
                    };
                    self.glyph_label(glyph, caption, lx, ly, ink, lit, p)?;
                }
                None => self.glyph_label(
                    crate::theme::fluent::glyph::MORE,
                    "More",
                    lx,
                    ly,
                    ink,
                    lit,
                    p,
                )?,
            }
        }
        Ok(())
    }

    /// A segment's format code in the code style (Body's 14 DIPs, Semibold,
    /// tracked: the same as every format code), centred on `(x, y)`.
    pub(super) fn code_label(&self, code: &str, x: f32, y: f32, ink: Rgb) -> Result<()> {
        let r = Rect::new(x - 40.0, y - 10.0, 80.0, 20.0);
        self.styled(code, Style::new(Ramp::Body).code().centered(), &r, ink, &[])
            .map(|_| ())
    }

    /// A segment's glyph over its caption, centred on `(x, y)`: Make
    /// smaller's and More's, and each tool's on the tools wheel, whose
    /// longer names wrap to two lines.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn glyph_label(
        &self,
        glyph: char,
        caption: &str,
        x: f32,
        y: f32,
        ink: Rgb,
        lit: bool,
        p: &FluentPalette,
    ) -> Result<()> {
        let g = Rect::new(x - GLYPH / 2.0, y - GLYPH_RISE - GLYPH / 2.0, GLYPH, GLYPH);
        self.glyph(glyph, &g, GLYPH, ink);
        let muted = if lit || p.high_contrast {
            ink
        } else {
            p.text_secondary
        };
        let long = caption.chars().count() > 9;
        let style = Style::new(Ramp::Caption).centered();
        if long {
            let r = Rect::new(x - 42.0, y + 1.0, 84.0, 32.0);
            self.styled(caption, style.wrapping(), &r, muted, &[])?;
        } else {
            let r = Rect::new(x - 42.0, y + GLYPH_RISE - 8.0, 84.0, 16.0);
            self.styled(caption, style, &r, muted, &[])?;
        }
        Ok(())
    }

    /// The hub: a disc of the base a gap inside the ring, the file's
    /// picture tile at its top (the window draws its thumbnail over it),
    /// then the pair and the hint.
    pub(super) fn hub(&self, wv: &WheelView, p: &FluentPalette) -> Result<()> {
        self.circle(wheel::R_HUB, p.base, 1.0);
        if p.high_contrast {
            self.ring_edge(wheel::R_HUB - 0.5, p.text, 1.0);
        }
        let tile = hub_thumb_rect();
        if wv.thumb.is_none() {
            let t = p.card.mica;
            self.fill_rr(&tile, radius::CONTROL, t.rgb, t.alpha);
            let s = p.card_stroke;
            self.stroke_rr(&tile, radius::CONTROL, s);
            let kind = wv.kind;
            self.glyph(
                super::list::kind_glyph(kind),
                &tile,
                20.0,
                p.kind(FileKind::of(kind)),
            );
        }
        let pair = Style::new(Ramp::BodyStrong).centered();
        self.styled(&wv.hub.pair, pair, &HUB_PAIR, p.text, &[])?;
        if !wv.hub.hint.is_empty() {
            let hint = Style::new(Ramp::Caption).centered();
            self.styled(&wv.hub.hint, hint, &HUB_HINT, p.text_secondary, &[])?;
        }
        Ok(())
    }

    /// More's list beside the ring: its frame, then each line inside it,
    /// faded in as `f.shown` says; the scroll indicator at rest where the
    /// lines overflow, and the footer row under a rule.
    pub(super) fn flyout(&self, wv: &WheelView, f: &FlyoutView, p: &FluentPalette) -> Result<()> {
        let shown = f.shown.clamp(0.0, 1.0);
        if shown <= 0.0 {
            return Ok(());
        }
        self.with_opacity(shown, || {
            let r = &f.place.rect;
            self.soft_shadow(r, radius::LAYER, p.flyout_shadow);
            self.fill_rr(r, radius::LAYER, p.overlay, 1.0);
            self.stroke_rr(r, radius::LAYER, p.overlay_stroke);
            let view = f.place.view();
            // SAFETY: a plain clip on this thread's target, popped below.
            unsafe {
                self.rt
                    .PushAxisAlignedClip(&rectf(&view), D2D1_ANTIALIAS_MODE_ALIASED);
            }
            let drawn = (|| -> Result<()> {
                for (row, at) in f.rows.iter().zip(&f.place.rows) {
                    let footer = matches!(row, FlyoutRow::Options(_));
                    if footer || at.bottom() < view.y || at.y > view.bottom() {
                        continue;
                    }
                    self.flyout_row(wv, row, at, f.lit, p)?;
                }
                Ok(())
            })();
            unsafe { self.rt.PopAxisAlignedClip() };
            drawn?;
            // The indicator shows at rest, so a list that goes on says so.
            let width = crate::theme::fluent::space::SCROLL_THUMB_REST;
            if let Some(thumb) = crate::layout::kit::scroll_thumb(
                view,
                view.h,
                f.place.content,
                f.place.scroll,
                width,
            ) {
                let t = p.strong_stroke;
                self.fill_rr(&thumb, width / 2.0, t.rgb, t.alpha);
            }
            if let Some(ft) = f.place.footer {
                let rule = Rect::new(ft.x + FLYOUT_INDENT, ft.y, ft.w - 2.0 * FLYOUT_INDENT, 1.0);
                let t = p.overlay_stroke;
                self.fill_rr(&rule, 0.0, t.rgb, t.alpha);
                for (row, at) in f.rows.iter().zip(&f.place.rows) {
                    if matches!(row, FlyoutRow::Options(_)) {
                        self.flyout_row(wv, row, at, f.lit, p)?;
                    }
                }
            }
            Ok(())
        })
    }

    /// One line of More's list in `r`: a heading, a format (its code in
    /// plain text and what it is, over two lines at most), a tool
    /// (its glyph, its name and what it does), or the footer row. The lit
    /// one has the hover's fill. The code and its words are where the side
    /// pane's rows have them (`layout::offer_words`).
    pub(super) fn flyout_row(
        &self,
        wv: &WheelView,
        row: &FlyoutRow,
        r: &Rect,
        lit: Option<Spot>,
        p: &FluentPalette,
    ) -> Result<()> {
        let on = wheel::row_spot(row).is_some_and(|s| lit == Some(s));
        let ink = if !on {
            p.text
        } else if p.high_contrast {
            self.fill_rr(r, radius::CONTROL, p.accent, 1.0);
            p.on_accent
        } else {
            let t = p.control_hover;
            self.fill_rr(r, radius::CONTROL, t.rgb, t.alpha);
            p.text
        };
        let muted = if on && p.high_contrast {
            ink
        } else {
            p.text_secondary
        };
        let x = r.x + layout::OFFER_PAD;
        let code_column = layout::offer_code_column(r);
        let words = layout::offer_words(r, 0.0);
        match row {
            FlyoutRow::Heading(title) => {
                let h = Rect::new(x, r.y + 10.0, r.w - 2.0 * layout::OFFER_PAD, 16.0);
                // The whole heading in Body Strong's weight, as the mock's.
                let all = 0..title.len();
                let strong = std::slice::from_ref(&all);
                self.styled(
                    title,
                    Style::new(Ramp::Caption),
                    &h,
                    p.text_secondary,
                    strong,
                )?;
            }
            FlyoutRow::Item(_, item) => match &item.action {
                Action::Convert(ext) => {
                    // The code is plain text, as the side pane's is.
                    self.styled(
                        &item.label,
                        Style::new(Ramp::Body).code(),
                        &code_column,
                        ink,
                        &[],
                    )?;
                    if let Some(what) = wheel::format_words(ext) {
                        let style = Style::new(Ramp::Caption).wrapping();
                        let h = self.res.text.styled_height(what, style, self.zoom, words.w);
                        let at = layout::centred_block(&words, h);
                        self.styled(what, style, &at, muted, &[])?;
                    }
                }
                Action::Tool(tool, kind) => {
                    let g = Rect::new(x, r.y, GLYPH, r.h);
                    self.glyph(wheel::glyph_for_tool(*tool), &g, GLYPH, muted);
                    let name = Rect::new(words.x, r.y + 6.0, words.w, 20.0);
                    self.styled(&item.label, Style::new(Ramp::Body), &name, ink, &[])?;
                    let note = Rect::new(words.x, r.y + 26.0, words.w, 32.0);
                    let say = wheel::tool_note(*tool, *kind);
                    let style = Style::new(Ramp::Caption).wrapping();
                    self.styled(say, style, &note, muted, &[])?;
                }
            },
            FlyoutRow::Options(ext) => {
                let g = Rect::new(x, r.y, GLYPH, r.h);
                self.glyph(crate::theme::fluent::glyph::SETTINGS, &g, GLYPH, ink);
                let t = Rect::new(words.x, r.y, words.w, r.h);
                self.styled(
                    &wheel::options_line(ext),
                    Style::new(Ramp::Body),
                    &t,
                    ink,
                    &[],
                )?;
            }
        }
        let _ = wv;
        Ok(())
    }

    /// The floating wheel's caption: a pill of the overlay under the ring,
    /// edged, its text wrapped to `CAPTION_WIDTH` and centred.
    pub(super) fn caption_pill(&self, s: &str, p: &FluentPalette) -> Result<()> {
        let style = Style::new(Ramp::Body).centered().wrapping();
        let h = self
            .res
            .text
            .styled_height_runs(s, style, self.zoom, CAPTION_WIDTH, &[]);
        let w = self
            .res
            .text
            .styled(s, Style::new(Ramp::Body), self.zoom, CAPTION_WIDTH, 20.0)?
            .width
            .min(CAPTION_WIDTH)
            .ceil();
        let top = R_OUTER + CAPTION_GAP;
        let pill = Rect::new(
            -w / 2.0 - CAPTION_PAD_X,
            top,
            w + 2.0 * CAPTION_PAD_X,
            h.ceil() + 2.0 * CAPTION_PAD_Y,
        );
        let corner = (pill.h / 2.0).min(radius::LAYER);
        self.soft_shadow(&pill, corner, p.flyout_shadow);
        self.fill_rr(&pill, corner, p.overlay, 1.0);
        self.stroke_rr(&pill, corner, p.overlay_stroke);
        let t = Rect::new(
            -CAPTION_WIDTH / 2.0,
            top + CAPTION_PAD_Y,
            CAPTION_WIDTH,
            h.ceil(),
        );
        self.styled(s, style, &t, p.text, &[])?;
        Ok(())
    }

    /// `s` in `style` in `r`, in `c`, with `strong` in Body Strong's
    /// weight, clipped to `r`.
    pub(super) fn styled(
        &self,
        s: &str,
        style: Style,
        r: &Rect,
        c: Rgb,
        strong: &[std::ops::Range<usize>],
    ) -> Result<f32> {
        let laid =
            self.res
                .text
                .styled_runs(s, style, self.zoom, r.w.max(0.0), r.h.max(0.0), strong)?;
        // SAFETY: a plain draw on this thread's render target, in a
        // BeginDraw, of a layout that outlives the call.
        unsafe {
            self.rt.DrawTextLayout(
                pt(r.x, r.y),
                &laid.layout,
                self.paint(c, 1.0),
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
            );
        }
        Ok(laid.width)
    }

    /// Glyph `code`, `size` DIPs, in the middle of `r` in `c`, its box on
    /// whole pixels where nothing scales it.
    pub(super) fn glyph(&self, code: char, r: &Rect, size: f32, c: Rgb) {
        let Ok(laid) = self.res.text.glyph(code, size) else {
            return;
        };
        let (x, y) = snapped(glyph_origin(r, size), &self.base.get(), self.dpi / 96.0);
        // SAFETY: a plain draw on this thread's render target, in a
        // BeginDraw, of a layout that outlives the call.
        unsafe {
            self.rt.DrawTextLayout(
                pt(x, y),
                &laid.layout,
                self.paint(c, 1.0),
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
        }
    }

    pub(super) fn fill_rr(&self, r: &Rect, corner: f32, c: Rgb, a: f32) {
        if a <= 0.0 {
            return;
        }
        unsafe {
            self.rt
                .FillRoundedRectangle(&rounded(r, corner), self.paint(c, a));
        }
    }

    /// A 1-DIP stroke inside `r` in `t`.
    pub(super) fn stroke_rr(&self, r: &Rect, corner: f32, t: Tint) {
        if t.alpha <= 0.0 {
            return;
        }
        let edge = r.inset(space::STROKE / 2.0);
        unsafe {
            self.rt.DrawRoundedRectangle(
                &rounded(&edge, (corner - space::STROKE / 2.0).max(0.0)),
                self.paint(t.rgb, t.alpha),
                space::STROKE,
                None,
            );
        }
    }

    /// A soft shadow under `r` (CSS `0 y blur`) for each of `shadows`:
    /// layers grown from `blur` inside to `blur` outside, each a share of
    /// the alpha, so the edge fades as a blur would. A contrast theme has
    /// none.
    pub(super) fn soft_shadow(&self, r: &Rect, corner: f32, shadows: &[Shadow]) {
        for s in shadows {
            let a = 1.0 - (1.0 - s.alpha).powf(1.0 / SHADOW_LAYERS as f32);
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
                self.fill_rr(&layer, c, s.rgb, a);
            }
        }
    }

    /// The ring's shadow: `shadows` as discs of radius `r` round its
    /// centre, layered as `soft_shadow` layers a box.
    pub(super) fn disc_shadow(&self, r: f32, shadows: &[Shadow]) {
        for s in shadows {
            let a = 1.0 - (1.0 - s.alpha).powf(1.0 / SHADOW_LAYERS as f32);
            for j in 0..SHADOW_LAYERS {
                let grow = s.blur * (1.0 - 2.0 * (j as f32 + 0.5) / SHADOW_LAYERS as f32) / 2.0;
                let radius = r + grow;
                if radius <= 0.0 {
                    continue;
                }
                unsafe {
                    self.rt.FillEllipse(
                        &D2D1_ELLIPSE {
                            point: pt(0.0, s.y),
                            radiusX: radius,
                            radiusY: radius,
                        },
                        self.paint(s.rgb, a),
                    );
                }
            }
        }
    }

    pub(super) fn circle(&self, r: f32, c: Rgb, a: f32) {
        unsafe {
            self.rt.FillEllipse(
                &D2D1_ELLIPSE {
                    point: pt(0.0, 0.0),
                    radiusX: r,
                    radiusY: r,
                },
                self.paint(c, a),
            );
        }
    }

    pub(super) fn ring_edge(&self, r: f32, c: Rgb, a: f32) {
        unsafe {
            self.rt.DrawEllipse(
                &D2D1_ELLIPSE {
                    point: pt(0.0, 0.0),
                    radiusX: r,
                    radiusY: r,
                },
                self.paint(c, a),
                space::STROKE,
                None,
            );
        }
    }

    /// `t`'s shapes: made the first time it is drawn, then kept.
    pub(super) fn segment_shapes(&self, t: &SegmentShape) -> Result<Rc<SegmentGeometry>> {
        let key = segment_key(t);
        if let Some(g) = self.res.segments.borrow().get(&key) {
            return Ok(g.clone());
        }
        let (a0, a1, r) = t.kind_arc();
        let made = Rc::new(SegmentGeometry {
            body: self.segment_geometry(t)?,
            arc: self.arc(a0, a1, r, t.full)?,
        });
        let mut segments = self.res.segments.borrow_mut();
        if segments.len() >= SEGMENT_CACHE {
            segments.clear();
        }
        segments.insert(key, made.clone());
        Ok(made)
    }

    /// A segment's outline as a path: the outer arc between its corners,
    /// the corner round onto the `a1` side, down that side, round onto the
    /// inner arc, back along it, and up the `a0` side the same way. A full
    /// ring is the whole annulus.
    pub(super) fn segment_geometry(&self, t: &SegmentShape) -> Result<ID2D1PathGeometry> {
        if t.full {
            return self.annulus(t.r0, t.r1);
        }
        let SegmentShape {
            a0, a1, r0, r1, rc, ..
        } = *t;
        let (o, i) = (t.inset(r1 - rc), t.inset(r0 + rc));
        let at = |r: f32, a: f32| pt(r * a.cos(), r * a.sin());
        let normal = |a: f32| (-a.sin(), a.cos());
        // Where a corner's circle meets its side: from its centre, `rc`
        // back toward the side's own line.
        let foot = |c: (f32, f32), a: f32, toward: f32| {
            let (nx, ny) = normal(a);
            pt(c.0 - toward * rc * nx, c.1 - toward * rc * ny)
        };
        let (out0, out1) = (t.corner(a0, 1.0, r1 - rc), t.corner(a1, -1.0, r1 - rc));
        let (in0, in1) = (t.corner(a0, 1.0, r0 + rc), t.corner(a1, -1.0, r0 + rc));
        unsafe {
            let g = self.res.factory.CreatePathGeometry()?;
            let sink = g.Open()?;
            let corner = |to: Vector2| {
                if rc > 0.01 {
                    sink.AddArc(&D2D1_ARC_SEGMENT {
                        point: to,
                        size: D2D_SIZE_F {
                            width: rc,
                            height: rc,
                        },
                        rotationAngle: 0.0,
                        sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                        arcSize: D2D1_ARC_SIZE_SMALL,
                    });
                } else {
                    sink.AddLine(to);
                }
            };
            sink.BeginFigure(at(r1, a0 + o), D2D1_FIGURE_BEGIN_FILLED);
            arc_to(&sink, r1, a0 + o, a1 - o);
            corner(foot(out1, a1, -1.0));
            sink.AddLine(foot(in1, a1, -1.0));
            corner(at(r0, a1 - i));
            arc_to(&sink, r0, a1 - i, a0 + i);
            corner(foot(in0, a0, 1.0));
            sink.AddLine(foot(out0, a0, 1.0));
            corner(at(r1, a0 + o));
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close()?;
            Ok(g)
        }
    }

    /// The ring from `r0` to `r1`, whole.
    pub(super) fn annulus(&self, r0: f32, r1: f32) -> Result<ID2D1PathGeometry> {
        unsafe {
            let g = self.res.factory.CreatePathGeometry()?;
            let sink = g.Open()?;
            sink.SetFillMode(D2D1_FILL_MODE_ALTERNATE);
            for r in [r1, r0] {
                sink.BeginFigure(pt(r, 0.0), D2D1_FIGURE_BEGIN_FILLED);
                arc_to(&sink, r, 0.0, TAU);
                sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            }
            sink.Close()?;
            Ok(g)
        }
    }

    /// An open arc at radius `r`, for a segment's kind.
    pub(super) fn arc(&self, a0: f32, a1: f32, r: f32, full: bool) -> Result<ID2D1PathGeometry> {
        let (a0, a1) = if full { (0.0, TAU) } else { (a0, a1) };
        unsafe {
            let g = self.res.factory.CreatePathGeometry()?;
            let sink = g.Open()?;
            sink.BeginFigure(pt(r * a0.cos(), r * a0.sin()), D2D1_FIGURE_BEGIN_HOLLOW);
            arc_to(&sink, r, a0, a1);
            sink.EndFigure(if full {
                D2D1_FIGURE_END_CLOSED
            } else {
                D2D1_FIGURE_END_OPEN
            });
            sink.Close()?;
            Ok(g)
        }
    }
}

/// The caption pill's distance below the ring, and the room round its
/// text.
pub(super) const CAPTION_GAP: f32 = 16.0;
pub(super) const CAPTION_PAD_X: f32 = 12.0;
pub(super) const CAPTION_PAD_Y: f32 = 6.0;

impl Draw<'_> {
    /// The wheel over the window: the smoke behind it, the wheel, and the
    /// file's thumbnail in the hub where the window has one.
    pub(super) fn wheel(&self, wv: &WheelView, p: &FluentPalette, w: f32, h: f32) -> Result<()> {
        if wv.smoke {
            self.smoke(w, h, wv.opacity * WHEEL_SMOKE, p);
        }
        let mut res = self.wheel.borrow_mut();
        let result = draw_wheel_on(
            self.rt,
            &mut res,
            wv,
            &WheelExtras::default(),
            p,
            self.dpi,
            self.zoom,
        );
        drop(res);
        result?;
        // The list alone has no hub, so no picture in it either.
        let thumb = wv.thumb.clone().filter(|_| !wv.wheel.list_only);
        if let Some(thumb) = thumb {
            let a = wv.opacity.clamp(0.0, 1.0);
            self.set_base(wheel_base(wv));
            self.fl_preview(
                &hub_thumb_rect(),
                radius::CONTROL,
                Some(thumb),
                wv.kind,
                20.0,
                a,
                p,
            );
        }
        self.set_base(Matrix3x2::identity());
        Ok(())
    }
}

#[cfg(test)]
mod tests;
