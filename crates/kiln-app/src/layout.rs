//! Where things go, in DIPs. Pure functions, so the 4 px grid and the
//! 16 px panel padding can be tested without a window.

use crate::theme::shape::{GRID, PANEL_PADDING};

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
}

/// Lay out a window of `w` x `h` DIPs holding `rows` files.
pub fn layout(w: f32, h: f32, rows: usize) -> Layout {
    let p = PANEL_PADDING;
    let header = Rect::new(0.0, 0.0, w, HEADER_HEIGHT);
    let icon = Rect::new(p, p, 24.0, 24.0);
    let wordmark = Rect::new(icon.right() + 12.0, 12.0, 160.0, 32.0);
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
