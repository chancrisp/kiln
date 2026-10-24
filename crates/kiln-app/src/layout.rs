//! Where things go, in DIPs. Pure functions, so the 4 px grid and the
//! 16 px panel padding can be tested without a window.

use kiln_core::{Knob, Preset};

use crate::quality::DefaultSetting;
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
pub const PROGRESS_HEIGHT: f32 = 4.0;
/// Smallest window, in DIPs.
pub const MIN_WINDOW: (f32, f32) = (480.0, 360.0);
pub const DEFAULT_WINDOW: (f32, f32) = (720.0, 480.0);

/// The largest Windows Text size (Settings > Accessibility > Text size),
/// as `UISettings.TextScaleFactor` reports it.
pub const MAX_TEXT_SCALE: f32 = 2.25;

/// Windows' Text size, kept to the range Windows allows.
fn text_scale(scale: f32) -> f32 {
    if scale.is_finite() {
        scale.clamp(1.0, MAX_TEXT_SCALE)
    } else {
        1.0
    }
}

/// How many DIPs one layout unit takes at Windows' Text size `scale`, in a
/// client area `client` DIPs across.
///
/// Every size in Kiln is on a 4 DIP grid tuned to the text it holds, so
/// Text size zooms the whole layout: fonts grow by `scale`, and rows,
/// controls, padding and the wheel grow with them. Layout functions keep
/// working in units, and every relation the layout tests pin holds at any
/// Text size. The window's minimum grows with the zoom (`min_window`);
/// where the screen is too small for that, the zoom gives way so the
/// layout still has its smallest window in units, and text is as large as
/// fits.
pub fn text_zoom(scale: f32, client: (f32, f32)) -> f32 {
    let (w, h) = client;
    let (mw, mh) = MIN_WINDOW;
    let fit = (w / mw).min(h / mh);
    text_scale(scale).min(fit).max(1.0)
}

/// The smallest client area, in DIPs, at Text size `scale`.
pub fn min_window(scale: f32) -> (f32, f32) {
    let s = text_scale(scale);
    (MIN_WINDOW.0 * s, MIN_WINDOW.1 * s)
}

/// The window's first client size, in DIPs, at Text size `scale`.
pub fn default_window(scale: f32) -> (f32, f32) {
    let s = text_scale(scale);
    (DEFAULT_WINDOW.0 * s, DEFAULT_WINDOW.1 * s)
}

/// Physical pixels per layout unit, at a monitor's `dpi` and a zoom.
/// A 12 unit caption is `12.0 * px_per_unit(dpi, zoom)` pixels tall.
pub fn px_per_unit(dpi: f32, zoom: f32) -> f32 {
    dpi / 96.0 * zoom
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub header: Rect,
    pub icon: Rect,
    pub wordmark: Rect,
    /// Everything below the header, inside the panel padding.
    pub body: Rect,
    /// The empty-state drop panel, or the file list panel.
    pub panel: Rect,
    /// The list's part of the panel: all of it while there is no
    /// inspector, else what is left of it beside the inspector and its
    /// rule.
    pub list: Rect,
    /// The inspector's part of the panel, at the panel's right edge, with
    /// a rule [`INSPECTOR_RULE`] wide between it and the list. `None`
    /// when there are no files or it isn't shown.
    pub inspector: Option<Rect>,
    /// Row rects in panel coordinates, before scrolling, as wide as the
    /// list. Empty when there are no files.
    pub rows: Vec<Rect>,
    pub footer: Option<Rect>,
    pub clear_button: Option<Rect>,
    /// The dial at the right of the header that opens settings.
    pub settings_button: Rect,
    /// The ? beside it that opens help.
    pub help_button: Rect,
}

/// Whether a window `w` zoomed DIPs wide is wide enough to show the
/// inspector beside the list. A larger Text size zooms `w` down, so it
/// folds the inspector away sooner.
pub fn inspector_fits(w: f32) -> bool {
    w >= INSPECTOR_FROM
}

/// Lay out a window of `w` x `h` DIPs holding `rows` files, with the
/// inspector shown whenever the window is wide enough for it.
pub fn layout(w: f32, h: f32, rows: usize) -> Layout {
    layout_with(w, h, rows, inspector_fits(w) as u8 as f32)
}

/// [`layout`] with the inspector `shown` of the way in, from 0 (folded
/// away) to 1 (all of it). It slides in from the panel's right edge, so
/// it is `INSPECTOR_WIDTH * shown` wide and the list gives up what it
/// takes. With no files there is no inspector, whatever `shown` says.
pub fn layout_with(w: f32, h: f32, rows: usize, shown: f32) -> Layout {
    let p = PANEL_PADDING;
    let shown = if shown.is_finite() {
        shown.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let inspected = rows > 0 && shown > 0.0;
    let header = Rect::new(0.0, 0.0, w, HEADER_HEIGHT);
    // Content stays in a column of a readable width, centred in a wide
    // window, so a row's link is never a thousand pixels from its name.
    // With files the column is wider by as much of the inspector as is
    // shown, so the list keeps its width while the inspector slides in
    // and the header and footer glide out with it instead of jumping.
    let cap = if rows > 0 {
        MAX_CONTENT_WIDTH + (MAX_CONTENT_WIDTH_WIDE - MAX_CONTENT_WIDTH) * shown
    } else {
        MAX_CONTENT_WIDTH
    };
    let col_w = (w - 2.0 * p).clamp(0.0, cap);
    // Centred to the 4 DIP grid: the wide column's width leaves a half
    // grid step at some window widths.
    let col_x = snap(((w - col_w) / 2.0).max(p));
    let icon = Rect::new(col_x, p, 24.0, 24.0);
    let wordmark = Rect::new(icon.right() + 12.0, 12.0, 160.0, 32.0);
    let settings_button = Rect::new((col_x + col_w - 32.0).max(0.0), 12.0, 32.0, 32.0);
    let help_button = Rect::new((settings_button.x - 8.0 - 32.0).max(0.0), 12.0, 32.0, 32.0);
    let body = Rect::new(
        col_x,
        HEADER_HEIGHT + p,
        col_w,
        (h - HEADER_HEIGHT - 2.0 * p).max(0.0),
    );

    if rows == 0 {
        return Layout {
            header,
            icon,
            wordmark,
            body,
            panel: body,
            list: body,
            inspector: None,
            rows: vec![],
            footer: None,
            clear_button: None,
            settings_button,
            help_button,
        };
    }

    let footer = Rect::new(body.x, body.bottom() - FOOTER_HEIGHT, body.w, FOOTER_HEIGHT);
    let panel = Rect::new(
        body.x,
        body.y,
        body.w,
        (body.h - FOOTER_HEIGHT - GRID).max(0.0),
    );
    let (list, inspector) = if inspected {
        let iw = INSPECTOR_WIDTH * shown;
        let inspector = Rect::new(panel.right() - iw, panel.y, iw, panel.h);
        let list = Rect::new(
            panel.x,
            panel.y,
            (panel.w - iw - INSPECTOR_RULE).max(0.0),
            panel.h,
        );
        (list, Some(inspector))
    } else {
        (panel, None)
    };
    let rows = (0..rows)
        .map(|i| Rect::new(list.x, list.y + i as f32 * ROW_HEIGHT, list.w, ROW_HEIGHT))
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
        list,
        inspector,
        rows,
        footer: Some(footer),
        clear_button: Some(clear_button),
        settings_button,
        help_button,
    }
}

/// Width of the "Show details" link at the right of a failed row.
pub const LINK_WIDTH: f32 = 104.0;
/// Height of the link's hit area: WCAG 2.5.8's 24 DIPs, on its own.
pub const LINK_HIT: f32 = 24.0;

/// Width of the "Cancel" button on a running row.
pub const CANCEL_WIDTH: f32 = 56.0;

/// The Cancel button on a running row, in the row's coordinates: a text
/// button at the right end of the second line, where the row's links sit.
pub fn row_cancel(row: &Rect) -> Rect {
    Rect::new(
        row.right() - 16.0 - CANCEL_WIDTH,
        row.y + 26.0,
        CANCEL_WIDTH,
        LINK_HIT,
    )
}

/// Where Cancel's text is drawn: the second line, right-aligned in its
/// hit area.
pub fn row_cancel_text(row: &Rect) -> Rect {
    let hit = row_cancel(row);
    Rect::new(hit.x, row.y + 28.0, hit.w, 20.0)
}

/// Where a click hits the "Show details" or "Export" link: 24 DIPs tall
/// around the link's text, within the row's own band so it never reaches
/// the next row.
pub fn row_link(row: &Rect) -> Rect {
    Rect::new(
        row.right() - 16.0 - LINK_WIDTH,
        row.y + 26.0,
        LINK_WIDTH,
        LINK_HIT,
    )
}

/// Where the link's text is drawn: on the second line, right-aligned in
/// its hit area.
pub fn row_link_text(row: &Rect) -> Rect {
    let hit = row_link(row);
    Rect::new(hit.x, row.y + 28.0, hit.w, 20.0)
}

/// Width of the "Retry" link on a failed or cancelled row.
pub const RETRY_WIDTH: f32 = 48.0;

/// Where a click hits a row's "Retry" link. `beside_link` says the row
/// also has "Show details": Retry then sits to its left, with a gap, so
/// the two targets never touch. Alone it takes the link's place at the
/// right edge.
pub fn row_retry(row: &Rect, beside_link: bool) -> Rect {
    let x = if beside_link {
        row_link(row).x - 8.0 - RETRY_WIDTH
    } else {
        row.right() - 16.0 - RETRY_WIDTH
    };
    Rect::new(x, row.y + 26.0, RETRY_WIDTH, LINK_HIT)
}

/// Where Retry's text is drawn: the second line, right-aligned in its hit
/// area.
pub fn row_retry_text(row: &Rect, beside_link: bool) -> Rect {
    let hit = row_retry(row, beside_link);
    Rect::new(hit.x, row.y + 28.0, hit.w, 20.0)
}

/// What a click on a row hits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowPart {
    /// The row itself: it selects.
    Row,
    /// A running row's cancel cross.
    Cancel,
    /// The row's link, or an exported row's "Saved to" line.
    Link,
    /// A failed or cancelled row's Retry.
    Retry,
}

/// The part of `row` under `(x, y)`, a point inside it. `running` says it
/// has a cancel cross; `link` says it has a link, `saved` a "Saved to"
/// line that opens its folder. The link's and the cross's areas are
/// targets of their own, taken out of the row's.
pub fn row_part(row: &Rect, x: f32, y: f32, running: bool, link: bool, saved: bool) -> RowPart {
    row_part_with_retry(row, x, y, running, link, saved, false)
}

/// [`row_part`] for a row that may also have a Retry (`retry`), which
/// sits beside the link when there is one.
pub fn row_part_with_retry(
    row: &Rect,
    x: f32,
    y: f32,
    running: bool,
    link: bool,
    saved: bool,
    retry: bool,
) -> RowPart {
    if running && row_cancel(row).contains(x, y) {
        RowPart::Cancel
    } else if retry && row_retry(row, link).contains(x, y) {
        RowPart::Retry
    } else if (link && row_link(row).contains(x, y))
        || (!link && saved && row_line(row).contains(x, y))
    {
        RowPart::Link
    } else {
        RowPart::Row
    }
}

/// Where a row's chip and text start: [`PREVIEW_GAP`] after its preview.
pub fn row_text_left(row: &Rect) -> f32 {
    row_preview(row).right() + PREVIEW_GAP
}

/// A row's second line, where a saved row's "Saved to" can be clicked.
/// It starts at [`row_text_left`], clear of the preview, and ends 16 DIPs
/// short of the row's right edge.
pub fn row_line(row: &Rect) -> Rect {
    let left = row_text_left(row);
    Rect::new(
        left,
        row.y + 28.0,
        (row.right() - 16.0 - left).max(0.0),
        20.0,
    )
}

/// A row's preview: [`PREVIEW_ROW`] square, 12 DIPs in from the row's
/// left edge and centred up and down. The chip and the text start
/// [`PREVIEW_GAP`] after it.
pub fn row_preview(row: &Rect) -> Rect {
    Rect::new(
        row.x + 12.0,
        row.y + (row.h - PREVIEW_ROW) / 2.0,
        PREVIEW_ROW,
        PREVIEW_ROW,
    )
}

/// Width of the footer's "Export all (n)" button.
pub const EXPORT_ALL_WIDTH: f32 = 128.0;

/// The footer's "Export all (n)" button, left of Clear. It shows only
/// while results are held.
pub fn export_all_button(clear: &Rect) -> Rect {
    Rect::new(
        clear.x - 8.0 - EXPORT_ALL_WIDTH,
        clear.y,
        EXPORT_ALL_WIDTH,
        clear.h,
    )
}

/// Width of the Add files buttons.
pub const ADD_FILES_WIDTH: f32 = 88.0;

/// The footer's Add files button: left of Export all while that shows,
/// else left of Clear.
pub fn add_files_button(clear: &Rect, export_all: bool) -> Rect {
    let right = if export_all {
        export_all_button(clear).x
    } else {
        clear.x
    };
    Rect::new(
        right - 8.0 - ADD_FILES_WIDTH,
        clear.y,
        ADD_FILES_WIDTH,
        clear.h,
    )
}

/// How many rows a Page Up or Page Down moves in a list `panel_h` tall:
/// a screenful less one, so the row last seen is still in view.
pub fn rows_per_page(panel_h: f32) -> isize {
    ((panel_h / ROW_HEIGHT).floor() as isize - 1).max(1)
}

/// The widest the content column grows in a wide window.
pub const MAX_CONTENT_WIDTH: f32 = 880.0;

/// The widest it grows with the inspector all the way in: the list keeps
/// about the width it has without one. With files the cap rises from
/// [`MAX_CONTENT_WIDTH`] to this as the inspector slides in.
pub const MAX_CONTENT_WIDTH_WIDE: f32 = 1180.0;

/// The inspector's width, in DIPs.
pub const INSPECTOR_WIDTH: f32 = 300.0;

/// The rule between the list and the inspector.
pub const INSPECTOR_RULE: f32 = 1.0;

/// The narrowest window, in zoomed DIPs, that shows the inspector beside
/// the list. Narrower, it folds away.
pub const INSPECTOR_FROM: f32 = 760.0;

/// A row's preview: a square this many DIPs on a side.
pub const PREVIEW_ROW: f32 = 34.0;

/// The corner radius of a row's preview.
pub const PREVIEW_RADIUS: f32 = 6.0;

/// The inspector's preview, in DIPs: the width and height at
/// [`INSPECTOR_WIDTH`], where it spans what the padding leaves.
pub const HERO: (f32, f32) = (268.0, 120.0);

/// Height of a format the inspector offers to convert to.
pub const TARGET_HEIGHT: f32 = 36.0;

/// Height of one labelled fact in the inspector.
pub const FACT_HEIGHT: f32 = 18.0;

/// Gap between a row's preview and the chip and text beside it.
pub const PREVIEW_GAP: f32 = 12.0;

/// Width of the footer's "Undo" link, after rows were removed.
pub const UNDO_WIDTH: f32 = 48.0;

/// The status area is wide enough for the Undo link beside its text.
const UNDO_MIN_STATUS: f32 = 220.0;

/// Where the "Undo" link sits: at the right end of the footer's status
/// line, 24 DIPs tall. `None` when the status is too narrow to hold it
/// beside the words (Ctrl+Z still undoes).
pub fn undo_link(status: &Rect) -> Option<Rect> {
    (status.w >= UNDO_MIN_STATUS).then(|| {
        Rect::new(
            status.right() - UNDO_WIDTH,
            status.y + (status.h - LINK_HIT) / 2.0,
            UNDO_WIDTH,
            LINK_HIT,
        )
    })
}

/// Width of the footer's Convert button: the word and, after it, its key
/// ("Enter") in a keycap.
pub const FOOTER_CONVERT_WIDTH: f32 = 124.0;

/// Width of the footer's Convert button without its keycap, where the
/// footer has no room for it.
pub const FOOTER_CONVERT_NARROW: f32 = 80.0;

/// The status line keeps at least this much beside the footer's buttons
/// before Convert gives up its keycap: room for the short idle hint
/// (155 DIPs at 13 px).
const STATUS_BEFORE_KEYCAP: f32 = 160.0;

/// The footer's Convert button in `footer`: left of Add files, so it is
/// the first control after the list in reading and Tab order. It is wide
/// enough for its keycap unless that would squeeze the status line below
/// the short idle hint, as in the smallest window.
pub fn convert_button(footer: &Rect, clear: &Rect, export_all: bool) -> Rect {
    let add = add_files_button(clear, export_all);
    let wide = add.x - 8.0 - FOOTER_CONVERT_WIDTH;
    let w = if wide - 12.0 - footer.x >= STATUS_BEFORE_KEYCAP {
        FOOTER_CONVERT_WIDTH
    } else {
        FOOTER_CONVERT_NARROW
    };
    Rect::new(add.x - 8.0 - w, clear.y, w, clear.h)
}

/// Whether the footer's Convert button, `convert`, has room for its keycap.
pub fn convert_keycap(convert: &Rect) -> bool {
    convert.w >= FOOTER_CONVERT_WIDTH
}

/// The footer's status line: from the footer's left edge to 12 DIPs short
/// of the leftmost button, `leftmost` (Convert).
pub fn footer_status(footer: &Rect, leftmost: &Rect) -> Rect {
    Rect::new(
        footer.x + 4.0,
        footer.y,
        (leftmost.x - 12.0 - footer.x).max(0.0),
        footer.h,
    )
}

/// The wheel's centre, in coordinates about the wheel's middle: its name,
/// the key that opens it, and the source type or the pair being pointed
/// at. Each line's box lies wholly inside the centre disc of `radius`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WheelCentre {
    pub name: Rect,
    pub key: Rect,
    pub source: Rect,
    /// Where "Options… (Tab)" can be clicked while a target with options
    /// is lit. It takes the key line's place, a little taller so it's
    /// easy to hit.
    pub options: Rect,
}

pub fn wheel_centre(radius: f32) -> WheelCentre {
    // Three lines stacked tight: 28 for the serif name, 16 for each
    // caption line, a little above the middle to balance the serif.
    let line = |y: f32, h: f32, w: f32| Rect::new(-w / 2.0, y, w, h);
    let inner = |y: f32| 2.0 * (radius * radius - y * y).max(0.0).sqrt();
    let width = |top: f32, h: f32| (inner(top.abs().max((top + h).abs())) / 4.0).floor() * 4.0;
    WheelCentre {
        name: line(-32.0, 28.0, width(-32.0, 28.0)),
        key: line(-4.0, 16.0, width(-4.0, 16.0)),
        source: line(12.0, 16.0, width(12.0, 16.0)),
        // On the key line, reaching 2 DIPs into the empty space under the
        // name and over the source, where no text is drawn.
        options: line(-6.0, 20.0, width(-6.0, 20.0)),
    }
}

/// The empty window's centred group, top to bottom.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EmptyState {
    /// The mark icon, drawn at three times its size.
    pub mark: Rect,
    pub headline: Rect,
    pub instruction: Rect,
    pub button: Rect,
    /// One muted line under the button for what the status says, such as
    /// "There are no files to paste.". Kept whether or not it's used, so
    /// the group doesn't move when a line appears.
    pub status: Rect,
    pub keys: Rect,
    pub privacy: Rect,
    /// A tile for each of the four kinds, in a row under the privacy
    /// line, with a word line [`GLAZE_WORD`] tall [`GLAZE_WORD_GAP`]
    /// below each. `None` when the panel is too short to hold them whole:
    /// the rest of the group then stands as it would without them.
    pub glazes: Option<[Rect; 4]>,
}

/// A glaze tile in the empty window: a square this many DIPs on a side.
pub const GLAZE_TILE: f32 = 64.0;

/// The gap between two glaze tiles.
pub const GLAZE_TILE_GAP: f32 = 12.0;

/// Height of the word line under a glaze tile.
pub const GLAZE_WORD: f32 = 16.0;

/// The gap between a glaze tile and its word.
pub const GLAZE_WORD_GAP: f32 = 4.0;

/// The empty window's group, centred in the drop panel.
pub fn empty_state(panel: &Rect) -> EmptyState {
    // Each part's height and the gap after it.
    const PARTS: [(f32, f32); 8] = [
        (72.0, 16.0),                                    // mark
        (24.0, 4.0),                                     // headline
        (20.0, 12.0),                                    // instruction
        (BUTTON_HEIGHT, 8.0),                            // button
        (16.0, 8.0),                                     // status
        (16.0, 4.0),                                     // keys
        (16.0, 20.0),                                    // privacy, and the gap to the tiles
        (GLAZE_TILE + GLAZE_WORD_GAP + GLAZE_WORD, 0.0), // tiles and words
    ];
    // The tiles' part; the parts before it are the text.
    const TILES: usize = 7;
    let sum = |parts: &[(f32, f32)]| parts.iter().map(|(h, gap)| h + gap).sum::<f32>();
    // The tiles show only when the whole group, with them, fits. Without
    // them the privacy line is the last part and has no gap after it.
    let tiles = panel.h >= sum(&PARTS);
    let total = if tiles {
        sum(&PARTS)
    } else {
        sum(&PARTS[..TILES]) - PARTS[TILES - 1].1
    };
    let mut y = snap(panel.y + ((panel.h - total) / 2.0).max(0.0));
    let cx = panel.x + panel.w / 2.0;
    let line = |y: f32, h: f32| Rect::new(panel.x + 16.0, y, (panel.w - 32.0).max(0.0), h);
    let mut next = |k: usize| {
        let top = y;
        y += PARTS[k].0 + PARTS[k].1;
        top
    };
    let mark = Rect::new(snap(cx - 36.0), next(0), 72.0, 72.0);
    let headline = line(next(1), 24.0);
    let instruction = line(next(2), 20.0);
    let button = Rect::new(
        snap(cx - ADD_FILES_EMPTY_WIDTH / 2.0),
        next(3),
        ADD_FILES_EMPTY_WIDTH,
        BUTTON_HEIGHT,
    );
    let status = line(next(4), 16.0);
    let keys = line(next(5), 16.0);
    let privacy = line(next(6), 16.0);
    // Four tiles with a gap between, centred on the panel. A whole DIP
    // keeps their edges crisp; the span is odd in grid units, so the row
    // can't be both centred and on the 4 DIP grid.
    let glazes = tiles.then(|| {
        let top = next(TILES);
        let span = 4.0 * GLAZE_TILE + 3.0 * GLAZE_TILE_GAP;
        let left = (cx - span / 2.0).round();
        std::array::from_fn(|i| {
            let x = left + i as f32 * (GLAZE_TILE + GLAZE_TILE_GAP);
            Rect::new(x, top, GLAZE_TILE, GLAZE_TILE)
        })
    });
    EmptyState {
        mark,
        headline,
        instruction,
        button,
        status,
        keys,
        privacy,
        glazes,
    }
}

/// The empty window's Add files button, a little wider than the footer's.
pub const ADD_FILES_EMPTY_WIDTH: f32 = 112.0;

/// The details dialog: panel, title, body, and the Copy and Close
/// buttons.
pub struct Dialog {
    pub panel: Rect,
    pub title: Rect,
    pub body: Rect,
    pub copy: Rect,
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
    let copy = Rect::new(
        close.x - 8.0 - COPY_WIDTH,
        close.y,
        COPY_WIDTH,
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
        copy,
        close,
    }
}

/// The help overlay: a panel like the details dialog's, a little taller,
/// with its text scrolling in the body and Close at the bottom right.
pub struct HelpPanel {
    pub panel: Rect,
    pub title: Rect,
    pub body: Rect,
    pub close: Rect,
}

pub fn help(w: f32, h: f32) -> HelpPanel {
    let pw = (w - 64.0).clamp(0.0, 560.0);
    let ph = (h - 64.0).clamp(0.0, 560.0);
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
    HelpPanel {
        panel,
        title,
        body,
        close,
    }
}

/// A button of the details dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogButton {
    Copy,
    Close,
}

impl Dialog {
    /// The button under a point.
    pub fn hit(&self, x: f32, y: f32) -> Option<DialogButton> {
        if self.copy.contains(x, y) {
            Some(DialogButton::Copy)
        } else if self.close.contains(x, y) {
            Some(DialogButton::Close)
        } else {
            None
        }
    }

    pub fn button(&self, b: DialogButton) -> Rect {
        match b {
            DialogButton::Copy => self.copy,
            DialogButton::Close => self.close,
        }
    }
}

/// The details dialog's Copy button, wide enough for "Couldn't copy".
pub const COPY_WIDTH: f32 = 104.0;

/// A line of the details dialog's text, for the arrow keys and the wheel.
pub const DETAILS_LINE: f32 = 16.0;

/// How the keyboard and the wheel move through the details dialog's text.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextScroll {
    LineUp,
    LineDown,
    PageUp,
    PageDown,
    Home,
    End,
    /// Wheel notches, fractions too as a touchpad sends them: positive
    /// scrolls back towards the top.
    Wheel(f32),
}

/// A scroll offset kept between the top and where the last line meets
/// the bottom of a `view_h` tall view of `content_h` of text.
pub fn clamp_text_scroll(scroll: f32, content_h: f32, view_h: f32) -> f32 {
    scroll.clamp(0.0, (content_h - view_h).max(0.0))
}

/// The scroll offset after `by`, clamped. A page keeps one line of the
/// last page in view; a wheel notch is three lines.
pub fn scroll_text(by: TextScroll, scroll: f32, content_h: f32, view_h: f32) -> f32 {
    // An offset left over from a smaller window first comes back inside,
    // so the step moves from where the text is drawn.
    let scroll = clamp_text_scroll(scroll, content_h, view_h);
    let page = (view_h - DETAILS_LINE).max(DETAILS_LINE);
    let to = match by {
        TextScroll::LineUp => scroll - DETAILS_LINE,
        TextScroll::LineDown => scroll + DETAILS_LINE,
        TextScroll::PageUp => scroll - page,
        TextScroll::PageDown => scroll + page,
        TextScroll::Home => 0.0,
        TextScroll::End => content_h,
        TextScroll::Wheel(n) => scroll - n * 3.0 * DETAILS_LINE,
    };
    clamp_text_scroll(to, content_h, view_h)
}

/// The pages of the settings panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsTab {
    #[default]
    General,
    Quality,
    Engines,
}

impl SettingsTab {
    pub const ALL: [SettingsTab; 3] = [
        SettingsTab::General,
        SettingsTab::Quality,
        SettingsTab::Engines,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SettingsTab::General => "General",
            SettingsTab::Quality => "Quality",
            SettingsTab::Engines => kiln_core::plain::HELPER_APPS_TAB,
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }
}

/// Something the settings panel lets you press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Tab(SettingsTab),
    OutputChoose,
    OutputReset,
    Theme(ThemeChoice),
    Key(WheelKind),
    /// The On (true) or Off (false) segment of the Suggestions control.
    Suggestions(bool),
    /// Clears the picks counted for suggestions.
    ForgetPicks,
    /// The On (true) or Off (false) segment of "Save results next to the
    /// original".
    AutoSave(bool),
    /// One segment of a quality preset control.
    Preset(DefaultSetting, Preset),
    /// A value that steps through its choices: "1080p", "30 fps".
    Step(DefaultSetting),
    EngineChoose(Engine),
    EngineReset(Engine),
    /// A missing engine's Get it link: its download page, in the browser.
    EngineGet(Engine),
    CheckUpdates,
    Close,
}

/// One labelled line of the settings panel.
pub struct SettingsRow {
    pub label: &'static str,
    pub label_rect: Rect,
    /// Where the row's current value is written, for rows that show one.
    pub value: Option<Rect>,
    /// A line explaining the row, for rows that need one.
    pub hint: Option<&'static str>,
    /// Where the hint goes: a muted line under the row.
    pub hint_rect: Option<Rect>,
}

pub struct SettingsLayout {
    /// The page shown.
    pub tab: SettingsTab,
    pub panel: Rect,
    pub title: Rect,
    pub rows: Vec<SettingsRow>,
    /// Every control, in Tab order.
    pub controls: Vec<(Control, Rect)>,
    /// Beside the update button: what the update check found.
    pub update_note: Rect,
}

pub const SETTINGS_ROW: f32 = 40.0;
/// The General page's row for the wheel keys. Too long for one line at
/// the label's width, it wraps onto two.
pub const WHEEL_KEYS_LABEL: &str = "Keys to hold while dropping";
/// The label of the setting that saves each result as it is ready.
pub const AUTOSAVE_LABEL: &str = "Save results next to the original";
/// The output folder's row: where the folder dialog opens when saving.
pub const OUTPUT_LABEL: &str = "Folder shown first when saving";
/// The Quality page's page DPI, which sets how sharp every page drawn as
/// an image comes out, not only PDF's. It wraps onto two lines in the
/// second column, as the sheet's knob of the same name doesn't need to.
pub const PAGE_DPI_LABEL: &str = "Sharpness (DPI)";
/// The muted hint under a settings row, room for two caption lines.
pub const SETTINGS_HINT: f32 = 32.0;
pub const SETTINGS_LABEL: f32 = 112.0;
/// A label in the second column of the quality page.
const SETTINGS_LABEL_2: f32 = 80.0;
/// Segments of the theme and preset controls.
pub const SEGMENT_WIDTH: f32 = 64.0;
/// The tabs at the top of the settings panel.
pub const TAB_WIDTH: f32 = 88.0;
/// A control that steps through values.
pub const STEP_WIDTH: f32 = 96.0;
/// The left end of a stepping control, where a click steps back.
pub const CHEVRON_ZONE: f32 = 28.0;
/// The Check for updates button.
pub const UPDATE_BUTTON_WIDTH: f32 = 136.0;
/// The Reset suggestions button, wider than Choose to fit its label.
pub const FORGET_BUTTON_WIDTH: f32 = 136.0;
/// Rows on the fullest page. Every page keeps this height, so switching
/// pages never moves the panel.
const SETTINGS_ROWS: usize = 5;
/// An engine's row: its name, where it is and its buttons, with what it's
/// for on a muted line underneath that runs the panel's width. Four of
/// them fit in the height of five rows.
pub const ENGINE_ROW: f32 = 48.0;
/// A missing engine's Get it link, left of Choose.
pub const GET_IT_WIDTH: f32 = 48.0;

/// What the settings panel needs to know to lay itself out: the page, and
/// which values are set, since those rows grow a Reset button.
#[derive(Debug, Clone, Copy, Default)]
pub struct SettingsShape {
    pub tab: SettingsTab,
    pub output_custom: bool,
    pub engine_custom: [bool; 4],
    /// Engines Kiln can't find anywhere, which get a Get it link.
    pub engine_missing: [bool; 4],
}

/// Where Tab, or Shift+Tab with `back`, goes in a settings panel from
/// control `from`: the next enabled control, but with the page tabs as one
/// stop. Tab leaves the strip for the first control after it, and coming
/// back into it lands on the page that is showing (`selected`); the arrow
/// keys move between the tabs themselves. `None` when nothing is enabled.
pub fn settings_tab_stop(
    controls: &[Control],
    enabled: &[bool],
    selected: SettingsTab,
    from: Option<usize>,
    back: bool,
) -> Option<usize> {
    let n = controls.len();
    let on_tabs = from.is_some_and(|i| matches!(controls[i], Control::Tab(_)));
    let mut f = from;
    for _ in 0..n {
        f = crate::wheel::step(f, if back { -1 } else { 1 }, n);
        let i = f?;
        if !enabled[i] {
            continue;
        }
        match controls[i] {
            // Still in the strip: it is one stop.
            Control::Tab(_) if on_tabs => continue,
            Control::Tab(_) => {
                return controls
                    .iter()
                    .position(|c| *c == Control::Tab(selected))
                    .or(f)
            }
            _ => return f,
        }
    }
    from
}

/// The control Settings opens with the keyboard focus on: its first tab,
/// General, whose ring then shows.
pub const SETTINGS_FIRST_FOCUS: usize = 0;

pub fn settings(w: f32, h: f32, shape: SettingsShape) -> SettingsLayout {
    let p = PANEL_PADDING;
    // General's first row has a hint under it, which the other pages have
    // no use for; they keep its height so the panel never moves.
    let ph = p
        + 32.0
        + 8.0
        + SETTINGS_ROWS as f32 * SETTINGS_ROW
        + SETTINGS_HINT
        + p
        + BUTTON_HEIGHT
        + p;
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
    // On General, the rows after Start exports in sit below its hint.
    let hinted = shape.tab == SettingsTab::General;
    let row_y = |i: usize| {
        let hint = if hinted && i > 0 { SETTINGS_HINT } else { 0.0 };
        title.bottom() + 8.0 + i as f32 * SETTINGS_ROW + hint
    };
    let ctl = |x: f32, i: usize, w: f32| Rect::new(x, row_y(i) + 4.0, w, BUTTON_HEIGHT);

    let mut rows = Vec::new();
    let mut controls = Vec::new();
    let row = |label, i: usize, value: Option<Rect>| SettingsRow {
        label,
        label_rect: Rect::new(panel.x + p, row_y(i) + 8.0, SETTINGS_LABEL, 24.0),
        value,
        hint: None,
        hint_rect: None,
    };

    let tabs_x = right - SettingsTab::ALL.len() as f32 * TAB_WIDTH;
    for (k, t) in SettingsTab::ALL.into_iter().enumerate() {
        let r = Rect::new(
            tabs_x + k as f32 * TAB_WIDTH,
            title.y,
            TAB_WIDTH,
            BUTTON_HEIGHT,
        );
        controls.push((Control::Tab(t), r));
    }

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
    // Small, High and Best; returns where the group ends.
    let presets = |controls: &mut Vec<(Control, Rect)>, i: usize, s: DefaultSetting| {
        for (k, preset) in Preset::ALL.into_iter().enumerate() {
            let r = ctl(x0 + k as f32 * SEGMENT_WIDTH, i, SEGMENT_WIDTH);
            controls.push((Control::Preset(s, preset), r));
        }
        x0 + Preset::ALL.len() as f32 * SEGMENT_WIDTH
    };

    match shape.tab {
        SettingsTab::General => {
            let v = chooser(
                &mut controls,
                0,
                Control::OutputChoose,
                shape.output_custom.then_some(Control::OutputReset),
            );
            // The hint sits under the value, below the row's buttons.
            let hint_rect = Rect::new(
                x0,
                row_y(0) + SETTINGS_ROW - 4.0,
                (right - x0).max(0.0),
                SETTINGS_HINT,
            );
            // A long label: in two lines, in the row's height.
            rows.push(SettingsRow {
                hint: Some(crate::settings::START_EXPORTS_HINT),
                hint_rect: Some(hint_rect),
                label_rect: Rect::new(panel.x + p, row_y(0), SETTINGS_LABEL, SETTINGS_ROW),
                ..row(OUTPUT_LABEL, 0, Some(v))
            });

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
            // The longest label: in two lines, filling the row's height.
            rows.push(SettingsRow {
                label_rect: Rect::new(panel.x + p, row_y(2), SETTINGS_LABEL, SETTINGS_ROW),
                ..row(WHEEL_KEYS_LABEL, 2, None)
            });

            for (k, on) in [true, false].into_iter().enumerate() {
                controls.push((
                    Control::Suggestions(on),
                    ctl(x0 + k as f32 * SEGMENT_WIDTH, 3, SEGMENT_WIDTH),
                ));
            }
            controls.push((
                Control::ForgetPicks,
                ctl(x0 + 2.0 * SEGMENT_WIDTH + 8.0, 3, FORGET_BUTTON_WIDTH),
            ));
            rows.push(row("Suggestions", 3, None));

            for (k, on) in [true, false].into_iter().enumerate() {
                controls.push((
                    Control::AutoSave(on),
                    ctl(x0 + k as f32 * SEGMENT_WIDTH, 4, SEGMENT_WIDTH),
                ));
            }
            // A long label: in two lines, filling the row's height.
            rows.push(SettingsRow {
                label_rect: Rect::new(panel.x + p, row_y(4), SETTINGS_LABEL, SETTINGS_ROW),
                ..row(AUTOSAVE_LABEL, 4, None)
            });
        }
        SettingsTab::Quality => {
            // Every stepper has its own label: a second one on a line
            // sits in a second column, so two values never read as one.
            let x1 = x0 + STEP_WIDTH + 16.0;
            let stepper = |controls: &mut Vec<(Control, Rect)>,
                           rows: &mut Vec<SettingsRow>,
                           (label, i, second): (&'static str, usize, bool),
                           s| {
                let (lx, x) = if second {
                    (x1, x1 + SETTINGS_LABEL_2 + 8.0)
                } else {
                    (panel.x + p, x0)
                };
                controls.push((Control::Step(s), ctl(x, i, STEP_WIDTH)));
                // A label too long for one line takes the row's height.
                let (y, h) = if label == PAGE_DPI_LABEL {
                    (row_y(i), SETTINGS_ROW)
                } else {
                    (row_y(i) + 8.0, 24.0)
                };
                rows.push(SettingsRow {
                    label,
                    label_rect: Rect::new(lx, y, x - 8.0 - lx, h),
                    value: None,
                    hint: None,
                    hint_rect: None,
                });
            };
            let (c, r) = (&mut controls, &mut rows);
            presets(c, 0, DefaultSetting::Video);
            r.push(row("Video", 0, None));
            stepper(c, r, ("Resolution", 1, false), DefaultSetting::Resolution);
            stepper(c, r, ("Frame rate", 1, true), DefaultSetting::Fps);
            presets(c, 2, DefaultSetting::Images);
            r.push(row("Images", 2, None));
            stepper(c, r, ("Image size", 3, false), DefaultSetting::ImageSize);
            stepper(c, r, (PAGE_DPI_LABEL, 3, true), DefaultSetting::PageDpi);
            stepper(c, r, ("Sound quality", 4, false), DefaultSetting::Audio);
        }
        SettingsTab::Engines => {
            for (k, e) in Engine::ALL.into_iter().enumerate() {
                let y = title.bottom() + 8.0 + k as f32 * ENGINE_ROW;
                // The buttons sit at the top of the row, so the purpose
                // line can run under them.
                let ctl = |x: f32, w: f32| Rect::new(x, y, w, BUTTON_HEIGHT);
                let choose = ctl(right - BUTTON_WIDTH, BUTTON_WIDTH);
                let mut left = choose.x;
                if shape.engine_custom[k] {
                    let r = ctl(left - 8.0 - BUTTON_WIDTH, BUTTON_WIDTH);
                    left = r.x;
                    controls.push((Control::EngineReset(e), r));
                } else if shape.engine_missing[k] {
                    let r = ctl(left - 8.0 - GET_IT_WIDTH, GET_IT_WIDTH);
                    left = r.x;
                    controls.push((Control::EngineGet(e), r));
                }
                controls.push((Control::EngineChoose(e), choose));
                let x = panel.x + p;
                rows.push(SettingsRow {
                    label: e.label(),
                    label_rect: Rect::new(x, y + 4.0, SETTINGS_LABEL, 24.0),
                    value: Some(Rect::new(x0, y + 4.0, (left - 8.0 - x0).max(0.0), 24.0)),
                    hint: Some(e.purpose()),
                    hint_rect: Some(Rect::new(
                        x,
                        y + BUTTON_HEIGHT,
                        (right - x).max(0.0),
                        ENGINE_ROW - BUTTON_HEIGHT,
                    )),
                });
            }
        }
    }

    let close = Rect::new(
        right - BUTTON_WIDTH,
        panel.bottom() - p - BUTTON_HEIGHT,
        BUTTON_WIDTH,
        BUTTON_HEIGHT,
    );
    let check = Rect::new(panel.x + p, close.y, UPDATE_BUTTON_WIDTH, BUTTON_HEIGHT);
    let note_x = check.right() + 8.0;
    let update_note = Rect::new(
        note_x,
        close.y,
        (close.x - 8.0 - note_x).max(0.0),
        BUTTON_HEIGHT,
    );
    controls.push((Control::CheckUpdates, check));
    controls.push((Control::Close, close));

    SettingsLayout {
        tab: shape.tab,
        panel,
        title,
        rows,
        controls,
        update_note,
    }
}

/// Something the options sheet lets you press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SheetControl {
    /// One segment of the preset control in a row.
    Preset(usize, Preset),
    /// The stepping control in a row.
    Step(usize),
    Cancel,
    Convert,
}

impl SheetControl {
    /// The knob row this control belongs to.
    pub fn row(self) -> Option<usize> {
        match self {
            SheetControl::Preset(r, _) | SheetControl::Step(r) => Some(r),
            _ => None,
        }
    }
}

/// The options sheet a conversion opens from the wheel.
pub struct SheetLayout {
    pub panel: Rect,
    pub title: Rect,
    /// What the target format is, muted under the title.
    pub target: Rect,
    /// What the source is, under that.
    pub context: Rect,
    /// One label per knob.
    pub labels: Vec<Rect>,
    /// Every control, in Tab order. Convert comes last.
    pub controls: Vec<(SheetControl, Rect)>,
    /// The result's size and a rough file size, beside the buttons.
    pub result: Rect,
}

pub const SHEET_WIDTH: f32 = 432.0;
pub const SHEET_LABEL: f32 = 112.0;
/// The sheet's stepping controls, wider than the settings page's since
/// each row holds only one.
pub const SHEET_STEP_WIDTH: f32 = 136.0;
pub const CONVERT_WIDTH: f32 = 88.0;

/// Lay out the sheet for `knobs`, centred in a `w` x `h` window.
pub fn sheet(w: f32, h: f32, knobs: &[Knob]) -> SheetLayout {
    let p = PANEL_PADDING;
    let n = knobs.len() as f32;
    let ph = p + 32.0 + 20.0 + 20.0 + 12.0 + n * SETTINGS_ROW + 12.0 + BUTTON_HEIGHT + p;
    let ph = (ph / GRID).ceil() * GRID;
    let pw = snap((w - 32.0).clamp(0.0, SHEET_WIDTH));
    let panel = Rect::new(
        snap(((w - pw) / 2.0).max(0.0)),
        snap(((h - ph) / 2.0).max(8.0)),
        pw,
        ph,
    );
    let title = Rect::new(panel.x + p, panel.y + p, panel.w - 2.0 * p, 32.0);
    let target = Rect::new(title.x, title.bottom(), title.w, 20.0);
    let context = Rect::new(title.x, target.bottom(), title.w, 20.0);
    let x0 = panel.x + p + SHEET_LABEL + 8.0;
    let row_y = |i: usize| context.bottom() + 12.0 + i as f32 * SETTINGS_ROW;
    let mut labels = vec![];
    let mut controls = vec![];
    for (i, knob) in knobs.iter().enumerate() {
        labels.push(Rect::new(panel.x + p, row_y(i) + 8.0, SHEET_LABEL, 24.0));
        let y = row_y(i) + 4.0;
        match knob {
            Knob::Preset => {
                for (k, preset) in Preset::ALL.into_iter().enumerate() {
                    let r = Rect::new(
                        x0 + k as f32 * SEGMENT_WIDTH,
                        y,
                        SEGMENT_WIDTH,
                        BUTTON_HEIGHT,
                    );
                    controls.push((SheetControl::Preset(i, preset), r));
                }
            }
            _ => controls.push((
                SheetControl::Step(i),
                Rect::new(x0, y, SHEET_STEP_WIDTH, BUTTON_HEIGHT),
            )),
        }
    }
    let right = panel.right() - p;
    let by = panel.bottom() - p - BUTTON_HEIGHT;
    let convert = Rect::new(right - CONVERT_WIDTH, by, CONVERT_WIDTH, BUTTON_HEIGHT);
    let cancel = Rect::new(
        convert.x - 8.0 - BUTTON_WIDTH,
        by,
        BUTTON_WIDTH,
        BUTTON_HEIGHT,
    );
    controls.push((SheetControl::Cancel, cancel));
    controls.push((SheetControl::Convert, convert));
    let result = Rect::new(
        panel.x + p,
        by,
        (cancel.x - 8.0 - panel.x - p).max(0.0),
        BUTTON_HEIGHT,
    );
    SheetLayout {
        panel,
        title,
        target,
        context,
        labels,
        controls,
        result,
    }
}

fn snap(v: f32) -> f32 {
    (v / GRID).floor() * GRID
}

/// What the inspector shows, as far as its geometry goes: which parts it
/// has, how tall the ones that hold wrapped text are, and how many buttons
/// it has (the count of its controls that are buttons).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InspectorShape {
    /// Nothing selected: the mark in the hero's place, a title, and
    /// two lines of body.
    Nothing,
    /// One file nothing has been done to. `what_h` is the height of what
    /// the format is, wrapped (0 for none); `facts` the labelled facts;
    /// `targets` the formats offered; `more` whether More follows them.
    Waiting {
        what_h: f32,
        facts: usize,
        targets: usize,
        more: bool,
    },
    /// A running job: the percentage and the time left in the body, and
    /// its `buttons` (Cancel).
    Converting { buttons: usize },
    /// A finished job. Its body holds a line for the result, one for the
    /// change when `change`, and one for the hint that the result can be
    /// dragged out when `drag`, each 20 DIPs; then its `buttons`.
    Done {
        change: bool,
        drag: bool,
        buttons: usize,
    },
    /// A failed or cancelled job: its sentence, `sentence_h` tall once
    /// wrapped, then its `buttons`.
    Failed { sentence_h: f32, buttons: usize },
    /// Several files. `targets` counts whichever list is shown: the
    /// formats they all share or, when they share none, the kinds
    /// offered. `none_h` is the height of the sentence saying why there
    /// is no shared format (0 when there is one).
    Several { targets: usize, none_h: f32 },
}

/// Where the inspector's parts go. A part the shape doesn't have, or that
/// doesn't fit whole, is `None` or empty; `hero`, `title` and `body` are
/// always there to draw into, as an empty rect when left out.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct InspectorLayout {
    /// The preview, [`HERO`] tall and as wide as the padding leaves. In
    /// `Nothing` it holds the 48 DIP mark, centred. It is the first
    /// thing to go when the inspector is short.
    pub hero: Rect,
    pub title: Rect,
    /// What the format is, the sentence, or the lines of a running or
    /// finished job (a running job's 40 DIP percentage, then a 20 DIP
    /// time line). Empty when the shape has none.
    pub body: Rect,
    /// One rect per fact, [`FACT_HEIGHT`] tall.
    pub facts: Vec<Rect>,
    /// The "Turn it into" line over the targets.
    pub label: Option<Rect>,
    /// One rect per target, [`TARGET_HEIGHT`] tall, however many fit.
    pub targets: Vec<Rect>,
    pub more: Option<Rect>,
    /// One slot for each of the shape's `buttons`, left to right, so a
    /// slot is never drawn as a button that isn't there. A slot that
    /// doesn't fit across the rect is left out, so a rect narrower than
    /// the buttons never places a control that sticks out of it.
    pub buttons: Vec<Rect>,
}

/// Width of an inspector button.
pub const INSPECTOR_BUTTON_WIDTH: f32 = 96.0;

/// The inspector's parts stacked down `r`, inside 16 DIPs of padding. `r`
/// is the whole inspector, [`INSPECTOR_WIDTH`] wide: the fold slides it in
/// by translation and clipping, and never lays it out narrower.
///
/// When the whole stack doesn't fit `r`'s height, parts give way so the
/// inspector keeps what it exists for, the actions: first the hero, then
/// all of the facts. Only then are the targets, More and the buttons cut
/// from the bottom. Whatever doesn't fit whole inside the padding, down
/// or across, is left out, and so is every part after it, so a short or
/// narrow inspector shows the top of its story and never a cut-off or
/// protruding control.
pub fn inspector_layout(r: &Rect, shape: &InspectorShape) -> InspectorLayout {
    for (hero, facts) in [(true, true), (false, true)] {
        let (layout, whole) = inspector_stack(r, shape, hero, facts);
        if whole {
            return layout;
        }
    }
    inspector_stack(r, shape, false, false).0
}

/// One try at [`inspector_layout`], offering the hero and the facts or
/// not, and whether every part it offered was placed whole.
fn inspector_stack(
    r: &Rect,
    shape: &InspectorShape,
    hero: bool,
    facts: bool,
) -> (InspectorLayout, bool) {
    /// Parts stacked down the padded rect, each after a gap, except the
    /// first.
    struct Stack {
        inner: Rect,
        y: f32,
        placed: bool,
        fits: bool,
    }
    impl Stack {
        fn place(&mut self, gap: f32, h: f32) -> Option<Rect> {
            let y = if self.placed { self.y + gap } else { self.y };
            if !self.fits || y + h > self.inner.bottom() {
                self.fits = false;
                return None;
            }
            self.y = y + h;
            self.placed = true;
            Some(Rect::new(self.inner.x, y, self.inner.w, h))
        }

        /// A part's place, or an empty rect where the stack stopped.
        fn or_empty(&self, at: Option<Rect>) -> Rect {
            at.unwrap_or(Rect::new(self.inner.x, self.y, self.inner.w, 0.0))
        }
    }

    const TITLE: f32 = 24.0;
    const LABEL: f32 = 16.0;
    const MORE: f32 = 24.0;
    const LINE: f32 = 20.0;
    let inner = r.inset(PANEL_PADDING);
    let mut s = Stack {
        inner,
        y: inner.y,
        placed: false,
        fits: true,
    };
    // The body's height, the facts, the targets, whether More follows
    // them, and the buttons.
    let (body_h, fact_count, targets, more, buttons) = match *shape {
        InspectorShape::Nothing => (40.0, 0, 0, false, 0),
        InspectorShape::Waiting {
            what_h,
            facts,
            targets,
            more,
        } => (what_h, facts, targets, more, 0),
        InspectorShape::Converting { buttons } => (60.0, 0, 0, false, buttons),
        InspectorShape::Done {
            change,
            drag,
            buttons,
        } => {
            let lines = 1 + u8::from(change) + u8::from(drag);
            (LINE * f32::from(lines), 0, 0, false, buttons)
        }
        InspectorShape::Failed {
            sentence_h,
            buttons,
        } => (sentence_h, 0, 0, false, buttons),
        InspectorShape::Several { targets, none_h } => (none_h, 0, targets, false, 0),
    };

    // Without the hero the stack starts at the title, and when the hero
    // is offered but doesn't fit, nothing after it does either.
    let hero = if hero { s.place(0.0, HERO.1) } else { None };
    let hero = s.or_empty(hero);
    let title = s.place(12.0, TITLE);
    let title = s.or_empty(title);
    let body = if body_h > 0.0 {
        s.place(8.0, body_h)
    } else {
        None
    };
    let body = s.or_empty(body);

    let facts: Vec<Rect> = if facts {
        (0..fact_count)
            .map_while(|i| s.place(if i == 0 { 12.0 } else { 0.0 }, FACT_HEIGHT))
            .collect()
    } else {
        vec![]
    };

    let mut label = None;
    let mut rows = vec![];
    if targets > 0 {
        label = s.place(12.0, LABEL);
        rows = (0..targets)
            .map_while(|_| s.place(6.0, TARGET_HEIGHT))
            .collect();
        // A heading over nothing is worse than none.
        if rows.is_empty() {
            label = None;
        }
    }
    let more = if more { s.place(6.0, MORE) } else { None };

    // The buttons in a row, as many as fit across the padded width.
    let step = INSPECTOR_BUTTON_WIDTH + 8.0;
    let across = (0..buttons)
        .take_while(|&i| i as f32 * step + INSPECTOR_BUTTON_WIDTH <= inner.w)
        .count();
    let row = if across > 0 {
        s.place(12.0, BUTTON_HEIGHT)
    } else {
        None
    };
    let slots: Vec<Rect> = row
        .map(|row| {
            (0..across)
                .map(|i| {
                    let x = row.x + i as f32 * step;
                    Rect::new(x, row.y, INSPECTOR_BUTTON_WIDTH, BUTTON_HEIGHT)
                })
                .collect()
        })
        .unwrap_or_default();
    if slots.len() < buttons {
        s.fits = false;
    }

    let layout = InspectorLayout {
        hero,
        title,
        body,
        facts,
        label,
        targets: rows,
        more,
        buttons: slots,
    };
    (layout, s.fits)
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
    // The inspector's part of the panel holds no rows.
    if !l.list.contains(x, y) {
        return None;
    }
    let i = ((y - l.list.y + scroll) / ROW_HEIGHT).floor();
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
        for (w, h) in [
            (480.0, 360.0),
            (720.0, 480.0),
            (1024.0, 768.0),
            (1200.0, 360.0),
            (1600.0, 900.0),
        ] {
            for rows in [0, 1, 5] {
                let l = layout(w, h, rows);
                assert_grid(&l.header, "header");
                assert_grid(&l.icon, "icon");
                assert_grid(&l.body, "body");
                assert_grid(&l.panel, "panel");
                // The list gives up the inspector's 1 DIP rule, so with the
                // rule counted back in, it and its rows stand on the grid.
                let ruled = |r: &Rect| {
                    let rule = if l.inspector.is_some() {
                        INSPECTOR_RULE
                    } else {
                        0.0
                    };
                    Rect::new(r.x, r.y, r.w + rule, r.h)
                };
                assert_grid(&ruled(&l.list), "list");
                for r in &l.rows {
                    assert_grid(&ruled(r), "row");
                }
                assert_eq!(l.inspector.is_some(), rows > 0 && inspector_fits(w));
                if let Some(i) = &l.inspector {
                    assert_grid(i, "inspector");
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
    fn retry_has_a_target_of_its_own_beside_show_details() {
        let l = layout(720.0, 480.0, 3);
        let (r, next) = (l.rows[0], l.rows[1]);
        let link = row_link(&r);
        for beside in [true, false] {
            let retry = row_retry(&r, beside);
            // WCAG 2.5.8: 24 DIPs each way, inside its row's band.
            assert!(retry.h >= 24.0 && retry.w >= 24.0, "{retry:?}");
            assert!(retry.y >= r.y && retry.bottom() <= r.bottom());
            assert!(apart(&retry, &next));
            let text = row_retry_text(&r, beside);
            assert!(text.y >= retry.y && text.bottom() <= retry.bottom());
            assert_eq!(text.y, row_line(&r).y);
            // Its points are Retry; the gap to Show details is the row.
            let mid = (retry.x + retry.w / 2.0, retry.y + retry.h / 2.0);
            assert_eq!(
                row_part_with_retry(&r, mid.0, mid.1, false, beside, false, true),
                RowPart::Retry
            );
            assert_eq!(
                row_part_with_retry(&r, mid.0, mid.1, false, beside, false, false),
                RowPart::Row
            );
        }
        // Beside Show details the two never touch: a gap of at least 8.
        let retry = row_retry(&r, true);
        assert!(link.x - retry.right() >= 8.0, "{retry:?} {link:?}");
        assert_eq!(
            row_part_with_retry(&r, link.x + 4.0, link.y + 4.0, false, true, false, true),
            RowPart::Link
        );
        // Alone it takes the link's right edge.
        assert_eq!(row_retry(&r, false).right(), link.right());
    }

    #[test]
    fn show_details_has_a_target_of_its_own() {
        let l = layout(720.0, 480.0, 3);
        let (r, next) = (l.rows[0], l.rows[1]);
        let link = row_link(&r);
        // WCAG 2.5.8: 24 DIPs each way, on its own.
        assert!(link.h >= 24.0 && link.w >= 24.0, "{link:?}");
        // Inside its row's band: it never reaches the next row's target.
        assert!(link.y >= r.y && link.bottom() <= r.bottom());
        assert!(link.bottom() <= next.y);
        assert!(apart(&link, &next));
        // Its text sits inside it, on the second line.
        let text = row_link_text(&r);
        assert!(text.y >= link.y && text.bottom() <= link.bottom());
        assert_eq!(text.y, row_line(&r).y);
        // Every point of it is the link, not the row; around it is the row.
        for (x, y) in [
            (link.x, link.y),
            (link.right() - 0.5, link.bottom() - 0.5),
            (link.x + link.w / 2.0, link.y + link.h / 2.0),
        ] {
            assert_eq!(row_part(&r, x, y, false, true, false), RowPart::Link);
        }
        let mid = link.x + link.w / 2.0;
        assert_eq!(
            row_part(&r, mid, link.y - 0.5, false, true, false),
            RowPart::Row
        );
        assert_eq!(
            row_part(&r, link.x - 0.5, link.y + 4.0, false, true, false),
            RowPart::Row
        );
        // A row without a link is all row there; a running row's cross
        // is its own too.
        assert_eq!(
            row_part(&r, mid, link.y + 4.0, false, false, false),
            RowPart::Row
        );
        let c = row_cancel(&r);
        assert_eq!(
            row_part(&r, c.x + 1.0, c.y + 1.0, true, false, false),
            RowPart::Cancel
        );
        // An exported row's "Saved to" line opens its folder.
        let line = row_line(&r);
        assert_eq!(
            row_part(&r, line.x + 1.0, line.y + 1.0, false, false, true),
            RowPart::Link
        );
        assert_eq!(row_at(&l, mid, link.bottom() - 0.5, 0.0), Some(0));
    }

    #[test]
    fn text_size_zooms_the_layout() {
        // Fonts multiply by the factor, in pixels at any display scale.
        let caption = crate::theme::type_scale::CAPTION;
        for (dpi, scale, px) in [
            (96.0, 1.0, 13.0),
            (96.0, 1.5, 19.5),
            (96.0, 2.25, 29.25),
            (120.0, 2.25, 36.5625),
        ] {
            let zoom = text_zoom(scale, min_window(scale));
            assert!(
                (caption * px_per_unit(dpi, zoom) - px).abs() < 1e-3,
                "{dpi} {scale}"
            );
        }
        let row_px = ROW_HEIGHT * px_per_unit(96.0, text_zoom(2.25, (2000.0, 1500.0)));
        assert_eq!(row_px, 126.0);
        // The window's smallest size grows with it, so the layout always
        // has its smallest window in units.
        for scale in [1.0, 1.25, 1.5, 2.0, MAX_TEXT_SCALE] {
            let (w, h) = min_window(scale);
            assert_eq!(text_zoom(scale, (w, h)), scale);
            assert_eq!((w / scale, h / scale), MIN_WINDOW);
            assert_eq!(text_zoom(scale, (w * 2.0, h * 3.0)), scale);
            assert_eq!(default_window(scale).0, DEFAULT_WINDOW.0 * scale);
        }
        // A screen too small for the grown minimum: the zoom gives way,
        // and the layout still gets its smallest window.
        for client in [(1366.0, 700.0), (1280.0, 680.0), (800.0, 560.0)] {
            let zoom = text_zoom(MAX_TEXT_SCALE, client);
            assert!((1.0..MAX_TEXT_SCALE).contains(&zoom));
            assert!(client.0 / zoom >= MIN_WINDOW.0 - 1e-3);
            assert!(client.1 / zoom >= MIN_WINDOW.1 - 1e-3);
        }
        // Out-of-range factors, or none, are kept in range.
        assert_eq!(text_zoom(0.5, (2000.0, 2000.0)), 1.0);
        assert_eq!(text_zoom(9.0, (5000.0, 5000.0)), MAX_TEXT_SCALE);
        assert_eq!(text_zoom(f32::NAN, (2000.0, 2000.0)), 1.0);
    }

    /// Every target in every view of a `client`-DIP window at Text size
    /// `scale`, laid out in units, must stand apart from its neighbours.
    fn assert_nothing_overlaps(scale: f32, client: (f32, f32)) {
        let zoom = text_zoom(scale, client);
        let (w, h) = (client.0 / zoom, client.1 / zoom);
        assert!(w >= MIN_WINDOW.0 - 1e-3 && h >= MIN_WINDOW.1 - 1e-3);
        let all_apart = |rects: &[(String, Rect)]| {
            for (i, (a, ra)) in rects.iter().enumerate() {
                assert!(ra.x >= 0.0 && ra.right() <= w, "{a} {ra:?} at {scale}");
                assert!(ra.y >= 0.0 && ra.bottom() <= h, "{a} {ra:?} at {scale}");
                for (b, rb) in &rects[i + 1..] {
                    assert!(apart(ra, rb), "{a} overlaps {b} at {scale}");
                }
            }
        };

        // The empty window.
        let l = layout(w, h, 0);
        let e = empty_state(&l.panel);
        let mut rects: Vec<(String, Rect)> = vec![
            ("icon".into(), l.icon),
            ("wordmark".into(), l.wordmark),
            ("help".into(), l.help_button),
            ("settings".into(), l.settings_button),
            ("mark".into(), e.mark),
            ("headline".into(), e.headline),
            ("instruction".into(), e.instruction),
            ("button".into(), e.button),
            ("status".into(), e.status),
            ("keys".into(), e.keys),
            ("privacy".into(), e.privacy),
        ];
        // The glaze tiles, each with the word line under it.
        for (i, tile) in e.glazes.iter().flatten().enumerate() {
            let word = Rect::new(tile.x, tile.bottom() + GLAZE_WORD_GAP, tile.w, GLAZE_WORD);
            rects.push((format!("glaze {i}"), *tile));
            rects.push((format!("glaze {i} word"), word));
        }
        all_apart(&rects);

        // The list, with every row control and the footer.
        let l = layout(w, h, 4);
        let clear = l.clear_button.unwrap();
        let footer = l.footer.unwrap();
        let add = add_files_button(&clear, true);
        let convert = convert_button(&footer, &clear, true);
        let mut rects: Vec<(String, Rect)> = vec![
            ("help".into(), l.help_button),
            ("settings".into(), l.settings_button),
            ("wordmark".into(), l.wordmark),
            ("clear".into(), clear),
            ("export all".into(), export_all_button(&clear)),
            ("add".into(), add),
            ("convert".into(), convert),
            ("status".into(), footer_status(&footer, &convert)),
        ];
        let visible: Vec<&Rect> = l
            .rows
            .iter()
            .filter(|r| r.bottom() <= l.panel.bottom())
            .collect();
        assert!(visible.len() >= 2, "two rows show at {scale}");
        for (i, r) in visible.into_iter().enumerate() {
            rects.push((format!("row {i} link"), row_link(r)));
            assert!(row_link(r).bottom() <= r.bottom() && row_cancel(r).bottom() <= r.bottom());
            assert!(
                r.x >= l.list.x && r.right() <= l.list.right(),
                "row in list"
            );
            // The preview sits inside its row, with room for the chip.
            let p = row_preview(r);
            assert!(p.x >= r.x && p.y >= r.y && p.bottom() <= r.bottom());
            assert!(p.right() + PREVIEW_GAP < row_link(r).x, "row {i} preview");
            // The saved line, which can be clicked, starts clear of it and
            // stays inside the row.
            let line = row_line(r);
            assert!(apart(&line, &p), "row {i}: the line overlaps the preview");
            assert!(line.x >= p.right() + PREVIEW_GAP && line.right() <= r.right());
        }
        all_apart(&rects);
        // The list and the inspector share the panel and never touch.
        if let Some(i) = l.inspector {
            all_apart(&[("list".into(), l.list), ("inspector".into(), i)]);
            assert!(i.x >= l.panel.x && i.right() <= l.panel.right());
            assert!((i.y, i.h) == (l.panel.y, l.panel.h));
        }

        // The dialogs.
        let d = dialog(w, h);
        all_apart(&[
            ("title".into(), d.title),
            ("body".into(), d.body),
            ("copy".into(), d.copy),
            ("close".into(), d.close),
        ]);
        let hp = help(w, h);
        all_apart(&[
            ("title".into(), hp.title),
            ("body".into(), hp.body),
            ("close".into(), hp.close),
        ]);

        // Settings, every page and shape.
        for tab in SettingsTab::ALL {
            for shape in [
                SettingsShape {
                    tab,
                    ..SettingsShape::default()
                },
                SettingsShape {
                    tab,
                    output_custom: true,
                    engine_custom: [true, false, true, false],
                    engine_missing: [false, true, false, true],
                },
            ] {
                let st = settings(w, h, shape);
                let mut rects: Vec<(String, Rect)> = st
                    .controls
                    .iter()
                    .map(|(c, r)| (format!("{c:?}"), *r))
                    .collect();
                rects.push(("update note".into(), st.update_note));
                all_apart(&rects);
                for row in &st.rows {
                    for (c, r) in &st.controls {
                        assert!(apart(&row.label_rect, r), "{} overlaps {c:?}", row.label);
                    }
                }
            }
        }

        // The options sheet at its fullest.
        let sh = sheet(
            w,
            h,
            &[Knob::Preset, Knob::Size, Knob::Fps, Knob::AudioBitrate],
        );
        let mut rects: Vec<(String, Rect)> = sh
            .controls
            .iter()
            .map(|(c, r)| (format!("{c:?}"), *r))
            .collect();
        rects.push(("result".into(), sh.result));
        all_apart(&rects);

        // The wheel, plain and with More's ring out, pointed at from each
        // corner: it stays whole and EDGE_GAP inside the window.
        use crate::wheel::{place, EDGE_GAP, R_MORE_OUTER, R_OUTER};
        for extent in [R_OUTER, R_MORE_OUTER] {
            for (x, y) in [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)] {
                let ((cx, cy), s) = place(x, y, w, h, extent);
                let r = extent * s;
                assert!(s > 0.0 && s <= 1.0, "wheel scale {s} at {scale}");
                for (c, side) in [(cx, w), (cy, h)] {
                    assert!(c - r >= EDGE_GAP - 0.01, "wheel {extent} at {scale}");
                    assert!(c + r <= side - EDGE_GAP + 0.01, "wheel {extent} at {scale}");
                }
            }
        }
    }

    #[test]
    fn settings_opens_with_focus_on_its_first_tab() {
        // Settings opens on General, and the focus index must name its
        // tab on every page and shape, so the ring draws on a tab.
        for tab in SettingsTab::ALL {
            for engine_custom in [[false; 4], [true; 4]] {
                let shape = SettingsShape {
                    tab,
                    output_custom: engine_custom[0],
                    engine_custom,
                    engine_missing: engine_custom,
                };
                let st = settings(720.0, 480.0, shape);
                assert_eq!(
                    st.controls[SETTINGS_FIRST_FOCUS].0,
                    Control::Tab(SettingsTab::General),
                    "{tab:?}"
                );
            }
        }
    }

    #[test]
    fn nothing_overlaps_in_the_smallest_window_at_any_text_size() {
        assert_nothing_overlaps(1.0, min_window(1.0));
        assert_nothing_overlaps(MAX_TEXT_SCALE, min_window(MAX_TEXT_SCALE));
        // Wide windows, where the inspector shows beside the list: a short
        // one and a roomy one.
        assert_nothing_overlaps(1.0, (1200.0, 360.0));
        assert_nothing_overlaps(1.0, (1600.0, 900.0));
        // A screen too small for the grown minimum: the zoom gives way and
        // the units come out above MIN_WINDOW and not round.
        let small = (1366.0, 700.0);
        let zoom = text_zoom(MAX_TEXT_SCALE, small);
        assert!(zoom < MAX_TEXT_SCALE);
        assert!(small.0 / zoom > MIN_WINDOW.0 || small.1 / zoom > MIN_WINDOW.1);
        assert_nothing_overlaps(MAX_TEXT_SCALE, small);
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
            assert_eq!(d.copy.right() + 8.0, d.close.x);
            assert_eq!(d.copy.y, d.close.y);
            // "Couldn't copy" is about 80 DIPs in 14 px Segoe UI.
            assert!(d.copy.w >= 100.0);
            assert!(d.copy.x >= d.panel.x + 16.0);
        }
    }

    #[test]
    fn help_fits_and_keeps_padding() {
        for (w, h) in [(480.0, 360.0), (720.0, 480.0), (1600.0, 1000.0)] {
            let d = help(w, h);
            assert!(d.panel.x >= 32.0 && d.panel.right() <= w - 32.0);
            assert!(d.panel.y >= 32.0 && d.panel.bottom() <= h - 32.0);
            assert_eq!(d.title.x - d.panel.x, 16.0);
            assert_eq!(d.panel.right() - d.close.right(), 16.0);
            assert_eq!(d.panel.bottom() - d.close.bottom(), 16.0);
            assert!(d.body.y >= d.title.bottom() && d.body.bottom() <= d.close.y);
            // The keys column and room beside it for what they do.
            assert!(
                d.body.w >= crate::help::KEYS_WIDTH + 8.0 + 200.0,
                "at {w}x{h}"
            );
            assert!(d.body.h >= 120.0, "at {w}x{h}");
            // "Help" is 48 DIPs in the serif.
            assert!(d.title.w >= 48.0);
        }
    }

    #[test]
    fn the_help_button_sits_beside_settings() {
        for w in [480.0, 720.0, 1600.0] {
            let l = layout(w, 360.0, 0);
            let (help, gear) = (l.help_button, l.settings_button);
            assert_grid(&help, "help button");
            assert_eq!(gear.x - help.right(), 8.0);
            assert_eq!((help.y, help.w, help.h), (gear.y, gear.w, gear.h));
            assert!(help.x > l.wordmark.right(), "at {w}");
        }
    }

    #[test]
    fn details_scroll_stays_inside_the_text() {
        // 400 of text in a 100 view: 0 to 300.
        assert_eq!(clamp_text_scroll(-5.0, 400.0, 100.0), 0.0);
        assert_eq!(clamp_text_scroll(150.0, 400.0, 100.0), 150.0);
        assert_eq!(clamp_text_scroll(900.0, 400.0, 100.0), 300.0);
        // Text that fits never scrolls.
        assert_eq!(clamp_text_scroll(40.0, 80.0, 100.0), 0.0);

        let s = |by, from| scroll_text(by, from, 400.0, 100.0);
        assert_eq!(s(TextScroll::LineDown, 0.0), DETAILS_LINE);
        assert_eq!(s(TextScroll::LineUp, 0.0), 0.0);
        assert_eq!(s(TextScroll::LineUp, 40.0), 40.0 - DETAILS_LINE);
        assert_eq!(s(TextScroll::PageDown, 0.0), 100.0 - DETAILS_LINE);
        assert_eq!(s(TextScroll::PageUp, 50.0), 0.0);
        assert_eq!(s(TextScroll::PageDown, 280.0), 300.0);
        assert_eq!(s(TextScroll::End, 0.0), 300.0);
        assert_eq!(s(TextScroll::Home, 250.0), 0.0);
        assert_eq!(s(TextScroll::Wheel(-1.0), 0.0), 3.0 * DETAILS_LINE);
        assert_eq!(s(TextScroll::Wheel(2.0), 100.0), 100.0 - 6.0 * DETAILS_LINE);
        assert_eq!(s(TextScroll::Wheel(-100.0), 0.0), 300.0);
        // A touchpad's small step moves less than a line.
        let small = s(TextScroll::Wheel(-0.1), 0.0);
        assert!(small > 0.0 && small < DETAILS_LINE, "{small}");
        assert_eq!(scroll_text(TextScroll::End, 0.0, 60.0, 100.0), 0.0);
        // An offset past the end, left by a resize, moves from the end.
        assert_eq!(s(TextScroll::LineUp, 900.0), 300.0 - DETAILS_LINE);
        assert_eq!(s(TextScroll::Wheel(1.0), 900.0), 300.0 - 3.0 * DETAILS_LINE);
    }

    #[test]
    fn the_settings_button_sits_in_the_header() {
        let l = layout(720.0, 480.0, 0);
        assert_grid(&l.settings_button, "settings button");
        assert_eq!(l.header.right() - l.settings_button.right(), 16.0);
        assert!(l.settings_button.bottom() <= l.header.bottom());
        assert!(l.settings_button.x > l.wordmark.right());
    }

    fn apart(a: &Rect, b: &Rect) -> bool {
        a.right() <= b.x || b.right() <= a.x || a.bottom() <= b.y || b.bottom() <= a.y
    }

    fn page(tab: SettingsTab) -> SettingsLayout {
        settings(
            720.0,
            480.0,
            SettingsShape {
                tab,
                ..SettingsShape::default()
            },
        )
    }

    #[test]
    fn settings_controls_fit_without_overlapping() {
        let mut shapes = vec![];
        for tab in SettingsTab::ALL {
            shapes.push(SettingsShape {
                tab,
                ..SettingsShape::default()
            });
            shapes.push(SettingsShape {
                tab,
                output_custom: true,
                engine_custom: [true; 4],
                engine_missing: [false; 4],
            });
            shapes.push(SettingsShape {
                tab,
                engine_custom: [true, false, false, true],
                engine_missing: [false, true, true, false],
                ..SettingsShape::default()
            });
            shapes.push(SettingsShape {
                tab,
                engine_missing: [true; 4],
                ..SettingsShape::default()
            });
        }
        for (w, h) in [(480.0, 360.0), (720.0, 480.0), (1600.0, 1000.0)] {
            for shape in &shapes {
                let s = settings(w, h, *shape);
                assert_grid(&s.panel, "settings panel");
                assert!(s.panel.x >= 16.0 && s.panel.right() <= w - 16.0);
                assert!(s.panel.bottom() <= h, "{:?} at {w}x{h}", s.panel);
                assert_eq!(s.title.x - s.panel.x, 16.0);
                for (c, r) in &s.controls {
                    assert_grid(r, "settings control");
                    assert!(r.x >= s.panel.x + 16.0, "{c:?} {r:?}");
                    assert!(
                        r.right() <= s.panel.right() - 16.0,
                        "{c:?} {r:?} at {w}x{h}"
                    );
                    assert!(r.bottom() <= s.panel.bottom() - 16.0, "{c:?} {r:?}");
                }
                for (i, (a, ra)) in s.controls.iter().enumerate() {
                    for (b, rb) in &s.controls[i + 1..] {
                        assert!(apart(ra, rb), "{a:?} overlaps {b:?} at {w}x{h}");
                    }
                }
                for row in &s.rows {
                    if let Some(v) = row.value {
                        assert!(v.x > row.label_rect.right());
                    }
                }
                let note = s.update_note;
                assert!(note.w >= 120.0, "update note too narrow at {w}x{h}");
                for (c, r) in &s.controls {
                    assert!(apart(&note, r), "update note overlaps {c:?}");
                }
            }
        }
    }

    #[test]
    fn missing_engines_get_a_link_and_every_row_fits() {
        for (w, h) in [(480.0, 360.0), (720.0, 480.0), (1600.0, 1000.0)] {
            let shape = SettingsShape {
                tab: SettingsTab::Engines,
                engine_custom: [true, false, false, false],
                engine_missing: [false, true, false, true],
                ..SettingsShape::default()
            };
            let s = settings(w, h, shape);
            let gets: Vec<_> = s
                .controls
                .iter()
                .filter_map(|(c, _)| match c {
                    Control::EngineGet(e) => Some(*e),
                    _ => None,
                })
                .collect();
            assert_eq!(gets, [Engine::Pandoc, Engine::Calibre]);
            // Get it comes before Choose in Tab order, as it reads.
            let at = |c| s.controls.iter().position(|(k, _)| *k == c).unwrap();
            assert_eq!(
                at(Control::EngineGet(Engine::Pandoc)) + 1,
                at(Control::EngineChoose(Engine::Pandoc))
            );
            // Every row, with its purpose line, ends above the bottom
            // buttons, so the page keeps the panel's height.
            let close = s.controls.last().unwrap().1;
            for row in &s.rows {
                let hint = row.hint_rect.unwrap();
                assert!(hint.bottom() + 16.0 <= close.y, "{} at {w}x{h}", row.label);
                assert!(hint.y >= row.label_rect.bottom());
                // It starts under the name and runs the panel's width.
                assert_eq!(hint.x, row.label_rect.x);
                assert_eq!(hint.right(), s.panel.right() - 16.0);
                assert!(hint.h >= 16.0);
                // "Word, Excel and PowerPoint files, and PDF output." is
                // about 315 DIPs in 13 px Segoe UI.
                assert!(hint.w >= 400.0, "{} at {w}x{h}", row.label);
                for (c, r) in &s.controls {
                    assert!(apart(&hint, r), "{} hint overlaps {c:?}", row.label);
                }
            }
        }
    }

    #[test]
    fn the_export_hint_has_its_own_line() {
        for (w, h) in [(480.0, 360.0), (720.0, 480.0), (1600.0, 1000.0)] {
            for output_custom in [false, true] {
                let s = settings(
                    w,
                    h,
                    SettingsShape {
                        output_custom,
                        ..SettingsShape::default()
                    },
                );
                let hint = s.rows[0]
                    .hint_rect
                    .expect("the output folder row has a hint");
                assert_grid(&hint, "hint");
                // The label takes two lines beside the value; the hint sits
                // to its right, under the value.
                assert!(hint.x >= s.rows[0].label_rect.right());
                assert!(hint.bottom() <= s.rows[1].label_rect.y);
                for (c, r) in &s.controls {
                    assert!(apart(&hint, r), "hint overlaps {c:?} at {w}x{h}");
                }
                // The page still fits the panel every page shares, above
                // the buttons along its bottom.
                let check = s.controls.iter().find(|(c, _)| *c == Control::CheckUpdates);
                let last = s
                    .controls
                    .iter()
                    .filter(|(c, _)| matches!(c, Control::Suggestions(_) | Control::ForgetPicks))
                    .map(|(_, r)| r.bottom())
                    .fold(0.0, f32::max);
                assert!(last + 16.0 <= check.unwrap().1.y, "at {w}x{h}");
            }
        }
        let quality = page(SettingsTab::Quality);
        assert!(quality.rows.iter().all(|r| r.hint_rect.is_none()));
        assert_eq!(page(SettingsTab::General).panel, quality.panel);
    }

    #[test]
    fn export_all_sits_left_of_clear() {
        for (w, h) in [(480.0, 360.0), (720.0, 480.0)] {
            let l = layout(w, h, 2);
            let (footer, clear) = (l.footer.unwrap(), l.clear_button.unwrap());
            let b = export_all_button(&clear);
            assert_grid(&b, "export all");
            assert_eq!(clear.x - b.right(), 8.0);
            assert_eq!((b.y, b.h), (clear.y, clear.h));
            assert!(b.x > footer.x + footer.w / 2.0);
        }
        let r = layout(720.0, 480.0, 1).rows[0];
        let line = row_line(&r);
        assert!(line.x >= r.x + 16.0 && line.right() <= r.right() - 16.0);
        assert!(line.y >= r.y && line.bottom() <= r.bottom());
    }

    #[test]
    fn add_files_sits_left_of_the_other_footer_buttons() {
        for (w, h) in [(480.0, 360.0), (720.0, 480.0)] {
            let l = layout(w, h, 2);
            let (footer, clear) = (l.footer.unwrap(), l.clear_button.unwrap());
            let alone = add_files_button(&clear, false);
            assert_grid(&alone, "add files");
            assert_eq!(clear.x - alone.right(), 8.0);
            assert_eq!((alone.y, alone.h), (clear.y, clear.h));
            let beside = add_files_button(&clear, true);
            assert_grid(&beside, "add files");
            assert_eq!(export_all_button(&clear).x - beside.right(), 8.0);
            // Convert sits left of it, with the same gap.
            let convert = convert_button(&footer, &clear, true);
            assert_grid(&convert, "convert");
            assert_eq!(beside.x - convert.right(), 8.0);
            assert_eq!((convert.y, convert.h), (clear.y, clear.h));
            // The buttons leave the left half of the footer to the status.
            assert!(convert.x >= footer.x + 4.0, "{convert:?} at {w}");
        }
    }

    #[test]
    fn the_footer_status_ends_short_of_the_buttons() {
        // DirectWrite caption widths of the full and short idle hints.
        const FULL: f32 = 303.0;
        const SHORT: f32 = 155.0;
        let status = |w: f32, export: bool| {
            let l = layout(w, 480.0, 2);
            let (footer, clear) = (l.footer.unwrap(), l.clear_button.unwrap());
            let convert = convert_button(&footer, &clear, export);
            let s = footer_status(&footer, &convert);
            assert_eq!(convert.x - s.right(), 8.0);
            s.w
        };
        // The default window has room for the full hint, and for the short
        // one while results are held and Export all takes its share.
        assert!(status(720.0, false) >= FULL);
        assert!(status(720.0, true) >= SHORT);
        // The smallest has room for the short one beside the buttons while
        // nothing is held.
        assert!(status(480.0, false) >= SHORT);
        assert!(status(480.0, false) < FULL);
        assert!(status(480.0, true) >= 0.0);
    }

    #[test]
    fn converts_keycap_gives_way_before_the_short_hint() {
        // The short idle hint's caption width, as DirectWrite measures it.
        const SHORT: f32 = 155.0;
        let at = |w: f32, export: bool| {
            let l = layout(w, 480.0, 2);
            let (footer, clear) = (l.footer.unwrap(), l.clear_button.unwrap());
            let convert = convert_button(&footer, &clear, export);
            (convert, footer_status(&footer, &convert))
        };
        // Wide enough: Convert carries its keycap, the status keeps the
        // short hint (the full one where there's room for it).
        for (w, export) in [(720.0, false), (720.0, true), (640.0, false), (640.0, true)] {
            let (convert, status) = at(w, export);
            assert_eq!(convert.w, FOOTER_CONVERT_WIDTH, "at {w}, export {export}");
            assert!(convert_keycap(&convert));
            assert!(status.w >= SHORT, "{status:?} at {w}, export {export}");
        }
        // The smallest window: the keycap goes, so the short hint still fits.
        let (convert, status) = at(480.0, false);
        assert_eq!(convert.w, FOOTER_CONVERT_NARROW);
        assert!(!convert_keycap(&convert));
        assert!(status.w >= SHORT, "{status:?}");
    }

    #[test]
    fn the_wheel_centre_stacks_inside_the_disc() {
        let r = crate::wheel::R_INNER;
        let c = wheel_centre(r);
        let lines = [c.name, c.key, c.source];
        for b in lines {
            assert_eq!(b.x, -b.w / 2.0, "{b:?} is centred");
            for (x, y) in [
                (b.x, b.y),
                (b.right(), b.y),
                (b.x, b.bottom()),
                (b.right(), b.bottom()),
            ] {
                assert!(
                    x * x + y * y <= r * r,
                    "{b:?} corner ({x}, {y}) is outside the disc"
                );
            }
        }
        for pair in lines.windows(2) {
            assert!(pair[0].bottom() <= pair[1].y, "{pair:?} overlap");
        }
        // The serif name and a long pair fit their lines: "Convert" is 80
        // DIPs, "MIXED → JPEG" 93 in the tracked monospace caption. Longer
        // pairs end in an ellipsis.
        assert!(c.name.w >= 80.0);
        assert!(c.source.w >= 93.0);
        // "Options… (Tab)" takes the key line's place: 79 DIPs in the
        // caption, its hit area 20 tall and inside the disc too.
        let o = c.options;
        assert_eq!(o.x, -o.w / 2.0);
        assert!(o.w >= 80.0 && o.h >= 20.0, "{o:?}");
        assert!(o.y <= c.key.y && o.bottom() >= c.key.bottom());
        for (x, y) in [(o.x, o.y), (o.right(), o.bottom())] {
            assert!(x * x + y * y <= r * r, "{o:?} leaves the disc");
        }
        // It reaches no more than 2 DIPs into the name's box or the
        // source's, which hold no text there.
        assert!(o.y >= c.name.bottom() - 2.0 && o.bottom() <= c.source.y + 2.0);
    }

    #[test]
    fn settings_labels_fit_at_the_smallest_window() {
        // DirectWrite widths in 14 px Segoe UI: every label on one line in
        // its rect, except two, which take two lines.
        const BODY: [(&str, f32); 7] = [
            ("Theme", 42.0),
            ("Suggestions", 75.3),
            ("Resolution", 65.0),
            ("Frame rate", 66.1),
            ("Image size", 65.3),
            ("Sound quality", 85.2),
            ("Video", 35.9),
        ];
        // "Keys to hold while" is 112.3, a hair over the 112 it has, so
        // the break falls after "hold". Were it to fall after "while", both
        // lines would still fit, as "dropping" is 57.3.
        const TWO_LINES: [(&str, [(&str, f32); 2]); 2] = [
            (
                WHEEL_KEYS_LABEL,
                [("Keys to hold", 76.4), ("while dropping", 93.3)],
            ),
            (PAGE_DPI_LABEL, [("Sharpness", 62.8), ("(DPI)", 29.9)]),
        ];
        const TABS: [(&str, f32); 3] =
            [("General", 47.6), ("Quality", 43.9), ("Helper apps", 74.4)];
        let (w, h) = MIN_WINDOW;
        for tab in SettingsTab::ALL {
            let s = settings(
                w,
                h,
                SettingsShape {
                    tab,
                    ..SettingsShape::default()
                },
            );
            for row in &s.rows {
                let r = row.label_rect;
                if let Some((_, parts)) = TWO_LINES.iter().find(|(l, _)| *l == row.label) {
                    // Two lines of about 19 DIPs, in the row's height.
                    assert!(r.h >= 38.0, "{r:?}");
                    for (part, pw) in parts {
                        assert!(*pw <= r.w, "{part}");
                    }
                    // Still clear of the controls.
                    for (c, cr) in &s.controls {
                        assert!(apart(&r, cr), "{} overlaps {c:?}", row.label);
                    }
                    continue;
                }
                if let Some((_, lw)) = BODY.iter().find(|(l, _)| *l == row.label) {
                    assert!(*lw <= r.w, "{} needs {lw}, has {}", row.label, r.w);
                }
                // Labels never run into their controls.
                for (c, cr) in &s.controls {
                    assert!(apart(&r, cr), "{} overlaps {c:?}", row.label);
                }
            }
            for (c, r) in &s.controls {
                if let Control::Tab(t) = c {
                    let (_, lw) = TABS.iter().find(|(l, _)| *l == t.label()).unwrap();
                    // The label with a little room either side.
                    assert!(lw + 8.0 <= r.w, "{} in {r:?}", t.label());
                }
            }
            // The tabs leave the title its room: "Settings" is about 80.
            let first = s.controls[0].1;
            assert!(first.x - s.title.x >= 96.0);
        }
        assert_eq!(WHEEL_KEYS_LABEL, "Keys to hold while dropping");
        assert_eq!(PAGE_DPI_LABEL, "Sharpness (DPI)");
        // The sheet's labels: "Sharpness (DPI)" is the longest.
        let sheet = sheet(w, h, &[Knob::Preset, Knob::PageDpi, Knob::AudioBitrate]);
        for (name, lw) in [("Sharpness (DPI)", 96.4), ("Sound quality", 85.2)] {
            for l in &sheet.labels {
                assert!(lw <= l.w, "{name}");
            }
        }
    }

    #[test]
    fn the_empty_state_stacks_on_the_grid_and_fits() {
        for (w, h) in [
            (480.0, 360.0),
            (720.0, 480.0),
            (1024.0, 768.0),
            (1200.0, 360.0),
            (1600.0, 900.0),
        ] {
            let panel = layout(w, h, 0).panel;
            let e = empty_state(&panel);
            let order = [
                e.mark,
                e.headline,
                e.instruction,
                e.button,
                e.status,
                e.keys,
                e.privacy,
            ];
            // The tiles show where the group with them fits, in a row of
            // four under the privacy line, and each has room for its word.
            // Where they don't fit, the rest of the group is still whole.
            // The group with them is 352 DIPs tall: the text's 248, a gap
            // of 20, and the tiles' 64 + 4 + 16.
            assert_eq!(e.glazes.is_some(), panel.h >= 352.0, "{w}x{h}");
            let mut last = e.privacy.bottom();
            if let Some(tiles) = e.glazes {
                for t in &tiles {
                    assert_eq!((t.w, t.h), (GLAZE_TILE, GLAZE_TILE));
                    assert_eq!(t.y, tiles[0].y);
                    assert!(on_grid(t.y), "tiles' top {}", t.y);
                    assert!(t.y >= e.privacy.bottom());
                    assert!(t.x >= panel.x && t.right() <= panel.right());
                }
                for pair in tiles.windows(2) {
                    assert_eq!(pair[1].x - pair[0].right(), GLAZE_TILE_GAP);
                }
                last = tiles[0].bottom() + GLAZE_WORD_GAP + GLAZE_WORD;
                assert!(last <= panel.bottom());
            }
            // The group is centred up and down, to the 4 DIP grid.
            let (top, bottom) = (order[0].y - panel.y, panel.bottom() - last);
            assert!(
                (bottom - top).abs() <= 2.0 * GRID,
                "{top} above, {bottom} below"
            );
            for (k, r) in order.iter().enumerate() {
                assert_grid(r, "empty state part");
                assert!(r.x >= panel.x && r.right() <= panel.right(), "{k}: {r:?}");
                assert!(r.y >= panel.y && r.bottom() <= panel.bottom(), "{k}: {r:?}");
                if k > 0 {
                    assert!(r.y >= order[k - 1].bottom(), "{k} overlaps the part above");
                }
            }
            // The button sits centred under the instruction.
            let mid = e.button.x + e.button.w / 2.0;
            assert!((mid - (panel.x + panel.w / 2.0)).abs() <= GRID);
            assert_eq!(e.button.h, BUTTON_HEIGHT);
        }
    }

    #[test]
    fn the_settings_tab_strip_is_one_tab_stop() {
        let s = settings(480.0, 360.0, SettingsShape::default());
        let controls: Vec<Control> = s.controls.iter().map(|(c, _)| *c).collect();
        let on = vec![true; controls.len()];
        let tabs: Vec<usize> = (0..controls.len())
            .filter(|&i| matches!(controls[i], Control::Tab(_)))
            .collect();
        assert_eq!(tabs.len(), 3);
        let first_after = tabs[2] + 1;
        // Tab from any tab leaves the strip; Shift+Tab from the first
        // control after it lands on the page showing, not the last tab.
        for &t in &tabs {
            assert_eq!(
                settings_tab_stop(&controls, &on, SettingsTab::Quality, Some(t), false),
                Some(first_after)
            );
        }
        assert_eq!(
            settings_tab_stop(
                &controls,
                &on,
                SettingsTab::Quality,
                Some(first_after),
                true
            ),
            Some(tabs[1])
        );
        // Going round from the last control reaches the strip once.
        assert_eq!(
            settings_tab_stop(
                &controls,
                &on,
                SettingsTab::General,
                Some(controls.len() - 1),
                false
            ),
            Some(tabs[0])
        );
        // Disabled controls are passed over.
        let mut some_off = on.clone();
        some_off[first_after] = false;
        assert_eq!(
            settings_tab_stop(
                &controls,
                &some_off,
                SettingsTab::General,
                Some(tabs[0]),
                false
            ),
            Some(first_after + 1)
        );
    }

    #[test]
    fn a_wide_window_keeps_its_content_in_a_readable_column() {
        for rows in [0, 3] {
            // At the default size nothing changes.
            let narrow = layout(720.0, 480.0, rows);
            assert_eq!(
                (narrow.body.x, narrow.body.w),
                (PANEL_PADDING, 720.0 - 2.0 * PANEL_PADDING)
            );
            // On an ultrawide the column is capped and centred, with the
            // header's buttons at its right end and the footer's links
            // within reach of the names.
            // The inspector's column is wider, so the list beside it keeps
            // about the width the column had without one.
            let wide = layout(3000.0, 1200.0, rows);
            let cap = if rows > 0 {
                MAX_CONTENT_WIDTH_WIDE
            } else {
                MAX_CONTENT_WIDTH
            };
            assert_eq!(wide.body.w, cap);
            assert!((wide.body.x + wide.body.w / 2.0 - 1500.0).abs() <= GRID / 2.0);
            assert_eq!(wide.settings_button.right(), wide.body.right());
            assert_eq!(wide.icon.x, wide.body.x);
            if rows > 0 {
                let row = wide.rows[0];
                assert_eq!((row.x, row.w), (wide.body.x, wide.list.w));
                assert!(wide.list.w >= MAX_CONTENT_WIDTH - 1.0);
                assert!(wide.footer.unwrap().right() <= wide.body.right());
                // The row's links sit within the column.
                assert!(row_link(&row).right() <= wide.body.right());
            }
        }
    }

    #[test]
    fn the_inspector_shows_from_760_and_takes_300() {
        let narrow = layout(759.0, 560.0, 3);
        assert!(narrow.inspector.is_none());
        assert_eq!(narrow.list, narrow.panel);
        let wide = layout(760.0, 560.0, 3);
        let i = wide.inspector.unwrap();
        assert_eq!(i.w, INSPECTOR_WIDTH);
        assert_eq!(i.right(), wide.panel.right());
        assert_eq!(wide.list.w + 1.0 + i.w, wide.panel.w);
        assert_eq!(wide.rows[0].w, wide.list.w);
    }

    #[test]
    fn a_wide_window_caps_the_content_at_1180() {
        let l = layout(2000.0, 900.0, 3);
        assert_eq!(l.body.w, MAX_CONTENT_WIDTH_WIDE);
        assert!(l.list.w >= 879.0);
    }

    #[test]
    fn half_shown_slides_from_the_right() {
        let l = layout_with(1000.0, 600.0, 3, 0.5);
        assert_eq!(l.inspector.unwrap().w, INSPECTOR_WIDTH * 0.5);
    }

    #[test]
    fn the_column_widens_as_the_inspector_slides_in() {
        // The cap with files is 880 plus what is shown of the inspector, so
        // half in it is 1030 in a window wide enough for that.
        let half = layout_with(2000.0, 900.0, 3, 0.5);
        assert_eq!(half.body.w, 1030.0);
        assert_eq!(layout_with(2000.0, 900.0, 3, 0.0).body.w, MAX_CONTENT_WIDTH);
        assert_eq!(
            layout_with(2000.0, 900.0, 3, 1.0).body.w,
            MAX_CONTENT_WIDTH_WIDE
        );
        // The list keeps its width while the inspector slides in, give or
        // take the rule.
        for i in 0..=20 {
            let l = layout_with(2000.0, 900.0, 3, i as f32 * 0.05);
            assert!((l.list.w - 879.5).abs() <= 0.6, "{i}: {}", l.list.w);
        }
    }

    #[test]
    fn nothing_jumps_while_the_inspector_slides_in() {
        // Across the slide in steps of 0.05, the column, the header's
        // buttons, the footer and the rows move no faster than the
        // inspector grows, plus a grid step for snapping the centred
        // column. A step in the column's cap at the start would be a jump
        // of 300 DIPs.
        let most = INSPECTOR_WIDTH * 0.05 + GRID;
        for w in [1000.0, 1300.0, 2000.0, 3000.0] {
            let at = |i: usize| layout_with(w, 600.0, 3, i as f32 * 0.05);
            let numbers = |l: &Layout| {
                [
                    l.body.x,
                    l.body.w,
                    l.body.right(),
                    l.icon.x,
                    l.settings_button.x,
                    l.help_button.x,
                    l.footer.unwrap().right(),
                    l.clear_button.unwrap().x,
                    l.list.w,
                    l.rows[0].w,
                    l.inspector.map_or(l.panel.right(), |i| i.x),
                ]
            };
            for i in 0..20 {
                let (a, b) = (numbers(&at(i)), numbers(&at(i + 1)));
                for (k, (x, y)) in a.iter().zip(&b).enumerate() {
                    assert!(
                        (x - y).abs() <= most,
                        "{w}: value {k} jumps {x} to {y} at {i}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_empty_window_has_no_inspector() {
        assert!(layout(1000.0, 600.0, 0).inspector.is_none());
        // Whatever the window is shown, no files means no inspector and
        // the column stays as narrow as the empty window has always had it.
        let l = layout_with(2000.0, 900.0, 0, 1.0);
        assert!(l.inspector.is_none());
        assert_eq!((l.list, l.body.w), (l.panel, MAX_CONTENT_WIDTH));
    }

    #[test]
    fn the_inspector_slides_in_from_the_panels_right_edge() {
        for shown in [0.25, 0.5, 0.75, 1.0] {
            let l = layout_with(1000.0, 600.0, 3, shown);
            let i = l.inspector.unwrap();
            assert_eq!(i.right(), l.panel.right());
            assert_eq!((i.y, i.h), (l.panel.y, l.panel.h));
            // The list gives up what the inspector takes, and the rule.
            assert_eq!(l.list.right() + INSPECTOR_RULE, i.x);
            assert_eq!(l.list.x, l.panel.x);
        }
        // Folded away it leaves the list whole and the column as narrow
        // as it was; shown too far in either direction is kept in range.
        let folded = layout_with(2000.0, 900.0, 3, 0.0);
        assert!(folded.inspector.is_none());
        assert_eq!(
            (folded.list, folded.body.w),
            (folded.panel, MAX_CONTENT_WIDTH)
        );
        assert_eq!(folded.rows[0].w, folded.panel.w);
        let w = |shown| layout_with(1000.0, 600.0, 3, shown).inspector.map(|i| i.w);
        assert_eq!(w(2.0), Some(INSPECTOR_WIDTH));
        assert_eq!(w(-1.0), None);
        assert_eq!(w(f32::NAN), None);
    }

    #[test]
    fn a_click_in_the_inspector_is_no_click_on_a_row() {
        let l = layout(1000.0, 600.0, 3);
        let i = l.inspector.unwrap();
        let y = l.panel.y + ROW_HEIGHT / 2.0;
        assert_eq!(row_at(&l, l.list.x + 10.0, y, 0.0), Some(0));
        assert_eq!(row_at(&l, i.x + 10.0, y, 0.0), None);
        assert_eq!(
            row_at(&l, l.list.right() + INSPECTOR_RULE / 2.0, y, 0.0),
            None
        );
    }

    #[test]
    fn the_row_preview_sits_at_the_left_on_the_grid() {
        let row = Rect::new(16.0, 72.0, 600.0, ROW_HEIGHT);
        let p = row_preview(&row);
        assert_eq!((p.x, p.w, p.h), (28.0, PREVIEW_ROW, PREVIEW_ROW));
        assert_eq!(p.y - row.y, row.bottom() - p.bottom());
    }

    #[test]
    fn the_row_text_starts_clear_of_the_preview() {
        let row = Rect::new(16.0, 72.0, 600.0, ROW_HEIGHT);
        let p = row_preview(&row);
        // The chip and the text start 12 DIPs after the preview.
        assert_eq!(row_text_left(&row), p.right() + 12.0);
        assert_eq!(row_text_left(&row), row.x + 58.0);
        // The saved line starts there too, and keeps its y, its height and
        // its right edge, 16 DIPs short of the row's.
        let line = row_line(&row);
        assert_eq!(line.x, row_text_left(&row));
        assert_eq!((line.y, line.h), (row.y + 28.0, 20.0));
        assert_eq!(line.right(), row.right() - 16.0);
        assert!(apart(&line, &p));
        // So a click on the preview is a click on the row, not the link.
        let on_preview = (p.x + p.w / 2.0, p.y + p.h / 2.0);
        assert!(row.contains(on_preview.0, on_preview.1));
        assert_eq!(
            row_part(&row, on_preview.0, on_preview.1, false, false, true),
            RowPart::Row
        );
        let on_line = (line.x + line.w / 2.0, line.y + line.h / 2.0);
        assert_eq!(
            row_part(&row, on_line.0, on_line.1, false, false, true),
            RowPart::Link
        );
        // A row too narrow to hold it has no line, rather than a negative one.
        assert_eq!(row_line(&Rect::new(0.0, 0.0, 60.0, ROW_HEIGHT)).w, 0.0);
    }

    #[test]
    fn a_short_inspector_drops_what_does_not_fit_whole() {
        let r = Rect::new(900.0, 72.0, 300.0, 220.0);
        let l = inspector_layout(
            &r,
            &InspectorShape::Waiting {
                what_h: 40.0,
                facts: 3,
                targets: 3,
                more: true,
            },
        );
        let inner = r.inset(16.0);
        for t in l.targets.iter().chain(l.more.iter()).chain(l.facts.iter()) {
            assert!(t.y >= inner.y && t.bottom() <= inner.bottom());
        }
        assert!(l.targets.len() < 3);
        assert!(l.more.is_none());
    }

    /// Every shape the inspector can take, with the most parts it has.
    fn every_shape() -> [InspectorShape; 7] {
        [
            InspectorShape::Nothing,
            InspectorShape::Waiting {
                what_h: 40.0,
                facts: 4,
                targets: 3,
                more: true,
            },
            InspectorShape::Converting { buttons: 1 },
            InspectorShape::Done {
                change: true,
                drag: true,
                buttons: 1,
            },
            InspectorShape::Failed {
                sentence_h: 60.0,
                buttons: 2,
            },
            InspectorShape::Several {
                targets: 3,
                none_h: 0.0,
            },
            InspectorShape::Several {
                targets: 0,
                none_h: 80.0,
            },
        ]
    }

    /// What a shape asks for: its facts, targets, whether More, its buttons.
    fn asks_for(shape: &InspectorShape) -> (usize, usize, bool, usize) {
        match *shape {
            InspectorShape::Nothing => (0, 0, false, 0),
            InspectorShape::Waiting {
                facts,
                targets,
                more,
                ..
            } => (facts, targets, more, 0),
            InspectorShape::Several { targets, .. } => (0, targets, false, 0),
            InspectorShape::Converting { buttons }
            | InspectorShape::Done { buttons, .. }
            | InspectorShape::Failed { buttons, .. } => (0, 0, false, buttons),
        }
    }

    /// The parts of an inspector layout that stand, in reading order, with
    /// every button slot.
    fn every_rect(l: &InspectorLayout) -> Vec<(String, Rect)> {
        let mut parts = vec![
            ("hero".to_string(), l.hero),
            ("title".to_string(), l.title),
            ("body".to_string(), l.body),
        ];
        let numbered = |name: &str, rs: &[Rect]| -> Vec<(String, Rect)> {
            rs.iter()
                .enumerate()
                .map(|(i, r)| (format!("{name} {i}"), *r))
                .collect()
        };
        parts.extend(numbered("fact", &l.facts));
        parts.extend(l.label.map(|r| ("label".to_string(), r)));
        parts.extend(numbered("target", &l.targets));
        parts.extend(l.more.map(|r| ("more".to_string(), r)));
        parts.extend(numbered("button", &l.buttons));
        parts.retain(|(_, r)| r.h > 0.0);
        parts
    }

    /// The same, one to a line: a row of buttons stands as its first.
    fn standing(l: &InspectorLayout) -> Vec<(String, Rect)> {
        let mut parts = every_rect(l);
        parts.retain(|(name, _)| name != "button 1" && name != "button 2");
        parts
    }

    #[test]
    fn the_inspector_stacks_its_parts_with_the_gaps_it_is_given() {
        let r = Rect::new(900.0, 72.0, INSPECTOR_WIDTH, 900.0);
        let inner = r.inset(16.0);
        let waiting = inspector_layout(
            &r,
            &InspectorShape::Waiting {
                what_h: 40.0,
                facts: 3,
                targets: 3,
                more: true,
            },
        );
        // The hero, then 12, the title, 8, the body, 12, the facts one
        // under the next, 12, the label, 6, the targets 6 apart, 6, More.
        assert_eq!(waiting.hero, Rect::new(inner.x, inner.y, HERO.0, HERO.1));
        assert_eq!(waiting.title.y, waiting.hero.bottom() + 12.0);
        assert_eq!(waiting.title.h, 24.0);
        assert_eq!(waiting.body.y, waiting.title.bottom() + 8.0);
        assert_eq!(waiting.body.h, 40.0);
        assert_eq!(waiting.facts.len(), 3);
        assert_eq!(waiting.facts[0].y, waiting.body.bottom() + 12.0);
        for pair in waiting.facts.windows(2) {
            assert_eq!(pair[1].y, pair[0].bottom());
        }
        assert!(waiting.facts.iter().all(|f| f.h == FACT_HEIGHT));
        let label = waiting.label.unwrap();
        assert_eq!(label.y, waiting.facts[2].bottom() + 12.0);
        assert_eq!(label.h, 16.0);
        assert_eq!(waiting.targets.len(), 3);
        assert_eq!(waiting.targets[0].y, label.bottom() + 6.0);
        for pair in waiting.targets.windows(2) {
            assert_eq!(pair[1].y, pair[0].bottom() + 6.0);
        }
        assert!(waiting.targets.iter().all(|t| t.h == TARGET_HEIGHT));
        let more = waiting.more.unwrap();
        assert_eq!(more.y, waiting.targets[2].bottom() + 6.0);
        assert_eq!(more.h, 24.0);
        assert!(waiting.buttons.is_empty());
        // Every part is as wide as the padding leaves.
        for (name, p) in standing(&waiting) {
            assert_eq!((p.x, p.w), (inner.x, inner.w), "{name}");
        }

        // A running job: the percentage and the time left share the body,
        // and Cancel follows, 12 below.
        let running = inspector_layout(&r, &InspectorShape::Converting { buttons: 1 });
        assert_eq!(running.body.h, 40.0 + 20.0);
        assert!(running.facts.is_empty() && running.label.is_none());
        assert_eq!(running.buttons.len(), 1);
        let cancel = running.buttons[0];
        assert_eq!(cancel.y, running.body.bottom() + 12.0);
        assert_eq!((cancel.w, cancel.h), (96.0, BUTTON_HEIGHT));

        // A failure has a slot for each button it has, 8 apart, left to
        // right, and none for one it hasn't.
        let failed = |buttons| {
            inspector_layout(
                &r,
                &InspectorShape::Failed {
                    sentence_h: 60.0,
                    buttons,
                },
            )
        };
        let two = failed(2);
        assert_eq!(two.body.h, 60.0);
        assert_eq!(two.buttons.len(), 2);
        assert_eq!(two.buttons[0].y, two.body.bottom() + 12.0);
        assert_eq!(two.buttons[1].x, two.buttons[0].right() + 8.0);
        assert_eq!(two.buttons[1].y, two.buttons[0].y);
        assert_eq!(failed(1).buttons.len(), 1);
        assert!(failed(0).buttons.is_empty());

        // A finished job's body holds a 20 DIP line for the result, one for
        // the change and one for the drag-out hint, when it has them.
        let done = |change, drag, buttons| {
            inspector_layout(
                &r,
                &InspectorShape::Done {
                    change,
                    drag,
                    buttons,
                },
            )
        };
        assert_eq!(done(false, false, 1).body.h, 20.0);
        assert_eq!(done(true, false, 1).body.h, 40.0);
        assert_eq!(done(false, true, 1).body.h, 40.0);
        assert_eq!(done(true, true, 1).body.h, 60.0);
        assert_eq!(done(true, true, 1).buttons.len(), 1);
        assert!(done(true, true, 0).buttons.is_empty());
        let full = done(true, true, 1);
        assert_eq!(full.buttons[0].y, full.body.bottom() + 12.0);

        // Several files list their shared formats, or when they share
        // none, the sentence and the kinds on offer: both as target rows.
        let shared = inspector_layout(
            &r,
            &InspectorShape::Several {
                targets: 3,
                none_h: 0.0,
            },
        );
        assert_eq!(shared.body.h, 0.0);
        assert!(shared.label.is_some() && shared.more.is_none());
        assert_eq!(shared.targets.len(), 3);
        let none = inspector_layout(
            &r,
            &InspectorShape::Several {
                targets: 2,
                none_h: 80.0,
            },
        );
        assert_eq!(none.body.h, 80.0);
        assert_eq!(none.targets.len(), 2);
        assert!(none.targets.iter().all(|t| t.h == TARGET_HEIGHT));
        let no_list = inspector_layout(
            &r,
            &InspectorShape::Several {
                targets: 0,
                none_h: 80.0,
            },
        );
        assert!(no_list.label.is_none() && no_list.targets.is_empty());

        // Nothing selected: the mark's hero, a title and two lines of
        // body, and no facts, targets or buttons.
        let nothing = inspector_layout(&r, &InspectorShape::Nothing);
        assert_eq!((nothing.hero.w, nothing.hero.h), HERO);
        assert!(nothing.hero.w >= 48.0 && nothing.hero.h >= 48.0);
        assert_eq!((nothing.title.h, nothing.body.h), (24.0, 40.0));
        assert!(nothing.facts.is_empty() && nothing.label.is_none());
        assert!(nothing.targets.is_empty() && nothing.more.is_none());
        assert!(nothing.buttons.is_empty());
    }

    #[test]
    fn a_short_inspector_gives_up_its_picture_and_facts_before_its_actions() {
        let waiting = InspectorShape::Waiting {
            what_h: 40.0,
            facts: 3,
            targets: 3,
            more: true,
        };
        // A window 800 x 480 gives the inspector 300 x 348: too short for
        // the picture and the facts with the targets. The targets and More
        // are the point of it, so they stay and the picture and facts go.
        let r = layout(800.0, 480.0, 3).inspector.unwrap();
        assert_eq!((r.w, r.h), (300.0, 348.0));
        let l = inspector_layout(&r, &waiting);
        assert_eq!(l.hero.h, 0.0);
        assert!(l.facts.is_empty());
        assert_eq!(l.targets.len(), 3);
        assert!(l.more.is_some());
        assert!(l.label.is_some());
        // Every part is in the padding, and the title now leads.
        let inner = r.inset(16.0);
        assert_eq!(l.title.y, inner.y);
        for (name, p) in every_rect(&l) {
            assert!(p.y >= inner.y && p.bottom() <= inner.bottom(), "{name}");
        }
        // With room for the facts but not the picture, the facts come back
        // first: the stack with them, no picture, is 322 DIPs of padding.
        let at = |h| inspector_layout(&Rect::new(r.x, r.y, 300.0, h), &waiting);
        let facts = at(322.0 + 32.0);
        assert_eq!((facts.hero.h, facts.facts.len()), (0.0, 3));
        assert_eq!((facts.targets.len(), facts.more.is_some()), (3, true));
        assert_eq!(at(321.0 + 32.0).facts.len(), 0);
        // With room for everything, the picture comes back, last.
        let whole = at(454.0 + 32.0);
        assert_eq!((whole.hero.h, whole.facts.len()), (HERO.1, 3));
        assert_eq!((whole.targets.len(), whole.more.is_some()), (3, true));
        assert_eq!(at(453.0 + 32.0).hero.h, 0.0);
        // The same goes for a job's buttons: a short inspector loses the
        // picture before Cancel.
        let running = InspectorShape::Converting { buttons: 1 };
        let cancel = |h| {
            let l = inspector_layout(&Rect::new(0.0, 0.0, 300.0, h), &running);
            (l.hero.h, l.buttons.len())
        };
        assert_eq!(cancel(200.0), (0.0, 1));
        assert_eq!(cancel(500.0), (HERO.1, 1));
    }

    #[test]
    fn a_shorter_inspector_never_overflows_and_cuts_actions_only_from_the_bottom() {
        for shape in every_shape() {
            let (asked_facts, asked_targets, asked_more, asked_buttons) = asks_for(&shape);
            let mut actions = 0;
            for h in (0..=900).step_by(4) {
                let r = Rect::new(900.0, 72.0, INSPECTOR_WIDTH, h as f32);
                let inner = r.inset(16.0);
                let l = inspector_layout(&r, &shape);
                let parts = standing(&l);
                for (i, (name, p)) in parts.iter().enumerate() {
                    assert!(
                        p.y >= inner.y && p.bottom() <= inner.bottom(),
                        "{shape:?} at {h}: {name} {p:?} leaves {inner:?}"
                    );
                    assert!(p.x >= inner.x && p.right() <= inner.right());
                    if i > 0 {
                        assert!(p.y >= parts[i - 1].1.bottom(), "{shape:?}: {name}");
                    }
                }
                // Taller never shows fewer actions.
                let now = l.targets.len() + usize::from(l.more.is_some()) + l.buttons.len();
                assert!(
                    now >= actions,
                    "{shape:?} at {h}: {now} actions after {actions}"
                );
                actions = now;
                // The picture shows only with every part the shape asks for.
                if l.hero.h > 0.0 {
                    assert_eq!(l.hero.h, HERO.1);
                    assert_eq!(l.facts.len(), asked_facts, "{shape:?} at {h}");
                    assert_eq!(l.targets.len(), asked_targets, "{shape:?} at {h}");
                    assert_eq!(l.more.is_some(), asked_more, "{shape:?} at {h}");
                    assert_eq!(l.buttons.len(), asked_buttons, "{shape:?} at {h}");
                }
                // The facts go before any action is cut.
                if !l.facts.is_empty() {
                    assert_eq!(l.targets.len(), asked_targets, "{shape:?} at {h}");
                    assert_eq!(l.buttons.len(), asked_buttons, "{shape:?} at {h}");
                }
                // A part left out takes the parts after it.
                if l.title.h == 0.0 {
                    assert!(parts.is_empty(), "{shape:?} at {h}");
                }
                if l.targets.len() < asked_targets {
                    assert!(l.more.is_none() && l.buttons.is_empty());
                }
                if l.targets.is_empty() {
                    assert!(l.label.is_none(), "{shape:?} at {h}: a label over nothing");
                }
            }
        }
    }

    #[test]
    fn a_narrow_rect_never_places_a_control_outside_it() {
        // The layout is meant for the full 300 DIPs, but a narrower rect
        // must still not place a part, a button slot included, that
        // reaches past its padding, or leaves a button without a slot's room.
        for w in [150.0, 100.0] {
            for shape in every_shape() {
                for h in (0..=900).step_by(4) {
                    let r = Rect::new(900.0, 72.0, w, h as f32);
                    let inner = r.inset(16.0);
                    let l = inspector_layout(&r, &shape);
                    for (name, p) in every_rect(&l) {
                        assert!(
                            p.x >= inner.x && p.right() <= inner.right(),
                            "{shape:?} at {w}x{h}: {name} {p:?} leaves {inner:?} across"
                        );
                        assert!(
                            p.y >= inner.y && p.bottom() <= inner.bottom(),
                            "{shape:?} at {w}x{h}: {name} {p:?} leaves {inner:?} down"
                        );
                    }
                    for pair in l.buttons.windows(2) {
                        assert!(apart(&pair[0], &pair[1]));
                    }
                    assert!(l.buttons.len() <= asks_for(&shape).3);
                }
            }
        }
        // 118 DIPs of padding hold one 96 button but not a second; 68 hold none.
        let two = InspectorShape::Failed {
            sentence_h: 20.0,
            buttons: 2,
        };
        let tall = |w| {
            inspector_layout(&Rect::new(0.0, 0.0, w, 600.0), &two)
                .buttons
                .len()
        };
        assert_eq!((tall(300.0), tall(150.0), tall(100.0)), (2, 1, 0));
        // At the inspector's own width every button has its slot.
        for buttons in 0..=2 {
            for shape in [
                InspectorShape::Converting { buttons },
                InspectorShape::Done {
                    change: true,
                    drag: true,
                    buttons,
                },
                InspectorShape::Failed {
                    sentence_h: 60.0,
                    buttons,
                },
            ] {
                let r = Rect::new(0.0, 0.0, INSPECTOR_WIDTH, 600.0);
                assert_eq!(inspector_layout(&r, &shape).buttons.len(), buttons);
            }
        }
    }

    #[test]
    fn a_roomy_window_holds_every_part_of_the_inspector() {
        let r = layout(1600.0, 900.0, 3).inspector.unwrap();
        let waiting = inspector_layout(&r, &every_shape()[1]);
        assert_eq!(
            (waiting.facts.len(), waiting.targets.len()),
            (4, 3),
            "{waiting:?}"
        );
        assert!(waiting.more.is_some());
        assert!(waiting.hero.h > 0.0);
        assert_eq!(inspector_layout(&r, &every_shape()[4]).buttons.len(), 2);
    }

    #[test]
    fn the_empty_window_shows_four_glaze_tiles_centred() {
        let e = empty_state(&Rect::new(16.0, 72.0, 688.0, 380.0));
        let glazes = e.glazes.unwrap();
        let span = glazes[3].right() - glazes[0].x;
        assert!((glazes[0].x - 16.0 - (688.0 - span) / 2.0).abs() < 1.0);
        assert_eq!(e.mark.w, 72.0);
    }

    #[test]
    fn the_glaze_tiles_give_way_in_a_short_panel() {
        // The smallest window has no room for them, and its group stays
        // whole without them; the default window has.
        let small = empty_state(&layout(480.0, 360.0, 0).panel);
        assert!(small.glazes.is_none());
        assert_eq!(small.mark.w, 72.0);
        assert!(small.privacy.bottom() <= layout(480.0, 360.0, 0).panel.bottom());
        let default = empty_state(&layout(720.0, 480.0, 0).panel);
        assert!(default.glazes.is_some());
        // Just short of the group's height, and just enough for it.
        let panel = |h| Rect::new(16.0, 72.0, 688.0, h);
        assert!(empty_state(&panel(351.0)).glazes.is_none());
        assert!(empty_state(&panel(352.0)).glazes.is_some());
    }

    #[test]
    fn a_page_of_the_list_is_a_screenful_less_one() {
        assert_eq!(rows_per_page(ROW_HEIGHT * 8.0), 7);
        assert_eq!(rows_per_page(ROW_HEIGHT * 1.0), 1);
        assert_eq!(rows_per_page(0.0), 1);
    }

    #[test]
    fn the_key_buttons_never_overlap_at_any_window_size() {
        for (w, h) in [MIN_WINDOW, (720.0, 480.0), (1600.0, 1000.0)] {
            let s = settings(w, h, SettingsShape::default());
            let keys: Vec<Rect> = s
                .controls
                .iter()
                .filter(|(c, _)| matches!(c, Control::Key(_)))
                .map(|(_, r)| *r)
                .collect();
            assert_eq!(keys.len(), 3);
            for pair in keys.windows(2) {
                assert!(apart(&pair[0], &pair[1]), "{pair:?} at {w}x{h}");
            }
            for r in &keys {
                assert!(r.x >= s.panel.x && r.right() <= s.panel.right(), "{r:?}");
                // Wide enough to stack the caption over its key.
                assert!(r.w >= 56.0 && r.h >= 32.0, "{r:?} at {w}x{h}");
            }
        }
    }

    #[test]
    fn settings_pages_hold_their_rows() {
        let general = page(SettingsTab::General);
        let labels: Vec<_> = general.rows.iter().map(|r| r.label).collect();
        assert_eq!(
            labels,
            [
                OUTPUT_LABEL,
                "Theme",
                "Keys to hold while dropping",
                "Suggestions",
                "Save results next to the original"
            ]
        );
        assert_eq!(
            general.rows[0].hint,
            Some("Unset: the last folder you exported to, or the file's own folder.")
        );
        assert!(general.rows[1..].iter().all(|r| r.hint.is_none()));
        assert_eq!(general.controls.len(), 3 + 1 + 3 + 3 + 2 + 1 + 2 + 1 + 1);
        assert_eq!(general.controls[0].0, Control::Tab(SettingsTab::General));
        let keys = general
            .controls
            .iter()
            .rposition(|(c, _)| matches!(c, Control::Key(_)))
            .unwrap();
        let after: Vec<_> = general.controls[keys + 1..keys + 4]
            .iter()
            .map(|(c, _)| *c)
            .collect();
        assert_eq!(
            after,
            [
                Control::Suggestions(true),
                Control::Suggestions(false),
                Control::ForgetPicks
            ]
        );
        for (c, r) in &general.controls {
            let p = general.panel;
            assert!(
                r.x >= p.x && r.right() <= p.right() && r.y >= p.y && r.bottom() <= p.bottom(),
                "{c:?} {r:?}"
            );
        }
        let off = general.controls[keys + 2].1;
        let forget = general.controls[keys + 3].1;
        assert_eq!(forget.w, FORGET_BUTTON_WIDTH);
        assert_eq!(forget.w, 136.0);
        assert_eq!(forget.x, off.right() + 8.0);
        // The fourth row fits the panel height every page keeps.
        assert_eq!(general.panel, page(SettingsTab::Quality).panel);
        let n = general.controls.len();
        assert_eq!(general.controls[n - 2].0, Control::CheckUpdates);
        assert_eq!(general.controls[n - 1].0, Control::Close);

        let quality = page(SettingsTab::Quality);
        let labels: Vec<_> = quality.rows.iter().map(|r| r.label).collect();
        assert_eq!(
            labels,
            [
                "Video",
                "Resolution",
                "Frame rate",
                "Images",
                "Image size",
                "Sharpness (DPI)",
                "Sound quality"
            ]
        );
        assert_eq!(quality.controls.len(), 3 + 3 + 2 + 3 + 2 + 1 + 2);
        // Tab order runs down the page, as the rows read.
        let steps: Vec<_> = quality
            .controls
            .iter()
            .filter_map(|(c, _)| match c {
                Control::Step(s) => Some(*s),
                _ => None,
            })
            .collect();
        assert_eq!(
            steps,
            [
                DefaultSetting::Resolution,
                DefaultSetting::Fps,
                DefaultSetting::ImageSize,
                DefaultSetting::PageDpi,
                DefaultSetting::Audio,
            ]
        );
        // Each stepper's label ends before the stepper starts.
        for (row, (_, r)) in quality
            .rows
            .iter()
            .filter(|r| r.label != "Video" && r.label != "Images")
            .zip(
                quality
                    .controls
                    .iter()
                    .filter(|(c, _)| matches!(c, Control::Step(_))),
            )
        {
            assert!(row.label_rect.right() < r.x, "{}", row.label);
            assert!(row.label_rect.w >= 72.0, "{}", row.label);
        }

        let engines = page(SettingsTab::Engines);
        assert_eq!(engines.rows.len(), 4);
        assert_eq!(engines.rows[3].label, "Calibre");
        assert_eq!(
            engines.rows[0].hint,
            Some("Audio, video, GIF, HEIC and AVIF.")
        );
        assert_eq!(SettingsTab::Engines.label(), "Helper apps");

        // Pages share one panel, so switching never moves it.
        assert_eq!(general.panel, quality.panel);
        assert_eq!(general.panel, engines.panel);
    }

    #[test]
    fn the_sheet_fits_its_knobs() {
        let sets: [&[Knob]; 3] = [
            &[Knob::Preset, Knob::Size, Knob::Fps, Knob::AudioBitrate],
            &[Knob::AudioBitrate],
            &[Knob::Preset, Knob::PageDpi],
        ];
        for (w, h) in [(480.0, 360.0), (720.0, 480.0), (1600.0, 1000.0)] {
            for knobs in sets {
                let s = sheet(w, h, knobs);
                assert_grid(&s.panel, "sheet");
                assert!(s.panel.bottom() <= h && s.panel.right() <= w - 16.0);
                assert_eq!(s.labels.len(), knobs.len());
                // Title, what the target is, what the source is, then the
                // rows, each in its own band and inside the panel.
                assert_eq!(s.target.y, s.title.bottom());
                assert_eq!(s.context.y, s.target.bottom());
                assert!(s.target.h >= 16.0 && s.context.h >= 16.0);
                assert!(s.target.right() <= s.panel.right() - 16.0);
                for l in &s.labels {
                    assert!(l.y >= s.context.bottom(), "a label rides over the context");
                }
                assert!(s.panel.y >= 8.0, "the panel is cut off at the top");
                assert_eq!(s.controls.last().unwrap().0, SheetControl::Convert);
                for (c, r) in &s.controls {
                    assert_grid(r, "sheet control");
                    assert!(r.right() <= s.panel.right() - 16.0, "{c:?}");
                    assert!(r.bottom() <= s.panel.bottom() - 16.0, "{c:?}");
                }
                for (i, (a, ra)) in s.controls.iter().enumerate() {
                    for (b, rb) in &s.controls[i + 1..] {
                        assert!(apart(ra, rb), "{a:?} overlaps {b:?}");
                    }
                    assert!(apart(ra, &s.result), "{a:?} overlaps the result line");
                }
                assert!(s.result.w >= 120.0);
            }
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
