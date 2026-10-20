//! Direct2D drawing. Everything is laid out in DIPs; the render target's
//! DPI maps them to pixels, so 100%, 150% and 200% scaling share one path.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::f32::consts::{PI, TAU};
use std::path::PathBuf;

use windows::core::Result;
use windows::Win32::Foundation::{D2DERR_RECREATE_TARGET, HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows_numerics::{Matrix3x2, Vector2};

use kiln_core::Preset;

use crate::icons::{self, Cmd, Icon, Style};
use crate::layout::{
    self, Control, Layout, Rect, SettingsLayout, SettingsTab, SheetControl, SheetLayout,
    CHIP_HEIGHT, CHIP_MIN_WIDTH, PROGRESS_HEIGHT, ROW_HEIGHT,
};
use crate::model::{self, Entry, JobState, Tone};
use crate::quality::{QualityDefaults, Value};
use crate::settings::{KeyMap, ThemeChoice};
use crate::theme::{effects, shape, Palette, Rgb};
use crate::ways_in::EmptyLines;
use crate::wheel::{self, Action, Spot, Wheel, R_INNER, R_MORE_INNER, R_MORE_OUTER, R_OUTER};

use super::text::Text;

/// How the wheel looks this frame.
pub struct WheelView<'a> {
    pub wheel: &'a Wheel,
    /// Its name and the key that opens it: "Convert · Shift".
    pub title: String,
    pub hover: Option<Spot>,
    /// 0 to 1: how far More's outer ring has poured open.
    pub more: f32,
    /// 0 to 1: the opening pour. Drives scale and opacity.
    pub open: f32,
    /// 1 to 0 while closing.
    pub fade: f32,
    /// The item pulsing after a drop, and how far through the pulse.
    pub pulse: Option<(Spot, f32)>,
    /// Where the suggestion's dots go: its segment, or More and its slot.
    pub marks: Vec<Spot>,
    /// 0 to 1: the dots' fade in, when the suggestion arrived while the
    /// wheel was open.
    pub dot: f32,
}

/// The suggestion's dot: a radius of 2 DIPs (4 across), 6 DIPs above the
/// icon or code it marks.
const DOT_R: f32 = 2.0;
const DOT_GAP: f32 = 6.0;

/// The options sheet this frame.
pub struct SheetView<'a> {
    pub layout: &'a SheetLayout,
    pub title: &'a str,
    /// What the source is.
    pub context: &'a str,
    /// The result's size and an estimate.
    pub result: &'a str,
    /// Each knob's name and its value as it reads.
    pub rows: &'a [(String, String)],
    pub preset: Preset,
    /// Indexes into `layout.controls`.
    pub hover: Option<usize>,
    pub pressed: Option<usize>,
    pub focus: Option<usize>,
    /// 0 to 1 as it rises; 1 to 0 as it fades.
    pub open: f32,
    pub fade: f32,
}

/// Per-row animation state.
#[derive(Clone, Copy)]
pub struct RowAnim {
    /// Fade-in when the row was added.
    pub appear: f32,
    /// 0 to 1 after a job finished: the result cooling into place.
    pub settle: f32,
    /// 0 to 1 after facts arrived: the second line crossing over to what
    /// Kiln found.
    pub facts: f32,
}

/// Everything the renderer needs to know about the app this frame.
pub struct View<'a> {
    pub palette: &'a Palette,
    pub entries: &'a [Entry],
    /// Lines that stand in for a row's own second line, by the row's
    /// file: a failed export, or a held result that's gone.
    pub notes: &'a HashMap<PathBuf, (String, Tone)>,
    /// Rows, by file, whose results are being exported: they read
    /// "Saving…" and have no link until it's done.
    pub saving: &'a HashSet<PathBuf>,
    /// The footer's "Export all (n)" label, while results are held.
    pub export_all: Option<&'a str>,
    pub export_hover: bool,
    pub export_pressed: bool,
    pub selected: Option<usize>,
    pub hover_row: Option<usize>,
    pub hover_cancel: bool,
    pub hover_link: bool,
    pub scroll: f32,
    /// 0 to 1: how far the drag-over highlight has faded in.
    pub drag: f32,
    /// 0 to 1: how hot running progress lines burn this frame.
    pub heat: f32,
    pub row_anim: &'a dyn Fn(usize) -> RowAnim,
    pub status: &'a str,
    /// The empty window's lines; its key hint names the current wheel
    /// keys.
    pub empty: &'a EmptyLines,
    /// The Add files button, in the empty window or the footer.
    pub add_hover: bool,
    pub add_pressed: bool,
    pub clear_hover: bool,
    pub clear_pressed: bool,
    pub wheel: Option<WheelView<'a>>,
    /// Engine output shown in the details dialog.
    pub details: Option<&'a str>,
    pub dialog_close_hover: bool,
    pub dialog_close_pressed: bool,
    pub settings_hover: bool,
    pub settings: Option<SettingsView<'a>>,
    pub sheet: Option<SheetView<'a>>,
}

/// The settings panel this frame.
pub struct SettingsView<'a> {
    pub layout: &'a SettingsLayout,
    /// Where the tab marker sits, in tabs from the left.
    pub tab_marker: f32,
    /// 0 to 1 after the page changed: the new page fades in.
    pub page: f32,
    pub quality: &'a QualityDefaults,
    /// The output folder line, and whether it is the default (muted).
    pub output: (&'a str, bool),
    pub theme: ThemeChoice,
    /// Whether suggestions are on.
    pub suggestions: bool,
    /// Whether any picks are counted; Forget picks is disabled without.
    pub has_habits: bool,
    pub keys: KeyMap,
    /// Each engine's line, and whether it is muted.
    pub engines: [(&'a str, bool); 4],
    /// Indexes into `layout.controls`.
    pub hover: Option<usize>,
    pub pressed: Option<usize>,
    /// The update check's note, whether it is muted, and the button label.
    pub update: (&'a str, bool),
    pub update_button: &'a str,
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
        if let (Ok(()), Some(sv)) = (&result, &v.sheet) {
            result = d.sheet(w, h, sv, p);
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

    /// Draw `f` at `opacity` as one layer, so overlapping parts don't
    /// show through each other.
    fn with_opacity<T>(&self, opacity: f32, f: impl FnOnce() -> Result<T>) -> Result<T> {
        if opacity >= 0.999 {
            return f();
        }
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
            opacity: opacity.max(0.0),
            opacityBrush: std::mem::ManuallyDrop::new(None),
            layerOptions: D2D1_LAYER_OPTIONS_NONE,
        };
        unsafe { self.rt.PushLayer(&params, &layer) };
        let result = f();
        unsafe { self.rt.PopLayer() };
        result
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

        // The centred group: the headline, how else to add files, the
        // button, the keys to hold, the privacy note and, last, the tagline.
        let e = layout::empty_state(&panel);
        let lines = v.empty;
        let vessel = mix(p.text_muted, p.accent, v.drag);
        self.icon("vessel", e.vessel.x, e.vessel.y, 2.0, vessel, 1.0);
        self.label(
            lines.headline,
            &self.text.lead_center,
            &e.headline,
            p.text,
            1.0,
        );
        self.label(
            lines.instruction,
            &self.text.body_center,
            &e.instruction,
            p.text,
            1.0,
        );
        self.primary(lines.button, &e.button, v.add_hover, v.add_pressed, p);
        for (text, r) in [(lines.keys.as_str(), &e.keys), (lines.privacy, &e.privacy)] {
            self.label(text, &self.text.caption_center, r, p.text_muted, 1.0);
        }
        self.label(
            lines.tagline,
            &self.text.body_center,
            &e.tagline,
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
        let saving = v.saving.contains(&e.path);
        let link = e.link().filter(|_| !saving);
        let right = if running {
            layout::row_cancel(r).x - 8.0
        } else if link.is_some() {
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

        let (line, tone) = if saving {
            (model::SAVING.to_owned(), Tone::Muted)
        } else {
            v.notes.get(&e.path).cloned().unwrap_or_else(|| e.line())
        };
        let (c, la) = match tone {
            Tone::Muted => (p.text_muted, a),
            // The result cools into place over the settle fade.
            Tone::Result => (p.text, a * anim.settle),
            Tone::Crack => (p.crack, a * anim.settle),
        };
        let line_rect = Rect::new(x, r.y + 28.0, w, 20.0);
        // When facts arrive, the line they replace fades out in place as
        // the new one fades in, both muted.
        let mut la = la;
        if anim.facts < 1.0 && matches!(e.job, JobState::Idle) {
            let before = e.plain_detail();
            if before != line {
                self.label(
                    &before,
                    &self.text.caption,
                    &line_rect,
                    p.text_muted,
                    a * (1.0 - anim.facts),
                );
                la *= anim.facts;
            }
        }
        self.label(&line, &self.text.caption, &line_rect, c, la);
        // An exported row's "Saved to" opens its folder: underlined while
        // the pointer is on it, like a link.
        if hovered && v.hover_link && e.saved().is_some() {
            let tw = self.text.measure(&line, &self.text.caption).min(w);
            self.hline(x, x + tw, line_rect.y + 15.0, c, la * 0.8);
        }

        match &e.job {
            JobState::Running { fraction, .. } => {
                self.progress(r, *fraction, v.heat, p, a)?;
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
                    self.progress(r, Some(1.0), 0.0, p, a * fade)?;
                }
            }
            _ => {}
        }
        // "Show details" on a failed row, "Export" on a held one.
        if let Some(text) = link {
            let link = layout::row_link(r);
            let hot = hovered && v.hover_link;
            let c = if hot { p.text } else { p.text_muted };
            let tw = self.text.measure(text, &self.text.caption);
            let lx = link.right() - tw;
            self.label(
                text,
                &self.text.caption,
                &Rect::new(lx, link.y, tw + 1.0, link.h),
                c,
                a,
            );
            self.hline(lx, lx + tw, link.y + 15.0, c, a * 0.8);
        }
        Ok(())
    }

    /// The temperature line along the bottom of a job row. While running it
    /// burns a little brighter at its leading edge, like a glaze taking
    /// heat, and that glow slowly rises and falls with `heat`. This glow is
    /// the only gradient in the app.
    fn progress(
        &self,
        r: &Rect,
        fraction: Option<f32>,
        heat: f32,
        p: &Palette,
        a: f32,
    ) -> Result<()> {
        let y = r.bottom() - PROGRESS_HEIGHT;
        let track = Rect::new(r.x, y, r.w, PROGRESS_HEIGHT);
        self.fill(&track, 0.0, p.line, a);
        let Some(f) = fraction else {
            // Unknown length: the whole line holds a low heat that breathes.
            self.fill(&track, 0.0, p.accent, (0.3 + 0.25 * heat) * a);
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
            let hot = mix(p.accent, p.text, 0.2 + 0.25 * heat);
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
        let export = v
            .export_all
            .map(|label| (label, layout::export_all_button(&button)));
        let add = layout::add_files_button(&button, export.is_some());
        let left = add.x;
        let status = Rect::new(
            footer.x + 4.0,
            footer.y,
            (left - 12.0 - footer.x).max(0.0),
            footer.h,
        );
        self.label(v.status, &self.text.caption, &status, p.text_muted, 1.0);
        self.button(v.empty.button, &add, v.add_hover, v.add_pressed, p);
        if let Some((label, b)) = export {
            self.button(label, &b, v.export_hover, v.export_pressed, p);
        }
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
            self.press_shadow(b, p);
        }
        let edge = if selected { p.accent } else { p.line };
        self.border(b, shape::RADIUS_CONTROL, edge, 1.0, shape::BORDER);
    }

    /// The soft inner shadow of a pressed control, along its top edge.
    fn press_shadow(&self, b: &Rect, p: &Palette) {
        for (k, alpha) in [(1.0, 0.35), (2.0, 0.2), (3.0, 0.1)] {
            let inner = Rect::new(b.x + 1.0, b.y + k, b.w - 2.0, 1.0);
            self.fill(&inner, 0.0, p.ground, alpha);
        }
    }

    /// The emphasised button: celadon, for the one action a panel is for.
    fn primary(&self, text: &str, b: &Rect, hover: bool, pressed: bool, p: &Palette) {
        let fill = if hover {
            mix(p.accent, p.text, 0.12)
        } else {
            p.accent
        };
        self.fill(b, shape::RADIUS_CONTROL, fill, 1.0);
        if pressed {
            self.press_shadow(b, p);
        }
        self.label(text, &self.text.body_center, b, p.on_accent, 1.0);
    }

    /// A control that steps through values: the value between two small
    /// chevrons. The left end steps back, the rest forward.
    fn stepper(&self, text: &str, b: &Rect, hover: bool, pressed: bool, p: &Palette) {
        self.button_face(b, hover, pressed, false, p);
        let inner = Rect::new(b.x + 20.0, b.y, (b.w - 40.0).max(0.0), b.h);
        self.label(text, &self.text.body_center, &inner, p.text, 1.0);
        let c = if hover { p.text } else { p.text_muted };
        let cy = b.y + b.h / 2.0;
        self.chevron(b.x + 13.0, cy, -1.0, c);
        self.chevron(b.right() - 13.0, cy, 1.0, c);
    }

    /// A small chevron at `(x, y)`, pointing right for `dir` 1, left for -1.
    fn chevron(&self, x: f32, y: f32, dir: f32, c: Rgb) {
        let (tip, back) = (x + 2.0 * dir, x - 2.0 * dir);
        unsafe {
            for (a, b) in [((back, y - 4.0), (tip, y)), ((tip, y), (back, y + 4.0))] {
                self.rt.DrawLine(
                    pt(a.0, a.1),
                    pt(b.0, b.1),
                    self.paint(c, 1.0),
                    shape::ICON_STROKE,
                    self.round,
                );
            }
        }
    }

    /// One segment of a segmented control: fills here, and one shared
    /// outline from `segment_group`, so neighbours never double up.
    fn segment(
        &self,
        text: &str,
        r: &Rect,
        hover: bool,
        pressed: bool,
        selected: bool,
        p: &Palette,
    ) {
        let fill = if hover { p.line } else { p.surface };
        self.fill(r, shape::RADIUS_CONTROL, fill, 1.0);
        if selected {
            self.fill(r, shape::RADIUS_CONTROL, p.accent, effects::HOVER_FILL);
        }
        if pressed {
            self.press_shadow(r, p);
        }
        self.label(text, &self.text.body_center, r, p.text, 1.0);
    }

    /// A segmented control's outline: one border round the group,
    /// hairline dividers, and the chosen segment's celadon edge on top.
    fn segment_group(&self, segs: &[(bool, Rect)], p: &Palette) {
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
        if let Some((_, r)) = segs.iter().find(|(chosen, _)| *chosen) {
            self.border(r, shape::RADIUS_CONTROL, p.accent, 1.0, shape::BORDER);
        }
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
        let opacity = (wv.open * wv.fade).clamp(0.0, 1.0);
        if opacity <= 0.0 {
            return Ok(());
        }
        // The pour: the wheel spreads out from the cursor.
        let s = (0.55 + 0.45 * wv.open) * w.scale;
        let base = scale_at(s, w.center.0, w.center.1);
        let result = self.with_opacity(opacity, || {
            // More's ring pours out from behind the dish.
            if wv.more > 0.0 && !w.more.is_empty() {
                let k = 0.66 + 0.34 * wv.more;
                self.set_base(compose(&scale_at(k, 0.0, 0.0), &base));
                self.with_opacity(wv.more, || self.outer_ring(wv, p))?;
            }
            self.set_base(base);
            self.inner_ring(wv, p)
        });
        self.set_base(Matrix3x2::identity());
        result
    }

    /// How strongly the pointed-at or pulsing spot is lit.
    fn lit(wv: &WheelView) -> (Option<Spot>, f32) {
        let pulse = wv
            .pulse
            .map(|(_, t)| (t.clamp(0.0, 1.0) * PI).sin() * 0.3)
            .unwrap_or(0.0);
        (
            wv.pulse.map(|(s, _)| s).or(wv.hover),
            effects::HOVER_FILL + pulse,
        )
    }

    /// A celadon wash on a segment, and a fine arc on its outer edge.
    fn light(&self, b: Band, wash: f32, edge: f32, p: &Palette) -> Result<()> {
        let Band {
            a0,
            a1,
            r0,
            r1,
            full,
        } = b;
        let sector = self.sector(a0, a1, r0, r1, full)?;
        unsafe {
            self.rt
                .FillGeometry(&sector, self.paint(p.accent, wash), None);
        }
        if edge > 0.0 {
            let arc = self.arc(a0, a1, r1 - 0.75, full)?;
            unsafe {
                self.rt.DrawGeometry(
                    &arc,
                    self.paint(p.accent, edge),
                    shape::ICON_STROKE,
                    self.round,
                );
            }
        }
        Ok(())
    }

    fn inner_ring(&self, wv: &WheelView, p: &Palette) -> Result<()> {
        let w = wv.wheel;
        let n = w.segments();
        // The dish.
        self.circle(R_OUTER, p.surface, 1.0)?;
        self.ring_edge(R_OUTER - 0.5, p.line)?;

        // Hover or pulse: a celadon wash and a fine arc. Icons keep their
        // colour.
        let (lit, wash) = Self::lit(wv);
        if let Some(Spot::Inner(i)) = lit {
            let (a0, a1) = wheel::segment(i, n);
            self.light(Band::new(a0, a1, R_INNER, R_OUTER, n == 1), wash, 1.0, p)?;
        }
        // While its ring is out, More stays softly lit as the way back.
        let more = w.items.len();
        if wv.more > 0.0 && w.is_more(more) && lit != Some(Spot::Inner(more)) {
            let (a0, a1) = wheel::segment(more, n);
            let k = wv.more;
            let band = Band::new(a0, a1, R_INNER, R_OUTER, false);
            self.light(band, effects::HOVER_FILL * 0.5 * k, 0.5 * k, p)?;
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
        for i in 0..n {
            let (lx, ly) = wheel::label_point(i, n);
            let (icon, label, is_code) = match w.items.get(i) {
                Some(item) => (item.icon, item.label.as_str(), item.is_code),
                None => ("more", "More", false),
            };
            let top = ly - 22.0;
            self.icon(icon, lx - 12.0, top, 1.0, p.text, 1.0);
            if wv.marks.contains(&Spot::Inner(i)) {
                self.dot(lx, top - DOT_GAP, wv.dot, p);
            }
            if is_code {
                let r = Rect::new(lx - 40.0, ly + 4.0, 80.0, 16.0);
                self.code(label, &r, p.text, 1.0)?;
            } else {
                let r = Rect::new(lx - 52.0, ly + 4.0, 104.0, 16.0);
                self.label(label, &self.text.caption_center, &r, p.text, 1.0);
            }
        }

        // The centre: the wheel's name over the key that opens it, as
        // "Convert · Shift" breaks across two lines where it can't fit on
        // one, then the source type, or where a conversion goes while one
        // is pointed at: "PNG → JPG".
        self.circle(R_INNER, p.ground, 1.0)?;
        self.ring_edge(R_INNER - 0.5, p.line)?;
        let (name, key) = wv
            .title
            .split_once(" · ")
            .unwrap_or((wv.title.as_str(), ""));
        let title = Rect::new(-R_INNER, -32.0, 2.0 * R_INNER, 28.0);
        self.label(name, &self.text.display_center, &title, p.text, 1.0);
        let key_line = Rect::new(-R_INNER + 8.0, -4.0, 2.0 * R_INNER - 16.0, 16.0);
        self.label(key, &self.text.caption_center, &key_line, p.text_muted, 1.0);
        let line = match lit.and_then(|s| w.item(s)) {
            Some(item) if matches!(item.action, Action::Convert(_)) => {
                format!("{} → {}", w.source_code, item.label)
            }
            _ => w.source_code.clone(),
        };
        let src = Rect::new(-R_INNER + 8.0, 14.0, 2.0 * R_INNER - 16.0, 16.0);
        self.code(&line, &src, p.text_muted, 1.0)?;
        Ok(())
    }

    /// The ring More pours open: every other format, grouped by kind, each
    /// group led by its kind's icon.
    fn outer_ring(&self, wv: &WheelView, p: &Palette) -> Result<()> {
        let w = wv.wheel;
        let slots = wheel::outer_slots(&w.more);
        let band = self.sector(0.0, TAU, R_MORE_INNER, R_MORE_OUTER, true)?;
        // The groove between the dish and this ring is solid ground, so
        // the list behind never shows through it.
        let groove = self.sector(0.0, TAU, R_OUTER - 1.0, R_MORE_INNER, true)?;
        unsafe {
            self.rt
                .FillGeometry(&band, self.paint(p.surface, 1.0), None);
            self.rt
                .FillGeometry(&groove, self.paint(p.ground, 1.0), None);
        }
        self.ring_edge(R_MORE_INNER + 0.5, p.line)?;
        self.ring_edge(R_MORE_OUTER - 0.5, p.line)?;

        let (lit, wash) = Self::lit(wv);
        if let Some(Spot::Outer(j)) = lit {
            if let Some(slot) = slots.iter().find(|s| s.item == Some(j)) {
                let band = Band::new(slot.a0, slot.a1, R_MORE_INNER, R_MORE_OUTER, false);
                self.light(band, wash, 1.0, p)?;
            }
        }

        // Dividers only where a group starts.
        for slot in slots.iter().filter(|s| s.item.is_none()) {
            let (c, si) = (slot.a0.cos(), slot.a0.sin());
            unsafe {
                self.rt.DrawLine(
                    pt(c * R_MORE_INNER, si * R_MORE_INNER),
                    pt(c * R_MORE_OUTER, si * R_MORE_OUTER),
                    self.paint(p.line, 1.0),
                    shape::BORDER,
                    None,
                );
            }
        }

        for slot in &slots {
            let (x, y) = wheel::outer_point(slot);
            match slot.item {
                // A group's kind, small and quiet.
                None => self.icon(slot.icon, x - 9.0, y - 9.0, 0.75, p.text_muted, 1.0),
                Some(j) => {
                    let r = Rect::new(x - 26.0, y - 8.0, 52.0, 16.0);
                    self.code(&w.more[j].label, &r, p.text, 1.0)?;
                    if wv.marks.contains(&Spot::Outer(j)) {
                        self.dot(x, r.y - DOT_GAP, wv.dot, p);
                    }
                }
            }
        }
        Ok(())
    }

    /// The suggestion's celadon dot, centred on `(x, y)`. It takes the
    /// ring's own opacity, and never moves or changes size.
    fn dot(&self, x: f32, y: f32, a: f32, p: &Palette) {
        unsafe {
            self.rt.FillEllipse(
                &D2D1_ELLIPSE {
                    point: pt(x, y),
                    radiusX: DOT_R,
                    radiusY: DOT_R,
                },
                self.paint(p.accent, a),
            );
        }
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
        self.button(
            "Close",
            &d.close,
            v.dialog_close_hover,
            v.dialog_close_pressed,
            p,
        );
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
        self.tabs(sv, p);
        // A new page fades in where the last one was.
        self.with_opacity(sv.page.clamp(0.0, 1.0), || self.settings_page(sv, p))?;

        for (k, (control, r)) in l.controls.iter().enumerate() {
            let (hover, pressed) = (sv.hover == Some(k), sv.pressed == Some(k));
            match control {
                Control::Close => self.button("Close", r, hover, pressed, p),
                Control::CheckUpdates => self.button(sv.update_button, r, hover, pressed, p),
                _ => continue,
            }
            if sv.focus == Some(k) {
                self.focus_ring(r, p);
            }
        }
        let (note, muted) = sv.update;
        let c = if muted { p.text_muted } else { p.text };
        self.label(note, &self.text.caption_wrap, &l.update_note, c, 1.0);
        Ok(())
    }

    /// The page tabs: one segmented control whose celadon marker slides to
    /// the chosen page.
    fn tabs(&self, sv: &SettingsView, p: &Palette) {
        let l = sv.layout;
        let tabs: Vec<(usize, SettingsTab, Rect)> = l
            .controls
            .iter()
            .enumerate()
            .filter_map(|(k, (c, r))| match c {
                Control::Tab(t) => Some((k, *t, *r)),
                _ => None,
            })
            .collect();
        let Some(&(_, _, first)) = tabs.first() else {
            return;
        };
        for &(k, t, r) in &tabs {
            if sv.hover == Some(k) && t != l.tab {
                self.fill(&r, shape::RADIUS_CONTROL, p.line, 1.0);
            }
        }
        let marker = Rect::new(first.x + sv.tab_marker * first.w, first.y, first.w, first.h);
        self.fill(
            &marker,
            shape::RADIUS_CONTROL,
            p.accent,
            effects::HOVER_FILL,
        );
        for &(k, t, r) in &tabs {
            if sv.pressed == Some(k) {
                self.press_shadow(&r, p);
            }
            let c = if t == l.tab { p.text } else { p.text_muted };
            self.label(t.label(), &self.text.body_center, &r, c, 1.0);
        }
        let segs: Vec<(bool, Rect)> = tabs.iter().map(|&(_, _, r)| (false, r)).collect();
        self.segment_group(&segs, p);
        self.border(&marker, shape::RADIUS_CONTROL, p.accent, 1.0, shape::BORDER);
        for &(k, _, r) in &tabs {
            if sv.focus == Some(k) {
                self.focus_ring(&r, p);
            }
        }
    }

    /// The current page's rows and controls.
    fn settings_page(&self, sv: &SettingsView, p: &Palette) -> Result<()> {
        let l = sv.layout;
        for (i, row) in l.rows.iter().enumerate() {
            self.label(
                row.label,
                &self.text.body,
                &row.label_rect,
                p.text_muted,
                1.0,
            );
            let Some(r) = row.value else { continue };
            let (text, muted) = match l.tab {
                SettingsTab::Engines => sv.engines[i],
                _ => sv.output,
            };
            let c = if muted { p.text_muted } else { p.text };
            self.label(text, &self.text.body, &r, c, 1.0);
            if let (Some(hint), Some(hr)) = (row.hint, row.hint_rect) {
                self.label(hint, &self.text.caption_wrap, &hr, p.text_muted, 1.0);
            }
        }

        // Segmented groups, outlined once each after their fills.
        let mut groups: Vec<(Control, Vec<(bool, Rect)>)> = vec![];
        for (k, (control, r)) in l.controls.iter().enumerate() {
            let (hover, pressed) = (sv.hover == Some(k), sv.pressed == Some(k));
            let chosen = match control {
                Control::OutputChoose | Control::EngineChoose(_) => {
                    self.button("Choose", r, hover, pressed, p);
                    None
                }
                Control::OutputReset | Control::EngineReset(_) => {
                    self.button("Reset", r, hover, pressed, p);
                    None
                }
                Control::Theme(t) => {
                    let chosen = *t == sv.theme;
                    self.segment(t.label(), r, hover, pressed, chosen, p);
                    Some(chosen)
                }
                Control::Suggestions(on) => {
                    let chosen = *on == sv.suggestions;
                    let label = if *on { "On" } else { "Off" };
                    self.segment(label, r, hover, pressed, chosen, p);
                    Some(chosen)
                }
                Control::ForgetPicks if sv.has_habits => {
                    self.button("Forget picks", r, hover, pressed, p);
                    None
                }
                Control::ForgetPicks => {
                    // Disabled: the label goes muted, with no hover or press.
                    self.button_face(r, false, false, false, p);
                    let text = "Forget picks";
                    self.label(text, &self.text.body_center, r, p.text_muted, 1.0);
                    None
                }
                Control::Preset(s, preset) => {
                    let chosen = sv.quality.get(*s) == Value::Preset(*preset);
                    self.segment(preset.label(), r, hover, pressed, chosen, p);
                    Some(chosen)
                }
                Control::Step(s) => {
                    let text = s.label(sv.quality.get(*s));
                    self.stepper(&text, r, hover, pressed, p);
                    None
                }
                Control::Key(wheel) => {
                    self.button_face(r, hover, pressed, false, p);
                    let name = Rect::new(r.x + 12.0, r.y, r.w - 12.0, r.h);
                    self.label(wheel.title(), &self.text.body, &name, p.text, 1.0);
                    let code = sv.keys.key_for(*wheel).code();
                    let (_, cw) = self.text.code_layout(code, r.w, r.h)?;
                    let cr = Rect::new(r.right() - 12.0 - cw.ceil(), r.y, cw.ceil(), r.h);
                    self.code(code, &cr, p.text_muted, 1.0)?;
                    None
                }
                Control::Tab(_) | Control::CheckUpdates | Control::Close => continue,
            };
            if let Some(chosen) = chosen {
                match groups.last_mut() {
                    Some((c, segs)) if same_group(*c, *control) => segs.push((chosen, *r)),
                    _ => groups.push((*control, vec![(chosen, *r)])),
                }
            }
        }
        for (_, segs) in &groups {
            self.segment_group(segs, p);
        }
        for (k, (control, r)) in l.controls.iter().enumerate() {
            let page = !matches!(
                control,
                Control::Tab(_) | Control::CheckUpdates | Control::Close
            );
            if page && sv.focus == Some(k) {
                self.focus_ring(r, p);
            }
        }
        Ok(())
    }

    /// The options sheet: what the source is, the choices that fit it, and
    /// what they will make. It rises a little as it fades in.
    fn sheet(&self, w: f32, h: f32, sv: &SheetView, p: &Palette) -> Result<()> {
        let a = (sv.open * sv.fade).clamp(0.0, 1.0);
        if a <= 0.0 {
            return Ok(());
        }
        self.fill(&Rect::new(0.0, 0.0, w, h), 0.0, p.ground, 0.7 * a);
        let rise = 8.0 * (1.0 - sv.open);
        let result = self.with_opacity(a, || {
            self.set_base(scale_at(1.0, 0.0, rise));
            let l = sv.layout;
            self.fill(&l.panel, shape::RADIUS_PANEL, p.surface, 1.0);
            self.border(&l.panel, shape::RADIUS_PANEL, p.line, 1.0, shape::BORDER);
            self.label(sv.title, &self.text.display, &l.title, p.text, 1.0);
            self.label(
                sv.context,
                &self.text.caption,
                &l.context,
                p.text_muted,
                1.0,
            );
            for (r, (name, _)) in l.labels.iter().zip(sv.rows) {
                self.label(name, &self.text.body, r, p.text_muted, 1.0);
            }
            let mut presets = vec![];
            for (k, (c, r)) in l.controls.iter().enumerate() {
                let (hover, pressed) = (sv.hover == Some(k), sv.pressed == Some(k));
                match c {
                    SheetControl::Preset(_, preset) => {
                        let chosen = *preset == sv.preset;
                        self.segment(preset.label(), r, hover, pressed, chosen, p);
                        presets.push((chosen, *r));
                    }
                    SheetControl::Step(row) => {
                        let text = sv.rows.get(*row).map_or("", |(_, v)| v.as_str());
                        self.stepper(text, r, hover, pressed, p);
                    }
                    SheetControl::Cancel => self.button("Cancel", r, hover, pressed, p),
                    SheetControl::Convert => self.primary("Convert", r, hover, pressed, p),
                }
            }
            self.segment_group(&presets, p);
            for (k, (_, r)) in l.controls.iter().enumerate() {
                if sv.focus == Some(k) {
                    self.focus_ring(r, p);
                }
            }
            let line = Rect::new(l.result.x, l.result.y + 8.0, l.result.w, 16.0);
            self.label(sv.result, &self.text.caption, &line, p.text_muted, 1.0);
            Ok(())
        });
        self.set_base(Matrix3x2::identity());
        result
    }
}

/// A stretch of ring between two angles and two radii; with `full`, the
/// whole ring.
#[derive(Clone, Copy)]
struct Band {
    a0: f32,
    a1: f32,
    r0: f32,
    r1: f32,
    full: bool,
}

impl Band {
    fn new(a0: f32, a1: f32, r0: f32, r1: f32, full: bool) -> Self {
        Self {
            a0,
            a1,
            r0,
            r1,
            full,
        }
    }
}

/// Whether two controls are segments of one group.
fn same_group(a: Control, b: Control) -> bool {
    match (a, b) {
        (Control::Theme(_), Control::Theme(_))
        | (Control::Suggestions(_), Control::Suggestions(_)) => true,
        (Control::Preset(x, _), Control::Preset(y, _)) => x == y,
        _ => false,
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
