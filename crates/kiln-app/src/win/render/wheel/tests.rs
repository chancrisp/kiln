use super::*;
use crate::theme::fluent::{FileKind, FluentPalette};
use kiln_core::Tool;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;

/// The test frames are this many DIPs square, with the wheel in the
/// middle: room for the ring, its shadow and More's list beside it.
const SIDE: u32 = 920;
const MID: f32 = SIDE as f32 / 2.0;

/// A premultiplied BGRA bitmap and a render target drawing into it,
/// `side` DIPs square at `k` pixels a DIP (1 at 100%, 1.5 at 150%).
struct Canvas {
    factory: ID2D1Factory,
    bitmap: IWICBitmap,
    rt: ID2D1RenderTarget,
    side: u32,
    k: f32,
}

/// A drawn frame's pixels, and where its wheel's centre is.
struct Frame {
    px: Vec<u8>,
    side: u32,
    k: f32,
    mid: f32,
}

impl Frame {
    /// The pixel at `(x, y)` DIPs from the wheel's centre: B, G, R, A.
    fn at(&self, x: f32, y: f32) -> [u8; 4] {
        let px = ((self.mid + x) * self.k).floor() as usize;
        let py = ((self.mid + y) * self.k).floor() as usize;
        let i = (py * self.side as usize + px) * 4;
        [self.px[i], self.px[i + 1], self.px[i + 2], self.px[i + 3]]
    }

    /// Every pixel, as its centre's DIPs from the wheel's centre.
    fn each(&self) -> impl Iterator<Item = (f32, f32, [u8; 4])> + '_ {
        let n = self.side as usize;
        (0..n * n).map(move |i| {
            let (px, py) = (i % n, i / n);
            let x = (px as f32 + 0.5) / self.k - self.mid;
            let y = (py as f32 + 0.5) / self.k - self.mid;
            let p = &self.px[i * 4..i * 4 + 4];
            (x, y, [p[0], p[1], p[2], p[3]])
        })
    }
}

fn canvas() -> Canvas {
    canvas_at(SIDE, 1.0)
}

fn canvas_at(side_dips: u32, k: f32) -> Canvas {
    let side = (side_dips as f32 * k).round() as u32;
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let factory: ID2D1Factory =
            D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).unwrap();
        let wic: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).unwrap();
        let bitmap = wic
            .CreateBitmap(
                side,
                side,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapCacheOnDemand,
            )
            .unwrap();
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0 * k,
            dpiY: 96.0 * k,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let rt = factory
            .CreateWicBitmapRenderTarget(&bitmap, &props)
            .unwrap();
        rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        Canvas {
            factory,
            bitmap,
            rt,
            side,
            k,
        }
    }
}

/// The canvas's pixels, premultiplied BGRA, row by row.
fn pixels(c: &Canvas, mid: f32) -> Frame {
    let mut px = vec![0u8; (c.side * c.side * 4) as usize];
    unsafe {
        c.bitmap
            .CopyPixels(std::ptr::null(), c.side * 4, &mut px)
            .unwrap()
    };
    Frame {
        px,
        side: c.side,
        k: c.k,
        mid,
    }
}

fn format(i: usize, kind: Kind) -> wheel::Item {
    wheel::Item {
        label: ["JPG", "WEBP", "AVIF", "GIF", "BMP", "TIFF", "MP4", "MP3"][i % 8].into(),
        is_code: true,
        action: Action::Convert(
            ["jpg", "webp", "avif", "gif", "bmp", "tiff", "mp4", "mp3"][i % 8].into(),
        ),
        kind: Some(kind),
    }
}

fn smaller() -> wheel::Item {
    wheel::Item {
        label: "Make smaller".into(),
        is_code: false,
        action: Action::Tool(Tool::Smaller, Kind::Image),
        kind: None,
    }
}

fn convert_wheel(n: usize) -> Wheel {
    Wheel {
        kind: wheel::WheelKind::Convert,
        origin: wheel::Origin::Window,
        items: (0..n).map(|i| format(i, Kind::Image)).collect(),
        flyout: vec![],
        center: (MID, MID),
        scale: 1.0,
        source_code: "PNG".into(),
        source_kind: Some(Kind::Image),
        list_only: false,
        settable: vec![],
    }
}

/// Make smaller, five formats and More, as a photo's ring is (mock 7).
fn photo_wheel() -> Wheel {
    let mut items = vec![smaller()];
    items.extend((0..5).map(|i| format(i, Kind::Image)));
    Wheel {
        items,
        flyout: wheel::grouped(vec![
            format(4, Kind::Image),
            format(5, Kind::Image),
            wheel::Item {
                label: "Resize".into(),
                is_code: false,
                action: Action::Tool(Tool::Resize, Kind::Image),
                kind: None,
            },
        ]),
        ..convert_wheel(0)
    }
}

fn hub(pair: &str, hint: &str) -> wheel::HubLines {
    wheel::HubLines {
        pair: pair.into(),
        hint: hint.into(),
    }
}

/// A wheel fully open, at rest, nothing lit.
fn view(w: &Wheel) -> WheelView<'_> {
    WheelView {
        wheel: w,
        hover: None,
        hub: hub("PNG", "Pick a format"),
        opacity: 1.0,
        grow: 1.0,
        smoke: false,
        flyout: None,
        thumb: None,
        kind: Some(Kind::Image),
    }
}

/// More's list open beside `w`, in a canvas `SIDE` square.
fn open_list(w: &Wheel, lit: Option<Spot>) -> FlyoutView<'_> {
    let rows = w.flyout_rows(Some(Spot::Inner(1)));
    let bounds = layout::Rect::new(0.0, 0.0, SIDE as f32, SIDE as f32);
    let place = wheel::flyout_place(w, &rows, bounds, 0.0);
    FlyoutView {
        rows,
        place,
        lit,
        shown: 1.0,
    }
}

fn dark() -> FluentPalette {
    crate::theme::fluent::DARK
}

fn light() -> FluentPalette {
    crate::theme::fluent::LIGHT
}

fn contrast() -> FluentPalette {
    crate::theme::fluent::contrast_palette(crate::theme::SysColors {
        window: Rgb::hex(0x000000),
        window_text: Rgb::hex(0xFFFFFF),
        highlight: Rgb::hex(0x1AEBFF),
        highlight_text: Rgb::hex(0x000000),
        gray_text: Rgb::hex(0x3FF23F),
        hotlight: Rgb::hex(0xFFFF00),
        button_face: Rgb::hex(0x000000),
        button_text: Rgb::hex(0xFFFFFF),
    })
}

/// The wheel drawn as the main window draws it, with resources made as
/// `Renderer` makes them, over a clear canvas.
fn main_window_frame(c: &Canvas, wv: &WheelView, p: &FluentPalette) -> Frame {
    let text = Text::new().unwrap();
    let rt = &c.rt;
    let round = round_stroke(&c.factory).unwrap();
    let dash = dash_stroke(&c.factory).unwrap();
    let mark = mark_geometry(&c.factory).unwrap();
    let brush = unsafe { rt.CreateSolidColorBrush(&color(p.text, 1.0), None).unwrap() };
    let wheel = RefCell::new(WheelResources::new(rt, &text.factory).unwrap());
    let pictures = RefCell::new(HashMap::new());
    let clips = RefCell::new(cache::clips());
    let d = Draw {
        factory: &c.factory,
        rt,
        dpi: 96.0 * c.k,
        zoom: 1.0,
        brush: &brush,
        wheel: &wheel,
        text: &text,
        round: &round,
        dash: &dash,
        mark: &mark,
        base: Cell::new(Matrix3x2::identity()),
        pictures: &pictures,
        frame: 1,
        clips: &clips,
    };
    unsafe {
        rt.BeginDraw();
        rt.SetTransform(&Matrix3x2::identity());
        rt.Clear(Some(&D2D1_COLOR_F::default()));
    }
    let side = c.side as f32 / c.k;
    d.wheel(wv, p, side, side).unwrap();
    unsafe { rt.EndDraw(None, None).unwrap() };
    pixels(c, wv.wheel.center.0)
}

/// The wheel drawn through `draw_wheel_on`, with resources of its own,
/// over a clear canvas.
fn frame(c: &Canvas, wv: &WheelView, extras: &WheelExtras, p: &FluentPalette) -> Frame {
    let dwrite: IDWriteFactory =
        unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).unwrap() };
    let mut res = WheelResources::new(&c.rt, &dwrite).unwrap();
    unsafe {
        c.rt.BeginDraw();
        c.rt.Clear(Some(&D2D1_COLOR_F::default()));
    }
    draw_wheel_on(&c.rt, &mut res, wv, extras, p, 96.0 * c.k, 1.0).unwrap();
    unsafe { c.rt.EndDraw(None, None).unwrap() };
    pixels(c, wv.wheel.center.0)
}

/// `c` as an opaque pixel.
fn opaque(c: Rgb) -> [u8; 4] {
    [c.2, c.1, c.0, 255]
}

fn distance(px: [u8; 4], c: Rgb) -> u32 {
    let o = opaque(c);
    (0..3).map(|i| px[i].abs_diff(o[i]) as u32).sum::<u32>() + px[3].abs_diff(255) as u32
}

/// Whether `px` is one of `colours` or an edge's blend of two of them,
/// as antialiasing makes: within a few levels of the line between them.
fn on_a_blend(px: [u8; 4], colours: &[Rgb]) -> bool {
    let v = |c: Rgb| [c.2 as f32, c.1 as f32, c.0 as f32];
    let q = [px[0] as f32, px[1] as f32, px[2] as f32];
    colours.iter().any(|&a| {
        colours.iter().any(|&b| {
            let (a, b) = (v(a), v(b));
            let d: Vec<f32> = (0..3).map(|i| b[i] - a[i]).collect();
            let len = d.iter().map(|x| x * x).sum::<f32>();
            let t = if len == 0.0 {
                0.0
            } else {
                ((0..3).map(|i| (q[i] - a[i]) * d[i]).sum::<f32>() / len).clamp(0.0, 1.0)
            };
            (0..3)
                .map(|i| (q[i] - (a[i] + t * d[i])).abs())
                .sum::<f32>()
                <= 9.0
        })
    })
}

/// The point `r` from the centre at angle `a`.
fn polar(r: f32, a: f32) -> (f32, f32) {
    (r * a.cos(), r * a.sin())
}

/// Whether any pixel in the box `(x, y, w, h)` about the centre is within
/// `slack` of `c`.
fn any_near(f: &Frame, (x, y, w, h): (f32, f32, f32, f32), c: Rgb, slack: u32) -> bool {
    f.each().any(|(px, py, p)| {
        px >= x && px <= x + w && py >= y && py <= y + h && distance(p, c) <= slack
    })
}

/// Where segment `i` of `n` is plain face: by its inner edge on its
/// middle, clear of its label and its kind's arc.
fn face_point(i: usize, n: usize) -> (f32, f32) {
    let (a0, a1) = wheel::segment(i, n);
    polar(wheel::R_INNER + 6.0, (a0 + a1) / 2.0)
}

#[test]
fn segments_are_the_overlay_two_dips_apart_at_100_125_150_and_200_percent() {
    let w = convert_wheel(6);
    for k in [1.0, 1.25, 1.5, 2.0] {
        let c = canvas_at(SIDE, k);
        for p in [dark(), light()] {
            let f = frame(&c, &view(&w), &WheelExtras::default(), &p);
            for i in 0..6 {
                let (x, y) = face_point(i, 6);
                assert_eq!(f.at(x, y), opaque(p.overlay), "{} {k} {i}", p.name);
                // Across the gap after it, at the middle of the ring: the
                // faces either side, and between them only the shadow.
                let (_, a1) = wheel::segment(i, 6);
                let r = (wheel::R_INNER + wheel::R_OUTER) / 2.0;
                let (nx, ny) = (-a1.sin(), a1.cos());
                let (bx, by) = polar(r, a1);
                let side = |off: f32| f.at(bx + nx * off, by + ny * off);
                assert_eq!(side(-3.0), opaque(p.overlay), "{} {k}", p.name);
                assert_eq!(side(3.0), opaque(p.overlay), "{} {k}", p.name);
                let gap = (-4..=4).map(|j| side(j as f32 * 0.25)[3]).min().unwrap();
                assert!(gap < 140, "{} {k} {i}: the gap shows ({gap})", p.name);
            }
        }
    }
}

#[test]
fn the_lit_segment_is_solid_accent_with_on_accent_ink() {
    let w = convert_wheel(6);
    let c = canvas();
    for p in [dark(), light()] {
        let mut v = view(&w);
        v.hover = Some(Spot::Inner(2));
        let f = frame(&c, &v, &WheelExtras::default(), &p);
        let (x, y) = face_point(2, 6);
        assert_eq!(f.at(x, y), opaque(p.accent), "{}", p.name);
        let (x, y) = face_point(1, 6);
        assert_eq!(f.at(x, y), opaque(p.overlay), "{}", p.name);
        // Its code is in the accent's ink; another's in the text colour.
        let (lx, ly) = wheel::label_point(2, 6);
        assert!(
            any_near(&f, (lx - 20.0, ly - 8.0, 40.0, 16.0), p.on_accent, 40),
            "{}",
            p.name
        );
        let (lx, ly) = wheel::label_point(1, 6);
        assert!(
            any_near(&f, (lx - 20.0, ly - 8.0, 40.0, 16.0), p.text, 40),
            "{}",
            p.name
        );
    }
}

/// Six formats of two kinds, as a video's ring is: its pictures, then its
/// sound.
fn mixed_wheel() -> Wheel {
    let mut w = convert_wheel(6);
    for i in 3..6 {
        w.items[i].kind = Some(Kind::Audio);
    }
    w
}

#[test]
fn a_formats_kind_is_a_two_dip_arc_inside_the_outer_edge_when_the_kinds_differ() {
    let w = mixed_wheel();
    let c = canvas_at(SIDE, 2.0);
    let p = dark();
    let mut v = view(&w);
    v.hover = Some(Spot::Inner(0));
    let f = frame(&c, &v, &WheelExtras::default(), &p);
    let r = wheel::R_OUTER - wheel::KIND_ARC_INSET;
    // A sound format's arc, in its kind's colour.
    let (a0, a1) = wheel::segment(3, 6);
    let (x, y) = polar(r, (a0 + a1) / 2.0);
    assert!(
        distance(f.at(x, y), p.kind(FileKind::Audio)) <= 12,
        "{:?}",
        f.at(x, y)
    );
    // Clear of it, the face.
    let (x, y) = polar(r - 4.0, (a0 + a1) / 2.0);
    assert_eq!(f.at(x, y), opaque(p.overlay));
    // A picture format has its own colour's arc.
    let (c0, c1) = wheel::segment(4 - 3, 6);
    let (x, y) = polar(r, (c0 + c1) / 2.0);
    assert!(
        distance(f.at(x, y), p.kind(FileKind::Image)) <= 12,
        "{:?}",
        f.at(x, y)
    );
    // The lit segment has none: its face runs flat to the edge.
    let (b0, b1) = wheel::segment(0, 6);
    let (x, y) = polar(r, (b0 + b1) / 2.0);
    assert_eq!(f.at(x, y), opaque(p.accent));
}

#[test]
fn formats_of_one_kind_have_no_arcs() {
    // A photo's formats are all pictures: an arc on each would say nothing
    // and a PDF's olive among them looked like a fault.
    let w = convert_wheel(6);
    let c = canvas_at(SIDE, 2.0);
    for p in [dark(), light()] {
        let f = frame(&c, &view(&w), &WheelExtras::default(), &p);
        let r = wheel::R_OUTER - wheel::KIND_ARC_INSET;
        for i in 0..6 {
            let (a0, a1) = wheel::segment(i, 6);
            let (x, y) = polar(r, (a0 + a1) / 2.0);
            assert_eq!(f.at(x, y), opaque(p.overlay), "{} {i}", p.name);
        }
    }
}

#[test]
fn make_smaller_and_more_have_a_glyph_and_a_caption() {
    let w = photo_wheel();
    let c = canvas_at(SIDE, 2.0);
    let p = dark();
    let f = frame(&c, &view(&w), &WheelExtras::default(), &p);
    let n = w.segments();
    for i in [0, n - 1] {
        let (lx, ly) = wheel::label_point(i, n);
        // The glyph above the label point, in the text colour, and the
        // caption under it in the secondary colour.
        assert!(
            any_near(&f, (lx - 8.0, ly - 17.0, 16.0, 16.0), p.text, 40),
            "{i} glyph"
        );
        assert!(
            any_near(&f, (lx - 25.0, ly + 2.0, 50.0, 14.0), p.text_secondary, 40),
            "{i} caption"
        );
        // No kind arc on them.
        let (a0, a1) = wheel::segment(i, n);
        let (x, y) = polar(wheel::R_OUTER - wheel::KIND_ARC_INSET, (a0 + a1) / 2.0);
        assert_eq!(f.at(x, y), opaque(p.overlay), "{i}");
    }
}

#[test]
fn the_hub_is_the_base_with_the_files_tile_and_two_lines() {
    let w = convert_wheel(5);
    let c = canvas_at(SIDE, 2.0);
    for p in [dark(), light()] {
        let mut v = view(&w);
        v.hover = Some(Spot::Inner(1));
        v.hub = hub("PNG \u{2192} WEBP", "about 740 KB");
        let f = frame(&c, &v, &WheelExtras::default(), &p);
        // The disc of the base, by its edge.
        assert_eq!(f.at(46.0, 0.0), opaque(p.base), "{}", p.name);
        assert_eq!(f.at(-46.0, 0.0), opaque(p.base), "{}", p.name);
        // The pair in the text colour, the hint in the secondary.
        let r = HUB_PAIR;
        assert!(
            any_near(&f, (r.x, r.y, r.w, r.h), p.text, 30),
            "{} pair",
            p.name
        );
        let r = HUB_HINT;
        assert!(
            any_near(&f, (r.x, r.y, r.w, r.h), p.text_secondary, 30),
            "{} hint",
            p.name
        );
        // The tile with the kind's glyph in the kind's colour.
        let t = hub_thumb_rect();
        assert!(
            any_near(&f, (t.x, t.y, t.w, t.h), p.kind(FileKind::Image), 40),
            "{} tile",
            p.name
        );
        // Between the hub and the ring, the base too: no heavy ring of
        // whatever is behind the wheel.
        assert_eq!(
            f.at(0.0, -(wheel::R_HUB + 2.0)),
            opaque(p.base),
            "{}",
            p.name
        );
    }
}

#[test]
fn the_hubs_stack_is_centred_with_room_between_its_lines() {
    // the picture 32 DIPs, 4 above the pair; the pair a 20 line and
    // the hint a 16 line, one under the other and not overlapping; the whole
    // stack raised 4 so it sits about the hub's centre.
    let t = hub_thumb_rect();
    assert_eq!((t.w, t.h), (32.0, 32.0));
    assert_eq!(HUB_THUMB, 32.0);
    assert_eq!(t.x, -16.0);
    assert_eq!(HUB_PAIR.y - t.bottom(), 4.0);
    assert_eq!((HUB_PAIR.y, HUB_PAIR.h), (0.0, 20.0));
    assert_eq!((HUB_HINT.y, HUB_HINT.h), (20.0, 16.0));
    assert_eq!(HUB_HINT.y, HUB_PAIR.bottom(), "the lines don't overlap");
    // Top and bottom of the stack are level about the centre.
    assert_eq!(t.y, -HUB_HINT.bottom());
}

#[test]
fn every_word_the_hub_says_fits_inside_the_disc_where_it_is_drawn() {
    // "Formats and tools" was wider than the hub at its line and crossed
    // the edge. Each hint is measured in Caption and must fit the disc at
    // the lower part of its line; each pair in Body Strong at its line.
    use crate::win::text::{Ramp, Style};
    let text = Text::new().unwrap();
    let chord = |y: f32| 2.0 * (wheel::R_HUB * wheel::R_HUB - y * y).sqrt();
    // At every Text size: the words are set at the zoom, in layout units,
    // and the hub is in the same units.
    for zoom in [1.0, 1.5, 2.25] {
        let width = |s: &str, ramp: Ramp, line: f32| {
            text.styled(s, Style::new(ramp), zoom, 4096.0, line)
                .unwrap()
                .width
        };
        // Glyphs sit in the box's lower three quarters: 14 DIPs down a 16 line.
        let hint_room = chord(HUB_HINT.y + 14.0);
        for hint in [
            "Formats, tools",
            "Keeps JPG",
            "Pick a format",
            "Pick a tool",
            "about 740 KB",
            "9 files",
        ] {
            let w = width(hint, Ramp::Caption, 16.0);
            assert!(
                w <= hint_room,
                "{hint} is {w} wide, room {hint_room} at {zoom}"
            );
        }
        let pair_room = chord(HUB_PAIR.bottom() - 2.0);
        for pair in ["Make smaller", "JPG \u{2192} PNG", "More", "JPG \u{b7} 12"] {
            let w = width(pair, Ramp::BodyStrong, 20.0);
            assert!(
                w <= pair_room,
                "{pair} is {w} wide, room {pair_room} at {zoom}"
            );
        }
    }
}

#[test]
fn a_contrast_theme_draws_system_colours_with_edges_and_no_shadow() {
    let w = convert_wheel(6);
    // At 200% the 1-DIP edge is two whole pixels, so one is plain text
    // colour.
    let c = canvas_at(SIDE, 2.0);
    let p = contrast();
    let mut v = view(&w);
    v.hover = Some(Spot::Inner(3));
    let f = frame(&c, &v, &WheelExtras::default(), &p);
    let (x, y) = face_point(3, 6);
    assert_eq!(f.at(x, y), opaque(p.accent), "the lit one in Highlight");
    let (x, y) = face_point(0, 6);
    assert_eq!(f.at(x, y), opaque(p.overlay));
    // Every segment has a WindowText edge.
    let (a0, a1) = wheel::segment(0, 6);
    let mid = (a0 + a1) / 2.0;
    let edge = (0..32).any(|j| {
        // The outline is 1 DIP across, on the segment's outer edge.
        let (x, y) = polar(wheel::R_OUTER - 6.0 + j as f32 * 0.25, mid);
        distance(f.at(x, y), p.text) <= 30
    });
    assert!(edge, "a WindowText edge on the outer arc");
    // No shadow: clear just past the ring and under it.
    for (x, y) in [(0.0, wheel::R_OUTER + 4.0), (0.0, wheel::R_OUTER + 12.0)] {
        assert_eq!(f.at(x, y)[3], 0, "{x} {y}");
    }
    // Every opaque pixel is one of the system colours, or an edge's blend
    // of two of them.
    let allowed = [p.overlay, p.text, p.accent, p.on_accent, p.base];
    let off = f
        .each()
        .filter(|&(_, _, px)| px[3] == 255)
        .filter(|&(_, _, px)| !on_a_blend(px, &allowed))
        .count();
    assert_eq!(off, 0);
}

#[test]
fn the_ring_casts_the_flyout_shadow_in_kilns_own_themes() {
    let w = convert_wheel(6);
    let c = canvas();
    let f = frame(&c, &view(&w), &WheelExtras::default(), &dark());
    // Below the ring, where the shadow drops 8.
    let a = f.at(0.0, wheel::R_OUTER + 4.0)[3];
    assert!(a > 0 && a < 90, "{a}");
    // Far from it, clear.
    assert_eq!(f.at(0.0, wheel::R_OUTER + 40.0)[3], 0);
    assert_eq!(f.at(-MID + 1.0, -MID + 1.0)[3], 0);
}

#[test]
fn it_opens_scaled_and_faded_about_its_centre() {
    let w = convert_wheel(6);
    let c = canvas();
    let p = dark();
    let mut v = view(&w);
    v.grow = 0.92;
    v.opacity = 0.5;
    let f = frame(&c, &v, &WheelExtras::default(), &p);
    // At 0.92 the outer edge is further in.
    let (a0, a1) = wheel::segment(0, 6);
    let (x, y) = polar(wheel::R_OUTER - 3.0, (a0 + a1) / 2.0);
    assert!(f.at(x, y)[3] < 60, "{:?}", f.at(x, y));
    // And what shows is half there.
    let (x, y) = face_point(0, 6);
    let a = f.at(x * 0.92, y * 0.92)[3];
    assert!((120..=135).contains(&a), "{a}");
}

#[test]
fn mores_list_is_a_flyout_beside_the_ring_with_its_lit_line() {
    let w = photo_wheel();
    let c = canvas_at(SIDE, 1.25);
    for p in [dark(), light()] {
        let mut v = view(&w);
        let more = w.items.len();
        v.hover = Some(Spot::Flyout(1));
        v.flyout = Some(open_list(&w, Some(Spot::Flyout(1))));
        let f = frame(&c, &v, &WheelExtras::default(), &p);
        let fl = v.flyout.as_ref().unwrap();
        let rect = fl.place.rect;
        let rel = |x: f32, y: f32| (x - MID, y - MID);
        // The frame in the overlay, under a heading's line.
        let (x, y) = rel(rect.right() - 6.0, rect.y + 6.0);
        assert_eq!(f.at(x, y), opaque(p.overlay), "{}", p.name);
        // The lit line has the hover's fill.
        let lit = fl.place.rows[2];
        let hover = p.control_hover.over(p.overlay);
        let (x, y) = rel(lit.right() - 4.0, lit.y + 2.0);
        assert!(
            distance(f.at(x, y), hover) <= 3,
            "{} {:?}",
            p.name,
            f.at(x, y)
        );
        let other = fl.place.rows[1];
        let (x, y) = rel(other.right() - 4.0, other.y + 2.0);
        assert_eq!(f.at(x, y), opaque(p.overlay), "{}", p.name);
        // A format's code is plain text, as the side pane's is, with no
        // kind colour.
        let (x, y) = rel(other.x + FLYOUT_INDENT, other.y + 8.0);
        let code = (x, y, crate::layout::OFFER_CODE, 20.0);
        assert!(any_near(&f, code, p.text, 50), "{}", p.name);
        assert!(
            !any_near(&f, code, p.kind(FileKind::Image), 50),
            "{}",
            p.name
        );
        // More stays lit while its list is open, as the way back.
        let (x, y) = face_point(more, w.segments());
        assert_eq!(f.at(x, y), opaque(p.accent), "{}", p.name);
    }
    // Under a contrast theme the lit line is Highlight.
    let p = contrast();
    let mut v = view(&w);
    v.flyout = Some(open_list(&w, Some(Spot::Flyout(0))));
    let f = frame(&c, &v, &WheelExtras::default(), &p);
    let lit = v.flyout.as_ref().unwrap().place.rows[1];
    assert_eq!(
        f.at(lit.right() - 4.0 - MID, lit.y + 2.0 - MID),
        opaque(p.accent)
    );
}

/// The vertical middle, in window DIPs, of the ink drawn in `r` over the
/// overlay `bg`: the rows between the first and last that differ from it.
fn ink_middle(f: &Frame, r: &layout::Rect, bg: Rgb) -> Option<f32> {
    let (mut top, mut bottom) = (f32::MAX, f32::MIN);
    for (x, y, px) in f.each() {
        let (x, y) = (x + MID, y + MID);
        if x >= r.x && x < r.right() && y >= r.y && y < r.bottom() && distance(px, bg) > 90 {
            top = top.min(y);
            bottom = bottom.max(y + 1.0 / f.k);
        }
    }
    (top <= bottom).then(|| (top + bottom) / 2.0)
}

#[test]
fn a_format_rows_words_are_centred_in_the_row_with_its_code() {
    // A description of one line, or of two, sits in the middle of its row,
    // on the same middle as the code, not at the top of the row.
    let w = photo_wheel();
    let c = canvas();
    for p in [dark(), light()] {
        let mut v = view(&w);
        v.flyout = Some(open_list(&w, None));
        let f = frame(&c, &v, &WheelExtras::default(), &p);
        let fl = v.flyout.as_ref().unwrap();
        let mut checked = 0;
        for (row, r) in fl.rows.iter().zip(&fl.place.rows) {
            let wheel::FlyoutRow::Item(_, item) = row else {
                continue;
            };
            let Action::Convert(ext) = &item.action else {
                continue;
            };
            assert!(wheel::format_words(ext).is_some(), "{ext} has words");
            let words = crate::layout::offer_words(r, 0.0);
            let code = crate::layout::offer_code_column(r);
            let bg = p.overlay;
            let (Some(mw), Some(mc)) = (ink_middle(&f, &words, bg), ink_middle(&f, &code, bg))
            else {
                panic!("{} {ext}: nothing drawn", p.name);
            };
            let mid = r.y + r.h / 2.0;
            assert!(
                (mw - mid).abs() <= 3.0,
                "{} {ext}: words at {mw}, row middle {mid}",
                p.name
            );
            assert!(
                (mw - mc).abs() <= 2.5,
                "{} {ext}: {mw} vs code {mc}",
                p.name
            );
            checked += 1;
        }
        assert!(checked >= 2, "{} rows checked", p.name);
    }
}

#[test]
fn the_list_alone_has_no_ring_and_shows_its_scroll_indicator_and_footer_at_rest() {
    // A list taller than its room: the indicator is there with the pointer
    // away, the footer row is under a rule, and no ring is drawn.
    let mut w = photo_wheel();
    w.items = vec![];
    w.list_only = true;
    // The frame's origin is the wheel's centre, on both axes.
    w.center = (MID + 300.0, MID + 300.0);
    w.flyout = wheel::grouped((0..30).map(|i| format(i, Kind::Image)).collect());
    w.settable = vec!["jpg".into()];
    let c = canvas();
    for p in [dark(), light()] {
        let rows = w.flyout_rows(None);
        let bounds = layout::Rect::new(0.0, 0.0, SIDE as f32, 420.0);
        let place = wheel::flyout_place(&w, &rows, bounds, 0.0);
        assert!(place.max_scroll() > 0.0);
        let fv = FlyoutView {
            rows,
            place,
            lit: None,
            shown: 1.0,
        };
        let rect = fv.place.rect;
        let (body, footer) = (fv.place.body, fv.place.footer.unwrap());
        let mut v = view(&w);
        v.flyout = Some(fv);
        let f = frame(&c, &v, &WheelExtras::default(), &p);
        let rel = |x: f32, y: f32| (x - w.center.0, y - w.center.0);
        // No ring: where the face would be, right of the list, is clear.
        let (x, y) = rel(w.center.0 + 100.0, w.center.1);
        assert_eq!(f.at(x, y)[3], 0, "{}", p.name);
        // The list is there.
        let (x, y) = rel(rect.x + 30.0, rect.y + 2.0);
        assert_eq!(f.at(x, y), opaque(p.overlay), "{}", p.name);
        // The thumb at rest, in the strong stroke over the overlay.
        let thumb = crate::layout::kit::scroll_thumb(
            body,
            body.h,
            c_content(&v),
            0.0,
            crate::theme::fluent::space::SCROLL_THUMB_REST,
        )
        .expect("a thumb");
        let (x, y) = rel(thumb.x + thumb.w / 2.0, thumb.y + thumb.h / 2.0);
        let want = p.strong_stroke.over(p.overlay);
        assert!(
            distance(f.at(x, y), want) <= 6,
            "{} {:?}",
            p.name,
            f.at(x, y)
        );
        // The footer: its rule, and its words in the ordinary ink (it is
        // usable whatever is lit).
        let (x, y) = rel(footer.x + 40.0, footer.y);
        assert_ne!(f.at(x, y), opaque(p.overlay), "{} the rule", p.name);
        let words = crate::layout::offer_words(&footer, 0.0);
        let (x, y) = rel(words.x, words.y);
        assert!(
            any_near(&f, (x, y, words.w, words.h), p.text, 120),
            "{} the footer row's words",
            p.name
        );
    }
}

#[test]
fn the_list_alone_draws_nothing_of_the_hub_not_even_the_files_thumbnail() {
    // Opened from the side pane there is no ring and no hub: a file's
    // picture must not be drawn where the hub's tile would be.
    let mut w = photo_wheel();
    w.items = vec![];
    w.list_only = true;
    w.center = (MID, MID);
    let c = canvas();
    let red = Arc::new(crate::previews::Pixels {
        width: 4,
        height: 4,
        bgra: [0u8, 0, 255, 255].repeat(16),
        icon: false,
    });
    for p in [dark(), light()] {
        let mut v = view(&w);
        v.thumb = Some((red.clone(), 1, 1.0));
        let f = main_window_frame(&c, &v, &p);
        let t = hub_thumb_rect();
        for (dx, dy) in [(0.5, 0.5), (0.2, 0.8), (0.8, 0.2)] {
            let (x, y) = (t.x + t.w * dx, t.y + t.h * dy);
            assert_eq!(f.at(x, y)[3], 0, "{} thumbnail at {x},{y}", p.name);
        }
    }
}

/// The scrolling lines' height of the list a view draws.
fn c_content(v: &WheelView) -> f32 {
    v.flyout.as_ref().unwrap().place.content
}

#[test]
fn over_the_window_the_smoke_is_twenty_percent_and_the_rest_is_the_same() {
    let w = convert_wheel(6);
    let c = canvas();
    let p = dark();
    let mut v = view(&w);
    v.smoke = true;
    let f = main_window_frame(&c, &v, &p);
    let corner = f.at(-MID + 2.0, -MID + 2.0);
    assert_eq!(corner[0..3], [0, 0, 0]);
    assert!((49..=53).contains(&corner[3]), "{corner:?}");
    let (x, y) = face_point(0, 6);
    assert_eq!(f.at(x, y), opaque(p.overlay));
    // Without smoke, as the drag-wheel draws it, the corner is clear.
    v.smoke = false;
    let f = main_window_frame(&c, &v, &p);
    assert_eq!(f.at(-MID + 2.0, -MID + 2.0)[3], 0);
}

#[test]
fn the_floating_wheels_caption_sits_in_a_pill_under_the_ring() {
    let w = Wheel {
        items: vec![],
        ..convert_wheel(0)
    };
    let c = canvas();
    let p = dark();
    let mut v = view(&w);
    v.hub = hub("No formats", "");
    let extras = WheelExtras {
        caption: Some("Kiln can't convert XYZ files."),
    };
    let f = frame(&c, &v, &extras, &p);
    let y = wheel::R_OUTER + CAPTION_GAP + 4.0;
    assert_eq!(f.at(0.0, y)[3], 255);
    assert!(any_near(&f, (-80.0, y, 160.0, 20.0), p.text, 40));
}

#[test]
fn segment_shapes_are_made_once_and_kept() {
    let w = convert_wheel(6);
    let c = canvas();
    let dwrite: IDWriteFactory =
        unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).unwrap() };
    let mut res = WheelResources::new(&c.rt, &dwrite).unwrap();
    for lit in [None, Some(Spot::Inner(1)), Some(Spot::Inner(4))] {
        let mut v = view(&w);
        v.hover = lit;
        v.grow = 0.95;
        unsafe { c.rt.BeginDraw() };
        draw_wheel_on(
            &c.rt,
            &mut res,
            &v,
            &WheelExtras::default(),
            &dark(),
            96.0,
            1.0,
        )
        .unwrap();
        unsafe { c.rt.EndDraw(None, None).unwrap() };
        assert_eq!(res.segments_made(), 6);
    }
}

#[test]
fn shots_of_the_wheel_for_the_record() {
    // Not a check: the wheel and More's list in each look at 125%, to
    // look at beside the mock.
    let w = photo_wheel();
    let c = canvas_at(SIDE, 1.25);
    for p in [dark(), light(), contrast()] {
        for (name, lit, list) in [
            ("rest", None, false),
            ("lit", Some(Spot::Inner(1)), false),
            ("more", Some(Spot::Flyout(1)), true),
        ] {
            let mut v = view(&w);
            v.hover = lit;
            if lit == Some(Spot::Inner(1)) {
                v.hub = hub("PNG \u{2192} JPG", "about 1.1 MB");
            }
            if list {
                v.flyout = Some(open_list(&w, lit));
            }
            let _ = frame(&c, &v, &WheelExtras::default(), &p);
            save(&c, &format!("wheel-{}-{name}-125.png", p.name));
        }
    }
}

/// Write the canvas to the render shots, as `shots.rs` does, over the
/// base so its alpha reads.
fn save(c: &Canvas, name: &str) {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::GENERIC_WRITE;
    let exe = std::env::current_exe().unwrap();
    let dir = exe.ancestors().nth(3).unwrap().join("render-shots");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let mut px = vec![0u8; (c.side * c.side * 4) as usize];
    unsafe {
        c.bitmap
            .CopyPixels(std::ptr::null(), c.side * 4, &mut px)
            .unwrap();
        let wic: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).unwrap();
        let stream = wic.CreateStream().unwrap();
        stream
            .InitializeFromFilename(&HSTRING::from(path.as_os_str()), GENERIC_WRITE.0)
            .unwrap();
        let encoder = wic
            .CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())
            .unwrap();
        encoder
            .Initialize(&stream, WICBitmapEncoderNoCache)
            .unwrap();
        let mut frame = None;
        encoder
            .CreateNewFrame(&mut frame, std::ptr::null_mut())
            .unwrap();
        let frame = frame.unwrap();
        frame.Initialize(None).unwrap();
        frame.SetSize(c.side, c.side).unwrap();
        let mut format = GUID_WICPixelFormat32bppPBGRA;
        frame.SetPixelFormat(&mut format).unwrap();
        frame.WritePixels(c.side, c.side * 4, &px).unwrap();
        frame.Commit().unwrap();
        encoder.Commit().unwrap();
    }
}

#[test]
fn a_segments_code_is_the_code_style_at_body_size_with_no_scale() {
    // one code style everywhere, Body's 14 DIPs, Semibold and tracked;
    // the wheel's codes were drawn 15/14 larger.
    use crate::win::text::{Ramp, Style};
    let dwrite: IDWriteFactory =
        unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).unwrap() };
    let ink = Rgb(255, 255, 255);
    for k in [1.0, 2.0] {
        let drawn = |f: &dyn Fn(&WheelDraw)| {
            let c = canvas_at(200, k);
            let res = WheelResources::new(&c.rt, &dwrite).unwrap();
            let extras = WheelExtras::default();
            let d = WheelDraw {
                rt: &c.rt,
                res: &res,
                extras: &extras,
                base: Cell::new(Matrix3x2::identity()),
                dpi: 96.0 * k,
                zoom: 1.0,
            };
            unsafe {
                c.rt.BeginDraw();
                c.rt.Clear(Some(&D2D1_COLOR_F::default()));
            }
            f(&d);
            unsafe { c.rt.EndDraw(None, None).unwrap() };
            pixels(&c, 0.0).px
        };
        let code = drawn(&|d| d.code_label("WEBP", 100.0, 100.0, ink).unwrap());
        let plain = drawn(&|d| {
            let r = Rect::new(60.0, 90.0, 80.0, 20.0);
            let style = Style::new(Ramp::Body).code().centered();
            d.styled("WEBP", style, &r, ink, &[]).unwrap();
        });
        assert!(code.iter().any(|b| *b != 0), "something was drawn");
        assert_eq!(code, plain, "at {k}: the label is the plain code style");
    }
}
