//! Pixel tests of the window's drawing on a WIC canvas: the title bar,
//! the controls kit and the empty window, at 100, 125 and 200% in the
//! light, dark and contrast looks. Some write PNGs to `render-shots` under
//! the build's target folder, to look at.

use super::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;

/// A window-sized canvas: `w` × `h` DIPs at `k` pixels a DIP.
struct Shot {
    factory: ID2D1Factory,
    bitmap: IWICBitmap,
    rt: ID2D1RenderTarget,
    px: (u32, u32),
    k: f32,
}

fn shot(w: f32, h: f32, k: f32) -> Shot {
    let px = ((w * k).round() as u32, (h * k).round() as u32);
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let factory: ID2D1Factory =
            D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).unwrap();
        let wic: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).unwrap();
        let bitmap = wic
            .CreateBitmap(
                px.0,
                px.1,
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
        Shot {
            factory,
            bitmap,
            rt,
            px,
            k,
        }
    }
}

impl Shot {
    /// Its pixels, BGRA, row by row.
    fn pixels(&self) -> Vec<u8> {
        let mut px = vec![0u8; (self.px.0 * self.px.1 * 4) as usize];
        unsafe {
            self.bitmap
                .CopyPixels(std::ptr::null(), self.px.0 * 4, &mut px)
                .unwrap()
        };
        px
    }

    /// The pixel at `(x, y)` DIPs in `px`.
    fn at(&self, px: &[u8], x: f32, y: f32) -> [u8; 4] {
        let (ix, iy) = ((x * self.k).floor() as u32, (y * self.k).floor() as u32);
        let i = ((iy * self.px.0 + ix) * 4) as usize;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    }

    /// Write it to `path` as a PNG.
    fn save(&self, path: &std::path::Path) {
        use windows::core::HSTRING;
        use windows::Win32::Foundation::GENERIC_WRITE;
        let (w, h) = self.px;
        let px = self.pixels();
        unsafe {
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
            frame.SetSize(w, h).unwrap();
            // The window's ground is opaque, so premultiplied is
            // straight.
            let mut format = GUID_WICPixelFormat32bppBGRA;
            frame.SetPixelFormat(&mut format).unwrap();
            assert_eq!(format, GUID_WICPixelFormat32bppBGRA);
            frame.WritePixels(h, w * 4, &px).unwrap();
            frame.Commit().unwrap();
            encoder.Commit().unwrap();
        }
    }
}

/// What a frame of the window draws with, made once for a canvas, as
/// `Renderer` keeps them.
struct Rig {
    text: Text,
    round: ID2D1StrokeStyle,
    dash: ID2D1StrokeStyle,
    mark: ID2D1PathGeometry,
    brush: ID2D1SolidColorBrush,
    wheel: RefCell<WheelResources>,
    pictures: RefCell<HashMap<u64, Picture>>,
    clips: RefCell<cache::Clips>,
}

fn rig(sh: &Shot) -> Rig {
    let text = Text::new().unwrap();
    let rt = &sh.rt;
    Rig {
        round: round_stroke(&sh.factory).unwrap(),
        dash: dash_stroke(&sh.factory).unwrap(),
        mark: mark_geometry(&sh.factory).unwrap(),
        brush: unsafe {
            rt.CreateSolidColorBrush(&color(Rgb(0, 0, 0), 1.0), None)
                .unwrap()
        },
        wheel: RefCell::new(WheelResources::new(rt, &text.factory).unwrap()),
        pictures: RefCell::new(HashMap::new()),
        clips: RefCell::new(cache::clips()),
        text,
    }
}

/// The title bar alone on the dark ground of a `w` x 48 canvas at
/// `k`, with `caption` lit, in Settings or not.
fn title_shot(
    w: f32,
    k: f32,
    caption: crate::win::titlebar::Caption,
    settings: bool,
) -> (Shot, Vec<u8>) {
    let (sh, px, _) = title_shot_zoomed(w, k, 1.0, caption, settings);
    (sh, px)
}

/// `title_shot` at Text size zoom `zoom` (the canvas `w` DIPs wide,
/// drawn in layout units as the window draws), with the glyph sizes
/// it made formats for.
fn title_shot_zoomed(
    w: f32,
    k: f32,
    zoom: f32,
    caption: crate::win::titlebar::Caption,
    settings: bool,
) -> (Shot, Vec<u8>, Vec<f32>) {
    use crate::win::titlebar;
    let sh = shot(w, 48.0, k);
    let g = rig(&sh);
    let fluent = crate::theme::fluent::DARK;
    let d = Draw {
        factory: &sh.factory,
        rt: &sh.rt,
        dpi: 96.0 * k,
        zoom,
        brush: &g.brush,
        wheel: &g.wheel,
        text: &g.text,
        round: &g.round,
        dash: &g.dash,
        mark: &g.mark,
        base: Cell::new(Matrix3x2::identity()),
        pictures: &g.pictures,
        frame: 1,
        clips: &g.clips,
    };
    let t = TitleView {
        bar: super::title_bar::bar_in_units((w / zoom, 48.0 / zoom), zoom, settings),
        title: titlebar::title(),
        active: true,
        maximized: false,
        caption,
        back_ring: false,
        palette: &fluent,
    };
    let still = |_: ButtonId| (0.0, 0.0);
    unsafe {
        sh.rt.BeginDraw();
        sh.rt.Clear(Some(&color(fluent.base, 1.0)));
        // Layout units: the zoom on top of the target's DPI.
        sh.rt.SetTransform(&scale_at(zoom, 0.0, 0.0));
    }
    d.base.set(scale_at(zoom, 0.0, 0.0));
    d.title_under(&t, &still).unwrap();
    d.title_over(&t).unwrap();
    unsafe { sh.rt.EndDraw(None, None).unwrap() };
    let px = sh.pixels();
    let sizes = g.text.glyph_sizes();
    (sh, px, sizes)
}

/// A `w` × `h` canvas at `k` on the Fluent base of `fluent`, with `f`
/// drawn on it.
fn kit_shot(
    w: f32,
    h: f32,
    k: f32,
    fluent: &crate::theme::fluent::FluentPalette,
    f: impl Fn(&Draw),
) -> (Shot, Vec<u8>) {
    let sh = shot(w, h, k);
    let g = rig(&sh);
    let d = Draw {
        factory: &sh.factory,
        rt: &sh.rt,
        dpi: 96.0 * k,
        zoom: 1.0,
        brush: &g.brush,
        wheel: &g.wheel,
        text: &g.text,
        round: &g.round,
        dash: &g.dash,
        mark: &g.mark,
        base: Cell::new(Matrix3x2::identity()),
        pictures: &g.pictures,
        frame: 1,
        clips: &g.clips,
    };
    unsafe {
        sh.rt.BeginDraw();
        sh.rt.Clear(Some(&color(fluent.base, 1.0)));
    }
    f(&d);
    unsafe { sh.rt.EndDraw(None, None).unwrap() };
    let px = sh.pixels();
    (sh, px)
}

#[test]
fn a_contrast_theme_keeps_lists_and_text_boxes_readable() {
    use crate::controls::{Lit, TextBoxState};
    use crate::layout::kit;
    use crate::theme::fluent::contrast_palette;
    let f = contrast_palette(crate::theme::SysColors {
        window: Rgb::hex(0x000000),
        window_text: Rgb::hex(0xFFFFFF),
        highlight: Rgb::hex(0x1AEBFF),
        highlight_text: Rgb::hex(0x000000),
        gray_text: Rgb::hex(0x3FF23F),
        hotlight: Rgb::hex(0xFFFF00),
        button_face: Rgb::hex(0x101010),
        button_text: Rgb::hex(0xFFFFFF),
    });
    let anchor = Rect::new(20.0, 100.0, 200.0, 32.0);
    let list = kit::combo_flyout(anchor, 3, 2, Rect::new(0.0, 0.0, 400.0, 300.0));
    let field = Rect::new(240.0, 20.0, 100.0, 32.0);
    let (sh, px) = kit_shot(400.0, 300.0, 1.0, &f, |d| {
        d.combo_list(&list, &["a", "b", "c"], &[], 2, 0, &f)
            .unwrap();
        d.text_box(
            &field,
            &TextBoxState::new("1", 4, true),
            None,
            false,
            Lit::HOVER,
            &f,
        )
        .unwrap();
    });
    let at = |x: f32, y: f32| rgb(sh.at(&px, x, y));
    // The highlight is Highlight; the value is told by its pill
    // alone, so the two never look alike.
    let (lit, value) = (list.items[0], list.items[2]);
    assert_eq!(at(lit.right() - 6.0, lit.y + 16.0), f.accent);
    assert_eq!(at(value.right() - 6.0, value.y + 16.0), f.overlay);
    assert_eq!(at(value.x + 1.0, value.y + 16.0), f.accent, "the pill");
    // A text box under the pointer keeps its face, so its text stays
    // on it; the pointer shows in its edge.
    assert_eq!(at(field.right() - 6.0, field.y + 10.0), f.control.rgb);
    assert_eq!(
        at(field.right() - 6.0, field.y + 0.5),
        f.control_hover_stroke.rgb
    );
}

#[test]
fn the_theme_list_draws_three_dots_at_the_right_of_each_item() {
    use crate::layout::kit;
    use crate::settings::ThemeChoice;
    use crate::theme::fluent::ALL_BUILT_IN;
    for f in ALL_BUILT_IN {
        let names: Vec<&str> = ThemeChoice::ALL.iter().map(|t| t.label()).collect();
        let dots: Vec<[Rgb; 3]> = ThemeChoice::ALL
            .iter()
            .map(|t| {
                let p = crate::theme::fluent::palette(None, *t, false);
                [p.base, p.text, p.accent]
            })
            .collect();
        let anchor = Rect::new(20.0, 20.0, 200.0, 32.0);
        let list = kit::combo_flyout(anchor, names.len(), 0, Rect::new(0.0, 0.0, 400.0, 500.0));
        let (sh, px) = kit_shot(400.0, 500.0, 1.0, &f, |d| {
            d.combo_list(&list, &names, &dots, 0, 1, &f).unwrap();
        });
        let at = |x: f32, y: f32| rgb(sh.at(&px, x, y));
        for (item, colours) in list.items.iter().zip(&dots) {
            for (dot, want) in kit::swatch_dots(*item).iter().zip(colours) {
                let (cx, cy) = (dot.x + dot.w / 2.0, dot.y + dot.h / 2.0);
                assert_eq!(at(cx, cy), *want, "{}: a dot's middle", f.name);
            }
        }
        // Without swatches the list is as before: nothing at the right.
        let (sh2, px2) = kit_shot(400.0, 500.0, 1.0, &f, |d| {
            d.combo_list(&list, &names, &[], 0, 1, &f).unwrap();
        });
        let item = list.items[3];
        let first = kit::swatch_dots(item)[0];
        let plain = rgb(sh2.at(&px2, first.x + 4.0, first.y + 4.0));
        assert_ne!(plain, dots[3][1], "{}: no dots without swatches", f.name);
    }
}

#[test]
fn the_controls_draw_in_their_tokens_at_every_scale() {
    use crate::controls::{ButtonKind, Lit};
    use crate::layout::kit;
    use crate::theme::fluent::{contrast_palette, ALL_BUILT_IN};
    let hc = contrast_palette(crate::theme::SysColors {
        window: Rgb::hex(0x000000),
        window_text: Rgb::hex(0xFFFFFF),
        highlight: Rgb::hex(0x1AEBFF),
        highlight_text: Rgb::hex(0x000000),
        gray_text: Rgb::hex(0x3FF23F),
        hotlight: Rgb::hex(0xFFFF00),
        button_face: Rgb::hex(0x000000),
        button_text: Rgb::hex(0xFFFFFF),
    });
    let accent = Rect::new(20.0, 20.0, 120.0, 32.0);
    let standard = Rect::new(160.0, 20.0, 120.0, 32.0);
    let bar = Rect::new(20.0, 80.0, 200.0, 3.0);
    for k in [1.0, 1.25, 2.0] {
        for f in ALL_BUILT_IN.into_iter().chain([hc]) {
            let lit = if f.high_contrast {
                Lit::HOVER
            } else {
                Lit::REST
            };
            let (sh, px) = kit_shot(320.0, 200.0, k, &f, |d| {
                d.fl_button(ButtonKind::Accent, &accent, "Go", None, Lit::REST, &f)
                    .unwrap();
                d.fl_button(ButtonKind::Standard, &standard, "Go", None, lit, &f)
                    .unwrap();
                d.focus_visual(&standard, 4.0, &f);
                d.progress_bar(&bar, (0.0, 0.5), &f);
                d.toggle_switch(300.0, 140.0, true, 1.0, Lit::REST, &f)
                    .unwrap();
            });
            let at = |x: f32, y: f32| rgb(sh.at(&px, x, y));
            let what = format!("{} at {k}", f.name);
            assert!(
                near(at(accent.x + 4.0, accent.y + 16.0), f.accent),
                "{what}"
            );
            let face = at(standard.x + 4.0, standard.y + 16.0);
            if f.high_contrast {
                // Lit under a contrast theme: Highlight.
                assert_eq!(face, f.accent, "{what}");
            } else {
                assert!(near(face, f.control.over(f.base)), "{what}: {face:?}");
            }
            // The focus rings: the outer 2 DIPs off the control, the
            // inner just inside it.
            assert_eq!(
                at(standard.x - 3.0, standard.y + 16.0),
                f.focus_outer,
                "{what}"
            );
            let inner = at(standard.x - 1.5, standard.y + 16.0);
            assert!(near(inner, f.focus_inner), "{what}: {inner:?}");
            // The bar: the accent half way, the thin track after.
            assert!(near(at(bar.x + 4.0, bar.y + 1.5), f.accent), "{what}");
            // At 125% the 1-DIP line straddles two pixels, so it is
            // only seen to be there.
            let track = at(bar.right() - 4.0, bar.y + 1.5);
            if k == 1.25 {
                assert_ne!(track, f.base, "{what}");
            } else {
                assert!(
                    near(track, f.strong_stroke.over(f.base)),
                    "{what}: {track:?}"
                );
            }
            // The toggle, on: its track's left end in the accent.
            assert!(near(at(300.0 - 40.0 + 4.0, 140.0), f.accent), "{what}");
        }
    }
    // A dialog's frame: smoke over the window, its band in the base.
    for f in ALL_BUILT_IN {
        let (sh, px) = kit_shot(640.0, 480.0, 1.0, &f, |d| {
            d.smoke(640.0, 480.0, 1.0, &f);
            let l = kit::dialog((640.0, 480.0), 28.0, 40.0, 2);
            d.dialog_frame(&l, "Title", &f).unwrap();
        });
        let at = |x: f32, y: f32| rgb(sh.at(&px, x, y));
        assert!(near(at(4.0, 4.0), f.smoke.over(f.base)));
        let l = kit::dialog((640.0, 480.0), 28.0, 40.0, 2);
        assert!(near(at(l.rect.x + 4.0, l.content.y + 4.0), f.overlay));
        assert!(near(at(l.band.x + 4.0, l.band.y + 40.0), f.base));
    }
}

#[test]
fn a_scrolled_settings_page_keeps_the_layers_own_edge() {
    // under a contrast theme the cards scrolled to the top were drawn
    // over the layer's 1-DIP border, and broke its outline. The page is
    // clipped one DIP inside the layer, so the border is whole.
    use crate::layout::{content_layer, settings, TITLE_BAR};
    use crate::settings::Settings;
    use crate::settings_page::{page, Facts};
    use crate::theme::fluent::contrast_palette;
    use crate::theme::SysColors;
    let f = contrast_palette(SysColors {
        window: Rgb::hex(0x000000),
        window_text: Rgb::hex(0xFFFFFF),
        highlight: Rgb::hex(0x1AEBFF),
        highlight_text: Rgb::hex(0x000000),
        gray_text: Rgb::hex(0x3FF23F),
        hotlight: Rgb::hex(0xFFFF00),
        button_face: Rgb::hex(0x101010),
        button_text: Rgb::hex(0xFFFFFF),
    });
    let (w, h) = (720.0, 480.0);
    let spec = page(&Settings::default(), &Facts::default(), [false; 5], false);
    let view = content_layer(w, h, TITLE_BAR);
    // Scrolled so a card straddles the layer's top edge.
    let layout = settings(view, 150.0, &spec, &crate::layout::Estimate);
    assert!(
        layout
            .cards
            .iter()
            .any(|c| c.rect.y < view.y && c.rect.bottom() > view.y + 8.0),
        "a card runs under the top edge"
    );
    let button = |_: ButtonId| (0.0, 0.0);
    let sv = SettingsView {
        layout,
        page: spec,
        open: 1.0,
        rise: 0.0,
        hover: None,
        pressed: None,
        focus: None,
        button: &button,
        combo: None,
        chevrons: [0.0; 5],
        knobs: [0.0; 1],
        scroll_shown: 0.0,
    };
    let (sh, px) = kit_shot(w, h, 1.0, &f, |d| {
        d.settings(&sv, &f).unwrap();
    });
    let at = |x: f32, y: f32| rgb(sh.at(&px, x, y));
    // Along the top edge, away from the corners, the border is WindowText.
    for x in [view.x + 60.0, view.x + view.w / 2.0, view.right() - 60.0] {
        assert_eq!(at(x, view.y + 0.5), f.text, "x = {x}");
    }
}

#[test]
fn a_dialogs_body_is_opaque_over_a_rows_focus_ring_beneath_it() {
    // A faint outline behind the options dialog's sub-line was the
    // selected row's focus ring. The body is solid: once the dialog has
    // faded in, nothing under it shows through.
    use crate::layout::kit;
    use crate::theme::fluent::ALL_BUILT_IN;
    for f in ALL_BUILT_IN {
        let l = kit::dialog((640.0, 480.0), 28.0, 120.0, 2);
        let row = layout::Rect::new(
            l.content.x - 8.0,
            l.content.y + 20.0,
            l.content.w + 16.0,
            40.0,
        );
        let (sh, px) = kit_shot(640.0, 480.0, 1.0, &f, |d| {
            d.focus_visual(&row, 4.0, &f);
            d.smoke(640.0, 480.0, 1.0, &f);
            d.dialog_frame(&l, "Title", &f).unwrap();
        });
        let at = |x: f32, y: f32| rgb(sh.at(&px, x, y));
        for (x, y) in [
            (row.x - 3.0, row.y + 20.0),
            (row.x - 1.5, row.y + 20.0),
            (row.x + 60.0, row.y - 3.0),
            (row.x + 60.0, row.y + row.h + 3.0),
        ] {
            assert!(
                near(at(x, y), f.overlay),
                "{}: ({x}, {y}) shows {:?}",
                f.name,
                at(x, y)
            );
        }
    }
}

#[test]
fn a_dialogs_focus_ring_keeps_its_sides_on_a_control_as_wide_as_the_content() {
    // Drawn inside the content's clip, a full-width control's ring
    // lost its sides; drawn after the clip closes, it has them.
    use crate::layout::kit;
    use crate::theme::fluent::ALL_BUILT_IN;
    let content = layout::Rect::new(24.0, 40.0, 400.0, 200.0);
    let header = layout::Rect::new(24.0, 100.0, 400.0, 48.0);
    let rings = [(header, 4.0)];
    for k in [1.0, 2.0] {
        for f in ALL_BUILT_IN {
            let what = format!("{} at {k}", f.name);
            // Where the outer ring's left side is: 3 DIPs out from the header.
            let (x, y) = (header.x - 3.0, header.y + header.h / 2.0);
            let (sh, px) = kit_shot(448.0, 280.0, k, &f, |d| {
                d.clipped(&content, 0.0, || {
                    d.focus_visual(&header, 4.0, &f);
                    Ok(())
                })
                .unwrap();
            });
            assert!(
                !near(rgb(sh.at(&px, x, y)), f.focus_outer),
                "{what}: inside the content's clip the side is cut off"
            );
            let (sh, px) = kit_shot(448.0, 280.0, k, &f, |d| {
                d.dialog_rings(&content, &rings, &f).unwrap();
            });
            let at = |x: f32, y: f32| rgb(sh.at(&px, x, y));
            assert!(near(at(x, y), f.focus_outer), "{what}: left side");
            assert!(
                near(at(header.right() + 3.0, y), f.focus_outer),
                "{what}: right"
            );
            assert!(
                near(at(header.x + 50.0, header.y - 3.0), f.focus_outer),
                "{what}: top"
            );
            // The inner ring too, and nothing past the ring clip.
            assert!(near(at(header.x - 1.5, y), f.focus_inner), "{what}: inner");
            let clip = kit::ring_clip(content);
            let far = layout::Rect::new(24.0, 100.0, 400.0, 48.0);
            let (sh, px) = kit_shot(448.0, 280.0, k, &f, |d| {
                d.dialog_rings(&content, &[(far, 4.0)], &f).unwrap();
                // A ring for a control scrolled above the content.
                d.dialog_rings(
                    &content,
                    &[(layout::Rect::new(24.0, 0.0, 400.0, 32.0), 4.0)],
                    &f,
                )
                .unwrap();
            });
            let at = |x: f32, y: f32| rgb(sh.at(&px, x, y));
            // The scrolled-away control's outer ring would be drawn at
            // y 34..36 along its bottom edge, just outside the clip
            // (which starts at content.y - 4 = 36).
            assert!(near(at(100.0, 35.0), f.base), "{what}: cut above {clip:?}");
            assert!(near(at(100.0, 28.0), f.base), "{what}: body");
        }
    }
}

#[test]
fn the_title_bar_asks_for_its_glyphs_at_their_tokens_at_any_text_size() {
    use crate::win::titlebar::{layout, Caption};
    for zoom in [1.0, 1.25, 1.5, 2.25] {
        for settings in [false, true] {
            let (sh, px, sizes) =
                title_shot_zoomed(720.0, 1.25, zoom, Caption::default(), settings);
            assert_eq!(sizes, vec![10.0, 16.0], "{zoom} {settings}");
            // The glyphs are still drawn 16 DIPs on screen: something
            // lands in the middle of Settings' or Back's box, and the
            // bar's K is where it is at 100%.
            let b = layout(720.0, settings);
            let fluent = crate::theme::fluent::DARK;
            assert_eq!(
                rgb(sh.at(&px, b.mark.x + 2.0, b.mark.y + 8.0)),
                fluent.accent,
                "{zoom}"
            );
            let r = b.back.or(b.settings).unwrap();
            let mut lit = false;
            for dx in -6..=6 {
                let at = rgb(sh.at(&px, r.x + r.w / 2.0 + dx as f32, r.y + r.h / 2.0));
                lit |= at != fluent.base;
            }
            assert!(lit, "{zoom} {settings}: a glyph in the button");
        }
    }
}

thread_local! {
    /// How lit the credit's link is in the shots: its hover, and whether
    /// the keyboard is on it.
    static CREDIT_LIT: std::cell::Cell<(f32, bool)> = const { std::cell::Cell::new((0.0, false)) };
}

/// Whether some pixel in `r` is plainly red: the heart.
fn red_in(sh: &Shot, px: &[u8], r: &layout::Rect) -> bool {
    let (nx, ny) = ((r.w / 0.25) as usize, (r.h / 0.25) as usize);
    (0..nx).any(|i| {
        (0..ny).any(|j| {
            let c = rgb(sh.at(px, r.x + i as f32 * 0.25, r.y + j as f32 * 0.25));
            i32::from(c.0) > i32::from(c.1) + 90 && i32::from(c.0) > i32::from(c.2) + 60
        })
    })
}

#[test]
fn the_credit_is_quiet_its_heart_red_and_its_link_lights_in_every_theme_and_size() {
    use crate::theme::fluent::ALL_BUILT_IN;
    let dir = shots_dir();
    for (name, f) in ALL_BUILT_IN
        .into_iter()
        .map(|f| (f.name, f))
        .chain([("contrast", fluent_contrast())])
    {
        for k in [1.0, 1.25, 1.5, 1.75, 2.0] {
            let at = format!("{name} at {k}");
            for (hover, ring) in [(0.0, false), (1.0, false), (0.0, true)] {
                CREDIT_LIT.set((hover, ring));
                let (sh, px, e) = empty_shot((880.0, 600.0), k, &f, true, 0.0);
                let g = rig(&sh);
                let c = crate::credit::in_empty(&e, super::credit_widths(&g.text, 1.0))
                    .expect("the credit shows in the empty window");
                // Its words in the secondary text colour.
                assert!(any_in(&sh, &px, &c.made, f.text_secondary), "{at}");
                assert!(any_in(&sh, &px, &c.by, f.text_secondary), "{at}");
                // The heart red, or in a contrast theme the theme's text.
                if f.high_contrast {
                    assert!(any_in(&sh, &px, &c.heart, f.text), "{at}");
                    assert!(!red_in(&sh, &px, &c.heart), "{at}");
                } else {
                    assert!(red_in(&sh, &px, &c.heart), "{at}");
                }
                // The name quiet at rest; pointed at or focused, in the text
                // colour and underlined.
                let lit = hover > 0.0 || ring;
                let want = if lit { f.text } else { f.text_secondary };
                assert!(any_in(&sh, &px, &c.name, want), "{at} {hover} {ring}");
                let under = layout::Rect::new(
                    c.name.x + 2.0,
                    c.name.y + c.name.h / 2.0 + 6.0,
                    c.name.w - 4.0,
                    1.0,
                );
                // Each half DIP along the underline, some pixel in its band
                // much nearer the ink than the ground beside the name.
                let ground = rgb(sh.at(&px, c.line.x - 4.0, under.y));
                let far = |a: Rgb, b: Rgb| {
                    let d = |x: u8, y: u8| i32::from(x.abs_diff(y));
                    d(a.0, b.0) + d(a.1, b.1) + d(a.2, b.2)
                };
                let lined = (0..((under.w / 0.5) as usize))
                    .filter(|i| {
                        let x = under.x + *i as f32 * 0.5;
                        [-0.5f32, 0.0, 0.5, 1.0, 1.5].iter().any(|dy| {
                            let p = rgb(sh.at(&px, x, under.y + dy));
                            far(p, want) * 2 < far(p, ground)
                        })
                    })
                    .count() as f32
                    * 0.5;
                if lit {
                    assert!(lined > under.w * 0.8, "{at}: underline {lined}");
                }
                if !lit || ring {
                    let tag = if ring { "-focus" } else { "" };
                    let pct = (k * 100.0).round() as u32;
                    sh.save(&dir.join(format!("credit-empty-{name}-{pct}{tag}.png")));
                }
            }
            CREDIT_LIT.set((0.0, false));
            // Under the footer's status, clear of its buttons.
            let (sh, px, l, _) = pane_shot((1100.0, 720.0), k, &f, Some(0), None);
            let g = rig(&sh);
            let row = crate::credit::in_footer(
                layout::footer_row(&l.footer.unwrap(), false),
                super::credit_widths(&g.text, 1.0),
            );
            let c = row.credit.expect("room under the status");
            assert!(c.line.right() < row.add.x, "{at}");
            assert!(any_in(&sh, &px, &c.made, f.text_secondary), "{at}");
            assert_eq!(f.high_contrast, !red_in(&sh, &px, &c.heart), "{at}");
            let pct = (k * 100.0).round() as u32;
            sh.save(&dir.join(format!("credit-list-{name}-{pct}.png")));
        }
    }
}

fn rgb(px: [u8; 4]) -> Rgb {
    Rgb::hex(u32::from(px[2]) << 16 | u32::from(px[1]) << 8 | u32::from(px[0]))
}

/// Within a step of blending of each other.
fn near(a: Rgb, b: Rgb) -> bool {
    let d = |x: u8, y: u8| x.abs_diff(y) <= 2;
    d(a.0, b.0) && d(a.1, b.1) && d(a.2, b.2)
}

/// The empty window, `w` x `h` DIPs at `k`, in `f`'s look: the title
/// bar, the content layer and its group, with the first-run card when
/// `card`, and the drop outline `drag` of the way in. With what it was
/// laid out as.
fn empty_shot(
    (w, h): (f32, f32),
    k: f32,
    f: &crate::theme::fluent::FluentPalette,
    card: bool,
    drag: f32,
) -> (Shot, Vec<u8>, layout::EmptyState) {
    let sh = shot(w, h, k);
    let g = rig(&sh);
    let d = Draw {
        factory: &sh.factory,
        rt: &sh.rt,
        dpi: 96.0 * k,
        zoom: 1.0,
        brush: &g.brush,
        wheel: &g.wheel,
        text: &g.text,
        round: &g.round,
        dash: &g.dash,
        mark: &g.mark,
        base: Cell::new(Matrix3x2::identity()),
        pictures: &g.pictures,
        frame: 1,
        clips: &g.clips,
    };
    let empty = ways_in::empty_lines();
    let fr = ways_in::first_run(crate::help::Results::Beside);
    let l = layout::layout_under(w, h, 0, 0.0, 48.0);
    let measured = super::first_run_text(&g.text, 1.0, &fr, l.panel.w);
    let e = layout::empty_state(&l.panel, card.then_some(measured));
    let (notes, saving, selected) = (HashMap::new(), HashSet::new(), BTreeSet::new());
    let still = |_: ButtonId| (0.0, 0.0);
    let v = View {
        backdrop: crate::win::titlebar::Backdrop::Solid,
        title: TitleView {
            bar: crate::win::titlebar::layout(w, false),
            title: "Kiln",
            active: true,
            maximized: false,
            caption: Default::default(),
            back_ring: false,
            palette: f,
        },
        entries: &[],
        notes: &notes,
        saving: &saving,
        saving_lines: &HashMap::new(),

        selected: &selected,
        kind: &|_| None,
        preview: &|_, _| None,
        row_links: true,
        focus: None,
        button_ring: None,
        shown: 0.0,
        selection_y: None,
        button: &still,
        hover_row: None,
        hover_cancel: false,
        hover_link: false,
        hover_retry: false,
        hover_options: false,
        scroll: 0.0,
        drag,
        progress_clock: None,
        progress_only: false,
        row_anim: &|_| RowAnim::at_rest(1.0),
        status: "",
        empty: &empty,
        empty_status: "",
        first_run: card.then_some(&fr),
        empty_at: Some(e),
        convert_enabled: false,
        convert_label: "Convert",
        footer_row: None,
        credit: crate::credit::in_empty(&e, super::credit_widths(&g.text, 1.0)),
        undo: false,
        wheel: None,
        menu: None,
        details: None,
        ask: None,
        tool: None,
        settings: None,
        sheet: None,
        help: None,
        inspector: None,
        top_info: None,
    };
    unsafe {
        sh.rt.BeginDraw();
        sh.rt.SetTransform(&Matrix3x2::identity());
        sh.rt.Clear(Some(&color(f.base, 1.0)));
    }
    let lit = CREDIT_LIT.get();
    let ring = v.credit.filter(|_| lit.1).map(|c| c.name);
    let lit_button = move |b: ButtonId| match b {
        ButtonId::Window(FocusButton::Credit) => (lit.0, 0.0),
        _ => (0.0, 0.0),
    };
    let v = View {
        button: &lit_button,
        button_ring: ring,
        ..v
    };
    d.empty_state(&l, &v).unwrap();
    d.credit(&v, f).unwrap();
    d.title_under(&v.title, v.button).unwrap();
    d.title_over(&v.title).unwrap();
    unsafe { sh.rt.EndDraw(None, None).unwrap() };
    let px = sh.pixels();
    (sh, px, e)
}

/// The high-contrast Fluent look the kit's tests use.
fn fluent_contrast() -> crate::theme::fluent::FluentPalette {
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

#[test]
fn the_empty_window_draws_its_k_add_files_and_first_run_card_in_their_tokens() {
    use crate::theme::fluent::ALL_BUILT_IN;
    let dir = shots_dir();
    for (name, f) in ALL_BUILT_IN
        .into_iter()
        .map(|f| (f.name, f))
        .chain([("contrast", fluent_contrast())])
    {
        for k in [1.0, 1.25, 2.0] {
            let at = format!("{name} at {k}");
            let (sh, px, e) = empty_shot((1100.0, 720.0), k, &f, true, 0.0);
            let c = |x: f32, y: f32| rgb(sh.at(&px, x, y));
            let layer = f.layer.mica.over(f.base);
            // The layer inside its edge, the card on it.
            assert!(
                near(c(e.layer.x + 4.0, e.layer.bottom() - 4.0), layer),
                "{at}"
            );
            let card = e.card.unwrap();
            let on_card = f.card.mica.over(layer);
            let corner = (card.card.right() - 6.0, card.card.bottom() - 6.0);
            assert!(near(c(corner.0, corner.1), on_card), "{at}");
            // The K's tile in the accent, beside its strokes.
            let m = e.mark.expect("the K shows in a tall window");
            assert_eq!(c(m.x + 32.0, m.y + 6.0), f.accent, "{at}");
            // Add files is the accent button, left of its glyph.
            let b = e.button;
            assert_eq!(c(b.x + 4.0, b.y + b.h / 2.0), f.accent, "{at}");
            // Each step's number sits in its circle: the selection's
            // tint on the card, or under a contrast theme a ring.
            for n in &card.numbers {
                let inside = c(n.x + 4.0, n.y + 10.0);
                if f.high_contrast {
                    assert_eq!(inside, f.card.solid, "{at}");
                } else {
                    assert!(near(inside, f.selection.over(on_card)), "{at}: {inside:?}");
                }
            }
            if k == 1.25 {
                sh.save(&dir.join(format!("empty-first-run-{name}-125.png")));
            }
        }
    }
}

#[test]
fn a_drag_over_the_empty_window_shows_the_dashed_outline() {
    use crate::theme::fluent::ALL_BUILT_IN;
    let dir = shots_dir();
    for (name, f) in ALL_BUILT_IN
        .into_iter()
        .map(|f| (f.name, f))
        .chain([("contrast", fluent_contrast())])
    {
        for k in [1.0, 1.25, 2.0] {
            let at = format!("{name} at {k}");
            // Along the outline's top edge, 1 DIP in: its dashes show
            // in the accent while dragging, and nothing there without.
            let along = |px: &[u8], sh: &Shot, e: &layout::EmptyState| {
                let y = e.drop.y + 1.0;
                let n = ((e.drop.w - 40.0) / 0.5) as usize;
                (0..n)
                    .map(|i| rgb(sh.at(px, e.drop.x + 20.0 + i as f32 * 0.5, y)))
                    .filter(|&c| c == f.accent)
                    .count()
            };
            let (sh, px, e) = empty_shot((720.0, 480.0), k, &f, false, 1.0);
            let lit = along(&px, &sh, &e);
            assert!(lit > 100, "{at}: {lit}");
            // Dashed: there are gaps along it too.
            let all = ((e.drop.w - 40.0) / 0.5) as usize;
            assert!(lit < all * 3 / 4, "{at}: {lit} of {all}");
            let (sh, px, e) = empty_shot((720.0, 480.0), k, &f, false, 0.0);
            assert_eq!(along(&px, &sh, &e), 0, "{at}");
            if k == 1.25 {
                let (sh, ..) = empty_shot((720.0, 480.0), k, &f, true, 1.0);
                sh.save(&dir.join(format!("empty-dragging-{name}-125.png")));
            }
        }
    }
}

#[test]
fn a_drag_over_the_empty_window_changes_only_its_outline_and_wash() {
    // The K, the lines and the button stay where they are: the outline
    // and the wash are all a drag adds.
    use crate::theme::fluent::ALL_BUILT_IN;
    for f in ALL_BUILT_IN {
        for k in [1.0, 1.25] {
            let at = format!("{} at {k}", f.name);
            let (sh0, px0, e0) = empty_shot((720.0, 480.0), k, &f, false, 0.0);
            let (sh1, px1, e1) = empty_shot((720.0, 480.0), k, &f, false, 1.0);
            // Laid out from the layer alone, a drag has no say in it.
            assert_eq!(e0.mark, e1.mark, "{at}");
            assert_eq!(e0.headline, e1.headline, "{at}");
            assert_eq!(e0.button, e1.button, "{at}");
            let m = e1.mark.expect("the K fits in 720 x 480");
            // The K's tile, in the accent, with and without the drag.
            let tile = (m.x + 8.0, m.y + m.h / 2.0);
            // (The wash is the accent over it, a level or two off.)
            let near = |a: Rgb, b: Rgb| {
                [(a.0, b.0), (a.1, b.1), (a.2, b.2)]
                    .iter()
                    .all(|&(x, y)| x.abs_diff(y) <= 2)
            };
            assert!(near(rgb(sh0.at(&px0, tile.0, tile.1)), f.accent), "{at}");
            assert!(near(rgb(sh1.at(&px1, tile.0, tile.1)), f.accent), "{at}");
            // The headline's words, in the text colour, where they were.
            for (sh, px) in [(&sh0, &px0), (&sh1, &px1)] {
                assert!(any_in(sh, px, &e1.headline, f.text), "{at}");
                // The button, in the accent.
                let b = e1.button;
                assert!(near(rgb(sh.at(px, b.x + 4.0, b.y + 4.0)), f.accent), "{at}");
            }
        }
    }
}

#[test]
fn every_selected_row_has_the_same_accent_pill_at_its_left_whatever_its_state() {
    // A waiting, a converting, a done and a failed row: one pill, 3 DIPs
    // wide and 16 tall (`space::SELECTION_PILL`), at the row's left edge.
    use crate::theme::fluent::ALL_BUILT_IN;
    let (pw, ph) = crate::theme::fluent::space::SELECTION_PILL;
    for f in ALL_BUILT_IN {
        for k in [1.0, 2.0] {
            let mut x0 = None;
            for i in 0..4 {
                let (sh, px, l, _) = pane_shot((1100.0, 720.0), k, &f, Some(i), None);
                let r = l.rows[i];
                let mid = r.y + r.h / 2.0;
                let at = |x: f32, y: f32| rgb(sh.at(&px, x, y));
                let name = format!("{} at {k}, row {i}", f.name);
                // Accent across its width, plain tint beyond it.
                assert_eq!(at(r.x + 0.5 * pw, mid), f.accent, "{name}");
                assert_eq!(at(r.x + pw - 0.25, mid), f.accent, "{name}");
                assert_ne!(at(r.x + pw + 1.0, mid), f.accent, "{name}");
                // Its height, centred on the row.
                assert_eq!(at(r.x + 0.5 * pw, mid - ph / 2.0 + 3.0), f.accent, "{name}");
                assert_ne!(at(r.x + 0.5 * pw, mid - ph / 2.0 - 2.0), f.accent, "{name}");
                // The same place on every row.
                let x = *x0.get_or_insert(r.x);
                assert_eq!(r.x, x, "{name}");
            }
        }
    }
}

#[test]
fn a_video_without_a_frame_shows_its_kind_tile_and_not_the_shells_icon() {
    use crate::previews::Pixels;
    use crate::theme::fluent::{FileKind, DARK};
    let f = DARK;
    // The shell's icon, here a field of pure red.
    let icon = Arc::new(Pixels {
        width: 32,
        height: 32,
        bgra: [0u8, 0, 255, 255].repeat(32 * 32),
        icon: true,
    });
    let draw = |kind: Kind| {
        let sh = shot(120.0, 120.0, 1.0);
        let g = rig(&sh);
        let d = Draw {
            factory: &sh.factory,
            rt: &sh.rt,
            dpi: 96.0,
            zoom: 1.0,
            brush: &g.brush,
            wheel: &g.wheel,
            text: &g.text,
            round: &g.round,
            dash: &g.dash,
            mark: &g.mark,
            base: Cell::new(Matrix3x2::identity()),
            pictures: &g.pictures,
            frame: 1,
            clips: &g.clips,
        };
        unsafe {
            sh.rt.BeginDraw();
            sh.rt.SetTransform(&Matrix3x2::identity());
            sh.rt.Clear(Some(&color(f.base, 1.0)));
        }
        let dst = layout::Rect::new(10.0, 10.0, 100.0, 100.0);
        d.fl_preview(
            &dst,
            8.0,
            Some((icon.clone(), 1, 1.0)),
            Some(kind),
            40.0,
            1.0,
            &f,
        );
        unsafe { sh.rt.EndDraw(None, None).unwrap() };
        let px = sh.pixels();
        (sh, px, dst)
    };
    let red = Rgb(255, 0, 0);
    // A video: the tile, and its glyph in the video colour; no red.
    let (sh, px, dst) = draw(Kind::Video);
    assert!(!any_in(&sh, &px, &dst, red), "the icon of the player");
    assert!(
        any_in(&sh, &px, &dst, f.kind(FileKind::Video)),
        "the kind's glyph"
    );
    // A file of another kind with no thumbnail still shows its icon.
    let (sh, px, dst) = draw(Kind::Document);
    assert!(any_in(&sh, &px, &dst, red), "a document's icon");
}

#[test]
fn the_title_bar_draws_its_k_and_lights_close_red() {
    use crate::win::titlebar::{layout, Caption, Part};
    let fluent = crate::theme::fluent::DARK;
    for k in [1.0, 1.25, 2.0] {
        let w = 720.0;
        let b = layout(w, false);
        // At rest: the K's tile in the accent at its corner, the bar's
        // ground under the buttons.
        let (sh, px) = title_shot(w, k, Caption::default(), false);
        assert_eq!(
            rgb(sh.at(&px, b.mark.x + 2.0, b.mark.y + 8.0)),
            fluent.accent,
            "{k}"
        );
        let corner = (b.close.x + 3.0, b.close.y + 3.0);
        assert_eq!(rgb(sh.at(&px, corner.0, corner.1)), fluent.base, "{k}");
        // The pointer on Close: it is red to its corners.
        let lit = Caption {
            hover: Some(Part::Close),
            pressed: None,
        };
        let (sh, px) = title_shot(w, k, lit, false);
        assert_eq!(
            rgb(sh.at(&px, corner.0, corner.1)),
            fluent.close_hover,
            "{k}"
        );
        // On Maximise: the control's hover over the ground, not red.
        let lit = Caption {
            hover: Some(Part::Max),
            pressed: None,
        };
        let (sh, px) = title_shot(w, k, lit, false);
        let at = rgb(sh.at(&px, b.max.x + 3.0, b.max.y + 3.0));
        assert!(
            near(at, fluent.control_hover.over(fluent.base)),
            "{k}: {at:?}"
        );
        assert_eq!(rgb(sh.at(&px, corner.0, corner.1)), fluent.base, "{k}");
        // In Settings, Back lit before the K.
        let lit = Caption {
            hover: Some(Part::Back),
            pressed: None,
        };
        let b = layout(w, true);
        let back = b.back.unwrap();
        let (sh, px) = title_shot(w, k, lit, true);
        let at = rgb(sh.at(&px, back.x + 2.0, back.y + 16.0));
        assert!(
            near(at, fluent.control_hover.over(fluent.base)),
            "{k}: {at:?}"
        );
        assert_eq!(
            rgb(sh.at(&px, b.mark.x + 2.0, b.mark.y + 8.0)),
            fluent.accent,
            "{k}"
        );
    }
}

/// `C:\kiln\target\render-shots`, or wherever the build's target is:
/// three folders up from this test's executable.
fn shots_dir() -> std::path::PathBuf {
    let exe = std::env::current_exe().unwrap();
    let dir = exe.ancestors().nth(3).unwrap().join("render-shots");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A listed file for the list's pixel tests, in `C:\v`.
fn listed(name: &str, code: &str, job: JobState) -> Entry {
    Entry {
        path: std::path::PathBuf::from(r"C:\v").join(name),
        name: name.to_owned(),
        folder: r"C:\v".to_owned(),
        code: code.to_owned(),
        size: 31_000_000,
        modified: None,
        targets: vec!["MP4".into(), "WEBM".into()],
        check: kiln_core::FileCheck::Ok,
        missing: None,
        ffmpeg_found: true,
        save_failed: false,
        job,
        facts: None,
        suggestion: None,
        facts_at: None,
        last: Some(crate::wheel::Action::Convert("mp4".into())),
        from_drag: false,
        job_note: None,
        result_bytes: Some(9_600_000),
        waits_on: None,
    }
}

/// The list with files (mock screens 2, 5 and 8b), `w` x `h` DIPs at `k`
/// in `f`'s look, with no side pane: a file waiting (selected, row 0), one
/// converting half way (row 1), one saved (row 2) and one failed (row 3).
fn list_shot(
    size: (f32, f32),
    k: f32,
    f: &crate::theme::fluent::FluentPalette,
) -> (Shot, Vec<u8>, Layout) {
    let (sh, px, l, _) = pane_shot(size, k, f, None, None);
    (sh, px, l)
}

/// The list with files, as [`list_shot`] draws it, with the side pane
/// shown for row `pane` (selected), and the pane's layout.
fn pane_shot(
    (w, h): (f32, f32),
    k: f32,
    f: &crate::theme::fluent::FluentPalette,
    pane: Option<usize>,
    menu: Option<&MenuView>,
) -> (Shot, Vec<u8>, Layout, Option<InspectorLayout>) {
    let sh = shot(w, h, k);
    let g = rig(&sh);
    let d = Draw {
        factory: &sh.factory,
        rt: &sh.rt,
        dpi: 96.0 * k,
        zoom: 1.0,
        brush: &g.brush,
        wheel: &g.wheel,
        text: &g.text,
        round: &g.round,
        dash: &g.dash,
        mark: &g.mark,
        base: Cell::new(Matrix3x2::identity()),
        pictures: &g.pictures,
        frame: 1,
        clips: &g.clips,
    };
    let convert = crate::wheel::Action::Convert("mp4".into());
    let entries = vec![
        listed("friends-portrait.jpg", "JPG", JobState::Idle),
        listed(
            "tos-1080p.mp4",
            "MP4",
            JobState::Running {
                action: convert.clone(),
                fraction: Some(0.5),
                started: None,
            },
        ),
        listed(
            "birthday.mp4",
            "MP4",
            JobState::Done {
                action: convert,
                outputs: vec![r"C:\v\birthday (1).mp4".into()],
                saved: Some(crate::model::Saved::Folder(r"C:\v".into())),
            },
        ),
        listed(
            "scan-2019.pdf",
            "PDF",
            JobState::Failed {
                message:
                    "This file looks damaged, or isn't really a PDF. Try opening it in another app."
                        .into(),
                details: String::new(),
                engine: None,
                store_product: None,
            },
        ),
    ];
    let empty = ways_in::empty_lines();
    let shown = if pane.is_some() { 1.0 } else { 0.0 };
    let l = layout::layout_under(w, h, entries.len(), shown, 48.0);
    let credit_row = crate::credit::in_footer(
        layout::footer_row(&l.footer.unwrap(), false),
        super::credit_widths(&g.text, 1.0),
    );
    let (notes, saving) = (HashMap::new(), HashSet::new());
    let selected: BTreeSet<usize> = [pane.unwrap_or(0)].into_iter().collect();
    // The pane for the selected row, laid out as the app lays it out.
    let mut files = crate::model::FileList::default();
    for (i, e) in entries.iter().enumerate() {
        files.insert(i, e.clone());
    }
    files.select_only(&[pane.unwrap_or(0)]);
    let look = crate::inspector::Lookups {
        kind_of: &|_| None,
        size_of: &|_| Some(1_000),
        note_of: &|_| None,
        saving_of: &|_| false,
        smaller_of: &|_| false,
    };
    let model = crate::inspector::inspect(&files, &look);
    let rect = l.inspector.unwrap_or_default();
    let text = &g.text;
    let measure = |s: &str, how: layout::Wrapped| {
        use crate::win::text::{Ramp, Style};
        let body = layout::inspector_text_width(rect.w);
        let (ramp, n, width) = match how {
            layout::Wrapped::Title(n) => (Ramp::Subtitle, n, body),
            layout::Wrapped::Body(n) => (Ramp::Caption, n, body),
            layout::Wrapped::Info(n) => (Ramp::Body, n, layout::info_text_width(rect.w)),
        };
        text.styled_height(s, Style::new(ramp), 1.0, width)
            .min(ramp.style().line * n as f32)
    };
    let button_w = |s: &str| {
        use crate::win::text::{Ramp, Style};
        let w = text
            .styled(s, Style::new(Ramp::Body), 1.0, 4096.0, 20.0)
            .map_or(0.0, |l| l.width);
        layout::kit::button_width(w, false).ceil()
    };
    let more_w = button_w(crate::inspector::MORE_FORMATS);
    let shape = layout::inspector_shape(&model, "", more_w, &measure, &button_w);
    let name = model
        .file_row()
        .and_then(|row| entries.get(row))
        .map_or("", |e| e.name.as_str());
    let title = layout::inspector_title_height(&model, name, &measure);
    let il = layout::inspector_layout_titled(&rect, &shape, title);
    let controls = crate::a11y::inspector_controls(&model, &il);
    let still = |_: ButtonId| (0.0, 0.0);
    let v = View {
        backdrop: crate::win::titlebar::Backdrop::Solid,
        title: TitleView {
            bar: crate::win::titlebar::layout(w, false),
            title: "Kiln",
            active: true,
            maximized: false,
            caption: Default::default(),
            back_ring: false,
            palette: f,
        },
        entries: &entries,
        notes: &notes,
        saving: &saving,
        saving_lines: &HashMap::new(),

        selected: &selected,
        kind: &|_| None,
        preview: &|_, _| None,
        row_links: true,
        focus: None,
        button_ring: None,
        shown: 0.0,
        selection_y: None,
        button: &still,
        hover_row: None,
        hover_cancel: false,
        hover_link: false,
        hover_retry: false,
        hover_options: false,
        scroll: 0.0,
        drag: 0.0,
        progress_clock: None,
        progress_only: false,
        row_anim: &|_| RowAnim::at_rest(1.0),
        status: "",
        empty: &empty,
        empty_status: "",
        first_run: None,
        empty_at: None,
        convert_enabled: true,
        convert_label: "Convert",
        footer_row: Some(credit_row),
        credit: credit_row.credit,
        undo: false,
        wheel: None,
        menu: menu.map(|m| MenuView {
            items: m.items,
            layout: m.layout,
            lit: m.lit,
        }),
        details: None,
        ask: None,
        tool: None,
        settings: None,
        sheet: None,
        help: None,
        inspector: pane.map(|_| InspectorView {
            now: InspectorPage {
                laid: rect,
                model: &model,
                layout: &il,
                controls: &controls,
                nothing: "",
                alpha: 1.0,
                rise: 0.0,
                live: false,
            },
            before: None,
        }),
        top_info: None,
    };
    unsafe {
        sh.rt.BeginDraw();
        sh.rt.SetTransform(&Matrix3x2::identity());
        sh.rt.Clear(Some(&color(f.base, 1.0)));
    }
    d.list(&l, &v).unwrap();
    d.footer(&l, &v).unwrap();
    d.credit(&v, f).unwrap();
    d.title_under(&v.title, v.button).unwrap();
    if let Some(mv) = &v.menu {
        d.row_menu(mv, f).unwrap();
    }
    d.title_over(&v.title).unwrap();
    unsafe { sh.rt.EndDraw(None, None).unwrap() };
    let px = sh.pixels();
    (sh, px, l, pane.map(|_| il.clone()))
}

/// Whether some pixel in `r` (DIPs), sampled every half DIP, is mostly
/// `want`: much nearer it than the colour at `r`'s corner, the ground a
/// thin glyph's antialiased strokes are drawn on.
fn any_in(sh: &Shot, px: &[u8], r: &layout::Rect, want: Rgb) -> bool {
    let ground = rgb(sh.at(px, r.x, r.y));
    let far = |a: Rgb, b: Rgb| {
        let d = |x: u8, y: u8| i32::from(x.abs_diff(y));
        d(a.0, b.0) + d(a.1, b.1) + d(a.2, b.2)
    };
    let (nx, ny) = ((r.w / 0.5) as usize, (r.h / 0.5) as usize);
    far(want, ground) > 60
        && (0..nx).any(|i| {
            (0..ny).any(|j| {
                let c = rgb(sh.at(px, r.x + i as f32 * 0.5, r.y + j as f32 * 0.5));
                far(c, want) * 3 < far(c, ground)
            })
        })
}

#[test]
fn rows_draw_their_bar_checkmark_and_error_glyph_in_their_tokens() {
    use crate::theme::fluent::ALL_BUILT_IN;
    let dir = shots_dir();
    for (name, f) in ALL_BUILT_IN
        .into_iter()
        .map(|f| (f.name, f))
        .chain([("contrast", fluent_contrast())])
    {
        for k in [1.0, 1.25, 2.0] {
            let at = format!("{name} at {k}");
            let (sh, px, l) = list_shot((1100.0, 720.0), k, &f);
            let c = |x: f32, y: f32| rgb(sh.at(&px, x, y));
            // The selected row's accent pill at its left.
            let r0 = l.rows[0];
            assert_eq!(c(r0.x + 1.5, r0.y + r0.h / 2.0), f.accent, "{at}");
            // The converting row's bar: the accent up to half way.
            let r1 = l.rows[1];
            let (end, _) = layout::row_text_ends(&r1, true, false, None, false, false);
            let bar = layout::row_bar(&r1, end);
            let mid_y = bar.y + bar.h / 2.0;
            assert_eq!(c(bar.x + bar.w * 0.25, mid_y), f.accent, "{at}");
            assert_ne!(c(bar.x + bar.w * 0.75, mid_y), f.accent, "{at}");
            // The saved row's checkmark in the success colour (the text's
            // under a contrast theme), the failed row's error glyph in the
            // critical colour.
            let ok = if f.high_contrast { f.text } else { f.success };
            // It sits just before its button, which is drawn as wide as its
            // words in its slot: somewhere across the slot's start.
            let before = |r: &layout::Rect| {
                let link = Some(crate::model::SHOW_IN_FOLDER);
                let start = layout::row_right_start(r, false, false, link, false);
                let m = layout::row_mark(r, start);
                layout::Rect::new(
                    m.x,
                    m.y,
                    layout::link_width(crate::model::SHOW_IN_FOLDER),
                    m.h,
                )
            };
            assert!(any_in(&sh, &px, &before(&l.rows[2]), ok), "{at}");
            assert!(any_in(&sh, &px, &before(&l.rows[3]), f.critical), "{at}");
            if k == 1.25 {
                sh.save(&dir.join(format!("list-rows-{name}-125.png")));
            }
        }
    }
}

#[test]
fn a_selected_row_under_contrast_draws_its_words_and_marks_in_highlight_text() {
    let f = fluent_contrast();
    let dir = shots_dir();
    for k in [1.0, 1.25, 2.0] {
        // Each row selected in turn: idle, converting, saved, failed.
        for i in 0..4 {
            let at = format!("row {i} at {k}");
            let (sh, px, l, _) = pane_shot((1100.0, 720.0), k, &f, Some(i), None);
            let r = l.rows[i];
            let running = i == 1;
            let name = layout::row_name(&r, running);
            let name = layout::Rect::new(name.x, name.y, 60.0, name.h);
            let line = layout::row_line_at(&r, running);
            let line = layout::Rect::new(line.x, line.y, 60.0, line.h);
            // On the Highlight fill: HighlightText, never WindowText.
            assert_eq!(
                rgb(sh.at(&px, name.x + 70.0, r.y + 4.0)),
                f.selection.rgb,
                "{at}"
            );
            for (what, rect) in [("name", name), ("line", line)] {
                assert!(any_in(&sh, &px, &rect, f.on_selection), "{at}: {what}");
                assert!(!any_in(&sh, &px, &rect, f.text), "{at}: {what}");
            }
            // The saved row's checkmark and the failed row's error glyph.
            if i >= 2 {
                let link = (i == 2).then_some(crate::model::SHOW_IN_FOLDER);
                let start = layout::row_right_start(&r, false, false, link, false);
                let m = layout::row_mark(&r, start);
                // Before its button, drawn as wide as its words in its slot.
                let w = link.map_or(m.w, layout::link_width);
                let m = layout::Rect::new(m.x, m.y, w, m.h);
                assert!(any_in(&sh, &px, &m, f.on_selection), "{at}: mark");
                assert!(!any_in(&sh, &px, &m, f.text), "{at}: mark");
            }
            if k == 1.25 {
                sh.save(&dir.join(format!("list-contrast-selected-{i}-125.png")));
            }
        }
    }
}

#[test]
fn the_footer_draws_add_files_clear_list_and_convert_in_their_tokens() {
    use crate::theme::fluent::ALL_BUILT_IN;
    for (name, f) in ALL_BUILT_IN
        .into_iter()
        .map(|f| (f.name, f))
        .chain([("contrast", fluent_contrast())])
    {
        for k in [1.0, 1.25, 2.0] {
            let at = format!("{name} at {k}");
            let (sh, px, l) = list_shot((1100.0, 720.0), k, &f);
            let c = |x: f32, y: f32| rgb(sh.at(&px, x, y));
            let foot = l.footer.unwrap();
            let row = layout::footer_row(&foot, false);
            // Convert is the accent, at the right; Clear list and Add files
            // are standard buttons, not the accent.
            let mid = |r: &layout::Rect| (r.x + 3.0, r.y + r.h / 2.0);
            let (x, y) = mid(&row.convert);
            assert_eq!(c(x, y), f.accent, "{at}");
            for b in [row.clear, row.add] {
                let (x, y) = mid(&b);
                assert_ne!(c(x, y), f.accent, "{at}");
            }
        }
    }
}

#[test]
fn the_side_pane_draws_its_infobars_and_formats_in_their_tokens() {
    use crate::theme::fluent::ALL_BUILT_IN;
    let dir = shots_dir();
    for (name, f) in ALL_BUILT_IN
        .into_iter()
        .map(|f| (f.name, f))
        .chain([("contrast", fluent_contrast())])
    {
        for k in [1.0, 1.25, 2.0] {
            let at = format!("{name} at {k}");
            for (row, state) in [
                (0, "waiting"),
                (1, "converting"),
                (2, "done"),
                (3, "failed"),
            ] {
                let (sh, px, l, il) = pane_shot((1100.0, 720.0), k, &f, Some(row), None);
                let il = il.unwrap();
                let c = |x: f32, y: f32| rgb(sh.at(&px, x, y));
                let pane = l.inspector.unwrap();
                assert!(pane.w >= 320.0 && pane.w <= 400.0, "{at}");
                match state {
                    "waiting" => {
                        // A format's row is stroked; the pane has a title.
                        let t = il.targets[0];
                        assert!(il.title.h > 0.0, "{at}");
                        let mid = t.x + t.w / 2.0;
                        assert_ne!(c(mid, t.y + 0.5), c(mid, t.y - 3.0), "{at}");
                    }
                    "converting" => {
                        // Its bar: the accent up to half way.
                        let bar = il.line(2).unwrap();
                        let y = bar.y + bar.h / 2.0;
                        assert_eq!(c(bar.x + bar.w * 0.25, y), f.accent, "{at}");
                        assert_ne!(c(bar.x + bar.w * 0.75, y), f.accent, "{at}");
                    }
                    _ => {
                        let sev = if state == "done" {
                            crate::controls::Severity::Success
                        } else {
                            crate::controls::Severity::Critical
                        };
                        let (fill, ink) = sev.colours(&f);
                        let bar = il.info.clone().unwrap();
                        // Its glyph in the severity's ink; its fill, where
                        // solid, at its right end past the text.
                        assert!(any_in(&sh, &px, &bar.glyph, ink), "{at} {state}");
                        if fill.alpha >= 1.0 {
                            let r = bar.rect;
                            let y = r.y + r.h / 2.0;
                            assert_eq!(c(r.right() - 4.0, y), fill.rgb, "{at} {state}");
                        }
                        assert!(!il.buttons.is_empty(), "{at} {state}");
                    }
                }
                if k == 1.25 {
                    sh.save(&dir.join(format!("pane-{state}-{name}-125.png")));
                }
            }
        }
    }
}

#[test]
fn the_side_panes_offer_rows_have_the_control_fill_and_a_visible_outline_at_rest() {
    // In dark the rows were a card-stroked outline nobody could
    // see; they now have the control fill at rest as well.
    use crate::theme::{contrast, fluent::DARK};
    let f = DARK;
    for k in [1.0, 1.25, 2.0] {
        let at = format!("dark at {k}");
        let (sh, px, _, il) = pane_shot((1100.0, 720.0), k, &f, Some(0), None);
        let t = il.unwrap().targets[0];
        let c = |x: f32, y: f32| rgb(sh.at(&px, x, y));
        let y = t.y + t.h / 2.0;
        let card = c(t.x - 3.0, y);
        let filled = c(t.x + 4.0, y);
        assert!(
            near(filled, f.control.over(card)),
            "{at}: {filled:?} on {card:?}"
        );
        assert_ne!(filled, card, "{at}");
        // The outline's own pixel is not the fill or the card.
        let edge = c(t.x + 0.25, y);
        assert!(!near(edge, filled) && !near(edge, card), "{at}: {edge:?}");
        assert!(contrast(edge, card) >= 1.15, "{at}: {edge:?} on {card:?}");
    }
}

#[test]
fn the_right_click_menu_draws_as_a_flyout_over_the_list() {
    use crate::theme::fluent::ALL_BUILT_IN;
    use crate::ways_in::{menu_items, RowState};
    use crate::wheel::WheelKind;
    let dir = shots_dir();
    let items = menu_items(&[RowState::Saved], &[WheelKind::Convert]);
    let rows: Vec<layout::kit::MenuRow> = items
        .iter()
        .map(|&i| match i {
            crate::ways_in::MenuItem::Separator => layout::kit::MenuRow::Separator,
            _ => layout::kit::MenuRow::Item {
                label: 100.0,
                accel: 40.0,
                glyph: true,
            },
        })
        .collect();
    let ml = layout::kit::menu_flyout(
        (200.0, 140.0),
        &rows,
        layout::Rect::new(0.0, 0.0, 1100.0, 720.0),
    );
    for (name, f) in ALL_BUILT_IN
        .into_iter()
        .map(|f| (f.name, f))
        .chain([("contrast", fluent_contrast())])
    {
        for k in [1.0, 1.25, 2.0] {
            let at = format!("{name} at {k}");
            let mv = MenuView {
                items: &items,
                layout: &ml,
                lit: Some(2),
            };
            let (sh, px, _, _) = pane_shot((1100.0, 720.0), k, &f, None, Some(&mv));
            // Each line's words in the text colour; the flyout covers the
            // rows under it.
            for (i, r) in ml.rows.iter().enumerate() {
                if items[i] != crate::ways_in::MenuItem::Separator {
                    // The glyph column, then the words.
                    let words = layout::Rect::new(r.x + 12.0 + 28.0, r.y + 4.0, 60.0, r.h - 8.0);
                    let ink = if f.high_contrast && i == 2 {
                        f.on_selection
                    } else {
                        f.text
                    };
                    assert!(any_in(&sh, &px, &words, ink), "{at} line {i}");
                    // Each line that has a glyph shows it in the column.
                    if items[i].glyph().is_some() {
                        let col = layout::Rect::new(r.x + 12.0, r.y + 4.0, 16.0, r.h - 8.0);
                        assert!(any_in(&sh, &px, &col, ink), "{at} glyph of line {i}");
                    }
                }
            }
            if k == 1.25 {
                sh.save(&dir.join(format!("row-menu-{name}-125.png")));
            }
        }
    }
}

#[test]
fn the_tray_menu_draws_its_check_mark_and_words_in_every_theme() {
    // The notification-area icon's menu is this flyout, alone in its own
    // window: a check mark in front of Drag-wheel, in the theme's text
    // colour on its overlay.
    use crate::theme::fluent::{glyph, ALL_BUILT_IN};
    use layout::kit::{MenuRow, BUTTON_GLYPH, BUTTON_PAD};
    let rows = [
        MenuRow::Item {
            label: 70.0,
            accel: 0.0,
            glyph: true,
        },
        MenuRow::Item {
            label: 70.0,
            accel: 0.0,
            glyph: true,
        },
        MenuRow::Separator,
        MenuRow::Item {
            label: 70.0,
            accel: 0.0,
            glyph: true,
        },
    ];
    let lines = [
        MenuLine::Item {
            label: "Open Kiln",
            glyph: None,
            shortcut: None,
            enabled: true,
        },
        MenuLine::Item {
            label: "Drag-wheel",
            glyph: Some(glyph::CHECKMARK),
            shortcut: None,
            enabled: true,
        },
        MenuLine::Separator,
        MenuLine::Item {
            label: "Quit Kiln",
            glyph: None,
            shortcut: None,
            enabled: true,
        },
    ];
    let ml = layout::kit::menu_flyout(
        (20.0, 20.0),
        &rows,
        layout::Rect::new(0.0, 0.0, 300.0, 200.0),
    );
    for (name, f) in ALL_BUILT_IN
        .into_iter()
        .map(|f| (f.name, f))
        .chain([("contrast", fluent_contrast())])
    {
        for k in [1.0, 1.5, 2.0] {
            let at = format!("{name} at {k}");
            let sh = shot(300.0, 200.0, k);
            let g = rig(&sh);
            let d = Draw {
                factory: &sh.factory,
                rt: &sh.rt,
                dpi: 96.0 * k,
                zoom: 1.0,
                brush: &g.brush,
                wheel: &g.wheel,
                text: &g.text,
                round: &g.round,
                dash: &g.dash,
                mark: &g.mark,
                base: Cell::new(Matrix3x2::identity()),
                pictures: &g.pictures,
                frame: 1,
                clips: &g.clips,
            };
            unsafe {
                sh.rt.BeginDraw();
                sh.rt.SetTransform(&Matrix3x2::identity());
                sh.rt.Clear(Some(&D2D1_COLOR_F::default()));
            }
            d.menu_flyout(&ml, &lines, Some(0), &f).unwrap();
            unsafe { sh.rt.EndDraw(None, None).unwrap() };
            let px = sh.pixels();
            // The menu's face is the overlay, and what is round it is
            // clear.
            let inside = ml.rows[3];
            assert_eq!(
                rgb(sh.at(&px, inside.x + 2.0, inside.y + 2.0)),
                f.overlay,
                "{at}"
            );
            assert_eq!(sh.at(&px, 2.0, 2.0)[3], 0, "{at}: clear outside");
            // The check mark in the glyph column of the checked line.
            let r = ml.rows[1];
            let mark = layout::Rect::new(r.x + BUTTON_PAD, r.y + 4.0, BUTTON_GLYPH, r.h - 8.0);
            assert!(any_in(&sh, &px, &mark, f.text), "{at}: no check mark");
            // The unchecked line has none, and every line has its words.
            let none = layout::Rect::new(
                ml.rows[0].x + BUTTON_PAD,
                ml.rows[0].y + 4.0,
                BUTTON_GLYPH,
                ml.rows[0].h - 8.0,
            );
            assert!(
                !any_in(&sh, &px, &none, f.text),
                "{at}: a mark on Open Kiln"
            );
            for i in [0, 1, 3] {
                let r = ml.rows[i];
                let x = r.x + BUTTON_PAD + BUTTON_GLYPH + BUTTON_PAD;
                let words = layout::Rect::new(x, r.y + 4.0, 50.0, r.h - 8.0);
                let ink = if f.high_contrast && i == 0 {
                    f.on_selection
                } else {
                    f.text
                };
                assert!(any_in(&sh, &px, &words, ink), "{at} line {i}");
            }
        }
    }
}

#[test]
fn the_facts_labels_fit_their_column_at_every_text_size() {
    // The side pane's facts: the label in a 56-unit column, the value after
    // it. A label is set at the Text size's zoom in the same units.
    use crate::win::text::{Ramp, Style};
    let text = Text::new().unwrap();
    for zoom in [1.0, 1.5, 2.25] {
        for label in ["Size", "Dimensions", "Length", "Folder"] {
            let w = text
                .styled(label, Style::new(Ramp::Caption), zoom, 4096.0, 20.0)
                .unwrap()
                .width;
            assert!(
                w <= layout::FACT_LABEL - 4.0,
                "{label} is {w} wide in a {} column at {zoom}",
                layout::FACT_LABEL
            );
        }
    }
}

#[test]
fn link_words_are_measured() {
    // a row's link slot is its button, sized to its words, so
    // "Get Media Feature Pack" fits. The table in `layout` is the words'
    // width in Body, rounded up to a whole DIP.
    use crate::win::text::{Ramp, Style};
    let text = Text::new().unwrap();
    let mut short = vec![];
    for (label, wide) in layout::link_measures() {
        let measured = text
            .styled(label, Style::new(Ramp::Body), 1.0, 4096.0, 20.0)
            .unwrap()
            .width;
        println!("{label:?}: {measured}");
        if measured.ceil() != wide {
            short.push((label, measured, wide));
        }
    }
    assert!(short.is_empty(), "{short:?}");
    // Every link a row can show has a measure.
    use crate::model;
    let mut labels = vec![
        model::SHOW_IN_FOLDER,
        "Show details",
        "Convert",
        model::get_label("audio-mf"),
        model::get_label("ffmpeg"),
        model::get_label("pandoc"),
        model::get_label("libreoffice"),
        model::get_label("calibre"),
    ];
    for p in [
        "9N4WGH0Z6VHQ",
        "9MVZQVXJBQ9V",
        "9N4D0MSMP0PT",
        "9PMMSR1CGPWG",
        "other",
    ] {
        labels.push(model::store_label(p));
    }
    let known: Vec<&str> = layout::link_measures().map(|(l, _)| l).collect();
    for l in labels {
        assert!(known.contains(&l), "{l} has no measure");
    }
    assert_eq!(model::get_label("audio-mf"), "Get Media Feature Pack");
}

#[test]
fn a_saved_rows_line_is_secondary_and_a_running_rows_bar_runs_to_its_percentage() {
    use crate::theme::fluent::ALL_BUILT_IN;
    for (name, f) in ALL_BUILT_IN.map(|f| (f.name, f)) {
        for k in [1.0, 2.0] {
            let at = format!("{name} at {k}");
            let (sh, px, l) = list_shot((1100.0, 720.0), k, &f);
            let c = |x: f32, y: f32| rgb(sh.at(&px, x, y));
            // The saved row's second line: the secondary text colour, never
            // the primary (the name above it is the primary).
            let r2 = l.rows[2];
            let line = layout::row_line_at(&r2, false);
            let line = layout::Rect::new(line.x, line.y, 60.0, line.h);
            assert!(any_in(&sh, &px, &line, f.text_secondary), "{at}");
            // The strongest ink in it is nearer the secondary than the
            // primary colour.
            let ground = rgb(sh.at(&px, line.x, line.y));
            let far = |a: Rgb, b: Rgb| {
                let d = |x: u8, y: u8| i32::from(x.abs_diff(y));
                d(a.0, b.0) + d(a.1, b.1) + d(a.2, b.2)
            };
            let mut strongest = 0;
            for i in 0..(line.w / 0.5) as usize {
                for j in 0..(line.h / 0.5) as usize {
                    let p = rgb(sh.at(&px, line.x + i as f32 * 0.5, line.y + j as f32 * 0.5));
                    strongest = strongest.max(far(p, ground));
                }
            }
            let (quiet, loud) = (far(f.text_secondary, ground), far(f.text, ground));
            assert!(strongest.abs_diff(quiet) < strongest.abs_diff(loud), "{at}");
            // The running row's bar ends where its text ends, 12 short of
            // the percentage, and the row has nothing of it past that.
            let r1 = l.rows[1];
            let (end, _) = layout::row_text_ends(&r1, true, false, None, false, false);
            assert_eq!(end, layout::row_percent(&r1).x - 12.0, "{at}");
            let bar = layout::row_bar(&r1, end);
            assert!(bar.w > 360.0 || bar.right() == end, "{at}");
            let mid_y = bar.y + bar.h / 2.0;
            assert_ne!(
                c(bar.right() - 2.0, mid_y),
                c(bar.right() + 6.0, mid_y),
                "{at}"
            );
        }
    }
}

/// Kiln's K in the window is the flame K of the app icon and the README,
/// flat in the accent's text colour on the accent tile: in the title bar
/// and About at 16 DIPs, and in the empty window at 64. On a 16-unit
/// tile, (9.6, 6.4) is inside the flame and outside the old three
/// strokes, and (9.15, 9.55) is on the old leg and outside the flame.
#[test]
fn the_k_is_the_flame_mark_flat_on_the_accent_tile_in_every_theme_and_size() {
    use crate::theme::fluent::ALL_BUILT_IN;
    const FLAME: (f32, f32) = (9.6, 6.4);
    const OLD_LEG: (f32, f32) = (9.15, 9.55);
    const TILE: (f32, f32) = (8.0, 1.0);
    let dir = shots_dir();
    for (name, f) in ALL_BUILT_IN
        .into_iter()
        .map(|f| (f.name, f))
        .chain([("contrast", fluent_contrast())])
    {
        for k in [1.0, 1.25, 1.5, 1.75, 2.0] {
            for side in [16.0_f32, 60.0] {
                let at = format!("{name} at {k}, {side} DIPs");
                let r = layout::Rect::new(4.0, 4.0, side, side);
                let (sh, px) = kit_shot(side + 8.0, side + 8.0, k, &f, |d| d.k_mark(&r, &f));
                let c = |(x, y): (f32, f32)| {
                    rgb(sh.at(&px, r.x + x * side / 16.0, r.y + y * side / 16.0))
                };
                assert_eq!(c(FLAME), f.on_accent, "{at}: the flame");
                assert_eq!(c(OLD_LEG), f.accent, "{at}: no old leg");
                assert_eq!(c(TILE), f.accent, "{at}: the tile");
                let pct = (k * 100.0).round() as u32;
                sh.save(&dir.join(format!("k-mark-{name}-{side}-{pct}.png")));
            }
            // The title bar's and the empty window's K are this one.
            if name == "dark" {
                use crate::win::titlebar::{layout as bar, Caption};
                let b = bar(720.0, false);
                let (sh, px) = title_shot(720.0, k, Caption::default(), false);
                let c = |(x, y): (f32, f32)| {
                    rgb(sh.at(
                        &px,
                        b.mark.x + x * b.mark.w / 16.0,
                        b.mark.y + y * b.mark.w / 16.0,
                    ))
                };
                assert_eq!(c(FLAME), f.on_accent, "title bar at {k}");
                assert_eq!(c(OLD_LEG), f.accent, "title bar at {k}");
            }
            let (sh, px, e) = empty_shot((1100.0, 720.0), k, &f, false, 0.0);
            let m = e.mark.expect("the K shows in a tall window");
            let s = m.w / 64.0;
            let (tx, ty, tw) = (m.x + 2.0 * s, m.y + 2.0 * s, 60.0 * s);
            let c = |(x, y): (f32, f32)| rgb(sh.at(&px, tx + x * tw / 16.0, ty + y * tw / 16.0));
            assert_eq!(c(FLAME), f.on_accent, "empty window, {name} at {k}");
            assert_eq!(c(OLD_LEG), f.accent, "empty window, {name} at {k}");
        }
    }
}
