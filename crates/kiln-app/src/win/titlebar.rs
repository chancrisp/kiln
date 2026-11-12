//! The title bar Kiln draws itself, 48 DIPs over the top of the window:
//! where its parts are, what a point on the window is to Windows (the
//! answer to `WM_NCHITTEST`), the caption buttons' hover and press, the
//! frame Windows keeps, and whether Mica is behind the window. Pure, so it
//! is tested without a window.

use crate::layout::Rect;
use crate::theme::fluent::{glyph, space};

/// The title bar's height in DIPs. It never grows: only its text does,
/// with Text size.
pub const HEIGHT: f32 = space::TITLE_BAR;

/// The caption buttons' glyphs, in DIPs, as Windows' own.
pub const CAPTION_GLYPH: f32 = 10.0;

/// Back, Help and Settings' glyphs, in DIPs.
pub const BUTTON_GLYPH: f32 = 16.0;

/// The gap after Back and between the subtle buttons, in DIPs.
const GAP: f32 = 4.0;

/// The gap between the K and the title, in DIPs.
const MARK_GAP: f32 = 12.0;

/// Where Back starts: it reaches into the bar's left padding, as in the
/// mock.
const BACK_X: f32 = 4.0;

/// A button on the title bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Part {
    Back,
    Help,
    Settings,
    Min,
    Max,
    Close,
}

impl Part {
    /// Its Segoe Fluent Icons glyph.
    pub fn glyph(self, maximized: bool) -> char {
        match self {
            Part::Back => glyph::BACK,
            Part::Help => glyph::HELP,
            Part::Settings => glyph::SETTINGS,
            Part::Min => glyph::MINIMISE,
            Part::Max if maximized => glyph::RESTORE,
            Part::Max => glyph::MAXIMISE,
            Part::Close => glyph::CLOSE,
        }
    }
}

/// Where the title bar's parts are, in DIPs from the window's top left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bar {
    /// The whole bar, as wide as the window.
    pub rect: Rect,
    /// Back, while Settings is open.
    pub back: Option<Rect>,
    /// The K.
    pub mark: Rect,
    /// From where the title starts to where the first button on the right
    /// leaves it, the bar's height tall: the text is centred in it.
    pub title: Rect,
    /// Help and Settings, while Settings isn't open.
    pub help: Option<Rect>,
    pub settings: Option<Rect>,
    pub min: Rect,
    pub max: Rect,
    pub close: Rect,
}

impl Bar {
    /// Every button there is, left to right.
    pub fn parts(&self) -> Vec<(Part, Rect)> {
        [
            (Part::Back, self.back),
            (Part::Help, self.help),
            (Part::Settings, self.settings),
            (Part::Min, Some(self.min)),
            (Part::Max, Some(self.max)),
            (Part::Close, Some(self.close)),
        ]
        .into_iter()
        .filter_map(|(p, r)| Some((p, r?)))
        .collect()
    }

    /// The button under `(x, y)`, in the bar's units.
    pub fn part_at(&self, x: f32, y: f32) -> Option<Part> {
        self.parts()
            .into_iter()
            .find_map(|(p, r)| r.contains(x, y).then_some(p))
    }

    /// Where `part` is, if it's there.
    pub fn rect_of(&self, part: Part) -> Option<Rect> {
        self.parts()
            .into_iter()
            .find_map(|(p, r)| (p == part).then_some(r))
    }

    /// The bar in other units: DIPs times `k`. Layout units are DIPs
    /// divided by the Text size's zoom.
    pub fn scaled(&self, k: f32) -> Bar {
        let s = |r: Rect| Rect::new(r.x * k, r.y * k, r.w * k, r.h * k);
        Bar {
            rect: s(self.rect),
            back: self.back.map(s),
            mark: s(self.mark),
            title: s(self.title),
            help: self.help.map(s),
            settings: self.settings.map(s),
            min: s(self.min),
            max: s(self.max),
            close: s(self.close),
        }
    }
}

/// What the title bar says: "Kiln", on every page. The Settings page has
/// its own heading, so the bar doesn't say it twice.
pub fn title() -> &'static str {
    "Kiln"
}

/// The title bar of a window `width` DIPs wide. While Settings is open,
/// Back comes first and Help and Settings go.
pub fn layout(width: f32, in_settings: bool) -> Bar {
    let (cw, ch) = space::CAPTION_BUTTON;
    let close = Rect::new(width - cw, 0.0, cw, ch);
    let max = Rect::new(close.x - cw, 0.0, cw, ch);
    let min = Rect::new(max.x - cw, 0.0, cw, ch);
    let (bw, bh) = space::TITLE_BUTTON;
    let by = (HEIGHT - bh) / 2.0;
    let back = in_settings.then(|| Rect::new(BACK_X, by, bw, bh));
    let mark_x = back.map_or(space::TITLE_MARK_X, |b| b.right() + GAP);
    let side = space::TITLE_MARK;
    let mark = Rect::new(mark_x, (HEIGHT - side) / 2.0, side, side);
    let settings = (!in_settings).then(|| Rect::new(min.x - GAP - bw, by, bw, bh));
    let help = settings.map(|s| Rect::new(s.x - GAP - bw, by, bw, bh));
    let right = help.map_or(min.x, |h| h.x);
    let title_x = mark.right() + MARK_GAP;
    Bar {
        rect: Rect::new(0.0, 0.0, width, HEIGHT),
        back,
        mark,
        title: Rect::new(title_x, 0.0, (right - MARK_GAP - title_x).max(0.0), HEIGHT),
        help,
        settings,
        min,
        max,
        close,
    }
}

/// What a point on the window is to Windows: `WM_NCHITTEST`'s answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// Kiln's own: the content, and Back, Help and Settings.
    Client,
    /// The bar where nothing is: it drags the window, and a double click
    /// maximises it.
    Caption,
    Min,
    /// Over Maximise: Windows shows Snap Layouts here.
    Max,
    Close,
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    /// A maximised window's frame, off the screen: no resizing.
    Border,
}

impl Hit {
    /// The `HT*` value Windows takes.
    pub fn code(self) -> u32 {
        match self {
            Hit::Client => 1,
            Hit::Caption => 2,
            Hit::Min => 8,
            Hit::Max => 9,
            Hit::Left => 10,
            Hit::Right => 11,
            Hit::Top => 12,
            Hit::TopLeft => 13,
            Hit::TopRight => 14,
            Hit::Bottom => 15,
            Hit::BottomLeft => 16,
            Hit::BottomRight => 17,
            Hit::Border => 18,
            Hit::Close => 20,
        }
    }
}

/// The frame Windows keeps around the client area, in pixels: the resize
/// borders at the sides and the bottom (`SM_CXSIZEFRAME` plus
/// `SM_CXPADDEDBORDER`, and the same for Y). The top has none outside the
/// client area: the title bar is Kiln's, and the top resize band is the
/// top `y` pixels of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub x: i32,
    pub y: i32,
    /// The top resize band over the caption buttons: `SM_CYSIZEFRAME`
    /// alone, as Windows' own frame leaves the rest of each button to it.
    pub band: i32,
}

/// The edges of the monitor with a taskbar that hides itself, which a
/// maximised Kiln leaves uncovered (`ABM_GETAUTOHIDEBAREX`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AutoHide {
    pub left: bool,
    pub top: bool,
    pub right: bool,
    pub bottom: bool,
}

/// How many pixels a maximised window leaves of the screen on a hidden
/// taskbar's edge, so the taskbar can be called up: as Windows Terminal
/// and Chromium do.
pub const AUTOHIDE_GAP: i32 = 2;

impl AutoHide {
    /// The edges `has_bar` says hold a hidden taskbar, asked by Windows'
    /// edge numbers: `ABE_LEFT` 0, `ABE_TOP` 1, `ABE_RIGHT` 2,
    /// `ABE_BOTTOM` 3.
    pub fn ask(has_bar: impl Fn(u32) -> bool) -> AutoHide {
        AutoHide {
            left: has_bar(0),
            top: has_bar(1),
            right: has_bar(2),
            bottom: has_bar(3),
        }
    }
}

/// The hidden taskbars of each monitor Kiln was maximised on, as the
/// shell last said: asked once per monitor, then read from here, so the
/// frame's calculation (`WM_NCCALCSIZE`) doesn't wait on Explorer each
/// time. Forgotten when the taskbars may have changed (the work area,
/// the display, a new taskbar), and asked again then, off that path.
#[derive(Debug, Default)]
pub struct BarCache {
    known: Vec<(isize, AutoHide)>,
}

impl BarCache {
    /// `monitor`'s hidden taskbars, asked with `ask` only if not known.
    pub fn get(&mut self, monitor: isize, ask: impl FnOnce() -> AutoHide) -> AutoHide {
        if let Some(&(_, bars)) = self.known.iter().find(|(m, _)| *m == monitor) {
            return bars;
        }
        let bars = ask();
        self.known.push((monitor, bars));
        bars
    }

    /// The taskbars may have changed: ask again from now on.
    pub fn forget(&mut self) {
        self.known.clear();
    }
}

/// How far the client area is in from each side of the window, in pixels:
/// left, top, right, bottom (`WM_NCCALCSIZE`). A maximised window hangs
/// its frame off the screen on every side, the top too, so its title bar
/// starts at the top of the screen; on an edge in `autohide` it leaves
/// [`AUTOHIDE_GAP`] pixels of the screen uncovered.
pub fn client_insets(frame: Frame, maximized: bool, autohide: AutoHide) -> (i32, i32, i32, i32) {
    if !maximized {
        return (frame.x, 0, frame.x, frame.y);
    }
    let gap = |on: bool| if on { AUTOHIDE_GAP } else { 0 };
    (
        frame.x + gap(autohide.left),
        frame.y + gap(autohide.top),
        frame.x + gap(autohide.right),
        frame.y + gap(autohide.bottom),
    )
}

/// The window's outer size, in pixels, for a client area of `client`
/// pixels, not maximised.
pub fn outer_size(client: (i32, i32), frame: Frame) -> (i32, i32) {
    let (l, t, r, b) = client_insets(frame, false, AutoHide::default());
    (client.0 + l + r, client.1 + t + b)
}

/// What `(x, y)`, in pixels from the window's top left, is on a window
/// `size` pixels big, with `frame`, at `scale` pixels per DIP, its title
/// bar `bar` (in DIPs from the client area's top left).
pub fn hit(
    (x, y): (f32, f32),
    size: (f32, f32),
    frame: Frame,
    scale: f32,
    maximized: bool,
    autohide: AutoHide,
    bar: &Bar,
) -> Hit {
    let (w, h) = size;
    let (l, t, r, b) = client_insets(frame, maximized, autohide);
    let (cl, ct, cr, cb) = (l as f32, t as f32, w - r as f32, h - b as f32);
    let inside = x >= cl && x < cr && y >= ct && y < cb;
    if maximized && !inside {
        return Hit::Border;
    }
    let (dx, dy) = ((x - cl) / scale, (y - ct) / scale);
    if !maximized {
        // Over Minimise, Maximise and Close the band is the size frame
        // alone, as Windows' own frame has it; elsewhere the whole frame.
        let over_button = matches!(
            bar.part_at(dx, bar.min.y),
            Some(Part::Min | Part::Max | Part::Close)
        );
        let top = if over_button { frame.band } else { frame.y };
        if let Some(edge) = edge_at((x, y), (cl, cr, cb), frame, top as f32) {
            return edge;
        }
    }
    if dy >= bar.rect.bottom() {
        return Hit::Client;
    }
    match bar.part_at(dx, dy) {
        Some(Part::Min) => Hit::Min,
        Some(Part::Max) => Hit::Max,
        Some(Part::Close) => Hit::Close,
        Some(Part::Back | Part::Help | Part::Settings) => Hit::Client,
        None => Hit::Caption,
    }
}

/// The resize edge or corner at `(x, y)` on a window that isn't maximised,
/// whose client area runs from `cl` to `cr` across and down to `cb`: the
/// frame at the sides and bottom, and the top `top` pixels of the client
/// area. A corner reaches a frame's width along each edge.
fn edge_at(
    (x, y): (f32, f32),
    (cl, cr, cb): (f32, f32, f32),
    frame: Frame,
    top: f32,
) -> Option<Hit> {
    let (fx, fy) = (frame.x as f32, frame.y as f32);
    let (near_left, near_right) = (x < cl + fx, x >= cr - fx);
    let (near_top, near_bottom) = (y < 2.0 * fy, y >= cb - fy);
    let pick = |first: bool, second: bool, a: Hit, b: Hit, c: Hit| {
        if first {
            a
        } else if second {
            b
        } else {
            c
        }
    };
    if x < cl {
        Some(pick(
            near_top,
            near_bottom,
            Hit::TopLeft,
            Hit::BottomLeft,
            Hit::Left,
        ))
    } else if x >= cr {
        Some(pick(
            near_top,
            near_bottom,
            Hit::TopRight,
            Hit::BottomRight,
            Hit::Right,
        ))
    } else if y >= cb {
        Some(pick(
            near_left,
            near_right,
            Hit::BottomLeft,
            Hit::BottomRight,
            Hit::Bottom,
        ))
    } else if y < top {
        Some(pick(
            near_left,
            near_right,
            Hit::TopLeft,
            Hit::TopRight,
            Hit::Top,
        ))
    } else {
        None
    }
}

/// The caption buttons' and Back's hover and press. A button acts on
/// release over the same button it went down on; sliding off and letting
/// go does nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Caption {
    pub hover: Option<Part>,
    pub pressed: Option<Part>,
}

impl Caption {
    /// The pointer is over `over`. Whether anything drawn changed.
    pub fn hover(&mut self, over: Option<Part>) -> bool {
        let changed = self.hover != over;
        self.hover = over;
        changed
    }

    /// The left button went down over `over`. Whether anything drawn
    /// changed.
    pub fn press(&mut self, over: Option<Part>) -> bool {
        let was = *self;
        self.pressed = over;
        self.hover = over;
        *self != was
    }

    /// The left button came up over `over`: the button to act on, if it
    /// is the one that went down.
    pub fn release(&mut self, over: Option<Part>) -> Option<Part> {
        let pressed = self.pressed.take()?;
        (over == Some(pressed)).then_some(pressed)
    }

    /// The pointer left: nothing is lit or held. Whether anything drawn
    /// changed.
    pub fn leave(&mut self) -> bool {
        let changed = *self != Caption::default();
        *self = Caption::default();
        changed
    }
}

/// What is behind the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backdrop {
    /// Windows' Mica, through what Kiln leaves transparent.
    Mica,
    /// Kiln's own solid base colour.
    Solid,
}

/// The first Windows build with `DWMWA_SYSTEMBACKDROP_TYPE`: Windows 11
/// 22H2.
pub const MICA_BUILD: u32 = 22621;

/// What is behind the window on Windows `build`, with transparency effects
/// on or off, with a theme that wants the solid base (a contrast theme or
/// one of the named themes) or not: Mica where Windows has it and allows
/// it, the solid base otherwise.
pub fn backdrop(build: u32, transparency: bool, solid_theme: bool) -> Backdrop {
    if build >= MICA_BUILD && transparency && !solid_theme {
        Backdrop::Mica
    } else {
        Backdrop::Solid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCALES: [f32; 3] = [1.0, 1.25, 2.0];

    /// The frame Windows 11 keeps at a scale: 4 + 4 pixels at 100%, the
    /// size frame alone 4.
    fn frame(scale: f32) -> Frame {
        let px = (8.0 * scale).round() as i32;
        let band = (4.0 * scale).round() as i32;
        Frame { x: px, y: px, band }
    }

    const NO_BAR: AutoHide = AutoHide {
        left: false,
        top: false,
        right: false,
        bottom: false,
    };

    /// A window `w` x `h` DIPs of client area at `scale`, not maximised:
    /// its outer size in pixels.
    fn window(w: f32, h: f32, scale: f32) -> (f32, f32) {
        let (ow, oh) = outer_size(
            ((w * scale).round() as i32, (h * scale).round() as i32),
            frame(scale),
        );
        (ow as f32, oh as f32)
    }

    /// The window point of a client point in DIPs, at `scale`.
    fn at(x: f32, y: f32, scale: f32, maximized: bool) -> (f32, f32) {
        let (l, t, _, _) = client_insets(frame(scale), maximized, NO_BAR);
        (l as f32 + x * scale, t as f32 + y * scale)
    }

    fn centre(r: Rect) -> (f32, f32) {
        (r.x + r.w / 2.0, r.y + r.h / 2.0)
    }

    #[test]
    fn the_k_sits_at_16_and_the_buttons_come_in_from_the_right_46_wide() {
        for w in [480.0, 720.0, 1100.0] {
            let b = layout(w, false);
            assert_eq!(b.rect, Rect::new(0.0, 0.0, w, 48.0));
            assert_eq!(b.mark, Rect::new(16.0, 16.0, 16.0, 16.0));
            assert_eq!(b.title.x, 16.0 + 16.0 + 12.0);
            assert_eq!((b.title.y, b.title.h), (0.0, 48.0));
            assert_eq!(b.close, Rect::new(w - 46.0, 0.0, 46.0, 48.0));
            assert_eq!(b.max, Rect::new(w - 92.0, 0.0, 46.0, 48.0));
            assert_eq!(b.min, Rect::new(w - 138.0, 0.0, 46.0, 48.0));
            // Settings, then Help, 40 x 32 subtle buttons before Minimise,
            // centred on the bar.
            let s = b.settings.expect("Settings");
            let h = b.help.expect("Help");
            assert_eq!(s, Rect::new(w - 138.0 - 4.0 - 40.0, 8.0, 40.0, 32.0));
            assert_eq!(h, Rect::new(s.x - 4.0 - 40.0, 8.0, 40.0, 32.0));
            assert_eq!(b.back, None);
            // The title runs up to Help, a gap short of it.
            assert_eq!(b.title.right(), h.x - 12.0);
            assert_eq!(
                b.parts().iter().map(|(p, _)| *p).collect::<Vec<_>>(),
                [
                    Part::Help,
                    Part::Settings,
                    Part::Min,
                    Part::Max,
                    Part::Close
                ]
            );
        }
    }

    #[test]
    fn back_comes_only_with_settings_and_moves_the_k_over() {
        let b = layout(720.0, true);
        let back = b.back.expect("Back in Settings");
        assert_eq!(back, Rect::new(4.0, 8.0, 40.0, 32.0));
        assert_eq!(b.mark.x, back.right() + 4.0);
        assert_eq!(b.title.x, b.mark.right() + 12.0);
        assert_eq!((b.help, b.settings), (None, None));
        assert_eq!(b.title.right(), b.min.x - 12.0);
        assert_eq!(
            b.parts().iter().map(|(p, _)| *p).collect::<Vec<_>>(),
            [Part::Back, Part::Min, Part::Max, Part::Close]
        );
        assert_eq!(layout(720.0, false).back, None);
    }

    #[test]
    fn the_title_is_kiln_on_every_page_and_settings_names_itself() {
        // the page's own 28-DIP "Settings" is the heading; the bar
        // says "Kiln" there as everywhere else.
        assert_eq!(title(), "Kiln");
        assert_eq!(crate::settings_page::TITLE, "Settings");
    }

    #[test]
    fn the_parts_have_their_glyphs() {
        assert_eq!(Part::Max.glyph(false), '\u{E922}');
        assert_eq!(Part::Max.glyph(true), '\u{E923}');
        assert_eq!(Part::Min.glyph(false), '\u{E921}');
        assert_eq!(Part::Close.glyph(false), '\u{E8BB}');
        assert_eq!(Part::Help.glyph(false), '\u{E897}');
        assert_eq!(Part::Settings.glyph(false), '\u{E713}');
        assert_eq!(Part::Back.glyph(false), '\u{E72B}');
    }

    #[test]
    fn the_bar_scales_into_layout_units() {
        let b = layout(720.0, true);
        let u = b.scaled(1.0 / 1.5);
        assert_eq!(u.rect.h, 32.0);
        assert!((u.close.w - 46.0 / 1.5).abs() < 1e-4);
        assert!((u.back.unwrap().x - 4.0 / 1.5).abs() < 1e-4);
        assert_eq!(u.part_at(u.close.x + 1.0, 1.0), Some(Part::Close));
        assert_eq!(u.rect_of(Part::Help), None);
    }

    #[test]
    fn the_client_area_is_the_window_less_the_frame_at_the_sides_and_bottom() {
        let f = Frame {
            x: 8,
            y: 8,
            band: 4,
        };
        assert_eq!(client_insets(f, false, NO_BAR), (8, 0, 8, 8));
        // Maximised, the top hangs off the screen too.
        assert_eq!(client_insets(f, true, NO_BAR), (8, 8, 8, 8));
        assert_eq!(outer_size((720, 480), f), (736, 488));
    }

    #[test]
    fn the_shell_is_asked_about_hidden_taskbars_once_per_monitor_until_they_change() {
        let asked = std::cell::Cell::new(0);
        let bottom = AutoHide {
            bottom: true,
            ..AutoHide::default()
        };
        let ask = |bars: AutoHide| {
            asked.set(asked.get() + 1);
            bars
        };
        let mut c = BarCache::default();
        assert_eq!(c.get(1, || ask(bottom)), bottom);
        assert_eq!(c.get(1, || ask(AutoHide::default())), bottom, "known");
        assert_eq!(asked.get(), 1);
        // Another monitor is asked about on its own.
        assert_eq!(c.get(2, || ask(AutoHide::default())), AutoHide::default());
        assert_eq!(c.get(1, || ask(AutoHide::default())), bottom);
        assert_eq!(asked.get(), 2);
        // The work area changed: asked again.
        c.forget();
        assert_eq!(c.get(1, || ask(AutoHide::default())), AutoHide::default());
        assert_eq!(asked.get(), 3);
    }

    #[test]
    fn a_maximised_window_leaves_an_auto_hide_taskbars_edge_uncovered() {
        let f = Frame {
            x: 8,
            y: 8,
            band: 4,
        };
        let on = |edge: u32| AutoHide::ask(|e| e == edge);
        assert_eq!(
            on(3),
            AutoHide {
                bottom: true,
                ..NO_BAR
            }
        );
        assert_eq!(
            on(0),
            AutoHide {
                left: true,
                ..NO_BAR
            }
        );
        assert_eq!(AutoHide::ask(|_| false), NO_BAR);
        assert_eq!(AUTOHIDE_GAP, 2);
        // Two pixels on the taskbar's edge stay the screen's, so Windows
        // doesn't take Kiln for a full-screen window and the taskbar can
        // still slide out.
        assert_eq!(client_insets(f, true, on(3)), (8, 8, 8, 10));
        assert_eq!(client_insets(f, true, on(0)), (10, 8, 8, 8));
        assert_eq!(client_insets(f, true, on(1)), (8, 10, 8, 8));
        assert_eq!(client_insets(f, true, on(2)), (8, 8, 10, 8));
        // A window that isn't maximised doesn't cover the taskbar.
        assert_eq!(client_insets(f, false, on(3)), (8, 0, 8, 8));
        // The uncovered rows resize nothing, and the bar is still the
        // bar, its buttons still theirs, under a top taskbar.
        let scale = 1.0;
        let (cw, ch) = (1200.0, 800.0);
        let size = (cw + 16.0, ch + 16.0);
        let bar = layout(1200.0, false);
        let bottom = on(3);
        assert_eq!(
            hit(
                (600.0, size.1 - 8.0 - 1.0),
                size,
                f,
                scale,
                true,
                bottom,
                &bar
            ),
            Hit::Border
        );
        assert_eq!(
            hit(
                (600.0, size.1 - 8.0 - 3.0),
                size,
                f,
                scale,
                true,
                bottom,
                &bar
            ),
            Hit::Client
        );
        let top = on(1);
        assert_eq!(
            hit((600.0, 8.0), size, f, scale, true, top, &bar),
            Hit::Border
        );
        assert_eq!(
            hit((600.0, 10.0), size, f, scale, true, top, &bar),
            Hit::Caption
        );
    }

    #[test]
    fn the_top_band_over_the_caption_buttons_is_the_size_frame_alone() {
        for scale in SCALES {
            let size = window(720.0, 480.0, scale);
            let (ow, _) = size;
            let f = frame(scale);
            let bar = layout(720.0, false);
            let h = |x: f32, y: f32| hit((x, y), size, f, scale, false, NO_BAR, &bar);
            let band = f.band as f32;
            let close = ow - f.x as f32 - 23.0 * scale;
            let max = close - 46.0 * scale;
            let min = max - 46.0 * scale;
            // Windows' own frame: the top few pixels over the buttons
            // resize, and the rest of each button is the button.
            for x in [close, max, min] {
                assert_eq!(h(x, band - 0.5), Hit::Top, "{scale} {x}");
            }
            assert_eq!(h(close, band + 0.5), Hit::Close, "{scale}");
            assert_eq!(h(max, band + 0.5), Hit::Max, "{scale}");
            assert_eq!(h(min, band + 0.5), Hit::Min, "{scale}");
            // Over the empty bar, the band is the whole frame.
            assert_eq!(h(ow / 2.0, f.y as f32 - 0.5), Hit::Top, "{scale}");
            // Close's corner of the window still resizes both ways.
            let corner = ow - f.x as f32 - 1.0;
            assert_eq!(h(corner, band - 0.5), Hit::TopRight, "{scale}");
        }
    }

    #[test]
    fn caption_buttons_answer_as_windows_own_at_every_scale() {
        for scale in SCALES {
            let (w, h) = (720.0, 480.0);
            let size = window(w, h, scale);
            let bar = layout(w, false);
            let hit_at = |p: (f32, f32)| {
                let (x, y) = at(p.0, p.1, scale, false);
                hit((x, y), size, frame(scale), scale, false, NO_BAR, &bar)
            };
            assert_eq!(hit_at(centre(bar.min)), Hit::Min, "{scale}");
            assert_eq!(hit_at(centre(bar.max)), Hit::Max, "{scale}");
            assert_eq!(hit_at(centre(bar.close)), Hit::Close, "{scale}");
            // Their lower edge is theirs too.
            assert_eq!(hit_at((bar.max.x + 2.0, 47.5)), Hit::Max, "{scale}");
            // The empty bar drags the window.
            assert_eq!(hit_at((300.0, 24.0)), Hit::Caption, "{scale}");
            assert_eq!(hit_at(centre(bar.mark)), Hit::Caption, "{scale}");
            assert_eq!(hit_at((bar.title.x + 4.0, 24.0)), Hit::Caption, "{scale}");
            // Help and Settings are Kiln's.
            assert_eq!(hit_at(centre(bar.help.unwrap())), Hit::Client, "{scale}");
            assert_eq!(hit_at(centre(bar.settings.unwrap())), Hit::Client);
            // Below the bar is the content.
            assert_eq!(hit_at((300.0, 48.5)), Hit::Client, "{scale}");
            assert_eq!(hit_at((300.0, 300.0)), Hit::Client, "{scale}");
            // Back, in Settings.
            let bar = layout(w, true);
            let (x, y) = at(centre(bar.back.unwrap()).0, 24.0, scale, false);
            assert_eq!(
                hit((x, y), size, frame(scale), scale, false, NO_BAR, &bar),
                Hit::Client
            );
        }
    }

    #[test]
    fn the_edges_resize_the_window_at_every_scale() {
        for scale in SCALES {
            let size = window(720.0, 480.0, scale);
            let (ow, oh) = size;
            let f = frame(scale);
            let bar = layout(720.0, false);
            let h = |x: f32, y: f32| hit((x, y), size, f, scale, false, NO_BAR, &bar);
            let edge = f.x as f32 / 2.0;
            assert_eq!(h(edge, oh / 2.0), Hit::Left, "{scale}");
            assert_eq!(h(ow - edge, oh / 2.0), Hit::Right, "{scale}");
            assert_eq!(h(ow / 2.0, oh - edge), Hit::Bottom, "{scale}");
            // The top band is the top of the bar, over the buttons too.
            assert_eq!(h(ow / 2.0, 1.0), Hit::Top, "{scale}");
            assert_eq!(h(ow - 60.0 * scale, 1.0), Hit::Top, "{scale}");
            assert_eq!(h(edge, 1.0), Hit::TopLeft, "{scale}");
            assert_eq!(h(ow - edge, 1.0), Hit::TopRight, "{scale}");
            assert_eq!(h(edge, oh - edge), Hit::BottomLeft, "{scale}");
            assert_eq!(h(ow - edge, oh - edge), Hit::BottomRight, "{scale}");
            // Just under the band, the bar.
            assert_eq!(h(ow / 2.0, f.y as f32 + 1.0), Hit::Caption, "{scale}");
            // Just inside the side frame along the bar, it's a corner.
            assert_eq!(h(edge, 4.0 * scale), Hit::TopLeft, "{scale}");
        }
    }

    #[test]
    fn a_maximised_window_has_no_top_band_and_no_edges() {
        for scale in SCALES {
            let f = frame(scale);
            let (cw, ch) = (1200.0 * scale, 800.0 * scale);
            let size = (cw + 2.0 * f.x as f32, ch + 2.0 * f.y as f32);
            let bar = layout(1200.0, false);
            let h = |p: (f32, f32)| {
                let (x, y) = at(p.0, p.1, scale, true);
                hit((x, y), size, f, scale, true, NO_BAR, &bar)
            };
            // The top row of the screen is the bar, and the buttons reach it.
            assert_eq!(h((600.0, 0.0)), Hit::Caption, "{scale}");
            assert_eq!(h((bar.close.x + 1.0, 0.0)), Hit::Close, "{scale}");
            assert_eq!(h((bar.max.x + 1.0, 0.0)), Hit::Max, "{scale}");
            assert_eq!(h((600.0, 400.0)), Hit::Client, "{scale}");
            // The frame is off the screen: it resizes nothing.
            assert_eq!(
                hit((1.0, 1.0), size, f, scale, true, NO_BAR, &bar),
                Hit::Border
            );
            assert_eq!(
                hit(
                    (size.0 / 2.0, size.1 - 1.0),
                    size,
                    f,
                    scale,
                    true,
                    NO_BAR,
                    &bar
                ),
                Hit::Border
            );
        }
    }

    #[test]
    fn the_codes_are_windows_own() {
        use windows::Win32::UI::WindowsAndMessaging as w;
        let pairs = [
            (Hit::Client, w::HTCLIENT),
            (Hit::Caption, w::HTCAPTION),
            (Hit::Min, w::HTMINBUTTON),
            (Hit::Max, w::HTMAXBUTTON),
            (Hit::Close, w::HTCLOSE),
            (Hit::Left, w::HTLEFT),
            (Hit::Right, w::HTRIGHT),
            (Hit::Top, w::HTTOP),
            (Hit::Bottom, w::HTBOTTOM),
            (Hit::TopLeft, w::HTTOPLEFT),
            (Hit::TopRight, w::HTTOPRIGHT),
            (Hit::BottomLeft, w::HTBOTTOMLEFT),
            (Hit::BottomRight, w::HTBOTTOMRIGHT),
            (Hit::Border, w::HTBORDER),
        ];
        for (hit, code) in pairs {
            assert_eq!(hit.code(), code, "{hit:?}");
        }
    }

    #[test]
    fn a_caption_button_acts_on_release_over_the_button_it_went_down_on() {
        let mut c = Caption::default();
        assert!(c.hover(Some(Part::Close)));
        assert!(!c.hover(Some(Part::Close)), "no change, nothing to draw");
        assert!(c.press(Some(Part::Close)));
        assert_eq!(c.pressed, Some(Part::Close));
        assert_eq!(c.release(Some(Part::Close)), Some(Part::Close));
        assert_eq!(c.pressed, None);
        // Slid off onto another button: nothing.
        c.press(Some(Part::Min));
        c.hover(Some(Part::Max));
        assert_eq!(c.release(Some(Part::Max)), None);
        // Slid off the bar and back: the press is kept while it's on the
        // bar, and leaving lets it go.
        c.press(Some(Part::Max));
        assert!(c.leave());
        assert_eq!(c, Caption::default());
        assert_eq!(c.release(Some(Part::Max)), None);
        // A press on nothing holds nothing.
        assert!(!c.press(None));
        assert_eq!(c.release(None), None);
        assert!(!c.leave());
    }

    #[test]
    fn mica_only_where_windows_has_it_and_allows_it() {
        assert_eq!(backdrop(22621, true, false), Backdrop::Mica);
        assert_eq!(backdrop(26300, true, false), Backdrop::Mica);
        // Windows 11 21H2 and Windows 10.
        assert_eq!(backdrop(22000, true, false), Backdrop::Solid);
        assert_eq!(backdrop(19045, true, false), Backdrop::Solid);
        // Transparency effects off.
        assert_eq!(backdrop(26300, false, false), Backdrop::Solid);
        // A contrast theme, or a named theme.
        assert_eq!(backdrop(26300, true, true), Backdrop::Solid);
        // A build that can't be read.
        assert_eq!(backdrop(0, true, false), Backdrop::Solid);
    }

    /// Kiln's answer to `WM_NCCALCSIZE` and `WM_NCHITTEST` on a hidden
    /// window of this thread, never shown: the frame Windows keeps, and,
    /// maximised (the style alone, so nothing moves on the screen), the
    /// whole frame off the screen and room for a hidden taskbar.
    #[test]
    fn the_window_answers_nccalcsize_and_nchittest_maximised_or_not() {
        use windows::core::w;
        use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, GetWindowLongPtrW, GetWindowRect, IsZoomed,
            RegisterClassW, SendMessageW, SetWindowLongPtrW, GWL_STYLE, WINDOW_EX_STYLE,
            WM_NCCALCSIZE, WM_NCHITTEST, WNDCLASSW, WS_MAXIMIZE, WS_OVERLAPPEDWINDOW,
        };

        extern "system" fn proc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
            crate::win::frame_message(h, m, wp, lp)
                // SAFETY: the message's own arguments, handed on.
                .unwrap_or_else(|| unsafe { DefWindowProcW(h, m, wp, lp) })
        }

        // SAFETY: the module handle of this test binary; a plain
        // registration (a second one fails harmlessly); a window of this
        // thread, never shown.
        let hwnd = unsafe {
            let instance = GetModuleHandleW(None).unwrap();
            let class = w!("KilnFrameTest");
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(proc),
                hInstance: instance.into(),
                lpszClassName: class,
                ..Default::default()
            });
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class,
                w!("Kiln frame test"),
                WS_OVERLAPPEDWINDOW,
                100,
                100,
                800,
                600,
                None,
                None,
                Some(instance.into()),
                None,
            )
        }
        .unwrap();
        let frame = crate::win::frame_of(hwnd);
        let outer = RECT {
            left: 100,
            top: 100,
            right: 900,
            bottom: 700,
        };
        let insets = || {
            let mut r = outer;
            // SAFETY: with wparam FALSE, lparam is a RECT, a local that
            // outlives the call.
            unsafe {
                SendMessageW(
                    hwnd,
                    WM_NCCALCSIZE,
                    Some(WPARAM(0)),
                    Some(LPARAM(&mut r as *mut RECT as isize)),
                )
            };
            (
                r.left - outer.left,
                r.top - outer.top,
                outer.right - r.right,
                outer.bottom - r.bottom,
            )
        };
        let mut rc = RECT::default();
        // SAFETY: this thread's own window, into a local.
        unsafe { GetWindowRect(hwnd, &mut rc) }.unwrap();
        let hit_at = |x: i32, y: i32| {
            let at = ((y as u32 as isize) << 16) | (x as u16 as isize);
            // SAFETY: a plain message to this thread's own window.
            unsafe { SendMessageW(hwnd, WM_NCHITTEST, Some(WPARAM(0)), Some(LPARAM(at))) }.0 as u32
        };
        let mid = (rc.left + rc.right) / 2;

        assert_eq!(insets(), (frame.x, 0, frame.x, frame.y));
        // The top row resizes the window.
        assert_eq!(hit_at(mid, rc.top + 1), Hit::Top.code());

        // SAFETY: this thread's own window; WS_MAXIMIZE alone makes
        // IsZoomed true, and nothing is shown or moved.
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
            SetWindowLongPtrW(hwnd, GWL_STYLE, style | WS_MAXIMIZE.0 as isize);
            assert!(IsZoomed(hwnd).as_bool());
        }
        // Whatever this PC's taskbar does, the window leaves the room the
        // taskbar asks for and the hit-test goes by the same edges.
        let bars = crate::win::autohide_of(hwnd, true);
        let got = insets();
        assert_eq!(got, client_insets(frame, true, bars));
        assert!(got.1 >= frame.y, "the top frame is off the screen: {got:?}");
        assert_eq!(crate::win::AUTOHIDE.get(), bars);
        // The client's top row is the bar: no top band while maximised.
        assert_eq!(hit_at(mid, rc.top + got.1), Hit::Caption.code());
        assert_eq!(hit_at(mid, rc.top + got.1 - 1), Hit::Border.code());

        drop_test_window(hwnd);
    }

    /// Destroy the test's own hidden window, which holds no `App`.
    fn drop_test_window(hwnd: windows::Win32::Foundation::HWND) {
        // SAFETY: this thread's own window, made by the test.
        unsafe { windows::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd) }.unwrap();
    }
}
