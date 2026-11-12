use super::*;
use crate::win::text;

pub(super) fn color(c: Rgb, a: f32) -> D2D1_COLOR_F {
    let [r, g, b] = c.unit();
    D2D1_COLOR_F { r, g, b, a }
}

pub(super) fn rectf(r: &Rect) -> D2D_RECT_F {
    D2D_RECT_F {
        left: r.x,
        top: r.y,
        right: r.right(),
        bottom: r.bottom(),
    }
}

pub(super) fn rounded(r: &Rect, radius: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT {
        rect: rectf(r),
        radiusX: radius,
        radiusY: radius,
    }
}

pub(super) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

pub(super) fn pt(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

pub(super) fn scale_at(s: f32, x: f32, y: f32) -> Matrix3x2 {
    Matrix3x2 {
        M11: s,
        M12: 0.0,
        M21: 0.0,
        M22: s,
        M31: x,
        M32: y,
    }
}

/// Scaled by `s` about `(x, y)`, which stays where it is.
pub(super) fn scale_about(s: f32, x: f32, y: f32) -> Matrix3x2 {
    scale_at(s, x * (1.0 - s), y * (1.0 - s))
}

/// Everything moved by `(dx, dy)`.
pub(super) fn moved(dx: f32, dy: f32) -> Matrix3x2 {
    scale_at(1.0, dx, dy)
}

/// Turned `deg` degrees clockwise about the origin, scaled by `s`, then
/// moved to `(x, y)`.
pub(super) fn turn_scale_at(deg: f32, s: f32, x: f32, y: f32) -> Matrix3x2 {
    let (sin, cos) = deg.to_radians().sin_cos();
    Matrix3x2 {
        M11: s * cos,
        M12: s * sin,
        M21: -s * sin,
        M22: s * cos,
        M31: x,
        M32: y,
    }
}

/// `inner` applied first, then `outer`.
pub(super) fn compose(inner: &Matrix3x2, outer: &Matrix3x2) -> Matrix3x2 {
    Matrix3x2 {
        M11: inner.M11 * outer.M11 + inner.M12 * outer.M21,
        M12: inner.M11 * outer.M12 + inner.M12 * outer.M22,
        M21: inner.M21 * outer.M11 + inner.M22 * outer.M21,
        M22: inner.M21 * outer.M12 + inner.M22 * outer.M22,
        M31: inner.M31 * outer.M11 + inner.M32 * outer.M21 + outer.M31,
        M32: inner.M31 * outer.M12 + inner.M32 * outer.M22 + outer.M32,
    }
}

/// `r` under a transform that only scales and moves, as `m` is when it
/// has no turn in it.
pub(super) fn mapped(r: &Rect, m: &Matrix3x2) -> Rect {
    Rect::new(
        r.x * m.M11 + m.M31,
        r.y * m.M22 + m.M32,
        r.w * m.M11,
        r.h * m.M22,
    )
}

/// Where a `size`-DIP glyph's box starts, to sit in the middle of `r`.
pub(super) fn glyph_origin(r: &Rect, size: f32) -> (f32, f32) {
    (r.x + (r.w - size) / 2.0, r.y + (r.h - size) / 2.0)
}

/// Point `p` moved to the nearest whole pixel of a target `k` pixels a
/// DIP, drawn under `m`, so a glyph's strokes land as their designer drew
/// them on the pixel grid. Under a transform that turns, nothing lands on
/// whole pixels, and `p` is as it was.
pub(super) fn snapped(p: (f32, f32), m: &Matrix3x2, k: f32) -> (f32, f32) {
    let turned = m.M12 != 0.0 || m.M21 != 0.0;
    if turned || m.M11 == 0.0 || m.M22 == 0.0 || k.is_nan() || k <= 0.0 {
        return p;
    }
    let snap = |v: f32, scale: f32, shift: f32| ((v * scale + shift) * k).round() / k;
    (
        (snap(p.0, m.M11, m.M31) - m.M31) / m.M11,
        (snap(p.1, m.M22, m.M32) - m.M32) / m.M22,
    )
}

pub(super) struct Draw<'a> {
    pub(super) factory: &'a ID2D1Factory,
    pub(super) rt: &'a ID2D1RenderTarget,
    /// The target's DPI, which the wheel is drawn at.
    pub(super) dpi: f32,
    /// The Text size's zoom, already in `dpi`, for the type ramp's
    /// optical sizes.
    pub(super) zoom: f32,
    pub(super) brush: &'a ID2D1SolidColorBrush,
    /// What the wheel draws with on this target.
    pub(super) wheel: &'a RefCell<WheelResources>,
    pub(super) text: &'a Text,
    pub(super) round: &'a ID2D1StrokeStyle,
    pub(super) dash: &'a ID2D1StrokeStyle,
    /// Kiln's mark, as an outline.
    pub(super) mark: &'a ID2D1PathGeometry,
    /// The transform everything is currently drawn under.
    pub(super) base: Cell<Matrix3x2>,
    pub(super) pictures: &'a RefCell<HashMap<u64, Picture>>,
    /// This frame's number, stamped on each picture drawn.
    pub(super) frame: u64,
    /// The rounded clips made so far.
    pub(super) clips: &'a RefCell<cache::Clips>,
}

/// What every drawing surface draws with: a render target, one brush
/// recoloured for each use, the text formats and the transform
/// everything is under. The window's drawing and the wheel's share these
/// primitives.
pub(super) trait Ink {
    fn rt(&self) -> &ID2D1RenderTarget;
    fn brush(&self) -> &ID2D1SolidColorBrush;
    /// The transform everything is currently drawn under.
    fn base(&self) -> &Cell<Matrix3x2>;

    fn set_base(&self, m: Matrix3x2) {
        self.base().set(m);
        unsafe { self.rt().SetTransform(&m) };
    }

    /// Draw `f` at `opacity` as one layer, so overlapping parts don't
    /// show through each other.
    fn with_opacity<T>(&self, opacity: f32, f: impl FnOnce() -> Result<T>) -> Result<T> {
        if opacity >= 0.999 {
            return f();
        }
        let rt = self.rt();
        let everywhere = D2D_RECT_F {
            left: -1e6,
            top: -1e6,
            right: 1e6,
            bottom: 1e6,
        };
        cache::push_layer(rt, everywhere, None, opacity.max(0.0))?;
        let result = f();
        unsafe { rt.PopLayer() };
        result
    }

    fn paint(&self, c: Rgb, a: f32) -> &ID2D1SolidColorBrush {
        let brush = self.brush();
        unsafe { brush.SetColor(&color(c, a)) };
        brush
    }
}

impl Ink for Draw<'_> {
    fn rt(&self) -> &ID2D1RenderTarget {
        self.rt
    }
    fn brush(&self) -> &ID2D1SolidColorBrush {
        self.brush
    }
    fn base(&self) -> &Cell<Matrix3x2> {
        &self.base
    }
}

/// Continue a figure along a circle of radius `r` around the origin, from
/// angle `a0` (where the figure already is) to `a1`, either direction. Built
/// from cubic Béziers of at most a quarter turn each, which stay within a
/// fraction of a pixel of the true circle at wheel sizes.
pub(super) fn arc_to(sink: &ID2D1GeometrySink, r: f32, a0: f32, a1: f32) {
    let steps = ((a1 - a0).abs() / (PI / 2.0)).ceil().max(1.0) as usize;
    let span = (a1 - a0) / steps as f32;
    let k = 4.0 / 3.0 * (span / 4.0).tan();
    for i in 0..steps {
        let (s, e) = (a0 + span * i as f32, a0 + span * (i + 1) as f32);
        let (cs, ss, ce, se) = (s.cos(), s.sin(), e.cos(), e.sin());
        unsafe {
            sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                point1: pt(r * (cs - k * ss), r * (ss + k * cs)),
                point2: pt(r * (ce + k * se), r * (se - k * ce)),
                point3: pt(r * ce, r * se),
            });
        }
    }
}

impl Draw<'_> {
    pub(super) fn fill(&self, r: &Rect, radius: f32, c: Rgb, a: f32) {
        unsafe {
            self.rt
                .FillRoundedRectangle(&rounded(r, radius), self.paint(c, a))
        };
    }

    /// A border drawn inside `r`, so edges stay crisp and borders never
    /// double up with neighbours.
    pub(super) fn border(&self, r: &Rect, radius: f32, c: Rgb, a: f32, width: f32) {
        let inner = r.inset(width / 2.0);
        unsafe {
            self.rt.DrawRoundedRectangle(
                &rounded(&inner, (radius - width / 2.0).max(0.0)),
                self.paint(c, a),
                width,
                None,
            )
        };
    }

    pub(super) fn hline(&self, x0: f32, x1: f32, y: f32, c: Rgb, a: f32) {
        let y = y + 0.5;
        unsafe {
            self.rt
                .DrawLine(pt(x0, y), pt(x1, y), self.paint(c, a), space::STROKE, None)
        };
    }

    /// The brush for preview `id`, made from `px` the first time it's
    /// drawn, and marked as drawn this frame. `None` when the pixels don't
    /// make a bitmap.
    pub(super) fn picture(&self, px: &Pixels, id: u64) -> Option<ID2D1BitmapBrush> {
        let mut pictures = self.pictures.borrow_mut();
        if let Some(pic) = pictures.get_mut(&id) {
            pic.drawn = self.frame;
            return Some(pic.brush.clone());
        }
        let stride = px.width.checked_mul(4)?;
        let len = (stride as usize).checked_mul(px.height as usize)?;
        if px.width == 0 || px.height == 0 || px.bgra.len() < len {
            return None;
        }
        let brush = unsafe {
            let bitmap = self
                .rt
                .CreateBitmap(
                    D2D_SIZE_U {
                        width: px.width,
                        height: px.height,
                    },
                    Some(px.bgra.as_ptr().cast()),
                    stride,
                    &D2D1_BITMAP_PROPERTIES {
                        pixelFormat: D2D1_PIXEL_FORMAT {
                            format: DXGI_FORMAT_B8G8R8A8_UNORM,
                            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                        },
                        dpiX: 96.0,
                        dpiY: 96.0,
                    },
                )
                .ok()?;
            self.rt
                .CreateBitmapBrush(
                    &bitmap,
                    Some(&D2D1_BITMAP_BRUSH_PROPERTIES {
                        extendModeX: D2D1_EXTEND_MODE_CLAMP,
                        extendModeY: D2D1_EXTEND_MODE_CLAMP,
                        interpolationMode: D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                    }),
                    None,
                )
                .ok()?
        };
        pictures.insert(
            id,
            Picture {
                brush: brush.clone(),
                drawn: self.frame,
            },
        );
        Some(brush)
    }

    /// Draw `f` clipped to `r` with corners of `radius`. `r` is where the
    /// current transform puts it, which may only scale and move: the clip
    /// is made where `r` lands on the target, so it lines up however it
    /// was moved.
    pub(super) fn clipped<T>(
        &self,
        r: &Rect,
        radius: f32,
        f: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let base = self.base.get();
        let at = mapped(r, &base);
        let mask = cache::rounded_clip(self.clips, self.factory, &rounded(&at, radius * base.M11))?;
        unsafe { self.rt.SetTransform(&Matrix3x2::identity()) };
        let pushed = cache::push_layer(self.rt, rectf(&at), Some(&mask), 1.0);
        unsafe { self.rt.SetTransform(&base) };
        pushed?;
        let result = f();
        unsafe { self.rt.PopLayer() };
        result
    }

    /// Kiln's mark at `scale` (from its 24-unit grid), its top left at
    /// `(x, y)`, its line `line` DIPs wide whatever the scale.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn mark_lined(&self, x: f32, y: f32, scale: f32, line: f32, c: Rgb, a: f32) {
        let base = self.base.get();
        // SAFETY: plain draws on this thread's render target, in a
        // BeginDraw, of a geometry made with its factory.
        unsafe {
            self.rt
                .SetTransform(&compose(&scale_at(scale, x, y), &base));
            // The stroke is drawn under the scale, so it's given in the
            // mark's own units.
            self.rt
                .DrawGeometry(self.mark, self.paint(c, a), line / scale, self.round);
            self.rt.SetTransform(&base);
        }
    }

    /// Glyph `code` of Segoe Fluent Icons (or MDL2 Assets), `size` DIPs,
    /// in the middle of `r` in `c`, its box on whole pixels.
    pub(super) fn glyph(&self, code: char, r: &Rect, size: f32, c: Rgb) {
        let Ok(laid) = self.text.glyph(code, size) else {
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

    /// `s` in type-ramp `style` in `r`, in `c`, at this frame's Text size,
    /// clipped to `r`. Returns how wide it runs.
    pub(super) fn styled(&self, s: &str, style: text::Style, r: &Rect, c: Rgb) -> Result<f32> {
        self.styled_runs(s, style, &[], r, c)
    }

    /// `styled` at `a` of its colour's strength, as a row fades in.
    pub(super) fn styled_at(
        &self,
        s: &str,
        style: text::Style,
        r: &Rect,
        c: Rgb,
        a: f32,
    ) -> Result<f32> {
        let laid = self.text.styled_runs(s, style, self.zoom, r.w, r.h, &[])?;
        // SAFETY: a plain draw on this thread's render target, in a
        // BeginDraw, of a layout that outlives the call.
        unsafe {
            self.rt.DrawTextLayout(
                pt(r.x, r.y),
                &laid.layout,
                self.paint(c, a),
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
            );
        }
        Ok(laid.width)
    }

    /// `styled`, with the byte ranges `strong` of `s` in Body Strong's
    /// weight (`Text::styled_runs`).
    pub(super) fn styled_runs(
        &self,
        s: &str,
        style: text::Style,
        strong: &[std::ops::Range<usize>],
        r: &Rect,
        c: Rgb,
    ) -> Result<f32> {
        let laid = self
            .text
            .styled_runs(s, style, self.zoom, r.w, r.h, strong)?;
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 1e-4 && (a.1 - b.1).abs() < 1e-4
    }

    #[test]
    fn a_glyph_sits_in_the_middle_of_its_box() {
        let r = Rect::new(0.0, 0.0, 32.0, 32.0);
        assert!(close(glyph_origin(&r, 16.0), (8.0, 8.0)));
        let r = Rect::new(10.0, 4.0, 46.0, 48.0);
        assert!(close(glyph_origin(&r, 20.0), (23.0, 18.0)));
    }

    #[test]
    fn a_glyph_box_starts_on_a_whole_pixel() {
        let id = Matrix3x2::identity();
        // 100%: whole DIPs are whole pixels.
        assert!(close(snapped((10.3, 7.6), &id, 1.0), (10.0, 8.0)));
        // 125%: 10.3 DIPs is 12.875 px, so 13 px, 10.4 DIPs; 7.5 is 9.375, so 9.
        assert!(close(snapped((10.3, 7.5), &id, 1.25), (10.4, 7.2)));
        // 200%: half DIPs are whole pixels.
        assert!(close(snapped((10.5, 7.2), &id, 2.0), (10.5, 7.0)));
        // Under a move, it is the pixel on the target that is whole.
        let moved = moved(0.5, 0.0);
        let (x, y) = snapped((10.3, 7.0), &moved, 1.0);
        assert!(close((x + 0.5, y), (11.0, 7.0)));
        // Under a scale too (the wheel's opening).
        let m = scale_at(0.92, 3.0, 0.0);
        let (x, _) = snapped((10.0, 0.0), &m, 1.5);
        let px = (x * 0.92 + 3.0) * 1.5;
        assert!((px - px.round()).abs() < 1e-3, "{px}");
        // A turned transform has no whole pixels to land on: as it was.
        let turned = turn_scale_at(30.0, 1.0, 0.0, 0.0);
        assert!(close(snapped((10.3, 7.6), &turned, 1.0), (10.3, 7.6)));
    }

    #[test]
    fn a_scale_about_a_point_leaves_that_point_where_it_is() {
        let at = |m: &Matrix3x2, (x, y): (f32, f32)| {
            (x * m.M11 + y * m.M21 + m.M31, x * m.M12 + y * m.M22 + m.M32)
        };
        // A dialog's centre stays; its corners come in towards it.
        let m = scale_about(1.05, 400.0, 300.0);
        assert!(close(at(&m, (400.0, 300.0)), (400.0, 300.0)));
        assert!(close(at(&m, (200.0, 100.0)), (190.0, 90.0)));
        assert!(close(
            at(&scale_about(1.0, 400.0, 300.0), (7.0, 9.0)),
            (7.0, 9.0)
        ));
    }
}
