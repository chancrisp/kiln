//! Direct2D drawing. Everything is laid out in DIPs; the render target's
//! DPI maps them to pixels, so 100%, 150% and 200% scaling share one path.

use windows::core::Result;
use windows::Win32::Foundation::{D2DERR_RECREATE_TARGET, HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows_numerics::{Matrix3x2, Vector2};

use crate::icons::{self, Cmd, Icon, Style};
use crate::layout::{self, Layout, Rect, CHIP_HEIGHT, CHIP_MIN_WIDTH, ROW_HEIGHT};
use crate::model::Entry;
use crate::theme::{effects, shape, Palette, Rgb};

use super::text::Text;

/// Everything the renderer needs to know about the app this frame.
pub struct View<'a> {
    pub palette: &'a Palette,
    pub entries: &'a [Entry],
    pub selected: Option<usize>,
    pub hover_row: Option<usize>,
    pub scroll: f32,
    /// 0 to 1: how far the drag-over highlight has faded in.
    pub drag: f32,
    /// Per-row opacity while new rows settle in.
    pub row_alpha: &'a dyn Fn(usize) -> f32,
    pub status: &'a str,
    pub clear_hover: bool,
    pub clear_pressed: bool,
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
        if let Some(t) = &self.target {
            unsafe { t.rt.SetDpi(dpi, dpi) };
        }
        // The grain bitmap is per-pixel, so rebuild it at the new scale.
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
        };
        d.header(&l, p)?;
        if v.entries.is_empty() {
            d.empty_state(&l, v)?;
        } else {
            d.list(&l, v)?;
            d.footer(&l, v)?;
        }

        match unsafe { rt.EndDraw(None, None) } {
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
}

impl Draw<'_> {
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

    /// A 1 px border drawn inside `r`, so edges stay crisp and borders
    /// never double up with neighbours.
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
            self.rt.DrawLine(
                Vector2 { X: x0, Y: y },
                Vector2 { X: x1, Y: y },
                self.paint(c, a),
                shape::BORDER,
                None,
            )
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

    fn icon(&self, name: &str, x: f32, y: f32, scale: f32, c: Rgb, a: f32) {
        let Some((_, geom)) = self.icons.iter().find(|(n, _)| *n == name) else {
            return;
        };
        let solid = icons::ALL
            .iter()
            .any(|i| i.name == name && i.style == Style::Fill);
        unsafe {
            self.rt.SetTransform(&Matrix3x2 {
                M11: scale,
                M12: 0.0,
                M21: 0.0,
                M22: scale,
                M31: x,
                M32: y,
            });
            if solid {
                self.rt.FillGeometry(geom, self.paint(c, a), None);
            } else {
                self.rt
                    .DrawGeometry(geom, self.paint(c, a), shape::ICON_STROKE, self.round);
            }
            self.rt.SetTransform(&Matrix3x2::identity());
        }
    }

    fn header(&self, l: &Layout, p: &Palette) -> Result<()> {
        // The app icon: a celadon vessel on the soot ground.
        self.icon("vessel-solid", l.icon.x, l.icon.y, 1.0, p.accent, 1.0);
        self.label("Kiln", &self.text.display, &l.wordmark, p.text, 1.0);
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

        // Centred group: vessel, line, tagline, privacy note.
        let cx = panel.x + panel.w / 2.0;
        let group_h = 48.0 + 16.0 + 24.0 + 4.0 + 20.0 + 12.0 + 16.0;
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
        for i in first..(first + visible).min(v.entries.len()) {
            let mut r = l.rows[i];
            r.y -= v.scroll;
            self.row(&r, &v.entries[i], i, v)?;
        }
        unsafe { self.rt.PopLayer() };
        // PushLayer took its own reference; release ours.
        drop(std::mem::ManuallyDrop::into_inner(params.geometricMask));

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
        let a = (v.row_alpha)(i);
        let selected = v.selected == Some(i);
        if selected {
            self.fill(r, 0.0, p.accent, effects::HOVER_FILL * a);
        } else if v.hover_row == Some(i) {
            self.fill(r, 0.0, p.line, 0.5 * a);
        }
        if i > 0 {
            self.hline(r.x, r.right(), r.y - 0.5, p.line, a);
        }

        // Format chip: monospace, uppercase, tracked.
        let chip_y = r.y + (ROW_HEIGHT - CHIP_HEIGHT) / 2.0;
        let code = if e.code.is_empty() { "—" } else { &e.code };
        let (layout, text_w) = self.text.code_layout(code, 120.0, CHIP_HEIGHT)?;
        let chip_w = (text_w + 16.0).max(CHIP_MIN_WIDTH).ceil();
        let chip = Rect::new(r.x + 16.0, chip_y, chip_w, CHIP_HEIGHT);
        self.fill(&chip, shape::RADIUS_CONTROL, p.line, a);
        unsafe {
            layout.SetMaxWidth(chip_w)?;
            self.rt.DrawTextLayout(
                Vector2 {
                    X: chip.x,
                    Y: chip.y,
                },
                &layout,
                self.paint(p.text, a),
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
            );
        }

        let x = r.x + 16.0 + CHIP_MIN_WIDTH.max(chip_w) + 16.0;
        let w = (r.right() - 16.0 - x).max(0.0);
        self.label(
            &e.name,
            &self.text.body,
            &Rect::new(x, r.y + 8.0, w, 20.0),
            p.text,
            a,
        );
        self.label(
            &e.detail(),
            &self.text.caption,
            &Rect::new(x, r.y + 28.0, w, 20.0),
            p.text_muted,
            a,
        );
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

        // A quiet secondary control. Pressed: a soft inner shadow, as if
        // the surface gave a little under the finger.
        let fill = if v.clear_hover { p.line } else { p.surface };
        self.fill(&button, shape::RADIUS_CONTROL, fill, 1.0);
        if v.clear_pressed {
            for (k, alpha) in [(1.0, 0.35), (2.0, 0.2), (3.0, 0.1)] {
                let inner = Rect::new(button.x + 1.0, button.y + k, button.w - 2.0, 1.0);
                self.fill(&inner, 0.0, p.ground, alpha);
            }
        }
        self.border(&button, shape::RADIUS_CONTROL, p.line, 1.0, shape::BORDER);
        self.label("Clear", &self.text.body_center, &button, p.text, 1.0);
        Ok(())
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
        let pt = |x: f32, y: f32| Vector2 { X: x, Y: y };
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
