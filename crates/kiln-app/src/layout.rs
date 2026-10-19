//! Where things go, in DIPs. Pure functions, so the 4 px grid and the
//! 16 px panel padding can be tested without a window.

use crate::settings::{Engine, ThemeChoice};
use crate::theme::shape::{GRID, PANEL_PADDING};
use crate::wheel::WheelKind;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.right() && py >= self.y && py < self.bottom()
    }

    pub fn inset(&self, d: f32) -> Rect {
        Rect::new(
            self.x + d,
            self.y + d,
            (self.w - 2.0 * d).max(0.0),
            (self.h - 2.0 * d).max(0.0),
        )
    }
}

pub const HEADER_HEIGHT: f32 = 56.0;
pub const ROW_HEIGHT: f32 = 56.0;
pub const FOOTER_HEIGHT: f32 = 40.0;
pub const CHIP_HEIGHT: f32 = 24.0;
pub const CHIP_MIN_WIDTH: f32 = 48.0;
pub const BUTTON_HEIGHT: f32 = 32.0;
pub const BUTTON_WIDTH: f32 = 72.0;
/// Height of the progress line at the bottom of a job row.
pub const PROGRESS_HEIGHT: f32 = 2.0;
/// Smallest window, in DIPs.
pub const MIN_WINDOW: (f32, f32) = (480.0, 360.0);
pub const DEFAULT_WINDOW: (f32, f32) = (720.0, 480.0);

#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub header: Rect,
    pub icon: Rect,
    pub wordmark: Rect,
    /// Everything below the header, inside the panel padding.
    pub body: Rect,
    /// The empty-state drop panel, or the file list panel.
    pub panel: Rect,
    /// Row rects in panel coordinates, before scrolling. Empty when there
    /// are no files.
    pub rows: Vec<Rect>,
    pub footer: Option<Rect>,
    pub clear_button: Option<Rect>,
    /// The dial at the right of the header that opens settings.
    pub settings_button: Rect,
}

/// Lay out a window of `w` x `h` DIPs holding `rows` files.
pub fn layout(w: f32, h: f32, rows: usize) -> Layout {
    let p = PANEL_PADDING;
    let header = Rect::new(0.0, 0.0, w, HEADER_HEIGHT);
    let icon = Rect::new(p, p, 24.0, 24.0);
    let wordmark = Rect::new(icon.right() + 12.0, 12.0, 160.0, 32.0);
    let settings_button = Rect::new((w - p - 32.0).max(0.0), 12.0, 32.0, 32.0);
    let body = Rect::new(
        p,
        HEADER_HEIGHT + p,
        (w - 2.0 * p).max(0.0),
        (h - HEADER_HEIGHT - 2.0 * p).max(0.0),
    );

    if rows == 0 {
        return Layout {
            header,
            icon,
            wordmark,
            body,
            panel: body,
            rows: vec![],
            footer: None,
            clear_button: None,
            settings_button,
        };
    }

    let footer = Rect::new(body.x, body.bottom() - FOOTER_HEIGHT, body.w, FOOTER_HEIGHT);
    let panel = Rect::new(
        body.x,
        body.y,
        body.w,
        (body.h - FOOTER_HEIGHT - GRID).max(0.0),
    );
    let rows = (0..rows)
        .map(|i| {
            Rect::new(
                panel.x,
                panel.y + i as f32 * ROW_HEIGHT,
                panel.w,
                ROW_HEIGHT,
            )
        })
        .collect();
    let clear_button = Rect::new(
        footer.right() - BUTTON_WIDTH,
        footer.y + (FOOTER_HEIGHT - BUTTON_HEIGHT) / 2.0,
        BUTTON_WIDTH,
        BUTTON_HEIGHT,
    );
    Layout {
        header,
        icon,
        wordmark,
        body,
        panel,
        rows,
        footer: Some(footer),
        clear_button: Some(clear_button),
        settings_button,
    }
}

/// Width of the "Show details" link at the right of a failed row.
pub const LINK_WIDTH: f32 = 88.0;

/// The cancel control on a running row, in the row's coordinates.
pub fn row_cancel(row: &Rect) -> Rect {
    Rect::new(row.right() - 16.0 - 24.0, row.y + 16.0, 24.0, 24.0)
}

/// The "Show details" link on a failed row.
pub fn row_link(row: &Rect) -> Rect {
    Rect::new(
        row.right() - 16.0 - LINK_WIDTH,
        row.y + 28.0,
        LINK_WIDTH,
        20.0,
    )
}

/// The details dialog: panel, title, body and close button.
pub struct Dialog {
    pub panel: Rect,
    pub title: Rect,
    pub body: Rect,
    pub close: Rect,
}

pub fn dialog(w: f32, h: f32) -> Dialog {
    let pw = (w - 64.0).clamp(0.0, 560.0);
    let ph = (h - 64.0).clamp(0.0, 360.0);
    let panel = Rect::new(((w - pw) / 2.0).round(), ((h - ph) / 2.0).round(), pw, ph);
    let p = PANEL_PADDING;
    let title = Rect::new(panel.x + p, panel.y + p, panel.w - 2.0 * p, 32.0);
    let close = Rect::new(
        panel.right() - p - BUTTON_WIDTH,
        panel.bottom() - p - BUTTON_HEIGHT,
        BUTTON_WIDTH,
        BUTTON_HEIGHT,
    );
    let body = Rect::new(
        panel.x + p,
        title.bottom() + 8.0,
        panel.w - 2.0 * p,
        (close.y - 16.0 - title.bottom() - 8.0).max(0.0),
    );
    Dialog {
        panel,
        title,
        body,
        close,
    }
}

/// Something the settings panel lets you press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    OutputChoose,
    OutputReset,
    Theme(ThemeChoice),
    Key(WheelKind),
    EngineChoose(Engine),
    EngineReset(Engine),
    Close,
}

/// One labelled line of the settings panel.
pub struct SettingsRow {
    pub label: &'static str,
    pub label_rect: Rect,
    /// Where the row's current value is written, for rows that show one.
    pub value: Option<Rect>,
}

pub struct SettingsLayout {
    pub panel: Rect,
    pub title: Rect,
    pub rows: Vec<SettingsRow>,
    /// Every control, in Tab order.
    pub controls: Vec<(Control, Rect)>,
}

pub const SETTINGS_ROW: f32 = 40.0;
pub const SETTINGS_LABEL: f32 = 112.0;
/// Theme segments and key buttons.
pub const SEGMENT_WIDTH: f32 = 72.0;

/// What the settings panel needs to know to lay itself out: which values
/// are set, since those rows grow a Reset button.
#[derive(Debug, Clone, Copy, Default)]
pub struct SettingsShape {
    pub output_custom: bool,
    pub engine_custom: [bool; 3],
}

pub fn settings(w: f32, h: f32, shape: SettingsShape) -> SettingsLayout {
    let p = PANEL_PADDING;
    let rows_n = 3.0 + Engine::ALL.len() as f32;
    let ph = p + 32.0 + 8.0 + rows_n * SETTINGS_ROW + p + BUTTON_HEIGHT + p;
    let pw = snap((w - 32.0).clamp(0.0, 560.0));
    let panel = Rect::new(
        snap(((w - pw) / 2.0).max(0.0)),
        snap(((h - ph) / 2.0).max(8.0)),
        pw,
        ph,
    );
    let title = Rect::new(panel.x + p, panel.y + p, panel.w - 2.0 * p, 32.0);
    let right = panel.right() - p;
    let x0 = panel.x + p + SETTINGS_LABEL + 8.0;
    let row_y = |i: usize| title.bottom() + 8.0 + i as f32 * SETTINGS_ROW;
    let ctl = |x: f32, i: usize, w: f32| Rect::new(x, row_y(i) + 4.0, w, BUTTON_HEIGHT);

    let mut rows = Vec::new();
    let mut controls = Vec::new();
    let row = |label, i: usize, value: Option<Rect>| SettingsRow {
        label,
        label_rect: Rect::new(panel.x + p, row_y(i) + 8.0, SETTINGS_LABEL, 24.0),
        value,
    };

    // A row with a value on the left and Choose (and maybe Reset) on the
    // right.
    let chooser =
        |controls: &mut Vec<(Control, Rect)>, i: usize, choose: Control, reset: Option<Control>| {
            let choose_r = ctl(right - BUTTON_WIDTH, i, BUTTON_WIDTH);
            let mut left = choose_r.x;
            if let Some(reset) = reset {
                let r = ctl(left - 8.0 - BUTTON_WIDTH, i, BUTTON_WIDTH);
                left = r.x;
                controls.push((reset, r));
            }
            controls.push((choose, choose_r));
            Rect::new(x0, row_y(i) + 8.0, (left - 8.0 - x0).max(0.0), 24.0)
        };

    let v = chooser(
        &mut controls,
        0,
        Control::OutputChoose,
        shape.output_custom.then_some(Control::OutputReset),
    );
    rows.push(row("Output folder", 0, Some(v)));

    for (k, t) in ThemeChoice::ALL.into_iter().enumerate() {
        controls.push((
            Control::Theme(t),
            ctl(x0 + k as f32 * SEGMENT_WIDTH, 1, SEGMENT_WIDTH),
        ));
    }
    rows.push(row("Theme", 1, None));

    let key_w = (((right - x0 - 16.0) / 3.0).min(120.0) / GRID).floor() * GRID;
    for (k, wheel) in WheelKind::ALL.into_iter().enumerate() {
        controls.push((
            Control::Key(wheel),
            ctl(x0 + k as f32 * (key_w + 8.0), 2, key_w),
        ));
    }
    rows.push(row("Wheel keys", 2, None));

    for (k, e) in Engine::ALL.into_iter().enumerate() {
        let v = chooser(
            &mut controls,
            3 + k,
            Control::EngineChoose(e),
            shape.engine_custom[k].then_some(Control::EngineReset(e)),
        );
        // The engines sit under one "Engines" label, the name error
        // messages point to.
        let label = if k == 0 { "Engines" } else { "" };
        rows.push(row(label, 3 + k, Some(v)));
    }

    let close = Rect::new(
        right - BUTTON_WIDTH,
        panel.bottom() - p - BUTTON_HEIGHT,
        BUTTON_WIDTH,
        BUTTON_HEIGHT,
    );
    controls.push((Control::Close, close));

    SettingsLayout {
        panel,
        title,
        rows,
        controls,
    }
}

fn snap(v: f32) -> f32 {
    (v / GRID).floor() * GRID
}

/// Largest scroll offset for a list of `rows` inside a panel `panel_h` tall.
pub fn max_scroll(rows: usize, panel_h: f32) -> f32 {
    (rows as f32 * ROW_HEIGHT - panel_h).max(0.0)
}

/// Scroll offset that keeps row `index` fully visible.
pub fn scroll_to_show(index: usize, scroll: f32, panel_h: f32) -> f32 {
    let top = index as f32 * ROW_HEIGHT;
    let bottom = top + ROW_HEIGHT;
    if top < scroll {
        top
    } else if bottom > scroll + panel_h {
        bottom - panel_h
    } else {
        scroll
    }
}

/// Which row is under a point, given the current scroll.
pub fn row_at(l: &Layout, x: f32, y: f32, scroll: f32) -> Option<usize> {
    if !l.panel.contains(x, y) {
        return None;
    }
    let i = ((y - l.panel.y + scroll) / ROW_HEIGHT).floor();
    (i >= 0.0 && (i as usize) < l.rows.len()).then_some(i as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on_grid(v: f32) -> bool {
        (v / GRID).fract() == 0.0
    }

    fn assert_grid(r: &Rect, what: &str) {
        for v in [r.x, r.y, r.w, r.h] {
            assert!(on_grid(v), "{what} {r:?} is off the 4 px grid");
        }
    }

    #[test]
    fn everything_sits_on_the_grid() {
        for (w, h) in [(480.0, 360.0), (720.0, 480.0), (1024.0, 768.0)] {
            for rows in [0, 1, 5] {
                let l = layout(w, h, rows);
                assert_grid(&l.header, "header");
                assert_grid(&l.icon, "icon");
                assert_grid(&l.body, "body");
                assert_grid(&l.panel, "panel");
                for r in &l.rows {
                    assert_grid(r, "row");
                }
                if let Some(f) = &l.footer {
                    assert_grid(f, "footer");
                }
                if let Some(b) = &l.clear_button {
                    assert_grid(b, "button");
                }
            }
        }
    }

    #[test]
    fn panels_keep_16_px_padding() {
        let l = layout(720.0, 480.0, 3);
        assert_eq!(l.body.x, 16.0);
        assert_eq!(l.body.y - l.header.bottom(), 16.0);
        assert_eq!(720.0 - l.body.right(), 16.0);
        assert_eq!(480.0 - l.body.bottom(), 16.0);
        assert_eq!(l.icon.x, 16.0);
        assert_eq!(l.icon.y, 16.0);
    }

    #[test]
    fn empty_state_is_one_drop_panel() {
        let l = layout(720.0, 480.0, 0);
        assert_eq!(l.panel, l.body);
        assert!(l.footer.is_none() && l.rows.is_empty());
    }

    #[test]
    fn rows_stack_inside_the_panel() {
        let l = layout(720.0, 480.0, 3);
        assert_eq!(l.rows[0].y, l.panel.y);
        assert_eq!(l.rows[2].y, l.panel.y + 2.0 * ROW_HEIGHT);
        assert!(l.panel.bottom() <= l.footer.unwrap().y);
    }

    #[test]
    fn row_controls_sit_inside_the_row() {
        let l = layout(720.0, 480.0, 1);
        let r = l.rows[0];
        for c in [row_cancel(&r), row_link(&r)] {
            assert!(c.x >= r.x && c.right() <= r.right() - 16.0);
            assert!(c.y >= r.y && c.bottom() <= r.bottom());
        }
    }

    #[test]
    fn dialog_fits_and_keeps_padding() {
        for (w, h) in [(480.0, 360.0), (720.0, 480.0), (1600.0, 1000.0)] {
            let d = dialog(w, h);
            assert!(d.panel.x >= 32.0 && d.panel.right() <= w - 32.0);
            assert_eq!(d.title.x - d.panel.x, 16.0);
            assert_eq!(d.panel.bottom() - d.close.bottom(), 16.0);
            assert!(d.body.bottom() <= d.close.y);
        }
    }

    #[test]
    fn the_settings_button_sits_in_the_header() {
        let l = layout(720.0, 480.0, 0);
        assert_grid(&l.settings_button, "settings button");
        assert_eq!(l.header.right() - l.settings_button.right(), 16.0);
        assert!(l.settings_button.bottom() <= l.header.bottom());
        assert!(l.settings_button.x > l.wordmark.right());
    }

    #[test]
    fn settings_controls_fit_without_overlapping() {
        let shapes = [
            SettingsShape::default(),
            SettingsShape {
                output_custom: true,
                engine_custom: [true, true, true],
            },
        ];
        for (w, h) in [(480.0, 400.0), (720.0, 480.0), (1600.0, 1000.0)] {
            for shape in shapes {
                let s = settings(w, h, shape);
                assert_grid(&s.panel, "settings panel");
                assert!(s.panel.x >= 16.0 && s.panel.right() <= w - 16.0);
                assert_eq!(s.title.x - s.panel.x, 16.0);
                for (c, r) in &s.controls {
                    assert_grid(r, "settings control");
                    assert!(r.x >= s.panel.x + 16.0, "{c:?} {r:?}");
                    assert!(r.right() <= s.panel.right() - 16.0, "{c:?} {r:?}");
                    assert!(r.bottom() <= s.panel.bottom() - 16.0, "{c:?} {r:?}");
                }
                for (i, (a, ra)) in s.controls.iter().enumerate() {
                    for (b, rb) in &s.controls[i + 1..] {
                        let apart = ra.right() <= rb.x
                            || rb.right() <= ra.x
                            || ra.bottom() <= rb.y
                            || rb.bottom() <= ra.y;
                        assert!(apart, "{a:?} overlaps {b:?} at {w}x{h}");
                    }
                }
                for row in &s.rows {
                    if let Some(v) = row.value {
                        assert!(v.x > row.label_rect.right());
                    }
                }
            }
        }
        let s = settings(720.0, 480.0, SettingsShape::default());
        assert_eq!(s.rows.len(), 6);
        assert_eq!(s.controls.len(), 1 + 3 + 3 + 3 + 1);
        assert_eq!(s.controls.last().unwrap().0, Control::Close);
    }

    #[test]
    fn scrolling_math() {
        assert_eq!(max_scroll(3, 300.0), 0.0);
        assert_eq!(max_scroll(10, 300.0), 260.0);
        assert_eq!(scroll_to_show(0, 100.0, 300.0), 0.0);
        assert_eq!(scroll_to_show(9, 0.0, 300.0), 260.0);
        assert_eq!(scroll_to_show(2, 0.0, 300.0), 0.0);
    }

    #[test]
    fn hit_testing_rows() {
        let l = layout(720.0, 480.0, 3);
        let y = l.panel.y + ROW_HEIGHT * 1.5;
        assert_eq!(row_at(&l, 100.0, y, 0.0), Some(1));
        assert_eq!(row_at(&l, 100.0, y, ROW_HEIGHT), Some(2));
        assert_eq!(row_at(&l, 100.0, l.panel.y + 3.5 * ROW_HEIGHT, 0.0), None);
        assert_eq!(row_at(&l, 4.0, y, 0.0), None);
    }
}
