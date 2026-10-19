//! Direct2D drawing. Everything is laid out in DIPs; the render target's
//! DPI maps them to pixels, so 100%, 150% and 200% scaling share one path.

use std::cell::Cell;
use std::f32::consts::{PI, TAU};

use windows::core::Result;
use windows::Win32::Foundation::{D2DERR_RECREATE_TARGET, HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows_numerics::{Matrix3x2, Vector2};

use crate::icons::{self, Cmd, Icon, Style};
use crate::layout::{
    self, Control, Layout, Rect, SettingsLayout, CHIP_HEIGHT, CHIP_MIN_WIDTH, PROGRESS_HEIGHT,
    ROW_HEIGHT,
};
use crate::model::{Entry, JobState, Tone};
use crate::settings::{KeyMap, ThemeChoice};
use crate::theme::{effects, shape, Palette, Rgb};
use crate::wheel::{self, Wheel, R_INNER, R_OUTER};

use super::text::Text;

/// How the wheel looks this frame.
pub struct WheelView<'a> {
    pub wheel: &'a Wheel,
    pub hover: Option<usize>,
    /// 0 to 1: the opening pour. Drives scale and opacity.
    pub open: f32,
    /// 1 to 0 while closing.
    pub fade: f32,
    /// The item pulsing after a drop, and how far through the pulse.
    pub pulse: Option<(usize, f32)>,
}

/// Per-row animation state.
#[derive(Clone, Copy)]
pub struct RowAnim {
    /// Fade-in when the row was added.
    pub appear: f32,
    /// 0 to 1 after a job finished: the result cooling into place.
    pub settle: f32,
}

/// Everything the renderer needs to know about the app this frame.
pub struct View<'a> {
    pub palette: &'a Palette,
    pub entries: &'a [Entry],
    pub selected: Option<usize>,
    pub hover_row: Option<usize>,
    pub hover_cancel: bool,
    pub hover_link: bool,
    pub scroll: f32,
    /// 0 to 1: how far the drag-over highlight has faded in.
    pub drag: f32,
    pub row_anim: &'a dyn Fn(usize) -> RowAnim,
    pub status: &'a str,
    pub clear_hover: bool,
    pub clear_pressed: bool,
    pub wheel: Option<WheelView<'a>>,
    /// Engine output shown in the details dialog.
    pub details: Option<&'a str>,
    pub dialog_close_hover: bool,
    pub settings_hover: bool,
    pub settings: Option<SettingsView<'a>>,
}

/// The settings panel this frame.
pub struct SettingsView<'a> {
    pub layout: &'a SettingsLayout,
    /// The output folder line, and whether it is the default (muted).
    pub output: (&'a str, bool),
    pub theme: ThemeChoice,
    pub keys: KeyMap,
    /// Each engine's line, and whether it is muted.
    pub engines: [(&'a str, bool); 3],
    /// Indexes into `layout.controls`.
    pub hover: Option<usize>,
    pub focus: Option<usize>,
}

pub struct Renderer {
    factory: ID2D1Factory,
    text: Text,
    round: ID2D1StrokeStyle,
    icons: Vec<(&'static str, ID2D1PathGeometry)>,
    target: Option<Target>,
    hwnd: HWND,
    dpi: f32,
}

/// Device-dependent resources, rebuilt if the device is lost.
struct Target {
    rt: ID2D1HwndRenderTarget,
    brush: ID2D1SolidColorBrush,
    grain: ID2D1BitmapBrush,
    palette_name: &'static str,
}

fn color(c: Rgb, a: f32) -> D2D1_COLOR_F {
    let [r, g, b] = c.unit();
    D2D1_COLOR_F { r, g, b, a }
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Rgb(l(a.0, b.0), l(a.1, b.1), l(a.2, b.2))
}

fn rectf(r: &Rect) -> D2D_RECT_F {
    D2D_RECT_F {
        left: r.x,
        top: r.y,
        right: r.right(),
        bottom: r.bottom(),
    }
}

fn rounded(r: &Rect, radius: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT {
        rect: rectf(r),
        radiusX: radius,
        radiusY: radius,
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn pt(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

fn scale_at(s: f32, x: f32, y: f32) -> Matrix3x2 {
    Matrix3x2 {
        M11: s,
        M12: 0.0,
        M21: 0.0,
        M22: s,
        M31: x,
        M32: y,
    }
}

/// `inner` applied first, then `outer`. Both are scale-and-translate only.
fn compose(inner: &Matrix3x2, outer: &Matrix3x2) -> Matrix3x2 {
    Matrix3x2 {
        M11: inner.M11 * outer.M11,
        M12: 0.0,
        M21: 0.0,
        M22: inner.M22 * outer.M22,
        M31: inner.M31 * outer.M11 + outer.M31,
        M32: inner.M32 * outer.M22 + outer.M32,
    }
}

impl Renderer {
    pub fn new(hwnd: HWND, dpi: f32) -> Result<Self> {
        let factory: ID2D1Factory =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)? };
        let props = D2D1_STROKE_STYLE_PROPERTIES {
            startCap: D2D1_CAP_STYLE_ROUND,
            endCap: D2D1_CAP_STYLE_ROUND,
            dashCap: D2D1_CAP_STYLE_ROUND,
            lineJoin: D2D1_LINE_JOIN_ROUND,
            miterLimit: 10.0,
            dashStyle: D2D1_DASH_STYLE_SOLID,
            dashOffset: 0.0,
        };
        let round = unsafe { factory.CreateStrokeStyle(&props, None)? };
        let icons = icons::ALL
            .iter()
            .map(|i| Ok((i.name, geometry(&factory, i)?)))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            factory,
            text: Text::new()?,
            round,
            icons,
            target: None,
            hwnd,
            dpi,
        })
    }

    pub fn display_font_bundled(&self) -> bool {
        self.text.display_bundled
    }

    pub fn set_dpi(&mut self, dpi: f32) {
        self.dpi = dpi;
        // The grain bitmap is per-pixel, so rebuild everything at the new
        // scale.
        self.target = None;
    }

    pub fn resize(&mut self) {
        if let Some(t) = &self.target {
            let size = client_size(self.hwnd);
            unsafe {
                let _ = t.rt.Resize(&size);
            }
        }
    }

    fn ensure_target(&mut self, palette: &Palette) -> Result<()> {
        if let Some(t) = &self.target {
            if t.palette_name == palette.name {
                return Ok(());
            }
        }
        let size = client_size(self.hwnd);
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: self.dpi,
            dpiY: self.dpi,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
            hwnd: self.hwnd,
            pixelSize: size,
            presentOptions: D2D1_PRESENT_OPTIONS_NONE,
        };
        let rt = unsafe { self.factory.CreateHwndRenderTarget(&props, &hwnd_props)? };
        unsafe {
            rt.SetDpi(self.dpi, self.dpi);
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        }
        let brush = unsafe { rt.CreateSolidColorBrush(&color(palette.text, 1.0), None)? };

        // The grain: one pre-rendered tile, one texel per physical pixel.
        let side = effects::GRAIN_TILE;
        let pixels = crate::theme::grain_tile(palette);
        let bitmap = unsafe {
            rt.CreateBitmap(
                D2D_SIZE_U {
                    width: side,
                    height: side,
                },
                Some(pixels.as_ptr().cast()),
                side * 4,
                &D2D1_BITMAP_PROPERTIES {
                    pixelFormat: D2D1_PIXEL_FORMAT {
                        format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                    },
                    dpiX: self.dpi,
                    dpiY: self.dpi,
                },
            )?
        };
        let grain = unsafe {
            rt.CreateBitmapBrush(
                &bitmap,
                Some(&D2D1_BITMAP_BRUSH_PROPERTIES {
                    extendModeX: D2D1_EXTEND_MODE_WRAP,
                    extendModeY: D2D1_EXTEND_MODE_WRAP,
                    interpolationMode: D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                }),
                None,
            )?
        };
        self.target = Some(Target {
            rt,
            brush,
            grain,
            palette_name: palette.name,
        });
        Ok(())
    }

    pub fn draw(&mut self, v: &View) -> Result<()> {
        self.ensure_target(v.palette)?;
        let t = self.target.as_ref().expect("target exists");
        let (w, h) = self.size_dips();
        let l = layout::layout(w, h, v.entries.len());
        let p = v.palette;
        let rt = &t.rt;

        unsafe {
            rt.BeginDraw();
            rt.SetTransform(&Matrix3x2::identity());
            rt.Clear(Some(&color(p.ground, 1.0)));
            rt.FillRectangle(&rectf(&Rect::new(0.0, 0.0, w, h)), &t.grain);
        }

        let d = Draw {
            factory: &self.factory,
            rt,
            brush: &t.brush,
            text: &self.text,
            round: &self.round,
            icons: &self.icons,
            base: Cell::new(Matrix3x2::identity()),
        };
        let mut result = d.header(&l, v);
        if result.is_ok() {
            result = if v.entries.is_empty() {
                d.empty_state(&l, v)
            } else {
                d.list(&l, v).and_then(|_| d.footer(&l, v))
            };
        }
        if let (Ok(()), Some(wv)) = (&result, &v.wheel) {
            result = d.wheel(wv, p);
        }
        if let (Ok(()), Some(details)) = (&result, v.details) {
            result = d.dialog(w, h, details, v);
        }
        if let (Ok(()), Some(sv)) = (&result, &v.settings) {
            result = d.settings(w, h, sv, p);
        }

        let end = unsafe { rt.EndDraw(None, None) };
        result?;
        match end {
            Err(e) if e.code() == D2DERR_RECREATE_TARGET => {
                self.target = None;
                Ok(())
            }
            other => other,
        }
    }

    pub fn size_dips(&self) -> (f32, f32) {
        let s = client_size(self.hwnd);
        let k = 96.0 / self.dpi;
        (s.width as f32 * k, s.height as f32 * k)
    }
}

struct Draw<'a> {
    factory: &'a ID2D1Factory,
    rt: &'a ID2D1HwndRenderTarget,
    brush: &'a ID2D1SolidColorBrush,
    text: &'a Text,
    round: &'a ID2D1StrokeStyle,
    icons: &'a [(&'static str, ID2D1PathGeometry)],
    /// The transform everything is currently drawn under.
    base: Cell<Matrix3x2>,
}

impl Draw<'_> {
    fn set_base(&self, m: Matrix3x2) {
        self.base.set(m);
        unsafe { self.rt.SetTransform(&m) };
    }

    fn paint(&self, c: Rgb, a: f32) -> &ID2D1SolidColorBrush {
        unsafe { self.brush.SetColor(&color(c, a)) };
        self.brush
    }

    fn fill(&self, r: &Rect, radius: f32, c: Rgb, a: f32) {
        unsafe {
            self.rt
                .FillRoundedRectangle(&rounded(r, radius), self.paint(c, a))
        };
    }

    /// A border drawn inside `r`, so edges stay crisp and borders never
    /// double up with neighbours.
    fn border(&self, r: &Rect, radius: f32, c: Rgb, a: f32, width: f32) {
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

    fn hline(&self, x0: f32, x1: f32, y: f32, c: Rgb, a: f32) {
        let y = y + 0.5;
        unsafe {
            self.rt
                .DrawLine(pt(x0, y), pt(x1, y), self.paint(c, a), shape::BORDER, None)
        };
    }

    fn label(&self, s: &str, format: &IDWriteTextFormat, r: &Rect, c: Rgb, a: f32) {
        unsafe {
            self.rt.DrawText(
                &wide(s),
                format,
                &rectf(r),
                self.paint(c, a),
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            )
        };
    }

    /// A format code: monospace, uppercase, tracked, centred in `r`.
    fn code(&self, code: &str, r: &Rect, c: Rgb, a: f32) -> Result<f32> {
        let (layout, w) = self.text.code_layout(code, r.w.max(1.0), r.h)?;
        unsafe {
            self.rt.DrawTextLayout(
                pt(r.x, r.y),
                &layout,
                self.paint(c, a),
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
            );
        }
        Ok(w)
    }

    fn icon(&self, name: &str, x: f32, y: f32, scale: f32, c: Rgb, a: f32) {
        let Some((_, geom)) = self.icons.iter().find(|(n, _)| *n == name) else {
            return;
        };
        let solid = icons::ALL
            .iter()
            .any(|i| i.name == name && i.style == Style::Fill);
        let base = self.base.get();
        unsafe {
            self.rt
                .SetTransform(&compose(&scale_at(scale, x, y), &base));
            if solid {
                self.rt.FillGeometry(geom, self.paint(c, a), None);
            } else {
                self.rt
                    .DrawGeometry(geom, self.paint(c, a), shape::ICON_STROKE, self.round);
            }
            self.rt.SetTransform(&base);
        }
    }

    fn header(&self, l: &Layout, v: &View) -> Result<()> {
        let p = v.palette;
        // The app icon: a celadon vessel on the soot ground.
        self.icon("vessel-solid", l.icon.x, l.icon.y, 1.0, p.accent, 1.0);
        self.label("Kiln", &self.text.display, &l.wordmark, p.text, 1.0);
        let b = l.settings_button;
        if v.settings_hover {
            self.fill(&b, shape::RADIUS_CONTROL, p.line, 1.0);
        }
        let c = if v.settings_hover {
            p.text
        } else {
            p.text_muted
        };
        self.icon("dial", b.x + 4.0, b.y + 4.0, 1.0, c, 1.0);
        self.hline(0.0, l.header.w, l.header.bottom() - 1.0, p.line, 1.0);
        Ok(())
    }

    fn empty_state(&self, l: &Layout, v: &View) -> Result<()> {
        let p = v.palette;
        let panel = l.panel;
        self.fill(&panel, shape::RADIUS_PANEL, p.surface, 1.0);
        if v.drag > 0.0 {
            self.fill(
                &panel,
                shape::RADIUS_PANEL,
                p.accent,
                effects::DROP_WASH * v.drag,
            );
        }
        let edge = mix(p.line, p.accent, v.drag);
        self.border(
            &panel,
            shape::RADIUS_PANEL,
            edge,
            1.0,
            shape::BORDER + 0.5 * v.drag,
        );

        // Centred group: vessel, line, tagline, hint, privacy note.
        let cx = panel.x + panel.w / 2.0;
        let group_h = 48.0 + 16.0 + 24.0 + 4.0 + 20.0 + 12.0 + 16.0 + 4.0 + 16.0;
        let mut y = (panel.y + (panel.h - group_h) / 2.0).round();
        let vessel = mix(p.text_muted, p.accent, v.drag);
        self.icon("vessel", cx - 24.0, y, 2.0, vessel, 1.0);
        y += 48.0 + 16.0;
        let row = |y: f32, h: f32| Rect::new(panel.x + 16.0, y, panel.w - 32.0, h);
        self.label(
            "Drop files here",
            &self.text.lead_center,
            &row(y, 24.0),
            p.text,
            1.0,
        );
        y += 24.0 + 4.0;
        self.label(
            "Fire raw files into finished ones.",
            &self.text.body_center,
            &row(y, 20.0),
            p.text_muted,
            1.0,
        );
        y += 20.0 + 12.0;
        self.label(
            "Hold Shift while dropping to convert, or Ctrl for tools.",
            &self.text.caption_center,
            &row(y, 16.0),
            p.text_muted,
            1.0,
        );
        y += 16.0 + 4.0;
        self.label(
            "Everything converts on this PC. Nothing is uploaded.",
            &self.text.caption_center,
            &row(y, 16.0),
            p.text_muted,
            1.0,
        );
        Ok(())
    }

    fn list(&self, l: &Layout, v: &View) -> Result<()> {
        let p = v.palette;
        let panel = l.panel;
        self.fill(&panel, shape::RADIUS_PANEL, p.surface, 1.0);

        // Clip rows to the panel's rounded shape so selection fills
        // follow its corners.
        let mask = unsafe {
            self.factory
                .CreateRoundedRectangleGeometry(&rounded(&panel, shape::RADIUS_PANEL))?
        };
        let layer = unsafe { self.rt.CreateLayer(None)? };
        let params = D2D1_LAYER_PARAMETERS {
            contentBounds: rectf(&panel),
            geometricMask: std::mem::ManuallyDrop::new(Some(mask.into())),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: Matrix3x2::identity(),
            opacity: 1.0,
            opacityBrush: std::mem::ManuallyDrop::new(None),
            layerOptions: D2D1_LAYER_OPTIONS_NONE,
        };
        unsafe { self.rt.PushLayer(&params, &layer) };
        let first = (v.scroll / ROW_HEIGHT).floor().max(0.0) as usize;
        let visible = (panel.h / ROW_HEIGHT).ceil() as usize + 1;
        let mut result = Ok(());
        for i in first..(first + visible).min(v.entries.len()) {
            let mut r = l.rows[i];
            r.y -= v.scroll;
            result = self.row(&r, &v.entries[i], i, v);
            if result.is_err() {
                break;
            }
        }
        unsafe { self.rt.PopLayer() };
        // PushLayer took its own reference; release ours.
        drop(std::mem::ManuallyDrop::into_inner(params.geometricMask));
        result?;

        if v.drag > 0.0 {
            self.fill(
                &panel,
                shape::RADIUS_PANEL,
                p.accent,
                effects::DROP_WASH * v.drag,
            );
        }
        let edge = mix(p.line, p.accent, v.drag);
        self.border(
            &panel,
            shape::RADIUS_PANEL,
            edge,
            1.0,
            shape::BORDER + 0.5 * v.drag,
        );

        // A thin scroll thumb when the list overflows.
        let max = layout::max_scroll(v.entries.len(), panel.h);
        if max > 0.0 {
            let content = v.entries.len() as f32 * ROW_HEIGHT;
            let track = panel.h - 16.0;
            let thumb_h = (track * panel.h / content).max(24.0);
            let thumb_y = panel.y + 8.0 + (track - thumb_h) * (v.scroll / max);
            let thumb = Rect::new(panel.right() - 6.0, thumb_y, 2.0, thumb_h);
            self.fill(&thumb, 1.0, p.text_muted, 0.5);
        }
        Ok(())
    }

    fn row(&self, r: &Rect, e: &Entry, i: usize, v: &View) -> Result<()> {
        let p = v.palette;
        let anim = (v.row_anim)(i);
        let a = anim.appear;
        let hovered = v.hover_row == Some(i);
        if v.selected == Some(i) {
            self.fill(r, 0.0, p.accent, effects::HOVER_FILL * a);
        } else if hovered {
            self.fill(r, 0.0, p.line, 0.5 * a);
        }
        if i > 0 {
            self.hline(r.x, r.right(), r.y - 0.5, p.line, a);
        }

        // Format chip: monospace, uppercase, tracked.
        let chip_y = r.y + (ROW_HEIGHT - CHIP_HEIGHT) / 2.0;
        let code = if e.code.is_empty() { "—" } else { &e.code };
        let (_, text_w) = self.text.code_layout(code, 120.0, CHIP_HEIGHT)?;
        let chip_w = (text_w + 16.0).max(CHIP_MIN_WIDTH).ceil();
        let chip = Rect::new(r.x + 16.0, chip_y, chip_w, CHIP_HEIGHT);
        self.fill(&chip, shape::RADIUS_CONTROL, p.line, a);
        self.code(code, &chip, p.text, a)?;

        let x = r.x + 16.0 + CHIP_MIN_WIDTH.max(chip_w) + 16.0;
        let running = matches!(e.job, JobState::Running { .. });
        let failed_with_details =
            matches!(&e.job, JobState::Failed { details, .. } if !details.is_empty());
        let right = if running {
            layout::row_cancel(r).x - 8.0
        } else if failed_with_details {
            layout::row_link(r).x - 8.0
        } else {
            r.right() - 16.0
        };
        let w = (right - x).max(0.0);
        self.label(
            &e.name,
            &self.text.body,
            &Rect::new(x, r.y + 8.0, w, 20.0),
            p.text,
            a,
        );

        let (line, tone) = e.line();
        let (c, la) = match tone {
            Tone::Muted => (p.text_muted, a),
            // The result cools into place over the settle fade.
            Tone::Result => (p.text, a * anim.settle),
            Tone::Crack => (p.crack, a * anim.settle),
        };
        self.label(
            &line,
            &self.text.caption,
            &Rect::new(x, r.y + 28.0, w, 20.0),
            c,
            la,
        );

        match &e.job {
            JobState::Running { fraction, .. } => {
                self.progress(r, *fraction, p, a)?;
                if hovered || v.selected == Some(i) {
                    let b = layout::row_cancel(r);
                    let hot = hovered && v.hover_cancel;
                    if hot {
                        self.fill(&b, shape::RADIUS_CONTROL, p.line, 1.0);
                    }
                    let c = if hot { p.text } else { p.text_muted };
                    self.icon("cross", b.x, b.y, 1.0, c, a);
                }
            }
            JobState::Done { .. } => {
                // The finished line fades as the result settles.
                let fade = 1.0 - anim.settle;
                if fade > 0.0 {
                    self.progress(r, Some(1.0), p, a * fade)?;
                }
            }
            JobState::Failed { details, .. } if !details.is_empty() => {
                let link = layout::row_link(r);
                let hot = hovered && v.hover_link;
                let c = if hot { p.text } else { p.text_muted };
                let tw = self.text.measure("Show details", &self.text.caption);
                let lx = link.right() - tw;
                self.label(
                    "Show details",
                    &self.text.caption,
                    &Rect::new(lx, link.y, tw + 1.0, link.h),
                    c,
                    a,
                );
                self.hline(lx, lx + tw, link.y + 15.0, c, a * 0.8);
            }
            _ => {}
        }
        Ok(())
    }

    /// The temperature line along the bottom of a job row. While running it
    /// burns a little brighter at its leading edge, like a glaze taking
    /// heat. This glow is the only gradient in the app.
    fn progress(&self, r: &Rect, fraction: Option<f32>, p: &Palette, a: f32) -> Result<()> {
        let y = r.bottom() - PROGRESS_HEIGHT;
        let track = Rect::new(r.x, y, r.w, PROGRESS_HEIGHT);
        self.fill(&track, 0.0, p.line, a);
        let Some(f) = fraction else {
            // Unknown length: the whole line holds a low, steady heat.
            self.fill(&track, 0.0, p.accent, 0.4 * a);
            return Ok(());
        };
        let end = r.x + r.w * f.clamp(0.0, 1.0);
        self.fill(
            &Rect::new(r.x, y, end - r.x, PROGRESS_HEIGHT),
            0.0,
            p.accent,
            a,
        );
        if f < 1.0 && end > r.x {
            let hot = mix(p.accent, p.text, 0.35);
            let glow_w = 40.0f32.min(end - r.x);
            let stops = [
                D2D1_GRADIENT_STOP {
                    position: 0.0,
                    color: color(hot, 0.0),
                },
                D2D1_GRADIENT_STOP {
                    position: 1.0,
                    color: color(hot, a),
                },
            ];
            unsafe {
                let coll = self.rt.CreateGradientStopCollection(
                    &stops,
                    D2D1_GAMMA_2_2,
                    D2D1_EXTEND_MODE_CLAMP,
                )?;
                let brush = self.rt.CreateLinearGradientBrush(
                    &D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                        startPoint: pt(end - glow_w, y),
                        endPoint: pt(end, y),
                    },
                    None,
                    &coll,
                )?;
                self.rt.FillRectangle(
                    &rectf(&Rect::new(
                        end - glow_w,
                        y - 1.0,
                        glow_w,
                        PROGRESS_HEIGHT + 1.0,
                    )),
                    &brush,
                );
            }
        }
        Ok(())
    }

    fn footer(&self, l: &Layout, v: &View) -> Result<()> {
        let p = v.palette;
        let (Some(footer), Some(button)) = (l.footer, l.clear_button) else {
            return Ok(());
        };
        let status = Rect::new(
            footer.x + 4.0,
            footer.y,
            footer.w - button.w - 16.0,
            footer.h,
        );
        self.label(v.status, &self.text.caption, &status, p.text_muted, 1.0);
        self.button("Clear", &button, v.clear_hover, v.clear_pressed, p);
        Ok(())
    }

    /// A quiet secondary control. Pressed: a soft inner shadow, as if the
    /// surface gave a little under the finger.
    fn button(&self, text: &str, b: &Rect, hover: bool, pressed: bool, p: &Palette) {
        self.button_face(b, hover, pressed, false, p);
        self.label(text, &self.text.body_center, b, p.text, 1.0);
    }

    /// A button's fill and border. Selected: the celadon wash and edge.
    fn button_face(&self, b: &Rect, hover: bool, pressed: bool, selected: bool, p: &Palette) {
        let fill = if hover { p.line } else { p.surface };
        self.fill(b, shape::RADIUS_CONTROL, fill, 1.0);
        if selected {
            self.fill(b, shape::RADIUS_CONTROL, p.accent, effects::HOVER_FILL);
        }
        if pressed {
            for (k, alpha) in [(1.0, 0.35), (2.0, 0.2), (3.0, 0.1)] {
                let inner = Rect::new(b.x + 1.0, b.y + k, b.w - 2.0, 1.0);
                self.fill(&inner, 0.0, p.ground, alpha);
            }
        }
        let edge = if selected { p.accent } else { p.line };
        self.border(b, shape::RADIUS_CONTROL, edge, 1.0, shape::BORDER);
    }

    /// The keyboard focus ring: a celadon line just outside the control.
    fn focus_ring(&self, b: &Rect, p: &Palette) {
        let r = Rect::new(b.x - 3.0, b.y - 3.0, b.w + 6.0, b.h + 6.0);
        self.border(
            &r,
            shape::RADIUS_CONTROL + 3.0,
            p.accent,
            1.0,
            shape::ICON_STROKE,
        );
    }

    /// The glaze wheel, drawn in its own coordinates around its centre.
    fn wheel(&self, wv: &WheelView, p: &Palette) -> Result<()> {
        let w = wv.wheel;
        let n = w.items.len();
        let opacity = (wv.open * wv.fade).clamp(0.0, 1.0);
        if opacity <= 0.0 {
            return Ok(());
        }
        // The pour: the wheel spreads out from the cursor.
        let s = 0.55 + 0.45 * wv.open;

        let layer = unsafe { self.rt.CreateLayer(None)? };
        let params = D2D1_LAYER_PARAMETERS {
            contentBounds: D2D_RECT_F {
                left: -1e6,
                top: -1e6,
                right: 1e6,
                bottom: 1e6,
            },
            geometricMask: std::mem::ManuallyDrop::new(None),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: Matrix3x2::identity(),
            opacity,
            opacityBrush: std::mem::ManuallyDrop::new(None),
            layerOptions: D2D1_LAYER_OPTIONS_NONE,
        };
        unsafe { self.rt.PushLayer(&params, &layer) };
        self.set_base(scale_at(s, w.center.0, w.center.1));

        let result = (|| -> Result<()> {
            // The dish.
            self.circle(R_OUTER, p.surface, 1.0)?;
            self.ring_edge(R_OUTER - 0.5, p.line)?;

            // Hover or pulse: a celadon wash on the segment and a fine arc
            // on its outer edge. Icons keep their colour.
            let lit = wv.pulse.map(|(i, _)| i).or(wv.hover);
            if let Some(i) = lit {
                let pulse = wv
                    .pulse
                    .map(|(_, t)| (t.clamp(0.0, 1.0) * PI).sin() * 0.3)
                    .unwrap_or(0.0);
                let (a0, a1) = wheel::segment(i, n);
                let sector = self.sector(a0, a1, R_INNER, R_OUTER, n == 1)?;
                unsafe {
                    self.rt.FillGeometry(
                        &sector,
                        self.paint(p.accent, effects::HOVER_FILL + pulse),
                        None,
                    );
                }
                let arc = self.arc(a0, a1, R_OUTER - 0.75, n == 1)?;
                unsafe {
                    self.rt.DrawGeometry(
                        &arc,
                        self.paint(p.accent, 1.0),
                        shape::ICON_STROKE,
                        self.round,
                    );
                }
            }

            // Dividers between segments.
            if n > 1 {
                for i in 0..n {
                    let (a0, _) = wheel::segment(i, n);
                    let (c, si) = (a0.cos(), a0.sin());
                    unsafe {
                        self.rt.DrawLine(
                            pt(c * R_INNER, si * R_INNER),
                            pt(c * R_OUTER, si * R_OUTER),
                            self.paint(p.line, 1.0),
                            shape::BORDER,
                            None,
                        );
                    }
                }
            }

            // Items: icon above, label below.
            for (i, item) in w.items.iter().enumerate() {
                let (lx, ly) = wheel::label_point(i, n);
                self.icon(item.icon, lx - 12.0, ly - 22.0, 1.0, p.text, 1.0);
                if item.is_code {
                    let r = Rect::new(lx - 40.0, ly + 4.0, 80.0, 16.0);
                    self.code(&item.label, &r, p.text, 1.0)?;
                } else {
                    let r = Rect::new(lx - 52.0, ly + 4.0, 104.0, 16.0);
                    self.label(&item.label, &self.text.caption_center, &r, p.text, 1.0);
                }
            }

            // The centre: the wheel's name and the source type.
            self.circle(R_INNER, p.ground, 1.0)?;
            self.ring_edge(R_INNER - 0.5, p.line)?;
            let title = Rect::new(-R_INNER, -22.0, 2.0 * R_INNER, 28.0);
            self.label(
                w.kind.title(),
                &self.text.display_center,
                &title,
                p.text,
                1.0,
            );
            let src = Rect::new(-R_INNER + 8.0, 8.0, 2.0 * R_INNER - 16.0, 16.0);
            self.code(&w.source_code, &src, p.text_muted, 1.0)?;
            Ok(())
        })();

        self.set_base(Matrix3x2::identity());
        unsafe { self.rt.PopLayer() };
        result
    }

    fn circle(&self, r: f32, c: Rgb, a: f32) -> Result<()> {
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
        Ok(())
    }

    fn ring_edge(&self, r: f32, c: Rgb) -> Result<()> {
        unsafe {
            self.rt.DrawEllipse(
                &D2D1_ELLIPSE {
                    point: pt(0.0, 0.0),
                    radiusX: r,
                    radiusY: r,
                },
                self.paint(c, 1.0),
                shape::BORDER,
                None,
            );
        }
        Ok(())
    }

    /// An annular sector from angle `a0` to `a1`. With `full`, the whole
    /// ring.
    fn sector(&self, a0: f32, a1: f32, r0: f32, r1: f32, full: bool) -> Result<ID2D1PathGeometry> {
        unsafe {
            let g = self.factory.CreatePathGeometry()?;
            let sink = g.Open()?;
            if full {
                sink.SetFillMode(D2D1_FILL_MODE_ALTERNATE);
                for r in [r1, r0] {
                    sink.BeginFigure(pt(r, 0.0), D2D1_FIGURE_BEGIN_FILLED);
                    arc_to(&sink, r, 0.0, TAU);
                    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                }
            } else {
                sink.BeginFigure(pt(r1 * a0.cos(), r1 * a0.sin()), D2D1_FIGURE_BEGIN_FILLED);
                arc_to(&sink, r1, a0, a1);
                sink.AddLine(pt(r0 * a1.cos(), r0 * a1.sin()));
                arc_to(&sink, r0, a1, a0);
                sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            }
            sink.Close()?;
            Ok(g)
        }
    }

    /// An open arc at radius `r`, for the hover edge.
    fn arc(&self, a0: f32, a1: f32, r: f32, full: bool) -> Result<ID2D1PathGeometry> {
        let (a0, a1) = if full { (0.0, TAU) } else { (a0, a1) };
        unsafe {
            let g = self.factory.CreatePathGeometry()?;
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

    /// Engine output for a failed job, over a scrim.
    fn dialog(&self, w: f32, h: f32, details: &str, v: &View) -> Result<()> {
        let p = v.palette;
        let d = layout::dialog(w, h);
        // A scrim, not a shadow: it quiets the window behind the dialog.
        self.fill(&Rect::new(0.0, 0.0, w, h), 0.0, p.ground, 0.7);
        self.fill(&d.panel, shape::RADIUS_PANEL, p.surface, 1.0);
        self.border(&d.panel, shape::RADIUS_PANEL, p.line, 1.0, shape::BORDER);
        self.label("Details", &self.text.display, &d.title, p.text, 1.0);
        unsafe {
            self.rt.DrawText(
                &wide(details.trim()),
                &self.text.mono_block,
                &rectf(&d.body),
                self.paint(p.text_muted, 1.0),
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        self.button("Close", &d.close, v.dialog_close_hover, false, p);
        Ok(())
    }

    /// The settings panel, over a scrim. Changes apply as they are made,
    /// so there is only Close.
    fn settings(&self, w: f32, h: f32, sv: &SettingsView, p: &Palette) -> Result<()> {
        let l = sv.layout;
        self.fill(&Rect::new(0.0, 0.0, w, h), 0.0, p.ground, 0.7);
        self.fill(&l.panel, shape::RADIUS_PANEL, p.surface, 1.0);
        self.border(&l.panel, shape::RADIUS_PANEL, p.line, 1.0, shape::BORDER);
        self.label("Settings", &self.text.display, &l.title, p.text, 1.0);

        for (i, row) in l.rows.iter().enumerate() {
            self.label(
                row.label,
                &self.text.body,
                &row.label_rect,
                p.text_muted,
                1.0,
            );
            let Some(r) = row.value else { continue };
            let (text, muted) = match i {
                0 => sv.output,
                // Rows 1 and 2, theme and keys, have no value text.
                n => sv.engines[n - 3],
            };
            let c = if muted { p.text_muted } else { p.text };
            self.label(text, &self.text.body, &r, c, 1.0);
        }

        for (k, (control, r)) in l.controls.iter().enumerate() {
            let hover = sv.hover == Some(k);
            match control {
                Control::OutputChoose | Control::EngineChoose(_) => {
                    self.button("Choose", r, hover, false, p)
                }
                Control::OutputReset | Control::EngineReset(_) => {
                    self.button("Reset", r, hover, false, p)
                }
                Control::Close => self.button("Close", r, hover, false, p),
                Control::Theme(t) => {
                    // One segmented control: fills here, one shared
                    // border below, so neighbours never double up.
                    let fill = if hover { p.line } else { p.surface };
                    self.fill(r, shape::RADIUS_CONTROL, fill, 1.0);
                    if *t == sv.theme {
                        self.fill(r, shape::RADIUS_CONTROL, p.accent, effects::HOVER_FILL);
                    }
                    self.label(t.label(), &self.text.body_center, r, p.text, 1.0);
                }
                Control::Key(wheel) => {
                    self.button_face(r, hover, false, false, p);
                    let name = Rect::new(r.x + 12.0, r.y, r.w - 12.0, r.h);
                    self.label(wheel.title(), &self.text.body, &name, p.text, 1.0);
                    let code = sv.keys.key_for(*wheel).code();
                    let (_, cw) = self.text.code_layout(code, r.w, r.h)?;
                    let cr = Rect::new(r.right() - 12.0 - cw.ceil(), r.y, cw.ceil(), r.h);
                    self.code(code, &cr, p.text_muted, 1.0)?;
                }
            }
            if sv.focus == Some(k) {
                self.focus_ring(r, p);
            }
        }
        self.segment_edges(l, sv, p);
        Ok(())
    }

    /// The theme control's outline: one border round the group, hairline
    /// dividers, and the chosen segment's celadon edge on top.
    fn segment_edges(&self, l: &SettingsLayout, sv: &SettingsView, p: &Palette) {
        let segs: Vec<(ThemeChoice, Rect)> = l
            .controls
            .iter()
            .filter_map(|(c, r)| match c {
                Control::Theme(t) => Some((*t, *r)),
                _ => None,
            })
            .collect();
        let (Some(first), Some(last)) = (segs.first(), segs.last()) else {
            return;
        };
        let group = Rect::new(first.1.x, first.1.y, last.1.right() - first.1.x, first.1.h);
        self.border(&group, shape::RADIUS_CONTROL, p.line, 1.0, shape::BORDER);
        for (_, r) in &segs[1..] {
            unsafe {
                self.rt.DrawLine(
                    pt(r.x + 0.5, r.y + 1.0),
                    pt(r.x + 0.5, r.bottom() - 1.0),
                    self.paint(p.line, 1.0),
                    shape::BORDER,
                    None,
                );
            }
        }
        if let Some((_, r)) = segs.iter().find(|(t, _)| *t == sv.theme) {
            self.border(r, shape::RADIUS_CONTROL, p.accent, 1.0, shape::BORDER);
        }
    }
}

/// Continue a figure along a circle of radius `r` around the origin, from
/// angle `a0` (where the figure already is) to `a1`, either direction. Built
/// from cubic Béziers of at most a quarter turn each, which stay within a
/// fraction of a pixel of the true circle at wheel sizes.
fn arc_to(sink: &ID2D1GeometrySink, r: f32, a0: f32, a1: f32) {
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

fn geometry(factory: &ID2D1Factory, icon: &Icon) -> Result<ID2D1PathGeometry> {
    unsafe {
        let geom = factory.CreatePathGeometry()?;
        let sink = geom.Open()?;
        let mut open = false;
        let begin = if icon.style == Style::Fill {
            D2D1_FIGURE_BEGIN_FILLED
        } else {
            D2D1_FIGURE_BEGIN_HOLLOW
        };
        for c in icon.path {
            match *c {
                Cmd::Move(x, y) => {
                    if open {
                        sink.EndFigure(D2D1_FIGURE_END_OPEN);
                    }
                    sink.BeginFigure(pt(x, y), begin);
                    open = true;
                }
                Cmd::Line(x, y) => sink.AddLine(pt(x, y)),
                Cmd::Cubic(a, b, c, d, x, y) => sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                    point1: pt(a, b),
                    point2: pt(c, d),
                    point3: pt(x, y),
                }),
                Cmd::Close => {
                    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                    open = false;
                }
            }
        }
        if open {
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
        }
        sink.Close()?;
        Ok(geom)
    }
}

fn client_size(hwnd: HWND) -> D2D_SIZE_U {
    let mut rc = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rc);
    }
    D2D_SIZE_U {
        width: (rc.right - rc.left).max(1) as u32,
        height: (rc.bottom - rc.top).max(1) as u32,
    }
}
