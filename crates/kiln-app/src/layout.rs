//! Where things go, in DIPs. Pure functions, so the 4 px grid and the
//! 16 px panel padding can be tested without a window.

use kiln_core::{Knob, Preset};

use crate::inspector::{Inspector, InspectorControl, NOTHING_TITLE};
use crate::theme::fluent::space::{GRID_UNIT as GRID, PANEL_PADDING};
use crate::tune_sheet::TuneField;

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

/// The title bar's height in layout units at Text size 100%: 48 DIPs
/// (`win::titlebar`). At a larger Text size it is still 48 DIPs, fewer
/// units, and the window lays out under it with `layout_under`.
pub const TITLE_BAR: f32 = 48.0;
pub const ROW_HEIGHT: f32 = 56.0;
pub const FOOTER_HEIGHT: f32 = 64.0;
pub const CHIP_HEIGHT: f32 = 24.0;
pub const CHIP_MIN_WIDTH: f32 = 48.0;
pub const BUTTON_HEIGHT: f32 = 32.0;
pub const BUTTON_WIDTH: f32 = 72.0;
/// Height of the progress line at the bottom of a job row.
pub const PROGRESS_HEIGHT: f32 = 3.0;
/// Smallest window, in DIPs.
pub const MIN_WINDOW: (f32, f32) = (480.0, 360.0);
pub const DEFAULT_WINDOW: (f32, f32) = (880.0, 600.0);

/// The largest Windows Text size (Settings > Accessibility > Text size),
/// as `UISettings.TextScaleFactor` reports it.
pub const MAX_TEXT_SCALE: f32 = 2.25;

/// Windows' Text size, kept to the range Windows allows.
pub fn text_scale(scale: f32) -> f32 {
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
    /// Everything below the title bar, inside the panel padding.
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
    /// The footer across the layer's foot; [`footer_row`] places its parts.
    pub footer: Option<Rect>,
    /// The InfoBar at the top of the list's part of the layer, while one
    /// shows (mock "infobar"): the rows start under it.
    pub top_info: Option<Rect>,
}

/// The gap above the InfoBar at the top of the list, from the panel's
/// edge, and the gap between it and the first row: both 8.
pub const TOP_INFO_GAP: f32 = 8.0;

impl Layout {
    /// Where an InfoBar at the top goes, as wide as it may be: the top of
    /// the list, or in the empty window 16 inside the content layer (the
    /// drop panel's own group moves down under it).
    pub fn top_info_slot(&self) -> Rect {
        if self.rows.is_empty() {
            let pad = crate::theme::fluent::space::CARD_PADDING;
            Rect::new(
                self.body.x + pad,
                self.body.y + pad,
                (self.body.w - 2.0 * pad).max(0.0),
                0.0,
            )
        } else {
            self.list
        }
    }

    /// The layout with an InfoBar `h` tall at the top of the list, the
    /// rows moved down under it; in the empty window, at the top of the
    /// layer, the drop panel under it. With no height it is as it was.
    pub fn with_top_info(mut self, h: f32) -> Layout {
        if h.is_nan() || h <= 0.0 {
            return self;
        }
        if self.rows.is_empty() {
            let slot = Rect::new(
                self.top_info_slot().x,
                self.top_info_slot().y,
                self.top_info_slot().w,
                h,
            );
            let top = slot.bottom().min(self.body.bottom());
            self.panel = Rect::new(
                self.body.x,
                top,
                self.body.w,
                (self.body.bottom() - top).max(0.0),
            );
            self.list = self.panel;
            self.top_info = Some(slot);
            return self;
        }
        // The list's own padding already is part of the gap above the bar.
        let above = TOP_INFO_GAP - LIST_PAD;
        let slot = Rect::new(self.list.x, self.list.y + above, self.list.w, h);
        let down = (above + h + TOP_INFO_GAP).min(self.list.h);
        self.list = Rect::new(
            self.list.x,
            self.list.y + down,
            self.list.w,
            (self.list.h - down).max(0.0),
        );
        for (i, r) in self.rows.iter_mut().enumerate() {
            r.y = self.list.y + i as f32 * ROW_HEIGHT;
        }
        self.top_info = Some(slot);
        self
    }
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
    layout_under(w, h, rows, shown, TITLE_BAR)
}

/// [`layout_with`] under a title bar `top` units tall: 48 DIPs at the
/// window's Text size zoom.
pub fn layout_under(w: f32, h: f32, rows: usize, shown: f32, top: f32) -> Layout {
    let top = if top.is_finite() { top.max(0.0) } else { 0.0 };
    let shown = if shown.is_finite() {
        shown.clamp(0.0, 1.0)
    } else {
        0.0
    };
    // Everything sits on the content layer (mock screens 1 and 2), which
    // fills the window: a wide window's list grows, and so does its pane.
    let layer = content_layer(w, h, top);
    if rows == 0 {
        // The empty window is the content layer itself (mock screen 1).
        return Layout {
            body: layer,
            panel: layer,
            list: layer,
            inspector: None,
            rows: vec![],
            footer: None,
            top_info: None,
        };
    }
    let inspected = shown > 0.0;
    // The footer across the layer's bottom, a rule over it; the list and
    // the side pane above.
    let footer_h = FOOTER_HEIGHT.min(layer.h);
    let footer = Rect::new(layer.x, layer.bottom() - footer_h, layer.w, footer_h);
    // The rule over the footer is its top edge, and the rule before the
    // pane the pane's left edge, so every part stays on the grid.
    let panel = Rect::new(layer.x, layer.y, layer.w, (layer.h - footer_h).max(0.0));
    let (main, inspector) = if inspected {
        let iw = pane_width(layer.w) * shown;
        let inspector = Rect::new(panel.right() - iw, panel.y, iw, panel.h);
        let main = Rect::new(panel.x, panel.y, (panel.w - iw).max(0.0), panel.h);
        (main, Some(inspector))
    } else {
        (panel, None)
    };
    // The rows sit LIST_PAD inside their part of the layer.
    let list = main.inset(LIST_PAD);
    let rows = (0..rows)
        .map(|i| Rect::new(list.x, list.y + i as f32 * ROW_HEIGHT, list.w, ROW_HEIGHT))
        .collect();
    Layout {
        body: layer,
        panel,
        list,
        inspector,
        rows,
        footer: Some(footer),
        top_info: None,
    }
}

/// The list's padding inside its part of the layer (the mock's 4).
pub const LIST_PAD: f32 = 4.0;

/// The footer's padding at its ends (the mock's 16).
pub const FOOTER_PAD: f32 = 16.0;

/// How wide the side pane is on a layer `layer_w` wide: it widens from
/// 320 to 400 as the window grows (look spec, Spacing), about a third of
/// the layer, on the 4-DIP grid. The mock's 1100-wide window has 340.
pub fn pane_width(layer_w: f32) -> f32 {
    let (least, most) = crate::theme::fluent::space::SIDE_PANE;
    ((layer_w * PANE_SHARE / GRID).round() * GRID).clamp(least, most)
}

/// The side pane's share of the layer's width, between its least and
/// most.
const PANE_SHARE: f32 = 0.32;

/// A row's padding at its ends, and the gap between the things at its
/// right end (the mock's 12 and 4).
pub const ROW_PAD: f32 = 12.0;
pub const ROW_GAP: f32 = 4.0;

/// The height of a row's buttons: subtle buttons, centred in the row.
pub const ROW_BUTTON: f32 = 32.0;

/// What a link's slot is taken to be for a label this module has no
/// measure of: generous, so such a link is never cut.
const LINK_UNKNOWN: f32 = 136.0;

/// The width of each link's words in Body, in layout DIPs, as measured by
/// `link_words_are_measured` in the renderer's tests. A link's slot is its
/// button: these words, the button's padding and, for Show in folder, its
/// glyph.
const LINK_WORDS: [(&str, f32); 16] = [
    ("Show in folder", 90.0),
    ("Show details", 78.0),
    ("Save", 29.0),
    ("Choose folder", 87.0),
    ("Convert", 49.0),
    ("Get HEVC extension", 123.0),
    ("Get AV1 extension", 111.0),
    ("Get VP9 extension", 113.0),
    ("Get HEIF extension", 117.0),
    ("Get extension", 85.0),
    ("Get FFmpeg", 76.0),
    ("Get Pandoc", 71.0),
    ("Get LibreOffice", 93.0),
    ("Get Calibre", 69.0),
    ("Get helper", 65.0),
    ("Get Media Feature Pack", 146.0),
];

/// Width of a row's link button saying `label`: sized to its words (and
/// its glyph, for Show in folder), so a long one such as "Get Media Feature
/// Pack" fits and a short one takes no more than it needs. The button is
/// drawn to this width at the slot's right.
pub fn link_width(label: &str) -> f32 {
    let glyph = label == crate::model::SHOW_IN_FOLDER;
    match LINK_WORDS.iter().find(|(l, _)| *l == label) {
        Some((_, w)) => kit::button_width(*w, glyph),
        None => LINK_UNKNOWN,
    }
}

/// The labels `link_width` has a measure of, and the width of their words.
#[cfg(test)]
pub fn link_measures() -> impl Iterator<Item = (&'static str, f32)> {
    LINK_WORDS.iter().copied()
}
/// Height of the link's hit area: the subtle button's.
pub const LINK_HIT: f32 = ROW_BUTTON;

/// Width of the subtle "Cancel" button on a running row.
pub const CANCEL_WIDTH: f32 = 68.0;

/// The running row's percentage before Cancel, right-aligned.
pub const PERCENT_WIDTH: f32 = 40.0;

/// The checkmark or error glyph before a row's buttons.
pub const MARK_WIDTH: f32 = 16.0;

/// A subtle button at the right of `row`, `w` wide, ending at `right`,
/// centred up and down.
fn row_button(row: &Rect, right: f32, w: f32) -> Rect {
    Rect::new(right - w, row.y + (row.h - ROW_BUTTON) / 2.0, w, ROW_BUTTON)
}

/// Where a row's buttons end: its padding in from its right edge.
fn row_end(row: &Rect) -> f32 {
    row.right() - ROW_PAD
}

/// The Cancel button on a running row: a subtle button at its right end
/// (mock screen 5).
pub fn row_cancel(row: &Rect) -> Rect {
    row_button(row, row_end(row), CANCEL_WIDTH)
}

/// Where Cancel's word is drawn: the button itself.
pub fn row_cancel_text(row: &Rect) -> Rect {
    row_cancel(row)
}

/// A running row's percentage, just before Cancel.
pub fn row_percent(row: &Rect) -> Rect {
    let cancel = row_cancel(row);
    Rect::new(
        cancel.x - ROW_GAP - PERCENT_WIDTH,
        cancel.y,
        PERCENT_WIDTH,
        cancel.h,
    )
}

/// Where a click hits the row's link: its button at the right end.
pub fn row_link(row: &Rect, label: &str) -> Rect {
    row_button(row, row_end(row), link_width(label))
}

/// Where the link's words are drawn: its button.
pub fn row_link_text(row: &Rect, label: &str) -> Rect {
    row_link_text_at(row, label, false)
}

/// [`row_link`] on a row that may also have Options (`options`): Options
/// then comes at the right end, and the link sits before it.
pub fn row_link_at(row: &Rect, label: &str, options: bool) -> Rect {
    if options {
        row_button(row, row_options(row).x - ROW_GAP, link_width(label))
    } else {
        row_link(row, label)
    }
}

/// [`row_link_text`] on a row that may also have Options.
pub fn row_link_text_at(row: &Rect, label: &str, options: bool) -> Rect {
    row_link_at(row, label, options)
}

/// The gap between two of a row's buttons, such as Retry and Show details.
pub const LINK_GAP: f32 = ROW_GAP;

/// Width of the subtle "Retry" button, its glyph and its word.
pub const RETRY_WIDTH: f32 = 84.0;

/// Where a click hits a row's Retry. `link` is the row's link, when it
/// has one: Retry then sits before it. Alone it takes the right end.
pub fn row_retry(row: &Rect, link: Option<&str>) -> Rect {
    let right = match link {
        Some(label) => row_link(row, label).x - ROW_GAP,
        None => row_end(row),
    };
    row_button(row, right, RETRY_WIDTH)
}

/// Where Retry is drawn: its button.
pub fn row_retry_text(row: &Rect, link: Option<&str>) -> Rect {
    row_retry(row, link)
}

/// Width of a finished drag-wheel conversion's "Options" button.
pub const OPTIONS_WIDTH: f32 = 72.0;

/// Where a click hits a row's Options: at the right end, the link before
/// it ([`row_link_at`]).
pub fn row_options(row: &Rect) -> Rect {
    row_button(row, row_end(row), OPTIONS_WIDTH)
}

/// Where Options is drawn: its button.
pub fn row_options_text(row: &Rect) -> Rect {
    row_options(row)
}

/// The checkmark or error glyph just before `left`, where what sits at a
/// row's right end starts, centred up and down.
pub fn row_mark(row: &Rect, left: f32) -> Rect {
    Rect::new(
        left - ROW_GAP - MARK_WIDTH,
        row.y + (row.h - MARK_WIDTH) / 2.0,
        MARK_WIDTH,
        MARK_WIDTH,
    )
}

/// Where what sits at a row's right end starts: Cancel and the percentage
/// on a running row (`running`), Retry (`retry`), the link (`link`) and
/// Options (`options`). The row's right padding when it has none.
pub fn row_right_start(
    row: &Rect,
    running: bool,
    retry: bool,
    link: Option<&str>,
    options: bool,
) -> f32 {
    if running {
        row_percent(row).x
    } else if retry {
        row_retry(row, link).x
    } else if let Some(label) = link {
        row_link_at(row, label, options).x
    } else if options {
        row_options(row).x
    } else {
        row_end(row)
    }
}

/// Where a row's name and its second line end. A row with a `mark` (the
/// checkmark or the error glyph) keeps the mark's room, and a gap, short
/// of what sits at its right end ([`row_right_start`]); one without keeps
/// only a gap short of a button, and none where nothing is: the
/// padding is already inside the row. Both lines end at one x: the
/// buttons are centred in the row.
pub fn row_text_ends(
    row: &Rect,
    running: bool,
    retry: bool,
    link: Option<&str>,
    options: bool,
    mark: bool,
) -> (f32, f32) {
    let start = row_right_start(row, running, retry, link, options);
    let something = running || retry || link.is_some() || options;
    let end = if mark {
        start - ROW_GAP - MARK_WIDTH - ROW_PAD
    } else if something {
        start - ROW_PAD
    } else {
        start
    };
    (end, end)
}

/// What a click on a row hits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowPart {
    /// The row itself: it selects.
    Row,
    /// A running row's Cancel.
    Cancel,
    /// The row's link.
    Link,
    /// A failed or cancelled row's Retry.
    Retry,
    /// A finished drag-wheel conversion's Options.
    Options,
}

/// The part of `row` under `(x, y)`, a point inside it. `running` says it
/// has Cancel; `link` says it has a link. (`saved` no longer makes a
/// target of its own: a saved row's Show in folder is its link.) Each
/// button's area is a target of its own, taken out of the row's.
pub fn row_part(
    row: &Rect,
    x: f32,
    y: f32,
    running: bool,
    link: Option<&str>,
    saved: bool,
) -> RowPart {
    row_part_with_retry(row, x, y, running, link, saved, false)
}

/// [`row_part`] for a row that may also have a Retry (`retry`), which
/// sits beside the link when there is one.
pub fn row_part_with_retry(
    row: &Rect,
    x: f32,
    y: f32,
    running: bool,
    link: Option<&str>,
    saved: bool,
    retry: bool,
) -> RowPart {
    row_part_full(row, x, y, running, link, saved, retry, false)
}

/// [`row_part_with_retry`] for a row that may also have Options
/// (`options`), which comes at the right end, the link before it.
#[allow(clippy::too_many_arguments)]
pub fn row_part_full(
    row: &Rect,
    x: f32,
    y: f32,
    running: bool,
    link: Option<&str>,
    _saved: bool,
    retry: bool,
    options: bool,
) -> RowPart {
    if running && row_cancel(row).contains(x, y) {
        RowPart::Cancel
    } else if retry && row_retry(row, link).contains(x, y) {
        RowPart::Retry
    } else if options && row_options(row).contains(x, y) {
        RowPart::Options
    } else if link.is_some_and(|l| row_link_at(row, l, options).contains(x, y)) {
        RowPart::Link
    } else {
        RowPart::Row
    }
}

/// Where a row's text starts: [`PREVIEW_GAP`] after its thumbnail.
pub fn row_text_left(row: &Rect) -> f32 {
    row_preview(row).right() + PREVIEW_GAP
}

/// A row's name: Body, 20 tall. With a progress bar under the facts
/// (`bar`) the text sits higher, so the three stay centred together.
pub fn row_name(row: &Rect, bar: bool) -> Rect {
    let left = row_text_left(row);
    Rect::new(
        left,
        row.y + row_text_top(bar),
        (row_end(row) - left).max(0.0),
        20.0,
    )
}

/// How far down a row its name starts: the name (20), the facts (16) and,
/// with `bar`, the gap and the bar, centred in [`ROW_HEIGHT`].
fn row_text_top(bar: bool) -> f32 {
    let block = 20.0
        + 16.0
        + if bar {
            ROW_BAR_GAP + PROGRESS_HEIGHT
        } else {
            0.0
        };
    ((ROW_HEIGHT - block) / 2.0).floor()
}

/// The gap between a running row's facts and its progress bar (the
/// mock's 6).
pub const ROW_BAR_GAP: f32 = 6.0;

/// A row's second line, the facts: Caption, 16 tall, under the name. It
/// runs from [`row_text_left`] to the row's right padding.
pub fn row_line(row: &Rect) -> Rect {
    row_line_at(row, false)
}

/// [`row_line`] on a row that may have a progress bar under it (`bar`).
pub fn row_line_at(row: &Rect, bar: bool) -> Rect {
    let n = row_name(row, bar);
    Rect::new(n.x, n.bottom(), n.w, 16.0)
}

/// A running row's progress bar under its facts, `end` the x its text
/// runs to: as wide as that, so it runs on to the percentage.
pub fn row_bar(row: &Rect, end: f32) -> Rect {
    let line = row_line_at(row, true);
    Rect::new(
        line.x,
        line.bottom() + ROW_BAR_GAP,
        (end - line.x).max(0.0),
        PROGRESS_HEIGHT,
    )
}

/// A row's thumbnail: [`PREVIEW_ROW`] square, the row's padding in from
/// its left edge and centred up and down.
pub fn row_preview(row: &Rect) -> Rect {
    Rect::new(
        row.x + ROW_PAD,
        row.y + (row.h - PREVIEW_ROW) / 2.0,
        PREVIEW_ROW,
        PREVIEW_ROW,
    )
}

/// Width of the footer's Add files button: its glyph and its words.
pub const ADD_FILES_WIDTH: f32 = 112.0;

/// Width of the footer's Add files button with only its glyph, where the
/// footer has no room for its words.
pub const ADD_FILES_GLYPH_WIDTH: f32 = 40.0;

/// Width of the footer's Clear list button.
pub const CLEAR_WIDTH: f32 = 96.0;

/// Width of the footer's Convert (the accent): for one file, "Convert"
/// (or "Cancel" while the selection runs), and for several, "Convert 120
/// files".
pub const CONVERT_ONE: f32 = 96.0;
pub const CONVERT_MANY: f32 = 148.0;

/// The footer's buttons are this far apart, and the status ends this far
/// short of the first.
const FOOTER_GAP: f32 = 8.0;
const STATUS_GAP: f32 = 12.0;

/// The status keeps at least this much before Add files gives up its words.
const STATUS_MIN: f32 = 48.0;

/// The footer's parts (mock `.foot`): the status at the left, then Add
/// files, Clear list, and Convert, the accent, at the right.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FooterRow {
    pub status: Rect,
    pub add: Rect,
    /// Add files shows only its glyph: the footer has no room for its
    /// words beside the status.
    pub add_glyph_only: bool,
    pub clear: Rect,
    pub convert: Rect,
    /// The credit under the status, where there is room for it
    /// ([`crate::credit::in_footer`]).
    pub credit: Option<crate::credit::Credit>,
}

/// The footer's parts in `footer`: 32-tall buttons centred, 8 apart, the
/// last [`FOOTER_PAD`] in from the right; Convert the width of "Convert N
/// files" when `several` are selected. Where the status would have less
/// than [`STATUS_MIN`], Add files keeps only its glyph.
pub fn footer_row(footer: &Rect, several: bool) -> FooterRow {
    footer_row_for(footer, if several { CONVERT_MANY } else { CONVERT_ONE })
}

/// The footer's parts as [`footer_row`] has them, with Convert `convert_w`
/// wide: as wide as its words ("Convert to JPG") need.
pub fn footer_row_for(footer: &Rect, convert_w: f32) -> FooterRow {
    let y = footer.y + (footer.h - BUTTON_HEIGHT) / 2.0;
    let fixed = convert_w + CLEAR_WIDTH;
    let gaps = FOOTER_GAP * 2.0;
    let inner = footer.w - 2.0 * FOOTER_PAD;
    let room = inner - fixed - gaps - ADD_FILES_WIDTH - STATUS_GAP;
    let add_glyph_only = room < STATUS_MIN;
    let add_w = if add_glyph_only {
        ADD_FILES_GLYPH_WIDTH
    } else {
        ADD_FILES_WIDTH
    };
    let mut right = footer.right() - FOOTER_PAD;
    let mut next = |w: f32| {
        let r = Rect::new(right - w, y, w, BUTTON_HEIGHT);
        right = r.x - FOOTER_GAP;
        r
    };
    let convert = next(convert_w);
    let clear = next(CLEAR_WIDTH);
    let add = next(add_w);
    let left = footer.x + FOOTER_PAD;
    let status = Rect::new(
        left,
        footer.y,
        (add.x - STATUS_GAP - left).max(0.0),
        footer.h,
    );
    FooterRow {
        status,
        add,
        add_glyph_only,
        clear,
        convert,
        credit: None,
    }
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
pub const PREVIEW_ROW: f32 = 40.0;

/// The corner radius of a row's preview.
pub const PREVIEW_RADIUS: f32 = 4.0;

/// The inspector's preview, in DIPs: the width and height at
/// [`INSPECTOR_WIDTH`], where it spans what the padding leaves.
pub const HERO: (f32, f32) = (268.0, 120.0);

/// Height of a format the inspector offers to convert to.
pub const TARGET_HEIGHT: f32 = 36.0;

/// Height of one labelled fact in the inspector.
pub const FACT_HEIGHT: f32 = 20.0;

/// A fact's value starts this far past its label's left edge: a column for
/// the widest of the labels ("Dimensions", Caption words about 62 wide).
pub const FACT_LABEL: f32 = 72.0;

/// Inside a format's row in the side pane: the padding at either end, the
/// code's column and the gap after it. The code is in a column of its own,
/// so what the format is for starts at one place down the list.
pub const OFFER_PAD: f32 = 12.0;
pub const OFFER_CODE: f32 = 52.0;
pub const OFFER_GAP: f32 = 8.0;

/// The room for what a format is for, in the row `b`: from after the code's
/// column to the row's right padding, less `reserved` for anything at the
/// right (an estimate). The side pane and the lists of formats beside the
/// wheel and the footer use this one row design.
pub fn offer_words(b: &Rect, reserved: f32) -> Rect {
    let x = offer_words_x(b);
    Rect::new(x, b.y, (b.right() - OFFER_PAD - reserved - x).max(0.0), b.h)
}

/// The column a format's code is set in, in the row `b`.
pub fn offer_code_column(b: &Rect) -> Rect {
    Rect::new(b.x + OFFER_PAD, b.y, OFFER_CODE, b.h)
}

/// Where the words after a format's code start, in the row `b`.
pub fn offer_words_x(b: &Rect) -> f32 {
    b.x + OFFER_PAD + OFFER_CODE + OFFER_GAP
}

/// A block of text `h` tall, one line or several, in the middle of the
/// room `r` it is set in, so a description sits level with the code beside
/// it. Taller than the room, it keeps the room's top.
pub fn centred_block(r: &Rect, h: f32) -> Rect {
    let h = h.max(0.0);
    Rect::new(r.x, r.y + ((r.h - h) / 2.0).max(0.0), r.w, h.min(r.h))
}

/// Several files' cards: each this far right and down from the one behind.
pub const CARD_STEP: (f32, f32) = (12.0, 8.0);

/// How far in from the picture's place the block of cards keeps at each
/// side, whatever their number.
pub const CARD_INSET: f32 = 12.0;

/// The cards of a stack of `n` (at most three) files' pictures in `hero`,
/// the one at the back first: a block [`CARD_INSET`] in from each side,
/// each card a step right and down from the one behind it, so the block is
/// the same width and place for two files or three.
pub fn card_stack(hero: &Rect, n: usize) -> Vec<Rect> {
    let n = n.min(3);
    if hero.h <= 0.0 || n == 0 {
        return vec![];
    }
    let (dx, dy) = CARD_STEP;
    let steps = (n - 1) as f32;
    let w = (hero.w - 2.0 * CARD_INSET - dx * steps).max(0.0);
    let h = (hero.h - 2.0 * dy).max(0.0);
    let y0 = hero.y + (hero.h - h - dy * steps) / 2.0;
    (0..n)
        .map(|back| {
            Rect::new(
                hero.x + CARD_INSET + dx * back as f32,
                y0 + dy * back as f32,
                w,
                h,
            )
        })
        .collect()
}

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

/// The content layer under a title bar `top` units tall, in a `w` x `h`
/// window (look spec, Window): the window edge at the sides and the
/// bottom, none below 600 wide, and right up to the title bar.
pub fn content_layer(w: f32, h: f32, top: f32) -> Rect {
    let edge = crate::theme::fluent::space::window_edge(w);
    Rect::new(
        edge,
        top,
        (w - 2.0 * edge).max(0.0),
        (h - top - edge).max(0.0),
    )
}

/// The empty window's centred group on the content layer, top to bottom
/// (mock screen 1): the K, the headline, the body line, Add files, a
/// status line, and the first-run card while it shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EmptyState {
    /// The content layer it sits on.
    pub layer: Rect,
    /// The K, 64 across. `None` where the layer is too short for the group
    /// with it: the K goes first.
    pub mark: Option<Rect>,
    pub headline: Rect,
    pub body: Rect,
    pub button: Rect,
    /// One line under the button for what the status says, such as
    /// "There are no files to paste.". Kept whether or not it's used, so
    /// the group doesn't move when a line appears.
    pub status: Rect,
    /// The first-run card, while it shows.
    pub card: Option<FirstRunCard>,
    /// The dashed drop outline a drag over the window shows, inset 12.
    pub drop: Rect,
}

/// The first-run card's parts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FirstRunCard {
    pub card: Rect,
    /// Dismiss, at the card's top right.
    pub dismiss: Rect,
    pub title: Rect,
    /// Each step's number in its circle, and its words.
    pub numbers: [Rect; 3],
    pub steps: [Rect; 3],
    /// The right-click line under the steps.
    pub more: Rect,
}

/// How tall the first-run card's wrapped lines run, at the widths
/// [`first_run_widths`] gives: each step in Body, the right-click line in
/// Caption.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CardText {
    pub steps: [f32; 3],
    pub more: f32,
}

impl CardText {
    /// One line each, for a tree or a test without a renderer.
    pub const ONE_LINE: CardText = CardText {
        steps: [EMPTY_LINE; 3],
        more: CARD_MORE_LINE,
    };
}

/// The K on the empty window, and the gap under it.
pub const EMPTY_MARK: f32 = crate::theme::fluent::space::EMPTY_MARK;
const EMPTY_MARK_GAP: f32 = 24.0;
/// The headline (Subtitle) and the gap under it.
const EMPTY_HEADLINE: f32 = 28.0;
const EMPTY_HEADLINE_GAP: f32 = 4.0;
/// A Body line, and the gap under the body line.
const EMPTY_LINE: f32 = 20.0;
const EMPTY_BODY_GAP: f32 = 24.0;
/// The status line (Caption) and its gaps above and below: together the
/// mock's 32 between Add files and the card.
const EMPTY_STATUS: f32 = 16.0;
const EMPTY_STATUS_GAP: f32 = 8.0;
/// The empty group's room from the layer's edges.
pub const EMPTY_PAD: f32 = 24.0;
/// The drop outline's inset from the layer.
pub const DROP_INSET: f32 = 12.0;
/// The first-run card: its widest, its padding (16 on every side, as the
/// other cards have it), the heading's line, and the gaps (12).
pub const CARD_WIDTH: f32 = 520.0;
const CARD_PAD: f32 = 16.0;
const CARD_TITLE: f32 = 20.0;
const CARD_TITLE_GAP: f32 = 12.0;
const CARD_STEP_GAP: f32 = 12.0;
/// A step's number in its circle, and the gap to its words.
pub const CARD_NUMBER: f32 = 20.0;
const CARD_NUMBER_GAP: f32 = 12.0;
const CARD_MORE_GAP: f32 = 12.0;
const CARD_MORE_LINE: f32 = 16.0;
/// Dismiss: a subtle icon button 8 in from the card's top right.
pub const CARD_DISMISS: (f32, f32) = (40.0, 32.0);
const CARD_DISMISS_INSET: f32 = 8.0;

/// How wide the first-run card is on a layer `layer_w` wide.
fn card_width(layer_w: f32) -> f32 {
    (layer_w - 2.0 * EMPTY_PAD).clamp(0.0, CARD_WIDTH)
}

/// How wide the first-run card's steps and its right-click line wrap on a
/// layer `layer_w` wide.
pub fn first_run_widths(layer_w: f32) -> (f32, f32) {
    let w = card_width(layer_w);
    let inner = (w - 2.0 * CARD_PAD).max(0.0);
    ((inner - CARD_NUMBER - CARD_NUMBER_GAP).max(0.0), inner)
}

/// A measured line's height, at least `floor`: a height that isn't a
/// number is one line.
fn at_least(h: f32, floor: f32) -> f32 {
    if h.is_finite() {
        h.max(floor)
    } else {
        floor
    }
}

/// The empty window's group on `layer`, with the first-run card when
/// `card` gives its lines' heights. The group is centred; where the layer
/// is too short for it, the K goes, and where it is still too short the
/// group starts at the top and the card runs past the layer's bottom.
pub fn empty_state(layer: &Rect, card: Option<CardText>) -> EmptyState {
    let cw = card_width(layer.w);
    let card = card.map(|t| CardText {
        steps: t.steps.map(|h| at_least(h, EMPTY_LINE)),
        more: at_least(t.more, CARD_MORE_LINE),
    });
    let card_h = card.map(|t| {
        CARD_PAD
            + CARD_TITLE
            + CARD_TITLE_GAP
            + t.steps.iter().sum::<f32>()
            + 2.0 * CARD_STEP_GAP
            + CARD_MORE_GAP
            + t.more
            + CARD_PAD
    });
    // The text and button, then the status and the card under them.
    let text = EMPTY_HEADLINE + EMPTY_HEADLINE_GAP + EMPTY_LINE + EMPTY_BODY_GAP + BUTTON_HEIGHT;
    let under = EMPTY_STATUS_GAP + EMPTY_STATUS + card_h.map_or(0.0, |h| EMPTY_STATUS_GAP + h);
    let mark = EMPTY_MARK + EMPTY_MARK_GAP;
    let room = layer.h - 2.0 * EMPTY_PAD;
    let (with_mark, total) = if text + under + mark <= room {
        (true, text + under + mark)
    } else {
        (false, text + under)
    };
    let top = if total <= room {
        layer.y + (layer.h - total) / 2.0
    } else {
        layer.y + EMPTY_PAD
    };
    let mut y = snap(top);
    let cx = layer.x + layer.w / 2.0;
    let line = |y: f32, h: f32| {
        Rect::new(
            layer.x + EMPTY_PAD,
            y,
            (layer.w - 2.0 * EMPTY_PAD).max(0.0),
            h,
        )
    };
    let mark = with_mark.then(|| {
        let r = Rect::new(snap(cx - EMPTY_MARK / 2.0), y, EMPTY_MARK, EMPTY_MARK);
        y += EMPTY_MARK + EMPTY_MARK_GAP;
        r
    });
    let headline = line(y, EMPTY_HEADLINE);
    y += EMPTY_HEADLINE + EMPTY_HEADLINE_GAP;
    let body = line(y, EMPTY_LINE);
    y += EMPTY_LINE + EMPTY_BODY_GAP;
    let button = Rect::new(
        snap(cx - ADD_FILES_EMPTY_WIDTH / 2.0),
        y,
        ADD_FILES_EMPTY_WIDTH,
        BUTTON_HEIGHT,
    );
    y += BUTTON_HEIGHT + EMPTY_STATUS_GAP;
    let status = line(y, EMPTY_STATUS);
    y += EMPTY_STATUS + EMPTY_STATUS_GAP;
    let card = card.zip(card_h).map(|(t, h)| {
        let r = Rect::new(snap(cx - cw / 2.0), y, cw, h);
        let (step_w, more_w) = first_run_widths(layer.w);
        let left = r.x + CARD_PAD;
        let dismiss = Rect::new(
            r.right() - CARD_DISMISS_INSET - CARD_DISMISS.0,
            r.y + CARD_DISMISS_INSET,
            CARD_DISMISS.0,
            CARD_DISMISS.1,
        );
        let title = Rect::new(
            left,
            r.y + CARD_PAD,
            (dismiss.x - left).max(0.0),
            CARD_TITLE,
        );
        let mut sy = title.bottom() + CARD_TITLE_GAP;
        let mut numbers = [Rect::default(); 3];
        let mut steps = [Rect::default(); 3];
        for (k, &h) in t.steps.iter().enumerate() {
            numbers[k] = Rect::new(left, sy, CARD_NUMBER, CARD_NUMBER);
            steps[k] = Rect::new(left + CARD_NUMBER + CARD_NUMBER_GAP, sy, step_w, h);
            sy += h + CARD_STEP_GAP;
        }
        let more = Rect::new(left, steps[2].bottom() + CARD_MORE_GAP, more_w, t.more);
        FirstRunCard {
            card: r,
            dismiss,
            title,
            numbers,
            steps,
            more,
        }
    });
    EmptyState {
        layer: *layer,
        mark,
        headline,
        body,
        button,
        status,
        card,
        drop: layer.inset(DROP_INSET),
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

/// A button of Help: Show first-run help, or Close.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HelpButton {
    Guide,
    Close,
}

/// A button of the details dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

/// Settings' controls, cards and sections live with the page's words
/// (`settings_page`); `settings` below places them.
pub use crate::settings_page::{Card, Control, Section};

/// How long a text runs, for a layout that wraps: the renderer's own
/// measures in the window, an estimate in tests (`Estimate`).
pub trait Fit {
    /// How wide `s` runs on one line, in Body (Caption when `caption`).
    fn width(&self, s: &str, caption: bool) -> f32;
    /// How tall `s` runs wrapped to `width`, in Body (Caption when
    /// `caption`). An empty string takes no height.
    fn height(&self, s: &str, caption: bool, width: f32) -> f32;
}

/// A measure without a renderer: 7 units a letter in Body and 6 in
/// Caption, wrapped at spaces, 20 and 16 a line.
pub struct Estimate;

impl Fit for Estimate {
    fn width(&self, s: &str, caption: bool) -> f32 {
        s.chars().count() as f32 * if caption { 6.0 } else { 7.0 }
    }

    fn height(&self, s: &str, caption: bool, width: f32) -> f32 {
        if s.is_empty() {
            return 0.0;
        }
        let (em, line) = if caption { (6.0, 16.0) } else { (7.0, 20.0) };
        let per = (width / em).floor().max(1.0) as usize;
        let mut lines = 1;
        let mut run = 0;
        for word in s.split(' ') {
            let n = word.chars().count();
            if run == 0 {
                run = n;
            } else if run + 1 + n <= per {
                run += 1 + n;
            } else {
                lines += 1;
                run = n;
            }
            // A word longer than the line breaks across lines.
            while run > per {
                lines += 1;
                run -= per;
            }
        }
        lines as f32 * line
    }
}

/// Settings' page, top to bottom (mock screen 6): the window edge, then
/// the page's padding: 8 over the title, 32 at the sides (16 on a narrow
/// layer) and 32 under the last card.
pub const SETTINGS_PAD_TOP: f32 = 8.0;
pub const SETTINGS_PAD_X: f32 = 32.0;
pub const SETTINGS_PAD_X_NARROW: f32 = 16.0;
/// A layer narrower than this keeps 16 at the sides.
pub const SETTINGS_NARROW_BELOW: f32 = 640.0;
pub const SETTINGS_PAD_BOTTOM: f32 = 32.0;
/// The cards grow no wider than this; a wider window centres them.
pub const SETTINGS_MAX_WIDTH: f32 = 1000.0;
/// The title (Title, 28/36), 12 under the page's top.
pub const SETTINGS_TITLE_TOP: f32 = 12.0;
pub const SETTINGS_TITLE: f32 = 36.0;
/// A section's heading (Body Strong): 24 above it, 8 under it.
pub const SECTION_GAP: f32 = 24.0;
pub const SECTION_HEADING: f32 = 20.0;
pub const SECTION_UNDER: f32 = 8.0;
/// A card: at least 68 tall, 16 across and 12 down inside, 4 between
/// cards; its glyph 20 across with 16 after it.
pub const CARD_MIN: f32 = 68.0;
pub const CARD_PAD_X: f32 = 16.0;
pub const CARD_PAD_Y: f32 = 12.0;
pub const CARD_GAP: f32 = 4.0;
pub const CARD_GLYPH: f32 = 20.0;
pub const CARD_GLYPH_GAP: f32 = 16.0;
/// The gap between a card's words and its control.
pub const CARD_COLUMN_GAP: f32 = 16.0;
/// A card's words are never narrower than this beside its control; where
/// they would be, the control goes under them.
pub const CARD_LABEL_MIN: f32 = 200.0;
/// The gap between a card's words and a control under them.
pub const CARD_STACK_GAP: f32 = 8.0;
/// Buttons side by side, and radio buttons in a row.
pub const CARD_BUTTON_GAP: f32 = 8.0;
pub const CARD_RADIO_GAP: f32 = 16.0;
/// A helper's status badge: Caption with 8 across and 2 down inside.
pub const BADGE_PAD: (f32, f32) = (8.0, 2.0);
/// An Expander's chevron, and the gap before it.
pub const CARD_CHEVRON: f32 = 16.0;
/// The Caption line over an open helper's place on disk.
pub const LINE_LABEL: f32 = 16.0;

/// One card on the Settings page, where its parts are now.
#[derive(Debug, Clone, PartialEq)]
pub struct CardLayout {
    pub card: Card,
    /// The whole card, header and (an open Expander's) content.
    pub rect: Rect,
    /// The card's own row: all of it but an open Expander's content.
    pub header: Rect,
    pub glyph: Rect,
    pub label: Rect,
    pub note: Option<Rect>,
    /// A helper's status badge.
    pub badge: Option<Rect>,
    /// An Expander's chevron.
    pub chevron: Option<Rect>,
    /// An open Expander's content, its line and the caption under it.
    pub content: Option<Rect>,
    pub line_label: Option<Rect>,
    pub line: Option<Rect>,
    pub line_note: Option<Rect>,
    /// Its control went under its words: the card is too narrow for both
    /// side by side.
    pub stacked: bool,
}

/// The Settings page laid out: one scrolling page of sections and cards.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsLayout {
    /// The content layer the page scrolls in.
    pub view: Rect,
    /// How far it is scrolled, and how tall it all is.
    pub scroll: f32,
    pub content_h: f32,
    pub title: Rect,
    pub sections: Vec<(Section, Rect)>,
    pub cards: Vec<CardLayout>,
    /// Every control, in Tab order, where it is now.
    pub controls: Vec<(Control, Rect)>,
}

impl SettingsLayout {
    /// How far the page scrolls at most.
    pub fn max_scroll(&self) -> f32 {
        (self.content_h - self.view.h).max(0.0)
    }

    /// Control `c`'s place among the controls.
    pub fn find(&self, c: Control) -> Option<usize> {
        self.controls.iter().position(|(k, _)| *k == c)
    }

    pub fn card(&self, c: Card) -> Option<&CardLayout> {
        self.cards.iter().find(|l| l.card == c)
    }

    /// The control under `(x, y)`, if it shows there.
    pub fn hit(&self, x: f32, y: f32) -> Option<usize> {
        if !self.view.contains(x, y) {
            return None;
        }
        self.controls.iter().position(|(_, r)| r.contains(x, y))
    }

    /// The scroll that shows `r` (where it is now) whole, as near the
    /// present one as can be.
    pub fn scroll_to_show(&self, r: &Rect) -> f32 {
        let margin = SECTION_UNDER;
        let top = r.y - margin;
        let bottom = r.bottom() + margin;
        let s = if top < self.view.y {
            self.scroll - (self.view.y - top)
        } else if bottom > self.view.bottom() {
            self.scroll + (bottom - self.view.bottom())
        } else {
            self.scroll
        };
        s.clamp(0.0, self.max_scroll())
    }
}

/// What a card's control column holds, measured.
struct Column {
    w: f32,
    h: f32,
}

/// The Settings page `page` in the content layer `view`, scrolled by
/// `scroll` (clamped to what it can scroll), its words measured by `fit`.
pub fn settings(
    view: Rect,
    scroll: f32,
    page: &crate::settings_page::PageSpec,
    fit: &dyn Fit,
) -> SettingsLayout {
    let pad_x = if view.w < SETTINGS_NARROW_BELOW {
        SETTINGS_PAD_X_NARROW
    } else {
        SETTINGS_PAD_X
    };
    let w = (view.w - 2.0 * pad_x).clamp(0.0, SETTINGS_MAX_WIDTH);
    let x = snap(view.x + ((view.w - w) / 2.0).max(0.0));
    let title = Rect::new(
        x,
        view.y + SETTINGS_PAD_TOP + SETTINGS_TITLE_TOP,
        w,
        SETTINGS_TITLE,
    );
    let mut y = title.bottom();
    let mut sections = vec![];
    let mut cards = vec![];
    let mut controls = vec![];
    // One drop-down width for the page's one column of controls: the widest
    // any of them needs.
    let combo_w = page
        .cards()
        .filter_map(|c| match &c.body {
            crate::settings_page::Body::Combo {
                choices, swatches, ..
            } => Some(kit::combo_width(
                choices
                    .iter()
                    .map(|c| fit.width(c, false).ceil())
                    .fold(0.0, f32::max)
                    // The dots at the right of each item of an open list
                    // need their room beside the longest label.
                    + if swatches.is_empty() {
                        0.0
                    } else {
                        kit::swatch_room()
                    },
            )),
            _ => None,
        })
        .fold(kit::COMBO_MIN_WIDTH, f32::max);
    for (section, specs) in &page.sections {
        y += SECTION_GAP;
        sections.push((*section, Rect::new(x, y, w, SECTION_HEADING)));
        y += SECTION_HEADING + SECTION_UNDER;
        for spec in specs {
            let (card, mut own) = card_layout(spec, x, y, w, combo_w, fit);
            y = card.rect.bottom() + CARD_GAP;
            cards.push(card);
            controls.append(&mut own);
        }
        y -= CARD_GAP;
    }
    y += SETTINGS_PAD_BOTTOM;
    let content_h = (y - view.y).max(0.0);
    let max = (content_h - view.h).max(0.0);
    let scroll = if scroll.is_finite() {
        scroll.clamp(0.0, max)
    } else {
        0.0
    };
    let up = |r: Rect| Rect::new(r.x, r.y - scroll, r.w, r.h);
    let up_opt = |r: Option<Rect>| r.map(up);
    SettingsLayout {
        view,
        scroll,
        content_h,
        title: up(title),
        sections: sections.into_iter().map(|(s, r)| (s, up(r))).collect(),
        cards: cards
            .into_iter()
            .map(|c| CardLayout {
                rect: up(c.rect),
                header: up(c.header),
                glyph: up(c.glyph),
                label: up(c.label),
                note: up_opt(c.note),
                badge: up_opt(c.badge),
                chevron: up_opt(c.chevron),
                content: up_opt(c.content),
                line_label: up_opt(c.line_label),
                line: up_opt(c.line),
                line_note: up_opt(c.line_note),
                ..c
            })
            .collect(),
        controls: controls.into_iter().map(|(c, r)| (c, up(r))).collect(),
    }
}

/// The widths of `buttons`' labels as buttons.
fn button_widths(buttons: &[(Control, String)], fit: &dyn Fit) -> Vec<f32> {
    buttons
        .iter()
        .map(|(_, label)| kit::button_width(fit.width(label, false).ceil(), false))
        .collect()
}

/// Buttons `widths` wide laid from `x` along `y`, wrapping at `right`
/// (when `wrap`) onto lines under; returns their rects and the height.
fn lay_buttons(widths: &[f32], x: f32, y: f32, right: f32, wrap: bool) -> (Vec<Rect>, f32) {
    let h = kit::CONTROL_HEIGHT;
    let mut out = vec![];
    let (mut bx, mut by) = (x, y);
    for &bw in widths {
        if wrap && bx > x && bx + bw > right {
            bx = x;
            by += h + CARD_BUTTON_GAP;
        }
        out.push(Rect::new(bx, by, bw, h));
        bx += bw + CARD_BUTTON_GAP;
    }
    let height = if out.is_empty() { 0.0 } else { by + h - y };
    (out, height)
}

/// The radio buttons Small, High and Best laid from `x` along `y`: in a
/// row when `row`, otherwise one under another. Returns their rects and
/// the height.
fn lay_radios(labels: &[f32], x: f32, y: f32, row: bool) -> (Vec<Rect>, f32) {
    if row {
        let mut rx = x;
        let rects = labels
            .iter()
            .map(|&lw| {
                let w = kit::RADIO + kit::RADIO_GAP + lw;
                let r = Rect::new(rx, y, w, kit::RADIO_ROW);
                rx += w + CARD_RADIO_GAP;
                r
            })
            .collect();
        (rects, kit::RADIO_ROW)
    } else {
        let rows = kit::radio_rows(x, y, labels);
        let h = rows.last().map_or(0.0, |r| r.row.bottom() - y);
        (rows.into_iter().map(|r| r.row).collect(), h)
    }
}

/// The width of radio buttons labelled `labels` in a row.
fn radios_row_width(labels: &[f32]) -> f32 {
    labels
        .iter()
        .map(|lw| kit::RADIO + kit::RADIO_GAP + lw)
        .sum::<f32>()
        + CARD_RADIO_GAP * labels.len().saturating_sub(1) as f32
}

/// One card at `(x, y)`, `w` wide, and its controls in Tab order.
fn card_layout(
    spec: &crate::settings_page::CardSpec,
    x: f32,
    y: f32,
    w: f32,
    combo_w: f32,
    fit: &dyn Fit,
) -> (CardLayout, Vec<(Control, Rect)>) {
    use crate::settings_page::Body;
    let inner_l = x + CARD_PAD_X;
    let inner_r = x + w - CARD_PAD_X;
    let text_x = inner_l + CARD_GLYPH + CARD_GLYPH_GAP;
    let radio_labels: Vec<f32> = Preset::ALL
        .iter()
        .map(|p| fit.width(p.label(), false).ceil())
        .collect();
    let toggle_w = kit::TOGGLE_WORD_MIN
        .max(fit.width("On", false).max(fit.width("Off", false)).ceil())
        + kit::TOGGLE_GAP
        + crate::theme::fluent::space::TOGGLE.0;
    let badge_w = |text: &str| fit.width(text, true).ceil() + 2.0 * BADGE_PAD.0;

    // What sits at the right, measured.
    let column = match &spec.body {
        Body::Combo { .. } => Column {
            w: combo_w,
            h: kit::CONTROL_HEIGHT,
        },
        Body::Toggle { .. } => Column {
            w: toggle_w,
            h: kit::CONTROL_HEIGHT,
        },
        Body::Radios { .. } => Column {
            w: radios_row_width(&radio_labels),
            h: kit::RADIO_ROW,
        },
        Body::Buttons(b) => {
            let widths = button_widths(b, fit);
            Column {
                w: widths.iter().sum::<f32>()
                    + CARD_BUTTON_GAP * widths.len().saturating_sub(1) as f32,
                h: kit::CONTROL_HEIGHT,
            }
        }
        Body::Expander(e) => Column {
            w: e.badge.map_or(0.0, |b| badge_w(b.text) + CARD_COLUMN_GAP) + CARD_CHEVRON,
            h: kit::CONTROL_HEIGHT,
        },
    };
    let expander = matches!(spec.body, Body::Expander(_));
    // Words beside the control while they keep their least width; else
    // the control goes under them. An Expander keeps its chevron at the
    // right and puts its badge under its words.
    let beside = inner_r - column.w - CARD_COLUMN_GAP - text_x;
    let stacked = beside < CARD_LABEL_MIN;
    let text_w = if !stacked {
        beside
    } else if expander {
        inner_r - CARD_CHEVRON - CARD_COLUMN_GAP - text_x
    } else {
        inner_r - text_x
    }
    .max(0.0);
    let label_h = fit.height(&spec.label, false, text_w).max(kit::LINE);
    let note_h = spec
        .note
        .as_deref()
        .map_or(0.0, |n| fit.height(n, true, text_w));
    let text_h = label_h + note_h;

    let mut controls = vec![];
    let mut badge = None;
    let mut chevron = None;
    // The header's height, and where its words start.
    let (header_h, text_y) = if !stacked {
        let h = CARD_MIN
            .max(text_h + 2.0 * CARD_PAD_Y)
            .max(column.h + 2.0 * CARD_PAD_Y);
        (h, y + (h - text_h) / 2.0)
    } else {
        (0.0, y + CARD_PAD_Y)
    };
    // The control column: beside the words, centred down the card, or
    // under them from the words' left.
    let under = text_y + text_h + CARD_STACK_GAP;
    let avail = inner_r - text_x;
    let mut body_h = 0.0;
    let place = |cw: f32, ch: f32, header_h: f32| -> (f32, f32) {
        if stacked {
            (text_x, under)
        } else {
            (inner_r - cw, y + (header_h - ch) / 2.0)
        }
    };
    match &spec.body {
        Body::Combo { control, .. } => {
            let cw = if stacked {
                column.w.min(avail)
            } else {
                column.w
            };
            let (cx, cy) = place(cw, column.h, header_h);
            controls.push((*control, Rect::new(cx, cy, cw, column.h)));
            body_h = column.h;
        }
        Body::Toggle { control, .. } => {
            let (cx, cy) = place(column.w, column.h, header_h);
            controls.push((*control, Rect::new(cx, cy, column.w, column.h)));
            body_h = column.h;
        }
        Body::Radios { setting, .. } => {
            let row = !stacked || column.w <= avail;
            let (cx, cy) = place(column.w, column.h, header_h);
            let (rects, h) = lay_radios(&radio_labels, cx, cy, row);
            controls.extend(
                Preset::ALL
                    .iter()
                    .zip(rects)
                    .map(|(p, r)| (Control::Preset(*setting, *p), r)),
            );
            body_h = h;
        }
        Body::Buttons(b) => {
            let widths = button_widths(b, fit);
            let (cx, cy) = place(column.w, column.h, header_h);
            let (rects, h) = lay_buttons(&widths, cx, cy, inner_r, stacked);
            controls.extend(b.iter().map(|(c, _)| *c).zip(rects));
            body_h = h;
        }
        Body::Expander(e) => {
            if let Some(b) = e.badge {
                let bw = badge_w(b.text);
                let bh = 16.0 + 2.0 * BADGE_PAD.1;
                badge = Some(if stacked {
                    Rect::new(text_x, under, bw, bh)
                } else {
                    let right = inner_r - CARD_CHEVRON - CARD_COLUMN_GAP;
                    Rect::new(right - bw, y + (header_h - bh) / 2.0, bw, bh)
                });
                if stacked {
                    body_h = bh;
                }
            }
        }
    }
    let header_h = if stacked {
        CARD_MIN.max(
            CARD_PAD_Y
                + text_h
                + if body_h > 0.0 {
                    CARD_STACK_GAP + body_h
                } else {
                    0.0
                }
                + CARD_PAD_Y,
        )
    } else {
        header_h
    };
    let header = Rect::new(x, y, w, header_h);
    let glyph_y = if stacked {
        text_y + (label_h.min(kit::LINE * 1.0) - CARD_GLYPH) / 2.0
    } else {
        y + (header_h - CARD_GLYPH) / 2.0
    };
    let glyph = Rect::new(inner_l, glyph_y, CARD_GLYPH, CARD_GLYPH);
    let label = Rect::new(text_x, text_y, text_w, label_h);
    let note = spec
        .note
        .as_ref()
        .map(|_| Rect::new(text_x, text_y + label_h, text_w, note_h));

    let (mut content, mut line_label, mut line, mut line_note) = (None, None, None, None);
    let mut rect = header;
    if let Body::Expander(e) = &spec.body {
        let c = Rect::new(
            inner_r - CARD_CHEVRON,
            y + (header_h - CARD_CHEVRON) / 2.0,
            CARD_CHEVRON,
            CARD_CHEVRON,
        );
        chevron = Some(c);
        // The header opens and closes it: the whole row is its control.
        controls.insert(0, (Control::Helper(e.helper), header));
        if e.open {
            let top = header.bottom();
            let widths = button_widths(&e.buttons, fit);
            let bw = widths.iter().sum::<f32>()
                + CARD_BUTTON_GAP * widths.len().saturating_sub(1) as f32;
            let beside = inner_r - bw - CARD_COLUMN_GAP - text_x;
            let stack = !e.buttons.is_empty() && beside < CARD_LABEL_MIN;
            let tw = if stack || e.buttons.is_empty() {
                avail
            } else {
                beside
            }
            .max(0.0);
            let lh = fit.height(&e.line, false, tw).max(kit::LINE);
            let nh = e
                .line_note
                .as_deref()
                .map_or(0.0, |n| fit.height(n, true, tw));
            // A place on disk has its Caption label over it.
            let llh = if e.line_label.is_some() {
                LINE_LABEL
            } else {
                0.0
            };
            let th = llh + lh + nh;
            let (rects, bh) = if stack {
                lay_buttons(
                    &widths,
                    text_x,
                    top + CARD_PAD_Y + th + CARD_STACK_GAP,
                    inner_r,
                    true,
                )
            } else {
                let inner = th.max(kit::CONTROL_HEIGHT);
                let by = top + CARD_PAD_Y + (inner - kit::CONTROL_HEIGHT) / 2.0;
                lay_buttons(&widths, inner_r - bw, by, inner_r, false)
            };
            let ch = if stack {
                CARD_PAD_Y + th + CARD_STACK_GAP + bh + CARD_PAD_Y
            } else {
                CARD_PAD_Y + th.max(if e.buttons.is_empty() { 0.0 } else { bh }) + CARD_PAD_Y
            };
            let ty = if stack || e.buttons.is_empty() {
                top + CARD_PAD_Y
            } else {
                top + (ch - th) / 2.0
            };
            let cr = Rect::new(x, top, w, ch);
            content = Some(cr);
            line_label = e.line_label.map(|_| Rect::new(text_x, ty, tw, llh));
            line = Some(Rect::new(text_x, ty + llh, tw, lh));
            line_note = e
                .line_note
                .as_ref()
                .map(|_| Rect::new(text_x, ty + llh + lh, tw, nh));
            controls.extend(e.buttons.iter().map(|(c, _)| *c).zip(rects));
            rect = Rect::new(x, y, w, header_h + ch);
        }
    }
    (
        CardLayout {
            card: spec.card,
            rect,
            header,
            glyph,
            label,
            note,
            badge,
            chevron,
            content,
            line_label,
            line,
            line_note,
            stacked,
        },
        controls,
    )
}

/// Something the options dialog lets you press.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SheetControl {
    /// One of the Quality radio buttons, for a pair whose knobs hold
    /// [`Knob::Preset`] in row `r`.
    Preset(usize, Preset),
    /// One of the sound or video Quality radio buttons, by its place:
    /// Small, High, Best and, for video, Original (no re-encode).
    Tune(usize),
    /// A knob's line under More options: its value, stepped.
    Step(usize),
    /// The More options Expander.
    Advanced,
    /// One of the quality's fields under More options.
    Field(TuneField),
    Convert,
    Cancel,
}

impl SheetControl {
    /// The knob row this control belongs to: a radio's or a stepper's.
    /// `None` for the quality's radios and the buttons.
    pub fn row(self) -> Option<usize> {
        match self {
            SheetControl::Preset(r, _) | SheetControl::Step(r) => Some(r),
            _ => None,
        }
    }

    /// Whether it is one of the Quality radio buttons, which are one Tab
    /// stop.
    pub fn is_radio(self) -> bool {
        matches!(self, SheetControl::Preset(..) | SheetControl::Tune(_))
    }
}

/// What the options dialog holds, for its layout.
#[derive(Debug, Clone, PartialEq)]
pub struct SheetShape {
    /// Quality as Small, High and Best radio buttons, for `Preset` in knob
    /// row `r`.
    pub quality: Option<usize>,
    /// Quality as this many radio buttons for sound or video Kiln writes
    /// itself instead: Small, High, Best, and Original for video. None
    /// when the pair has no steps.
    pub tune_radios: usize,
    /// The lines under More options, in order: `Step` and `Field`.
    pub more: Vec<SheetControl>,
    pub more_open: bool,
    /// Lines of what the conversion leaves out, under the sub-line.
    pub notes: usize,
    /// Whether there is an estimate to show in its card.
    pub card: bool,
    /// How far the content is scrolled.
    pub scroll: f32,
}

/// The options dialog laid out in a window.
#[derive(Debug, Clone, PartialEq)]
pub struct SheetLayout {
    /// The ContentDialog: Convert, then Cancel, in its band.
    pub frame: kit::DialogLayout,
    /// The whole dialog: a press outside it is on the smoke.
    pub panel: Rect,
    pub title: Rect,
    /// The one line under the title: the source, then what it becomes
    /// (mock `.sub`).
    pub sub: Rect,
    /// What the conversion leaves out, a [`SHEET_NOTE`] line for each
    /// note (`note_line`); `None` when it leaves out nothing worth saying.
    pub notes: Option<Rect>,
    /// "Quality", over its radio buttons.
    pub heading: Option<Rect>,
    /// The Quality radio buttons: Small, High, Best, and for video
    /// Original (no re-encode).
    pub radios: Vec<kit::RadioParts>,
    /// The estimate card, and its line.
    pub card: Option<Rect>,
    pub result: Rect,
    pub expander: Option<kit::ExpanderParts>,
    /// The lines under More options while it is open: each control, its
    /// label and its stepper.
    pub lines: Vec<(SheetControl, Rect, Rect)>,
    /// Every control, in Tab order: the radios, More options and its
    /// lines, then Convert and Cancel.
    pub controls: Vec<(SheetControl, Rect)>,
    /// How far the content can scroll, and how far it is scrolled.
    pub scroll_max: f32,
    pub scroll: f32,
}

/// A line of what the conversion leaves out, under the dialog's sub-line.
pub const SHEET_NOTE: f32 = 16.0;
/// A line under More options, and its stepper's width.
pub const SHEET_LINE: f32 = 40.0;
pub const SHEET_STEP_WIDTH: f32 = 168.0;
/// The estimate card's padding across and down: 16 x 12 as every other
/// card has it.
pub const SHEET_CARD_PAD: (f32, f32) = (16.0, 12.0);
/// The dialog's title line: Subtitle text.
pub const SHEET_TITLE: f32 = 28.0;

impl SheetLayout {
    /// How many lines of notes the dialog was laid out with.
    pub fn note_count(&self) -> usize {
        self.notes
            .map_or(0, |r| (r.h / SHEET_NOTE).round() as usize)
    }

    /// The line for note `k`: empty when there is no such note.
    pub fn note_line(&self, k: usize) -> Rect {
        match self.notes {
            Some(r) if (k as f32 + 1.0) * SHEET_NOTE <= r.h + 0.5 => {
                Rect::new(r.x, r.y + k as f32 * SHEET_NOTE, r.w, SHEET_NOTE)
            }
            _ => Rect::default(),
        }
    }

    /// The control `c`'s rectangle, if it is laid out.
    pub fn rect_of(&self, c: SheetControl) -> Option<Rect> {
        self.controls.iter().find(|(k, _)| *k == c).map(|(_, r)| *r)
    }
}

/// The shape a pair with `knobs` has before anything is read: Quality for
/// `Preset` or the sound or video radios, a line under More options for each other knob,
/// closed, with a card.
pub fn sheet_shape(knobs: &[Knob]) -> SheetShape {
    let mut quality = None;
    let mut tune_radios = 0;
    let mut more = vec![];
    for (i, k) in knobs.iter().enumerate() {
        match k {
            Knob::Preset => quality = Some(i),
            Knob::AudioTune | Knob::VideoTune => tune_radios = 3,
            _ => more.push(SheetControl::Step(i)),
        }
    }
    SheetShape {
        quality,
        tune_radios,
        more,
        more_open: false,
        notes: 0,
        card: true,
        scroll: 0.0,
    }
}

/// [`options_dialog`] for `knobs` in a `w` x `h` window, as it opens.
pub fn sheet(w: f32, h: f32, knobs: &[Knob]) -> SheetLayout {
    options_dialog((w, h), &sheet_shape(knobs))
}

/// The options dialog (spec 2.2) for `s` in a window `size`: a
/// ContentDialog titled "Convert to MP4" with one sub-line and the notes
/// under it, Quality as radio buttons, the estimate card,
/// More options with its lines, and Convert and Cancel in the band.
/// Content taller than the window lets it be scrolls.
pub fn options_dialog(size: (f32, f32), s: &SheetShape) -> SheetLayout {
    let gap = 12.0;
    let line = kit::LINE;
    let radio_count = if s.quality.is_some() {
        3
    } else {
        s.tune_radios
    };
    let radios_h = match radio_count {
        0 => 0.0,
        n => n as f32 * kit::RADIO_ROW + (n - 1) as f32 * kit::RADIO_ROW_GAP,
    };
    let card_h = 2.0 * SHEET_CARD_PAD.1 + line;
    let more_h = if s.more_open {
        s.more.len() as f32 * SHEET_LINE + 8.0
    } else {
        0.0
    };
    let mut content_h = 16.0 + s.notes as f32 * SHEET_NOTE;
    if radio_count > 0 {
        content_h += gap + line + 4.0 + radios_h;
    }
    if s.card {
        content_h += gap + card_h;
    }
    if !s.more.is_empty() {
        content_h += gap + kit::EXPANDER_HEADER + more_h;
    }
    let frame = kit::dialog(size, SHEET_TITLE, content_h, 2);
    let c = frame.content;
    let scroll_max = (content_h - c.h).max(0.0);
    let scroll = s.scroll.clamp(0.0, scroll_max);
    let mut y = c.y - scroll;
    let sub = Rect::new(c.x, y, c.w, 16.0);
    y = sub.bottom();
    let notes = (s.notes > 0).then(|| Rect::new(c.x, y, c.w, s.notes as f32 * SHEET_NOTE));
    y += s.notes as f32 * SHEET_NOTE;
    let mut controls = vec![];
    let mut heading = None;
    let mut radios = vec![];
    if radio_count > 0 {
        y += gap;
        heading = Some(Rect::new(c.x, y, c.w, line));
        y += line + 4.0;
        radios = kit::radio_rows(
            c.x,
            y,
            &vec![c.w - kit::RADIO - kit::RADIO_GAP; radio_count],
        );
        for (i, p) in radios.iter().enumerate() {
            let control = match s.quality {
                Some(row) => SheetControl::Preset(row, Preset::ALL[i]),
                None => SheetControl::Tune(i),
            };
            controls.push((control, p.row));
        }
        y += radios_h;
    }
    let mut card = None;
    let mut result = Rect::default();
    if s.card {
        y += gap;
        let r = Rect::new(c.x, y, c.w, card_h);
        result = Rect::new(
            r.x + SHEET_CARD_PAD.0,
            r.y + SHEET_CARD_PAD.1,
            r.w - 2.0 * SHEET_CARD_PAD.0,
            line,
        );
        card = Some(r);
        y += card_h;
    }
    let mut expander = None;
    let mut lines = vec![];
    if !s.more.is_empty() {
        y += gap;
        let shown = if s.more_open { 1.0 } else { 0.0 };
        let e = kit::expander_parts(c.x, y, c.w, more_h, shown);
        controls.push((SheetControl::Advanced, e.header));
        if let (true, Some(ct)) = (s.more_open, e.content) {
            for (k, &m) in s.more.iter().enumerate() {
                let top = ct.y + 4.0 + k as f32 * SHEET_LINE;
                let control = Rect::new(
                    ct.right() - 16.0 - SHEET_STEP_WIDTH,
                    top + (SHEET_LINE - kit::CONTROL_HEIGHT) / 2.0,
                    SHEET_STEP_WIDTH,
                    kit::CONTROL_HEIGHT,
                );
                let label = Rect::new(
                    ct.x + 16.0,
                    top,
                    (control.x - 8.0 - ct.x - 16.0).max(0.0),
                    SHEET_LINE,
                );
                lines.push((m, label, control));
                controls.push((m, control));
            }
        }
        expander = Some(e);
    }
    controls.push((SheetControl::Convert, frame.buttons[0]));
    controls.push((SheetControl::Cancel, frame.buttons[1]));
    SheetLayout {
        panel: frame.rect,
        title: frame.title,
        sub,
        notes,
        heading,
        radios,
        card,
        result,
        expander,
        lines,
        controls,
        scroll_max,
        scroll,
        frame,
    }
}

fn snap(v: f32) -> f32 {
    (v / GRID).floor() * GRID
}

/// An InfoBar in the side pane, as far as its geometry goes: how tall its
/// message runs once wrapped at [`info_text_width`], whether it has a
/// title over it, and how wide each of its actions is (`count` of them).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InfoShape {
    pub message_h: f32,
    pub titled: bool,
    pub actions: [f32; 4],
    pub count: usize,
}

impl InfoShape {
    /// The InfoBar laid out at `(x, y)`, `w` wide. The title always sits
    /// over the message, which wraps.
    pub fn at(&self, x: f32, y: f32, w: f32) -> kit::InfoBarLayout {
        let h = self.message_h;
        kit::info_bar(
            x,
            y,
            w,
            if self.titled { 1.0 } else { 0.0 },
            f32::INFINITY,
            &|_| h,
            &self.actions[..self.count.min(4)],
            false,
        )
    }
}

/// What the side pane shows, as far as its geometry goes: which parts it
/// has, how tall the ones that hold wrapped text are, and how many buttons
/// it has (the count of its controls that are buttons).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InspectorShape {
    /// Nothing selected: the mark in the hero's place, then a title
    /// `title_h` tall and the key hint `hint_h` tall, each wrapped.
    Nothing { title_h: f32, hint_h: f32 },
    /// One file nothing has been done to. `what_h` is the height of why it
    /// has nothing to offer, wrapped (0 for a healthy file, which has no
    /// line there); `facts` the labelled facts; Make smaller's row when
    /// `smaller`; `targets` the formats offered; `more` how wide More's
    /// words are, when it follows them. Then its `buttons`, where the
    /// formats would be: Get, for a file waiting on its helper app.
    Waiting {
        what_h: f32,
        facts: usize,
        smaller: bool,
        targets: usize,
        more: Option<f32>,
        buttons: usize,
    },
    /// A running job: what it is doing, the percentage in the Title size
    /// when the engine says how far along it is (`figure`), its bar and the
    /// time left once known (`left`). Progress only: Cancel is the row's.
    Converting { figure: bool, left: bool },
    /// A finished job: its success InfoBar, whose actions are its buttons,
    /// then Make smaller's row when `smaller` and `again` formats under
    /// Convert again.
    Done {
        info: InfoShape,
        smaller: bool,
        again: usize,
    },
    /// A failed or cancelled job: its InfoBar first, whose actions are its
    /// buttons, then the file.
    Failed { info: InfoShape },
    /// Several files: their kinds under the title, the InfoBar saying why
    /// they share no format (`info`), Make smaller's row when `smaller`,
    /// and `targets` rows: the formats they share or, when they share
    /// none, the kinds offered. `more` how wide More's words are, when it
    /// follows the shared formats; then its `buttons`: Get, when they all
    /// wait on one helper app.
    Several {
        info: Option<InfoShape>,
        smaller: bool,
        targets: usize,
        more: Option<f32>,
        buttons: usize,
    },
}

/// Where the side pane's parts go. A part the shape doesn't have, or that
/// doesn't fit whole, is `None` or empty; `hero`, `title` and `body` are
/// always there to draw into, as an empty rect when left out.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct InspectorLayout {
    /// The preview, [`HERO`] tall and as wide as the padding leaves. In
    /// `Nothing` it holds the 48 DIP mark, centred. It is the first thing
    /// to go when the pane is short.
    pub hero: Rect,
    pub title: Rect,
    /// The texts under the title, one rect each, in the order the shape
    /// has them whether or not there is one to show:
    /// - Nothing: the key hint;
    /// - Waiting: what the format is, or why it has nothing to offer;
    /// - Converting: what it is doing, the 40 DIP percentage (0 tall
    ///   without one), the bar, then the time left (0 tall until known);
    /// - Several: their kinds.
    ///
    /// A text the shape doesn't have, or that has no room, is 0 tall:
    /// it is neither drawn nor read out. See [`InspectorLayout::line`].
    pub lines: Vec<Rect>,
    /// Where those texts run, from the first placed to the last. Empty
    /// when none is.
    pub body: Rect,
    /// One rect per fact, [`FACT_HEIGHT`] tall.
    pub facts: Vec<Rect>,
    /// The InfoBar: a finished file's, a failed one's, or why several
    /// share no format. Its actions are `buttons`.
    pub info: Option<kit::InfoBarLayout>,
    /// The "Convert to" line over the targets.
    pub label: Option<Rect>,
    /// Make smaller's row, first under the label.
    pub smaller: Option<Rect>,
    /// One rect per target, [`TARGET_HEIGHT`] tall, however many fit.
    pub targets: Vec<Rect>,
    pub more: Option<Rect>,
    /// One slot for each of the shape's `buttons`, in reading order, so a
    /// slot is never drawn as a button that isn't there. A slot that
    /// doesn't fit across the rect is left out, so a rect narrower than
    /// the buttons never places a control that sticks out of it.
    pub buttons: Vec<Rect>,
}

impl InspectorLayout {
    /// Where the body's `k`th text goes (see [`InspectorLayout::lines`]),
    /// while it has room.
    pub fn line(&self, k: usize) -> Option<Rect> {
        self.lines.get(k).copied().filter(|r| r.h > 0.0)
    }
}

/// Width of a side pane button outside an InfoBar.
pub const INSPECTOR_BUTTON_WIDTH: f32 = 96.0;

/// How far a link's hit area runs past the end of its words, so the
/// pointer has some room past the last letter and the focus ring clears it.
pub const LINK_PAD: f32 = 8.0;

/// How wide the side pane's text runs, `pane_w` wide: inside the padding.
pub fn inspector_text_width(pane_w: f32) -> f32 {
    (pane_w - 2.0 * PANEL_PADDING).max(0.0)
}

/// How wide an InfoBar's message runs in a side pane `pane_w` wide: inside
/// the padding, the InfoBar's own padding and its glyph.
pub fn info_text_width(pane_w: f32) -> f32 {
    (inspector_text_width(pane_w) - 2.0 * kit::INFO_PAD_X - kit::INFO_GLYPH - kit::INFO_GLYPH_GAP)
        .max(0.0)
}

/// How a text of the side pane wraps: in the title's Subtitle, the body's
/// Caption or an InfoBar's Body, and the most lines it takes before it is
/// cut short. Title and Body run at [`inspector_text_width`], Info at
/// [`info_text_width`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Wrapped {
    Title(usize),
    Body(usize),
    Info(usize),
}

/// Nothing's title takes at most two lines.
pub const TITLE_LINES: usize = 2;
/// What the format is and why several files share no format take at most
/// four lines.
pub const BODY_LINES: usize = 4;
/// Nothing's key hint takes at most four lines.
pub const HINT_LINES: usize = 4;
/// An InfoBar's message takes at most five.
pub const INFO_LINES: usize = 5;

/// The shape of what `model` shows. Each text that wraps is measured with
/// `measure`: its height in DIPs as it wraps (`Wrapped`). `nothing` is
/// what Nothing says under its title: the key hint. `more_w` is how wide
/// More's words are; `button_w` how wide a button saying a label is. The
/// buttons are the controls that aren't a format, a kind, Make smaller or
/// More.
pub fn inspector_shape(
    model: &Inspector,
    nothing: &str,
    more_w: f32,
    measure: &dyn Fn(&str, Wrapped) -> f32,
    button_w: &dyn Fn(&str) -> f32,
) -> InspectorShape {
    let wrapped = |s: Option<&str>, w: Wrapped| s.map_or(0.0, |s| measure(s, w));
    let buttons: Vec<InspectorControl> = model
        .controls()
        .into_iter()
        .filter(|c| {
            !matches!(
                c,
                InspectorControl::Target(_)
                    | InspectorControl::Kind(_)
                    | InspectorControl::More
                    | InspectorControl::Folder
                    | InspectorControl::Smaller
            )
        })
        .collect();
    let info = |with_actions: bool| {
        model.bar().map(|bar| {
            let mut actions = [0.0; 4];
            let mut count = 0;
            if with_actions {
                for (slot, c) in actions.iter_mut().zip(&buttons) {
                    *slot = button_w(model.label(*c).unwrap_or_default());
                    count += 1;
                }
            }
            InfoShape {
                message_h: wrapped(Some(&bar.message), Wrapped::Info(INFO_LINES)),
                titled: !bar.title.is_empty(),
                actions,
                count,
            }
        })
    };
    let zero = InfoShape {
        message_h: 0.0,
        titled: false,
        actions: [0.0; 4],
        count: 0,
    };
    match model {
        Inspector::Nothing => InspectorShape::Nothing {
            title_h: measure(NOTHING_TITLE, Wrapped::Title(TITLE_LINES)),
            hint_h: measure(nothing, Wrapped::Body(HINT_LINES)),
        },
        Inspector::Waiting {
            facts,
            targets,
            more,
            reason,
            smaller,
            ..
        } => InspectorShape::Waiting {
            what_h: wrapped(
                reason.as_ref().map(|(r, _)| r.as_str()),
                Wrapped::Body(BODY_LINES),
            ),
            facts: facts.len(),
            smaller: *smaller,
            targets: targets.len(),
            more: more.then_some(more_w),
            buttons: buttons.len(),
        },
        Inspector::Converting { percent, left, .. } => InspectorShape::Converting {
            figure: percent.is_some(),
            left: left.is_some(),
        },
        Inspector::Done { again, smaller, .. } => InspectorShape::Done {
            info: info(true).unwrap_or(zero),
            smaller: *smaller,
            again: again.len(),
        },
        Inspector::Failed { .. } => InspectorShape::Failed {
            info: info(true).unwrap_or(zero),
        },
        Inspector::Several {
            targets,
            more,
            offered,
            smaller,
            ..
        } => InspectorShape::Several {
            info: info(false),
            smaller: *smaller,
            // The shared formats, or with none, each kind to narrow to.
            targets: if targets.is_empty() {
                offered.len()
            } else {
                targets.len()
            },
            more: more.then_some(more_w),
            buttons: buttons.len(),
        },
    }
}

/// The side pane's parts stacked down `r`, inside 16 DIPs of padding. `r`
/// is the whole pane: the fold slides it in by translation and clipping,
/// and never lays it out narrower.
///
/// When the whole stack doesn't fit `r`'s height, parts give way so the
/// pane keeps what it exists for, the actions: first the hero, then all
/// of the facts. Only then are the targets, More and the buttons cut from
/// the bottom. Whatever doesn't fit whole inside the padding, down or
/// across, is left out, and so is every part after it, so a short or
/// narrow pane shows the top of its story and never a cut-off or
/// protruding control.
pub fn inspector_layout(r: &Rect, shape: &InspectorShape) -> InspectorLayout {
    inspector_layout_titled(r, shape, None)
}

/// [`inspector_layout`] with the title as tall as `title` says: a file's
/// name wrapped to at most two lines ([`inspector_title_height`]). `None`
/// is the one line it has when it fits.
pub fn inspector_layout_titled(
    r: &Rect,
    shape: &InspectorShape,
    title: Option<f32>,
) -> InspectorLayout {
    for (hero, facts) in [(true, true), (false, true)] {
        let (layout, whole) = inspector_stack(r, shape, hero, facts, title);
        if whole {
            return layout;
        }
    }
    inspector_stack(r, shape, false, false, title).0
}

/// How tall the title is for `model`'s pane, when it is a file's name:
/// the name wrapped in Subtitle to at most [`TITLE_LINES`] lines. `None`
/// for the panes whose title is a line of its own.
pub fn inspector_title_height(
    model: &Inspector,
    name: &str,
    measure: &dyn Fn(&str, Wrapped) -> f32,
) -> Option<f32> {
    match model {
        Inspector::Waiting { .. }
        | Inspector::Converting { .. }
        | Inspector::Done { .. }
        | Inspector::Failed { .. } => Some(measure(name, Wrapped::Title(TITLE_LINES))),
        Inspector::Nothing | Inspector::Several { .. } => None,
    }
}

/// The side pane's title is one Subtitle line.
pub const INSPECTOR_TITLE: f32 = 28.0;

/// The converting percentage's line, in the Title size.
pub const INSPECTOR_FIGURE: f32 = 40.0;

/// The converting bar's height in the side pane.
pub const INSPECTOR_BAR: f32 = 4.0;

/// One try at [`inspector_layout`], offering the hero and the facts or
/// not, and whether every part it offered was placed whole.
fn inspector_stack(
    r: &Rect,
    shape: &InspectorShape,
    hero: bool,
    facts: bool,
    title: Option<f32>,
) -> (InspectorLayout, bool) {
    let file_title = title.unwrap_or(INSPECTOR_TITLE);
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

        /// An InfoBar of `shape`, placed whole or not at all.
        fn info(&mut self, gap: f32, shape: &InfoShape) -> Option<kit::InfoBarLayout> {
            let h = shape.at(0.0, 0.0, self.inner.w).rect.h;
            let at = self.place(gap, h)?;
            Some(shape.at(at.x, at.y, at.w))
        }
    }

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
    // The title's height, each text's under it (see `lines`), the facts,
    // the InfoBar, Make smaller, the targets, how wide More is when it
    // follows them, and the buttons.
    let none: Option<&InfoShape> = None;
    let (title_h, heights, fact_count, info, smaller, targets, more, buttons) = match shape {
        InspectorShape::Nothing { title_h, hint_h } => {
            (*title_h, vec![*hint_h], 0, none, false, 0, None, 0)
        }
        InspectorShape::Waiting {
            what_h,
            facts,
            smaller,
            targets,
            more,
            buttons,
        } => (
            file_title,
            vec![*what_h],
            *facts,
            none,
            *smaller,
            *targets,
            *more,
            *buttons,
        ),
        InspectorShape::Converting { figure, left } => {
            let figure_h = if *figure { INSPECTOR_FIGURE } else { 0.0 };
            let left_h = if *left { LINE } else { 0.0 };
            (
                file_title,
                vec![LINE, figure_h, INSPECTOR_BAR, left_h],
                0,
                none,
                false,
                0,
                None,
                0,
            )
        }
        InspectorShape::Done {
            info,
            smaller,
            again,
        } => (file_title, vec![], 0, Some(info), *smaller, *again, None, 0),
        InspectorShape::Failed { info } => (file_title, vec![], 0, Some(info), false, 0, None, 0),
        InspectorShape::Several {
            info,
            smaller,
            targets,
            more,
            buttons,
        } => (
            INSPECTOR_TITLE,
            vec![LINE],
            0,
            info.as_ref(),
            *smaller,
            *targets,
            *more,
            *buttons,
        ),
    };

    // Without the hero the stack starts at the title, and when the hero
    // is offered but doesn't fit, nothing after it does either.
    let hero = if hero { s.place(12.0, HERO.1) } else { None };
    let hero = s.or_empty(hero);
    let title = s.place(12.0, title_h);
    let title = s.or_empty(title);
    // The texts one under the next, the first 4 below the title. One with
    // nothing to show takes no room, and one without room is 0 tall.
    let mut lines = Vec::with_capacity(heights.len());
    let mut span: Option<(Rect, Rect)> = None;
    for h in heights {
        let at = if h > 0.0 { s.place(4.0, h) } else { None };
        if let Some(r) = at {
            span = Some(span.map_or((r, r), |(first, _)| (first, r)));
        }
        lines.push(s.or_empty(at));
    }
    let body = match span {
        Some((first, last)) => Rect::new(first.x, first.y, first.w, last.bottom() - first.y),
        None => s.or_empty(None),
    };

    let facts: Vec<Rect> = if facts {
        (0..fact_count)
            .map_while(|i| s.place(if i == 0 { 12.0 } else { 0.0 }, FACT_HEIGHT))
            .collect()
    } else {
        vec![]
    };

    // The InfoBar of a file that finished or failed comes after its name:
    // one order for every state.
    let bar = info.and_then(|i| s.info(16.0, i));

    let mut label = None;
    let mut smaller_at = None;
    let mut rows = vec![];
    if targets > 0 || smaller {
        label = s.place(16.0, LABEL);
        if smaller {
            smaller_at = s.place(8.0, TARGET_HEIGHT);
        }
        rows = (0..targets)
            .map_while(|i| {
                let gap = if i == 0 && smaller_at.is_none() {
                    8.0
                } else {
                    4.0
                };
                s.place(gap, TARGET_HEIGHT)
            })
            .collect();
        // A heading over nothing is worse than none.
        if rows.is_empty() && smaller_at.is_none() {
            label = None;
        }
    }
    // More's hit area is its words and a link's room past them, as wide
    // as the pointer, the focus ring and the tree all take it.
    let more = more.and_then(|w| {
        let line = s.place(4.0, MORE)?;
        Some(Rect::new(
            line.x,
            line.y,
            (w + LINK_PAD).min(line.w),
            line.h,
        ))
    });

    // The buttons in a row, as many as fit across the padded width; a
    // finished or failed file's InfoBar's actions are its buttons.
    let in_bar = matches!(
        shape,
        InspectorShape::Done { .. } | InspectorShape::Failed { .. }
    );
    let slots: Vec<Rect> = if in_bar {
        bar.as_ref().map(|b| b.actions.clone()).unwrap_or_default()
    } else {
        let step = INSPECTOR_BUTTON_WIDTH + 8.0;
        let across = (0..buttons)
            .take_while(|&i| i as f32 * step + INSPECTOR_BUTTON_WIDTH <= inner.w)
            .count();
        let row = if across > 0 {
            s.place(16.0, BUTTON_HEIGHT)
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
        slots
    };

    let layout = InspectorLayout {
        hero,
        title,
        lines,
        body,
        facts,
        info: bar,
        label,
        smaller: smaller_at,
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

/// Where the parts of phase 3's controls go (`crate::controls`), in
/// layout units, from the look spec and the mock: the measure helpers the
/// surfaces lay their controls out with. Text widths and wrapped heights
/// come in from DirectWrite as numbers or a closure, so these stay pure.
pub mod kit {
    use super::Rect;

    /// A button's text sits 12 in from either end; a glyph before it is
    /// 16 with 8 after it.
    pub const BUTTON_PAD: f32 = 12.0;
    pub const BUTTON_GLYPH: f32 = 16.0;
    pub const BUTTON_GLYPH_GAP: f32 = 8.0;
    /// An icon-only subtle button, as the title bar's.
    pub const ICON_BUTTON: (f32, f32) = (40.0, 32.0);
    /// Every control row is this tall.
    pub const CONTROL_HEIGHT: f32 = 32.0;
    /// A ToggleSwitch's "On"/"Off" is at least this wide, right-aligned,
    /// 12 before the track.
    pub const TOGGLE_WORD_MIN: f32 = 24.0;
    pub const TOGGLE_GAP: f32 = 12.0;
    /// A radio button's circle, the gap to its label, and its row.
    pub const RADIO: f32 = 20.0;
    pub const RADIO_GAP: f32 = 8.0;
    pub const RADIO_ROW: f32 = 32.0;
    pub const RADIO_ROW_GAP: f32 = 4.0;
    /// The InfoBar's padding across and down, its glyph and the glyph's
    /// gap to the text, the gap between its title and message on one line
    /// and before its actions, and its least height.
    pub const INFO_PAD_X: f32 = 16.0;
    pub const INFO_PAD_Y: f32 = 12.0;
    pub const INFO_GLYPH: f32 = 16.0;
    pub const INFO_GLYPH_GAP: f32 = 12.0;
    pub const INFO_TITLE_GAP: f32 = 8.0;
    pub const INFO_ACTIONS_GAP: f32 = 12.0;
    pub const INFO_MIN_HEIGHT: f32 = 48.0;
    pub const INFO_DISMISS: f32 = 32.0;
    /// A line of Body text.
    pub const LINE: f32 = 20.0;
    /// The two-tone focus ring: the outer ring 2 DIPs wide, 2 DIPs off the
    /// control, and the 1-DIP inner ring inside it.
    pub const FOCUS_OFF: f32 = 2.0;
    /// The ComboBox's least width.
    pub const COMBO_MIN_WIDTH: f32 = 140.0;
    /// A chevron's glyph, and a Dismiss button's.
    pub const CHEVRON: f32 = 12.0;
    /// A flyout's padding round its items, a ComboBox's or menu's item
    /// height, a menu's least width and a separator's height.
    pub const FLYOUT_PAD: f32 = 4.0;
    pub const ITEM_HEIGHT: f32 = 32.0;
    pub const MENU_MIN_WIDTH: f32 = 220.0;
    pub const SEPARATOR: f32 = 9.0;
    /// A menu item's accelerator sits at least this far after its label.
    pub const ACCEL_GAP: f32 = 24.0;
    /// An Expander's header row.
    pub const EXPANDER_HEADER: f32 = 48.0;
    /// A text box's text inset.
    pub const TEXT_PAD: f32 = 10.0;
    /// The ContentDialog: its width, how far it keeps from the window's
    /// edges, the gap under its title, and its action band's height.
    pub const DIALOG_WIDTH: f32 = 480.0;
    pub const DIALOG_MARGIN: f32 = 24.0;
    pub const DIALOG_TITLE_GAP: f32 = 12.0;
    pub const DIALOG_BAND: f32 = 80.0;
    pub const DIALOG_BUTTON_GAP: f32 = 8.0;
    /// The scroll indicator's thumb keeps this far from the edge, and is
    /// never shorter than this.
    pub const SCROLL_EDGE: f32 = 2.0;
    pub const SCROLL_MIN_THUMB: f32 = 24.0;

    /// A button's width for a label `label_w` wide, with a glyph before it
    /// or not.
    pub fn button_width(label_w: f32, glyph: bool) -> f32 {
        BUTTON_PAD * 2.0 + label_w + glyph_room(glyph)
    }

    fn glyph_room(glyph: bool) -> f32 {
        if glyph {
            BUTTON_GLYPH + BUTTON_GLYPH_GAP
        } else {
            0.0
        }
    }

    /// A button's glyph (if it has one) and label in `r`, centred as one.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct ButtonParts {
        pub glyph: Option<Rect>,
        pub label: Rect,
    }

    pub fn button_parts(r: Rect, label_w: f32, glyph: bool) -> ButtonParts {
        let room = glyph_room(glyph);
        let x = r.x + (r.w - label_w - room) / 2.0;
        ButtonParts {
            glyph: glyph.then(|| {
                Rect::new(
                    x,
                    r.y + (r.h - BUTTON_GLYPH) / 2.0,
                    BUTTON_GLYPH,
                    BUTTON_GLYPH,
                )
            }),
            label: Rect::new(x + room, r.y, label_w, r.h),
        }
    }

    /// A ToggleSwitch's word, track and knob.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct ToggleParts {
        pub word: Rect,
        pub track: Rect,
        pub knob: Rect,
    }

    /// A ToggleSwitch ending at `right`, centred on `cy`, its word `word_w`
    /// wide, its knob `knob` across and `at` of the way from off (0) to on
    /// (1).
    pub fn toggle_parts(right: f32, cy: f32, word_w: f32, knob: f32, at: f32) -> ToggleParts {
        let (tw, th) = crate::theme::fluent::space::TOGGLE;
        let track = Rect::new(right - tw, cy - th / 2.0, tw, th);
        let word_w = word_w.max(TOGGLE_WORD_MIN);
        let word = Rect::new(track.x - TOGGLE_GAP - word_w, track.y, word_w, th);
        // The knob's centre runs from 10 in from the left end to 10 in
        // from the right, as the mock's 12-DIP knob 4 from the rim.
        let inset = th / 2.0;
        let cx = track.x + inset + (tw - 2.0 * inset) * at.clamp(0.0, 1.0);
        ToggleParts {
            word,
            track,
            knob: Rect::new(cx - knob / 2.0, cy - knob / 2.0, knob, knob),
        }
    }

    /// One radio button: its row, circle and label.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct RadioParts {
        pub row: Rect,
        pub circle: Rect,
        pub label: Rect,
    }

    /// Radio buttons one under another from `(x, y)`, labels `labels`
    /// wide.
    pub fn radio_rows(x: f32, y: f32, labels: &[f32]) -> Vec<RadioParts> {
        labels
            .iter()
            .enumerate()
            .map(|(i, &w)| {
                let top = y + i as f32 * (RADIO_ROW + RADIO_ROW_GAP);
                RadioParts {
                    row: Rect::new(x, top, RADIO + RADIO_GAP + w, RADIO_ROW),
                    circle: Rect::new(x, top + (RADIO_ROW - RADIO) / 2.0, RADIO, RADIO),
                    label: Rect::new(x + RADIO + RADIO_GAP, top, w, RADIO_ROW),
                }
            })
            .collect()
    }

    /// A ProgressBar in `r` (3 tall): the 1-DIP track line through its
    /// middle and the lit part from `span.0` to `span.1` of the way.
    pub fn progress_parts(r: Rect, span: (f32, f32)) -> (Rect, Rect) {
        let line = Rect::new(r.x, r.y + (r.h - 1.0) / 2.0, r.w, 1.0);
        let a = span.0.clamp(0.0, 1.0);
        let b = span.1.clamp(a, 1.0);
        (line, Rect::new(r.x + r.w * a, r.y, r.w * (b - a), r.h))
    }

    /// An InfoBar laid out: the whole, its glyph, its title and message
    /// (on one line, `inline`, or the message wrapped under the title),
    /// its action buttons and its Dismiss button.
    #[derive(Debug, Clone, PartialEq)]
    pub struct InfoBarLayout {
        pub rect: Rect,
        pub glyph: Rect,
        pub title: Rect,
        pub message: Rect,
        pub inline: bool,
        pub actions: Vec<Rect>,
        pub dismiss: Option<Rect>,
    }

    /// An InfoBar at `(x, y)`, `w` wide: its title `title_w` wide on one
    /// line, its message `message_w` wide on one line or
    /// `message_h(width)` tall wrapped to `width`, its actions' widths,
    /// and whether it can be dismissed. Title and message share a line
    /// when they fit; otherwise the message wraps under the title at the
    /// width there is.
    #[allow(clippy::too_many_arguments)]
    pub fn info_bar(
        x: f32,
        y: f32,
        w: f32,
        title_w: f32,
        message_w: f32,
        message_h: &dyn Fn(f32) -> f32,
        actions: &[f32],
        dismiss: bool,
    ) -> InfoBarLayout {
        let glyph = Rect::new(
            x + INFO_PAD_X,
            y + INFO_PAD_Y + (LINE - INFO_GLYPH) / 2.0,
            INFO_GLYPH,
            INFO_GLYPH,
        );
        let tx = glyph.right() + INFO_GLYPH_GAP;
        let dismiss = dismiss.then(|| {
            Rect::new(
                x + w - 8.0 - INFO_DISMISS,
                y + 8.0,
                INFO_DISMISS,
                INFO_DISMISS,
            )
        });
        let right = dismiss.map_or(x + w - INFO_PAD_X, |d| d.x - 8.0);
        let tw = (right - tx).max(0.0);
        let ty = y + INFO_PAD_Y;
        let titled = title_w > 0.0;
        let gap = if titled { INFO_TITLE_GAP } else { 0.0 };
        let inline = title_w + gap + message_w <= tw;
        let (title, message) = if inline {
            let title = Rect::new(tx, ty, title_w, LINE);
            let mx = title.right() + gap;
            (title, Rect::new(mx, ty, (right - mx).max(0.0), LINE))
        } else {
            let title = Rect::new(tx, ty, tw, if titled { LINE } else { 0.0 });
            (title, Rect::new(tx, title.bottom(), tw, message_h(tw)))
        };
        let mut bottom = message.bottom().max(title.bottom());
        let mut buttons = Vec::with_capacity(actions.len());
        if !actions.is_empty() {
            let ay = bottom + INFO_ACTIONS_GAP;
            let mut ax = tx;
            let mut ay = ay;
            // Actions that don't fit across wrap to a line of their own.
            for &aw in actions {
                if ax > tx && ax + aw > x + w - INFO_PAD_X {
                    ax = tx;
                    ay += CONTROL_HEIGHT + 8.0;
                }
                buttons.push(Rect::new(ax, ay, aw, CONTROL_HEIGHT));
                ax += aw + 8.0;
            }
            bottom = ay + CONTROL_HEIGHT;
        }
        InfoBarLayout {
            rect: Rect::new(x, y, w, (bottom + INFO_PAD_Y - y).max(INFO_MIN_HEIGHT)),
            glyph,
            title,
            message,
            inline,
            actions: buttons,
            dismiss,
        }
    }

    /// The focus rings round a control `r` with corners of `radius`, for
    /// `Draw::border` (which draws inside the rect it is given): the outer
    /// ring's rect, corner and width, then the inner's.
    pub fn focus_rings(r: Rect, radius: f32) -> [(Rect, f32, f32); 2] {
        let grown = |d: f32| Rect::new(r.x - d, r.y - d, r.w + 2.0 * d, r.h + 2.0 * d);
        let outer = FOCUS_OFF + 2.0;
        [
            (grown(outer), radius + outer, 2.0),
            (grown(FOCUS_OFF), radius + FOCUS_OFF, 1.0),
        ]
    }

    /// Where a dialog's focus rings may be drawn: its content's place and
    /// as far round it as a ring stands off a control (`focus_rings`), so
    /// the sides of a control as wide as the content keep their ring while
    /// a control scrolled out of the content stays out of the title and
    /// the buttons.
    pub fn ring_clip(content: Rect) -> Rect {
        let d = FOCUS_OFF + 2.0;
        Rect::new(
            content.x - d,
            content.y - d,
            content.w + 2.0 * d,
            content.h + 2.0 * d,
        )
    }

    /// The scroll indicator's thumb in `track` (the list's height, at its
    /// right edge), `width` wide, for a view `view` tall over content
    /// `content` tall scrolled `scroll`: none when it all fits.
    pub fn scroll_thumb(
        track: Rect,
        view: f32,
        content: f32,
        scroll: f32,
        width: f32,
    ) -> Option<Rect> {
        if content <= view || view <= 0.0 {
            return None;
        }
        let h = (track.h * view / content)
            .max(SCROLL_MIN_THUMB)
            .min(track.h);
        let f = (scroll / (content - view)).clamp(0.0, 1.0);
        Some(Rect::new(
            track.right() - SCROLL_EDGE - width,
            track.y + (track.h - h) * f,
            width,
            h,
        ))
    }

    /// A theme swatch's dots, each a circle this wide, and the space
    /// between them.
    pub const SWATCH_DOT: f32 = 8.0;
    pub const SWATCH_GAP: f32 = 4.0;

    /// The room a list item gives its three swatch dots, and the space
    /// between them and the item's label.
    pub fn swatch_room() -> f32 {
        3.0 * SWATCH_DOT + 2.0 * SWATCH_GAP + 8.0
    }

    /// The three dots of a swatch at the right of list item `item`, with
    /// its padding kept, each centred on the item's middle.
    pub fn swatch_dots(item: Rect) -> [Rect; 3] {
        let first = item.right() - BUTTON_PAD - 3.0 * SWATCH_DOT - 2.0 * SWATCH_GAP;
        let y = item.y + (item.h - SWATCH_DOT) / 2.0;
        std::array::from_fn(|i| {
            Rect::new(
                first + i as f32 * (SWATCH_DOT + SWATCH_GAP),
                y,
                SWATCH_DOT,
                SWATCH_DOT,
            )
        })
    }

    /// A ComboBox's width for its widest choice `widest`.
    pub fn combo_width(widest: f32) -> f32 {
        (BUTTON_PAD + widest + 8.0 + CHEVRON + BUTTON_PAD).max(COMBO_MIN_WIDTH)
    }

    /// A spin box's parts, a value stepped back and forward: its value,
    /// then its back and forward chevrons at the right, each a square.
    pub fn spin_parts(r: Rect) -> (Rect, Rect, Rect) {
        let side = r.h;
        let next = Rect::new(r.right() - side, r.y, side, r.h);
        let back = Rect::new(next.x - side, r.y, side, r.h);
        let value = Rect::new(r.x + 12.0, r.y, (back.x - r.x - 12.0).max(0.0), r.h);
        (value, back, next)
    }

    /// A ComboBox in `r`: its value and its chevron's box.
    pub fn combo_parts(r: Rect) -> (Rect, Rect) {
        let chevron = Rect::new(r.right() - BUTTON_PAD - CHEVRON, r.y, CHEVRON, r.h);
        let x = r.x + BUTTON_PAD;
        (
            Rect::new(x, r.y, (chevron.x - 8.0 - x).max(0.0), r.h),
            chevron,
        )
    }

    /// An open flyout list: where it is, where each item it shows is, and
    /// the first item shown (a list too long for the window shows part).
    #[derive(Debug, Clone, PartialEq)]
    pub struct FlyoutLayout {
        pub rect: Rect,
        pub items: Vec<Rect>,
        pub first: usize,
    }

    /// A ComboBox's open list of `len` choices for the box `anchor`, with
    /// choice `selected` over the box as WinUI places it, kept inside
    /// `bounds`.
    pub fn combo_flyout(anchor: Rect, len: usize, selected: usize, bounds: Rect) -> FlyoutLayout {
        let w = anchor.w + 2.0 * FLYOUT_PAD;
        let x = (anchor.x - FLYOUT_PAD)
            .min(bounds.right() - w)
            .max(bounds.x);
        if len == 0 {
            return FlyoutLayout {
                rect: Rect::new(x, anchor.y - FLYOUT_PAD, w, 2.0 * FLYOUT_PAD),
                items: vec![],
                first: 0,
            };
        }
        let fit = (((bounds.h - 2.0 * FLYOUT_PAD) / ITEM_HEIGHT).floor() as usize).max(1);
        let shown = len.min(fit);
        let sel = selected.min(len - 1);
        let first = if shown < len {
            sel.saturating_sub(shown / 2).min(len - shown)
        } else {
            0
        };
        let h = shown as f32 * ITEM_HEIGHT + 2.0 * FLYOUT_PAD;
        let y = (anchor.y - FLYOUT_PAD - (sel - first) as f32 * ITEM_HEIGHT)
            .min(bounds.bottom() - h)
            .max(bounds.y);
        let items = (0..shown)
            .map(|i| {
                Rect::new(
                    x + FLYOUT_PAD,
                    y + FLYOUT_PAD + i as f32 * ITEM_HEIGHT,
                    w - 2.0 * FLYOUT_PAD,
                    ITEM_HEIGHT,
                )
            })
            .collect();
        FlyoutLayout {
            rect: Rect::new(x, y, w, h),
            items,
            first,
        }
    }

    /// A row of a menu flyout.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum MenuRow {
        /// An item: its label's width, its accelerator's (0 for none), and
        /// whether a glyph comes before it.
        Item {
            label: f32,
            accel: f32,
            glyph: bool,
        },
        Separator,
    }

    /// An open menu: where it is, and each row's rect.
    #[derive(Debug, Clone, PartialEq)]
    pub struct MenuLayout {
        pub rect: Rect,
        pub rows: Vec<Rect>,
    }

    /// A menu of `rows` opening at `at` (down and to the right), turned up
    /// or to the left where `bounds` has no room, and kept inside it.
    pub fn menu_flyout(at: (f32, f32), rows: &[MenuRow], bounds: Rect) -> MenuLayout {
        let content = rows
            .iter()
            .map(|r| match *r {
                MenuRow::Item {
                    label,
                    accel,
                    glyph,
                } => {
                    let g = if glyph {
                        BUTTON_GLYPH + BUTTON_PAD
                    } else {
                        0.0
                    };
                    let a = if accel > 0.0 { ACCEL_GAP + accel } else { 0.0 };
                    BUTTON_PAD + g + label + a + BUTTON_PAD
                }
                MenuRow::Separator => 0.0,
            })
            .fold(0.0f32, f32::max);
        let w = (content + 2.0 * FLYOUT_PAD).max(MENU_MIN_WIDTH);
        let height = |r: &MenuRow| match r {
            MenuRow::Item { .. } => ITEM_HEIGHT,
            MenuRow::Separator => SEPARATOR,
        };
        let h = rows.iter().map(height).sum::<f32>() + 2.0 * FLYOUT_PAD;
        let place = |at: f32, size: f32, lo: f32, hi: f32| {
            let v = if at + size > hi { at - size } else { at };
            v.min(hi - size).max(lo)
        };
        let x = place(at.0, w, bounds.x, bounds.right());
        let y = place(at.1, h, bounds.y, bounds.bottom());
        let mut top = y + FLYOUT_PAD;
        let rows = rows
            .iter()
            .map(|r| {
                let rh = height(r);
                let rect = Rect::new(x + FLYOUT_PAD, top, w - 2.0 * FLYOUT_PAD, rh);
                top += rh;
                rect
            })
            .collect();
        MenuLayout {
            rect: Rect::new(x, y, w, h),
            rows,
        }
    }

    /// An Expander: its header, title, chevron box, and its content while
    /// open.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct ExpanderParts {
        pub header: Rect,
        pub title: Rect,
        pub chevron: Rect,
        pub content: Option<Rect>,
    }

    /// An Expander at `(x, y)`, `w` wide, its content `content_h` tall
    /// shown `shown` of the way (0 closed, 1 open).
    pub fn expander_parts(x: f32, y: f32, w: f32, content_h: f32, shown: f32) -> ExpanderParts {
        let header = Rect::new(x, y, w, EXPANDER_HEADER);
        let chevron = Rect::new(x + w - 8.0 - 32.0, y + 8.0, 32.0, 32.0);
        let title = Rect::new(
            x + 16.0,
            y,
            (chevron.x - 8.0 - x - 16.0).max(0.0),
            EXPANDER_HEADER,
        );
        let shown = shown.clamp(0.0, 1.0);
        ExpanderParts {
            header,
            title,
            chevron,
            content: (shown > 0.0).then(|| Rect::new(x, header.bottom(), w, content_h * shown)),
        }
    }

    /// A ContentDialog laid out: the whole, its title, its content, the
    /// action band and the band's buttons.
    #[derive(Debug, Clone, PartialEq)]
    pub struct DialogLayout {
        pub rect: Rect,
        pub title: Rect,
        pub content: Rect,
        pub band: Rect,
        pub buttons: Vec<Rect>,
    }

    /// A dialog in a window `size`, its title `title_h` tall and content
    /// `content_h` tall, with `buttons` buttons sharing the band: centred,
    /// 480 wide or the window less 24 each side, its content cut to fit
    /// the content layer: it never reaches the title bar.
    pub fn dialog(size: (f32, f32), title_h: f32, content_h: f32, buttons: usize) -> DialogLayout {
        dialog_of_width(size, DIALOG_WIDTH, title_h, content_h, buttons)
    }

    /// The details dialog: one width for every dialog, so the engine's
    /// lines wrap as in the others.
    pub const DETAILS_WIDTH: f32 = DIALOG_WIDTH;
    /// The details text's card: its padding across and down.
    pub const DETAILS_TEXT_PAD: (f32, f32) = (12.0, 10.0);
    /// The tallest the card grows before its text scrolls.
    pub const DETAILS_TEXT_MAX: f32 = 320.0;
    /// Between the plain line and the card (mock screen 8's 12).
    pub const DETAILS_LEAD_GAP: f32 = 12.0;

    /// The details dialog laid out: the ContentDialog, the plain line at
    /// the top of its content, the card under it, and where the engine's
    /// text runs inside the card.
    #[derive(Debug, Clone, PartialEq)]
    pub struct DetailsDialog {
        pub frame: DialogLayout,
        pub lead: Rect,
        /// No taller than nothing when there is no engine text.
        pub card: Rect,
        pub text: Rect,
    }

    /// The width the details dialog's plain line is wrapped at in a window
    /// `w` wide: the content's.
    pub fn details_lead_width(w: f32) -> f32 {
        let pad = crate::theme::fluent::space::DIALOG_PADDING;
        (DETAILS_WIDTH.min(w - 2.0 * DIALOG_MARGIN) - 2.0 * pad).max(0.0)
    }

    /// The width the details text is wrapped at in a window `w` wide.
    pub fn details_text_width(w: f32) -> f32 {
        (details_lead_width(w) - 2.0 * DETAILS_TEXT_PAD.0).max(0.0)
    }

    /// The details dialog in a window `size`, its title `title_h` tall,
    /// its plain line `lead_h` tall and the engine's text `text_h` tall
    /// (0 for none, and then no card), with Copy and Close in the band.
    /// A window too short cuts the card first.
    pub fn details_dialog(
        size: (f32, f32),
        title_h: f32,
        lead_h: f32,
        text_h: f32,
    ) -> DetailsDialog {
        let (px, py) = DETAILS_TEXT_PAD;
        let card_h = if text_h > 0.0 {
            (text_h + 2.0 * py).min(DETAILS_TEXT_MAX)
        } else {
            0.0
        };
        let below = if card_h > 0.0 {
            DETAILS_LEAD_GAP + card_h
        } else {
            0.0
        };
        let frame = dialog_of_width(size, DETAILS_WIDTH, title_h, lead_h + below, 2);
        let c = frame.content;
        let lead = Rect::new(c.x, c.y, c.w, lead_h.min(c.h));
        let card = if card_h > 0.0 {
            let y = (lead.bottom() + DETAILS_LEAD_GAP).min(c.bottom());
            Rect::new(c.x, y, c.w, (c.bottom() - y).max(0.0))
        } else {
            Rect::new(c.x, c.bottom(), c.w, 0.0)
        };
        let text = Rect::new(
            card.x + px,
            card.y + py,
            (card.w - 2.0 * px).max(0.0),
            (card.h - 2.0 * py).max(0.0),
        );
        DetailsDialog {
            frame,
            lead,
            card,
            text,
        }
    }

    /// Help: one width for every dialog; 480 fits its two columns.
    pub const HELP_WIDTH: f32 = DIALOG_WIDTH;

    /// The width Help's content is laid out in, in a window `w` wide.
    pub fn help_content_width(w: f32) -> f32 {
        let pad = crate::theme::fluent::space::DIALOG_PADDING;
        (HELP_WIDTH.min(w - 2.0 * DIALOG_MARGIN) - 2.0 * pad).max(0.0)
    }

    /// Help in a window `size`: its title `title_h` tall over its content
    /// `content_h` tall, cut to fit the window (it scrolls there), and
    /// Close alone in the band.
    pub fn help_dialog(size: (f32, f32), title_h: f32, content_h: f32) -> DialogLayout {
        dialog_of_width(size, HELP_WIDTH, title_h, content_h, 1)
    }

    /// `dialog` for buttons whose labels are `labels` wide: they share
    /// the band when each fits its share, and otherwise stand one under
    /// another, each the band's whole width.
    pub fn fitting_dialog(
        size: (f32, f32),
        title_h: f32,
        content_h: f32,
        labels: &[f32],
    ) -> DialogLayout {
        let pad = crate::theme::fluent::space::DIALOG_PADDING;
        let inner = (DIALOG_WIDTH.min(size.0 - 2.0 * DIALOG_MARGIN) - 2.0 * pad).max(0.0);
        let n = labels.len().max(1) as f32;
        let share = (inner - DIALOG_BUTTON_GAP * (n - 1.0)) / n;
        let stacked = labels.iter().any(|&w| button_width(w, false) > share);
        laid_dialog(
            size,
            DIALOG_WIDTH,
            title_h,
            content_h,
            labels.len(),
            stacked,
        )
    }

    /// `dialog`, `width` wide at most.
    pub fn dialog_of_width(
        size: (f32, f32),
        width: f32,
        title_h: f32,
        content_h: f32,
        buttons: usize,
    ) -> DialogLayout {
        laid_dialog(size, width, title_h, content_h, buttons, false)
    }

    fn laid_dialog(
        size: (f32, f32),
        width: f32,
        title_h: f32,
        content_h: f32,
        buttons: usize,
        stacked: bool,
    ) -> DialogLayout {
        let pad = crate::theme::fluent::space::DIALOG_PADDING;
        let w = width.min(size.0 - 2.0 * DIALOG_MARGIN).max(0.0);
        let band_h = if stacked {
            let n = buttons.max(1) as f32;
            2.0 * pad + n * CONTROL_HEIGHT + (n - 1.0) * DIALOG_BUTTON_GAP
        } else {
            DIALOG_BAND
        };
        let fixed = pad + title_h + DIALOG_TITLE_GAP + pad + band_h;
        // It stands on the content layer, below the title bar, and keeps
        // its margin from the layer's top and bottom: a tall body scrolls
        // there.
        let layer = super::content_layer(size.0, size.1, super::TITLE_BAR);
        let room = (layer.h - 2.0 * DIALOG_MARGIN).max(fixed);
        let content_h = content_h.min(room - fixed).max(0.0);
        let h = fixed + content_h;
        let (x, y) = (
            (size.0 - w) / 2.0,
            (layer.y + (layer.h - h) / 2.0).max(layer.y),
        );
        let inner = (w - 2.0 * pad).max(0.0);
        let title = Rect::new(x + pad, y + pad, inner, title_h);
        let content = Rect::new(x + pad, title.bottom() + DIALOG_TITLE_GAP, inner, content_h);
        let band = Rect::new(x, y + h - band_h, w, band_h);
        let n = buttons.max(1) as f32;
        let bw = ((inner - DIALOG_BUTTON_GAP * (n - 1.0)) / n).max(0.0);
        // A dialog with one button gets the width a button has beside
        // another, at the right.
        let one = ((inner - DIALOG_BUTTON_GAP) / 2.0).max(0.0);
        let step = CONTROL_HEIGHT + DIALOG_BUTTON_GAP;
        let buttons = (0..buttons)
            .map(|i| {
                let i = i as f32;
                if stacked {
                    Rect::new(band.x + pad, band.y + pad + i * step, inner, CONTROL_HEIGHT)
                } else if n == 1.0 {
                    Rect::new(
                        band.x + pad + inner - one,
                        band.y + pad,
                        one,
                        CONTROL_HEIGHT,
                    )
                } else {
                    Rect::new(
                        band.x + pad + i * (bw + DIALOG_BUTTON_GAP),
                        band.y + pad,
                        bw,
                        CONTROL_HEIGHT,
                    )
                }
            })
            .collect();
        DialogLayout {
            rect: Rect::new(x, y, w, h),
            title,
            content,
            band,
            buttons,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::layout::{content_layer, TITLE_BAR};

        fn inside(a: &Rect, b: &Rect) -> bool {
            a.x >= b.x - 1e-3
                && a.y >= b.y - 1e-3
                && a.right() <= b.right() + 1e-3
                && a.bottom() <= b.bottom() + 1e-3
        }

        #[test]
        fn a_button_fits_its_label_and_glyph() {
            assert_eq!(button_width(40.0, false), 64.0);
            assert_eq!(button_width(40.0, true), 88.0);
            let r = Rect::new(10.0, 0.0, 120.0, 32.0);
            let p = button_parts(r, 40.0, true);
            let g = p.glyph.unwrap();
            assert_eq!((g.w, g.h), (16.0, 16.0));
            assert_eq!(g.y, 8.0);
            assert_eq!(p.label.x - g.right(), 8.0);
            // Centred as one: as much room before the glyph as after the
            // label.
            assert!((g.x - r.x - (r.right() - p.label.right())).abs() < 1e-3);
            let plain = button_parts(r, 40.0, false);
            assert_eq!(plain.glyph, None);
            assert_eq!(plain.label.x, 50.0);
            assert_eq!(plain.label.h, 32.0);
        }

        #[test]
        fn a_toggles_knob_travels_its_track() {
            let off = toggle_parts(200.0, 50.0, 20.0, 12.0, 0.0);
            assert_eq!(off.track, Rect::new(160.0, 40.0, 40.0, 20.0));
            assert_eq!(off.word.right(), 148.0, "12 before the track");
            assert_eq!(off.word.w, 24.0, "at least 24 wide");
            assert_eq!(off.knob, Rect::new(164.0, 44.0, 12.0, 12.0));
            let on = toggle_parts(200.0, 50.0, 20.0, 12.0, 1.0);
            assert_eq!(on.knob.right(), 196.0);
            let hover = toggle_parts(200.0, 50.0, 30.0, 14.0, 0.5);
            assert_eq!(hover.word.w, 30.0);
            assert!((hover.knob.x + 7.0 - 180.0).abs() < 1e-3, "centred halfway");
        }

        #[test]
        fn radio_buttons_stack_in_rows() {
            let rows = radio_rows(0.0, 10.0, &[30.0, 50.0]);
            assert_eq!(rows.len(), 2);
            assert_eq!(rows[0].circle, Rect::new(0.0, 16.0, 20.0, 20.0));
            assert_eq!(rows[0].label.x, 28.0);
            assert_eq!(rows[0].label.w, 30.0);
            assert_eq!(rows[1].row.y, 10.0 + 32.0 + 4.0);
            assert_eq!(rows[1].row.w, 28.0 + 50.0);
        }

        #[test]
        fn a_progress_bar_lights_its_part_over_a_thin_track() {
            let r = Rect::new(0.0, 10.0, 200.0, 3.0);
            let (line, lit) = progress_parts(r, (0.0, 0.42));
            assert_eq!(line, Rect::new(0.0, 11.0, 200.0, 1.0));
            assert_eq!(lit, Rect::new(0.0, 10.0, 84.0, 3.0));
            let (_, lit) = progress_parts(r, (0.25, 1.5));
            assert_eq!(lit, Rect::new(50.0, 10.0, 150.0, 3.0), "held to the track");
        }

        #[test]
        fn an_infobar_wraps_its_message_at_the_width_it_has() {
            // A message 20 tall per 300 DIPs of text, wrapped.
            let wrapped = |w: f32| 20.0 * (600.0 / w).ceil();
            let wide = info_bar(0.0, 0.0, 900.0, 100.0, 600.0, &wrapped, &[], false);
            assert!(wide.inline, "title and message on one line");
            assert_eq!(wide.title.y, wide.message.y);
            assert_eq!(wide.message.x, wide.title.right() + 8.0);
            assert_eq!(wide.rect.h, 48.0, "one line: the least height");
            assert_eq!(wide.glyph, Rect::new(16.0, 14.0, 16.0, 16.0));
            assert_eq!(wide.title.x, 44.0);
            let narrow = info_bar(0.0, 0.0, 360.0, 100.0, 600.0, &wrapped, &[], false);
            assert!(!narrow.inline);
            let text_w = 360.0 - 44.0 - 16.0;
            assert_eq!(narrow.message.w, text_w);
            assert_eq!(narrow.message.y, narrow.title.bottom());
            assert_eq!(narrow.message.h, wrapped(text_w));
            assert_eq!(narrow.rect.h, 12.0 + 20.0 + wrapped(text_w) + 12.0);
            // Narrower still, more lines.
            let narrower = info_bar(0.0, 0.0, 240.0, 100.0, 600.0, &wrapped, &[], false);
            assert!(narrower.rect.h > narrow.rect.h);
            // Actions sit in a row under the text; Dismiss takes room at the
            // right.
            let full = info_bar(
                0.0,
                0.0,
                360.0,
                100.0,
                600.0,
                &wrapped,
                &[80.0, 120.0],
                true,
            );
            assert_eq!(full.actions.len(), 2);
            assert_eq!(full.actions[0].y, full.message.bottom() + 12.0);
            assert_eq!(full.actions[0].x, full.title.x);
            assert_eq!(full.actions[1].x, full.actions[0].right() + 8.0);
            assert_eq!(full.rect.bottom(), full.actions[0].bottom() + 12.0);
            let d = full.dismiss.unwrap();
            assert_eq!(d.right(), 360.0 - 8.0);
            assert!(full.message.right() <= d.x - 8.0 + 1e-3);
            // No title: the message starts the text.
            let bare = info_bar(0.0, 0.0, 360.0, 0.0, 600.0, &wrapped, &[], false);
            assert_eq!(bare.message.y, bare.title.y);
        }

        #[test]
        fn a_dialogs_rings_for_a_control_as_wide_as_its_content_fit_the_ring_clip() {
            // an Expander is as wide as the dialog's content, so its
            // ring's sides are outside the content, inside the ring clip.
            let d = dialog((800.0, 700.0), 28.0, 300.0, 2);
            let e = expander_parts(d.content.x, d.content.y + 100.0, d.content.w, 0.0, 0.0);
            let [(outer, ..), _] = focus_rings(e.header, 4.0);
            let clip = ring_clip(d.content);
            assert!(outer.x < d.content.x && outer.right() > d.content.right());
            assert!(clip.x <= outer.x && clip.right() >= outer.right());
            assert!(clip.y <= outer.y && clip.bottom() >= outer.bottom());
            // It stays inside the dialog, clear of the title and the buttons.
            assert!(clip.x >= d.rect.x && clip.right() <= d.rect.right());
            assert!(clip.y >= d.title.bottom() && clip.bottom() <= d.band.y);
        }

        #[test]
        fn focus_rings_stand_off_the_control() {
            let [outer, inner] = focus_rings(Rect::new(10.0, 10.0, 100.0, 32.0), 4.0);
            assert_eq!(outer, (Rect::new(6.0, 6.0, 108.0, 40.0), 8.0, 2.0));
            assert_eq!(inner, (Rect::new(8.0, 8.0, 104.0, 36.0), 6.0, 1.0));
        }

        #[test]
        fn a_scroll_thumb_shows_where_the_view_is() {
            let track = Rect::new(0.0, 0.0, 300.0, 400.0);
            assert_eq!(scroll_thumb(track, 400.0, 400.0, 0.0, 2.0), None);
            let top = scroll_thumb(track, 400.0, 800.0, 0.0, 2.0).unwrap();
            assert_eq!(top, Rect::new(296.0, 0.0, 2.0, 200.0));
            let end = scroll_thumb(track, 400.0, 800.0, 400.0, 6.0).unwrap();
            assert_eq!(end, Rect::new(292.0, 200.0, 6.0, 200.0));
            let tiny = scroll_thumb(track, 400.0, 400_000.0, 0.0, 2.0).unwrap();
            assert_eq!(tiny.h, 24.0, "never shorter than the least");
            let past = scroll_thumb(track, 400.0, 800.0, 9999.0, 2.0).unwrap();
            assert_eq!(past.bottom(), 400.0);
        }

        #[test]
        fn a_combo_fits_its_widest_choice_and_puts_the_chevron_at_the_right() {
            assert_eq!(combo_width(50.0), 140.0);
            assert_eq!(combo_width(200.0), 12.0 + 200.0 + 8.0 + 12.0 + 12.0);
            let (value, chevron) = combo_parts(Rect::new(0.0, 0.0, 160.0, 32.0));
            assert_eq!(value.x, 12.0);
            assert_eq!(chevron, Rect::new(136.0, 0.0, 12.0, 32.0));
            assert_eq!(value.right(), chevron.x - 8.0);
        }

        #[test]
        fn a_combos_list_opens_with_the_value_over_the_box() {
            let bounds = Rect::new(0.0, 0.0, 800.0, 600.0);
            let anchor = Rect::new(100.0, 300.0, 160.0, 32.0);
            let f = combo_flyout(anchor, 5, 2, bounds);
            assert_eq!(f.items.len(), 5);
            assert_eq!(f.first, 0);
            assert_eq!(f.items[2].y, anchor.y, "the value sits over the box");
            assert_eq!(f.rect.x, anchor.x - 4.0);
            assert_eq!(f.rect.w, anchor.w + 8.0);
            assert_eq!(f.rect.h, 5.0 * 32.0 + 8.0);
            // Near the top, it's pushed down into the window.
            let high = combo_flyout(Rect::new(100.0, 10.0, 160.0, 32.0), 5, 4, bounds);
            assert_eq!(high.rect.y, 0.0);
            // Near the bottom, pushed up.
            let low = combo_flyout(Rect::new(100.0, 580.0, 160.0, 32.0), 5, 0, bounds);
            assert_eq!(low.rect.bottom(), 600.0);
            for i in &low.items {
                assert!(inside(i, &low.rect));
            }
            // Too many for the window: as many as fit, the value among them.
            let small = Rect::new(0.0, 0.0, 800.0, 200.0);
            let long = combo_flyout(Rect::new(100.0, 80.0, 160.0, 32.0), 40, 30, small);
            assert_eq!(long.items.len(), 6);
            assert!(long.first <= 30 && 30 < long.first + 6);
            assert!(inside(&long.rect, &small));
            // An empty list has no items.
            assert!(combo_flyout(anchor, 0, 0, bounds).items.is_empty());
        }

        #[test]
        fn a_menu_opens_down_and_right_unless_there_is_no_room() {
            let bounds = Rect::new(0.0, 0.0, 800.0, 600.0);
            let rows = [
                MenuRow::Item {
                    label: 80.0,
                    accel: 40.0,
                    glyph: true,
                },
                MenuRow::Separator,
                MenuRow::Item {
                    label: 300.0,
                    accel: 0.0,
                    glyph: false,
                },
            ];
            let m = menu_flyout((100.0, 100.0), &rows, bounds);
            assert_eq!((m.rect.x, m.rect.y), (100.0, 100.0));
            assert_eq!(m.rect.h, 4.0 + 32.0 + 9.0 + 32.0 + 4.0);
            assert_eq!(m.rect.w, 4.0 + 12.0 + 300.0 + 12.0 + 4.0);
            assert_eq!(m.rows[1].h, 9.0);
            assert_eq!(m.rows[2].y, m.rows[1].bottom());
            let short = menu_flyout((0.0, 0.0), &rows[..1], bounds);
            assert_eq!(short.rect.w, 220.0, "at least 220 wide");
            let corner = menu_flyout((790.0, 590.0), &rows, bounds);
            assert_eq!(corner.rect.right(), 790.0, "turned to the left");
            assert_eq!(corner.rect.bottom(), 590.0, "turned up");
            let squeezed = menu_flyout((5.0, 5.0), &rows, Rect::new(0.0, 0.0, 200.0, 60.0));
            assert!(squeezed.rect.x >= 0.0 && squeezed.rect.y >= 0.0);
        }

        #[test]
        fn an_expander_shows_its_content_as_it_opens() {
            let shut = expander_parts(0.0, 0.0, 400.0, 120.0, 0.0);
            assert_eq!(shut.header, Rect::new(0.0, 0.0, 400.0, 48.0));
            assert_eq!(shut.content, None);
            assert_eq!(shut.chevron, Rect::new(360.0, 8.0, 32.0, 32.0));
            assert_eq!(shut.title.x, 16.0);
            assert!(shut.title.right() <= shut.chevron.x);
            let open = expander_parts(0.0, 0.0, 400.0, 120.0, 1.0);
            assert_eq!(open.content, Some(Rect::new(0.0, 48.0, 400.0, 120.0)));
            let half = expander_parts(0.0, 0.0, 400.0, 120.0, 0.5);
            assert_eq!(half.content.unwrap().h, 60.0);
        }

        #[test]
        fn a_dialog_is_centred_with_its_buttons_sharing_the_band() {
            let d = dialog((800.0, 600.0), 28.0, 100.0, 3);
            assert_eq!(d.rect.w, 480.0);
            assert_eq!(d.rect.x, (800.0 - 480.0) / 2.0);
            let h = 24.0 + 28.0 + 12.0 + 100.0 + 24.0 + 80.0;
            assert_eq!(d.rect.h, h);
            // Centred on the content layer, under the title bar.
            let layer = content_layer(800.0, 600.0, TITLE_BAR);
            assert_eq!(d.rect.y, layer.y + (layer.h - h) / 2.0);
            assert_eq!(
                d.title,
                Rect::new(d.rect.x + 24.0, d.rect.y + 24.0, 432.0, 28.0)
            );
            assert_eq!(d.content.y, d.title.bottom() + 12.0);
            assert_eq!(d.band.h, 80.0);
            assert_eq!(d.band.bottom(), d.rect.bottom());
            assert_eq!(d.buttons.len(), 3);
            let bw = (432.0 - 16.0) / 3.0;
            for (i, b) in d.buttons.iter().enumerate() {
                assert!((b.w - bw).abs() < 1e-3);
                assert_eq!(b.h, 32.0);
                assert_eq!(b.y, d.band.y + 24.0);
                assert!((b.x - (d.band.x + 24.0 + i as f32 * (bw + 8.0))).abs() < 1e-3);
            }
            // A narrow window: 24 from each side.
            let n = dialog((400.0, 600.0), 28.0, 100.0, 2);
            assert_eq!(n.rect.w, 352.0);
            // A short window: the content is cut, the dialog fits.
            let s = dialog((800.0, 300.0), 28.0, 500.0, 2);
            assert!(s.content.h < 500.0);
            assert_eq!(s.band.bottom(), s.rect.bottom());
        }

        #[test]
        fn a_dialog_stays_on_the_content_layer_and_scrolls_its_body_sooner() {
            // an 1100 x 720 window with a tall dialog. It was
            // 634 tall at y = 43, over the 48-DIP title bar; now it keeps
            // 24 from the layer's top and bottom.
            let size = (1100.0, 720.0);
            let layer = content_layer(size.0, size.1, TITLE_BAR);
            let d = dialog(size, 28.0, 1000.0, 2);
            assert!(d.rect.y >= layer.y + DIALOG_MARGIN);
            assert!(d.rect.bottom() <= layer.bottom() - DIALOG_MARGIN + 1e-3);
            assert_eq!(d.rect.y, 72.0);
            assert_eq!(d.rect.h, 720.0 - 48.0 - 16.0 - 48.0);
            // The body scrolls from a shorter content than it did.
            // 720 - 48 (title bar) - 16 (edge) - 48 (margins) leaves 608,
            // less the dialog's own 168: 440 of body, where it was 504.
            let fits = dialog(size, 28.0, 440.0, 2);
            let cut = dialog(size, 28.0, 460.0, 2);
            assert!(fits.content.h == 440.0 && cut.content.h == 440.0);
            // Windows of any height keep it clear of the title bar.
            for h in [300.0, 480.0, 600.0, 900.0, 2000.0] {
                let d = dialog((900.0, h), 28.0, 5000.0, 2);
                assert!(d.rect.y >= TITLE_BAR, "{h}: {:?}", d.rect);
            }
        }

        #[test]
        fn one_dialog_width_for_every_dialog() {
            assert_eq!(DIALOG_WIDTH, 480.0);
            assert_eq!(HELP_WIDTH, DIALOG_WIDTH);
            assert_eq!(DETAILS_WIDTH, DIALOG_WIDTH);
            let widths = [
                dialog((900.0, 700.0), 28.0, 100.0, 2).rect.w,
                help_dialog((900.0, 700.0), 28.0, 100.0).rect.w,
                details_dialog((900.0, 700.0), 28.0, 20.0, 40.0)
                    .frame
                    .rect
                    .w,
                fitting_dialog((900.0, 700.0), 28.0, 20.0, &[50.0, 50.0])
                    .rect
                    .w,
            ];
            assert!(widths.iter().all(|&w| w == 480.0), "{widths:?}");
        }

        #[test]
        fn a_dialog_with_one_button_has_a_normal_width_one_at_the_right() {
            let d = dialog((900.0, 700.0), 28.0, 100.0, 1);
            let two = dialog((900.0, 700.0), 28.0, 100.0, 2);
            // As wide as a button beside another, and not the whole band.
            assert_eq!(d.buttons[0].w, two.buttons[0].w);
            assert_eq!(d.buttons[0].w, (432.0 - 8.0) / 2.0);
            // At the right, level with the band's padding.
            assert_eq!(d.buttons[0].right(), d.band.right() - 24.0);
            assert_eq!(d.buttons[0].y, two.buttons[0].y);
            assert_eq!(d.buttons[0].h, CONTROL_HEIGHT);
            // It is where the second of two is.
            assert_eq!(d.buttons[0], two.buttons[1]);
        }

        #[test]
        fn buttons_too_long_to_share_the_band_stand_one_under_another() {
            // Short labels share the band, as `dialog` lays them.
            let fits = fitting_dialog((800.0, 600.0), 28.0, 20.0, &[50.0, 50.0, 45.0]);
            assert_eq!(fits, dialog((800.0, 600.0), 28.0, 20.0, 3));
            // "Save next to the original" doesn't fit a third of the band:
            // each button takes the whole width, one under another, and
            // the band grows to hold them.
            let wide = 432.0 / 3.0;
            let d = fitting_dialog((800.0, 600.0), 28.0, 20.0, &[wide, 90.0, 50.0]);
            assert_eq!(d.buttons.len(), 3);
            let band = 24.0 + 3.0 * 32.0 + 2.0 * 8.0 + 24.0;
            assert_eq!(d.band.h, band);
            assert_eq!(d.band.bottom(), d.rect.bottom());
            assert_eq!(d.rect.h, 24.0 + 28.0 + 12.0 + 20.0 + 24.0 + band);
            for (i, b) in d.buttons.iter().enumerate() {
                assert_eq!(b.w, 432.0);
                assert_eq!(b.x, d.band.x + 24.0);
                assert_eq!(b.y, d.band.y + 24.0 + i as f32 * 40.0);
            }
            // Still within a short window.
            let s = fitting_dialog((800.0, 300.0), 28.0, 500.0, &[wide, 90.0, 50.0]);
            assert!(s.rect.y >= TITLE_BAR);
            assert_eq!(s.band.bottom(), s.rect.bottom());
        }

        #[test]
        fn help_is_the_one_dialog_width_with_close_at_the_right() {
            let d = help_dialog((900.0, 700.0), 28.0, 300.0);
            assert_eq!(d.rect.w, HELP_WIDTH);
            assert_eq!(d.rect.x, (900.0 - HELP_WIDTH) / 2.0);
            assert_eq!(d.content.w, help_content_width(900.0));
            assert_eq!(d.content.w, HELP_WIDTH - 48.0);
            assert_eq!(d.content.h, 300.0);
            // One button, a normal width, at the right.
            assert_eq!(d.buttons.len(), 1);
            assert_eq!(d.buttons[0].w, (d.content.w - DIALOG_BUTTON_GAP) / 2.0);
            assert_eq!(d.buttons[0].right(), d.content.right());
            // In a short window the content is cut, to scroll; the dialog
            // stays inside the window's margins.
            let short = help_dialog((900.0, 360.0), 28.0, 2000.0);
            assert!(short.content.h < 2000.0 && short.content.h > 0.0);
            assert!(short.rect.y >= TITLE_BAR + DIALOG_MARGIN);
            assert!(short.rect.bottom() <= 360.0 - DIALOG_MARGIN + 0.5);
            // A narrow window takes it narrower.
            let narrow = help_dialog((400.0, 700.0), 28.0, 100.0);
            assert_eq!(narrow.rect.w, 400.0 - 2.0 * DIALOG_MARGIN);
            assert_eq!(narrow.content.w, help_content_width(400.0));
        }

        #[test]
        fn the_details_dialog_is_wider_with_its_text_in_a_card() {
            let d = details_dialog((900.0, 700.0), 28.0, 20.0, 60.0);
            let f = &d.frame;
            assert_eq!(f.rect.w, DETAILS_WIDTH);
            assert_eq!(f.rect.x, (900.0 - DETAILS_WIDTH) / 2.0);
            // Copy, then Close, sharing the band.
            assert_eq!(f.buttons.len(), 2);
            assert!(f.buttons[0].x < f.buttons[1].x);
            // The plain line first, across the content (mock screen 8);
            // then the card, the text inside its padding, the card as tall
            // as the text and its padding.
            assert_eq!(
                d.lead,
                Rect::new(f.content.x, f.content.y, f.content.w, 20.0)
            );
            assert_eq!(d.card.y, d.lead.bottom() + DETAILS_LEAD_GAP);
            assert_eq!(d.card.bottom(), f.content.bottom());
            assert_eq!(d.text.x, d.card.x + DETAILS_TEXT_PAD.0);
            assert_eq!(d.text.y, d.card.y + DETAILS_TEXT_PAD.1);
            assert_eq!(d.text.w, f.content.w - 2.0 * DETAILS_TEXT_PAD.0);
            assert_eq!(
                f.content.h,
                20.0 + DETAILS_LEAD_GAP + 60.0 + 2.0 * DETAILS_TEXT_PAD.1
            );
            assert_eq!(d.text.h, 60.0);
            // With no engine text there is no card.
            let bare = details_dialog((900.0, 700.0), 28.0, 20.0, 0.0);
            assert_eq!(bare.frame.content.h, 20.0);
            assert_eq!(bare.card.h, 0.0);
            // A long text is cut to the most the card shows, and scrolls.
            let long = details_dialog((900.0, 900.0), 28.0, 20.0, 5000.0);
            assert_eq!(long.card.h, DETAILS_TEXT_MAX);
            assert_eq!(long.text.h, DETAILS_TEXT_MAX - 2.0 * DETAILS_TEXT_PAD.1);
            // A short window cuts it further, and the dialog fits.
            let short = details_dialog((900.0, 360.0), 28.0, 20.0, 5000.0);
            assert!(short.frame.rect.y >= TITLE_BAR);
            assert!(short.text.h >= 0.0);
            // A narrow window: 24 from each side.
            let narrow = details_dialog((400.0, 700.0), 28.0, 20.0, 60.0);
            assert_eq!(narrow.frame.rect.w, 352.0);
            // The widths the line and the text are measured at.
            assert_eq!(details_lead_width(900.0), DETAILS_WIDTH - 48.0);
            assert_eq!(
                details_text_width(900.0),
                DETAILS_WIDTH - 48.0 - 2.0 * DETAILS_TEXT_PAD.0
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SHOW_IN_FOLDER;

    #[test]
    fn an_infobar_at_the_top_moves_the_rows_down_under_it() {
        let l = layout(1100.0, 720.0, 3);
        let t = l.clone().with_top_info(48.0);
        let slot = t.top_info.unwrap();
        // 8 above the bar, 8 below it, and the rows where they were
        // with the 4-DIP list padding and the old 12 below.
        assert_eq!(
            (slot.x, slot.y, slot.w, slot.h),
            (l.list.x, l.panel.y + 8.0, l.list.w, 48.0)
        );
        assert_eq!(slot.y - l.panel.y, 8.0);
        assert_eq!(t.list.y - slot.bottom(), 8.0);
        assert_eq!(TOP_INFO_GAP, 8.0);
        assert_eq!(t.list.y, l.list.y + 48.0 + 12.0);
        assert_eq!(t.list.bottom(), l.list.bottom());
        assert_eq!(t.rows[0].y, t.list.y);
        assert_eq!(t.rows[2].y - t.rows[1].y, ROW_HEIGHT);
        // Nothing to show: as it was.
        assert_eq!(l.clone().with_top_info(0.0), l);
        // The empty window (mock: the InfoBar sits in the layer whatever
        // it shows): 16 inside the layer, the drop panel under it.
        let empty = layout(1100.0, 720.0, 0);
        assert_eq!(empty.clone().with_top_info(0.0), empty);
        let e = empty.clone().with_top_info(48.0);
        let slot = e.top_info.unwrap();
        assert_eq!(
            slot,
            Rect::new(
                empty.body.x + 16.0,
                empty.body.y + 16.0,
                empty.body.w - 32.0,
                48.0
            )
        );
        assert_eq!(e.body, empty.body);
        assert_eq!(e.panel.y, slot.bottom());
        assert_eq!(e.panel.bottom(), empty.panel.bottom());
    }

    fn on_grid(v: f32) -> bool {
        (v / GRID).fract() == 0.0
    }

    fn assert_grid(r: &Rect, what: &str) {
        for v in [r.x, r.y, r.w, r.h] {
            assert!(on_grid(v), "{what} {r:?} is off the 4 px grid");
        }
    }

    // Task 18: the list on the content layer (mock screens 2 and 5).

    #[test]
    fn the_footer_has_its_status_then_add_files_clear_list_and_convert() {
        for (w, h) in [(720.0, 480.0), (1100.0, 720.0), (480.0, 360.0)] {
            {
                for several in [false, true] {
                    let at = format!("{w} several {several}");
                    let foot = layout(w, h, 3).footer.unwrap();
                    let f = footer_row(&foot, several);
                    // Left to right (mock `.foot`): the status, Add files,
                    // Clear list, then Convert at the right, 8 apart and 16
                    // in from the end.
                    let buttons = vec![f.add, f.clear, f.convert];
                    assert_eq!(f.convert.right(), foot.right() - FOOTER_PAD, "{at}");
                    for pair in buttons.windows(2) {
                        assert_eq!(pair[1].x - pair[0].right(), 8.0, "{at}");
                    }
                    for b in &buttons {
                        // 32 tall, centred in the 64-DIP footer, inside it.
                        assert_eq!(b.h, 32.0, "{at}");
                        assert_eq!(b.y - foot.y, foot.bottom() - b.bottom(), "{at}");
                        assert!(b.x >= foot.x + FOOTER_PAD, "{at}: {b:?}");
                        assert_grid(b, &at);
                    }
                    // Convert is wide enough for "Convert 120 files" when
                    // several are selected.
                    let want = if several { CONVERT_MANY } else { CONVERT_ONE };
                    assert_eq!(f.convert.w, want, "{at}");
                    // The status runs from the footer's padding to 12 short
                    // of Add files.
                    assert_eq!(f.status.x, foot.x + FOOTER_PAD, "{at}");
                    assert!(f.status.right() <= f.add.x - 12.0, "{at}");
                    assert_eq!((f.status.y, f.status.h), (foot.y, foot.h), "{at}");
                }
            }
        }
        // In a roomy footer Add files has its glyph and its words; with a
        // Convert as wide as its longest words in the smallest window it
        // keeps only its glyph, so nothing sticks out of the footer.
        let roomy = footer_row(&layout(720.0, 480.0, 3).footer.unwrap(), true);
        assert_eq!(roomy.add.w, ADD_FILES_WIDTH);
        assert!(!roomy.add_glyph_only);
        let tight = footer_row_for(&layout(480.0, 360.0, 3).footer.unwrap(), 268.0);
        assert!(tight.add_glyph_only);
        assert_eq!(tight.add.w, 40.0);
    }

    #[test]
    fn the_list_sits_on_the_content_layer_with_the_footer_across_its_foot() {
        for (w, h) in [(720.0, 480.0), (1100.0, 720.0), (480.0, 360.0)] {
            let l = layout(w, h, 3);
            let layer = content_layer(w, h, TITLE_BAR);
            assert_eq!(l.body, layer, "{w}");
            let foot = l.footer.unwrap();
            // The footer: 64 tall across the layer's bottom (mock `.foot`).
            assert_eq!(
                (foot.x, foot.w, foot.h, foot.bottom()),
                (layer.x, layer.w, FOOTER_HEIGHT, layer.bottom())
            );
            assert_eq!(FOOTER_HEIGHT, 64.0);
            // The list and the pane fill what is above it.
            assert_eq!(l.panel.bottom(), foot.y);
            assert_eq!((l.panel.x, l.panel.w), (layer.x, layer.w));
            // The rows sit 4 inside, one under the next, 56 tall.
            assert_eq!(l.rows[0].x, l.panel.x + LIST_PAD);
            assert_eq!(l.rows[0].y, l.panel.y + LIST_PAD);
            assert_eq!(l.rows[2].y, l.rows[0].y + 2.0 * ROW_HEIGHT);
            assert_eq!(ROW_HEIGHT, 56.0);
        }
        // Under 600 wide the layer meets the window's edges.
        let l = layout(480.0, 360.0, 3);
        assert_eq!((l.body.x, 480.0 - l.body.right()), (0.0, 0.0));
        let l = layout(720.0, 480.0, 3);
        assert_eq!((l.body.x, 720.0 - l.body.right()), (16.0, 16.0));
        assert_eq!(480.0 - l.body.bottom(), 16.0);
    }

    #[test]
    fn a_wide_window_fills_and_its_pane_widens_from_320_to_400() {
        // The pane folds below 760, as before.
        let narrow = layout(759.0, 560.0, 3);
        assert!(narrow.inspector.is_none());
        assert_eq!(narrow.list, narrow.panel.inset(LIST_PAD));
        let at = |w: f32| layout(w, 720.0, 3).inspector.unwrap();
        assert_eq!(at(760.0).w, 320.0);
        // The mock's 1100-wide window has a 340-wide pane.
        assert_eq!(at(1100.0).w, 340.0);
        assert_eq!(at(2000.0).w, 400.0);
        assert_eq!(at(3000.0).w, 400.0);
        let mut last = 0.0;
        for w in (760..3000).step_by(40) {
            let pw = at(w as f32).w;
            assert!((320.0..=400.0).contains(&pw) && pw >= last, "{w}: {pw}");
            assert!(on_grid(pw), "{w}: {pw}");
            last = pw;
        }
        // The list takes the rest: no column cap in a wide window.
        let wide = layout(3000.0, 1200.0, 3);
        let pane = wide.inspector.unwrap();
        assert_eq!(wide.body.w, 3000.0 - 32.0);
        assert_eq!(pane.right(), wide.panel.right());
        assert_eq!(wide.list.right() + LIST_PAD, pane.x);
        assert_eq!(wide.rows[0].w, wide.list.w);
    }

    #[test]
    fn a_row_is_its_thumbnail_then_its_name_over_its_facts() {
        let row = Rect::new(20.0, 52.0, 600.0, ROW_HEIGHT);
        let p = row_preview(&row);
        // A 40-DIP thumbnail 12 in from the left, centred, 4-DIP corners.
        assert_eq!((p.x, p.w, p.h), (row.x + 12.0, 40.0, 40.0));
        assert_eq!(p.y - row.y, row.bottom() - p.bottom());
        assert_eq!(PREVIEW_RADIUS, 4.0);
        // The text starts 12 after it.
        assert_eq!(row_text_left(&row), p.right() + 12.0);
        // The name (20) over the facts (16), centred together; with a
        // progress bar under them, the three are centred.
        for bar in [false, true] {
            let name = row_name(&row, bar);
            let line = row_line_at(&row, bar);
            assert_eq!((name.h, line.h), (20.0, 16.0));
            assert_eq!(line.y, name.bottom());
            let bottom = if bar {
                row_bar(&row, line.right()).bottom()
            } else {
                line.bottom()
            };
            assert!(
                ((name.y - row.y) - (row.bottom() - bottom)).abs() <= 1.0,
                "{bar}"
            );
        }
        // The bar: 3 tall, 6 under the facts, and as wide as the text
        // runs: to the percentage, with no limit of its own.
        let bar = row_bar(&row, row.right());
        assert_eq!(bar.h, 3.0);
        assert_eq!(bar.y, row_line_at(&row, true).bottom() + 6.0);
        assert_eq!(bar.right(), row.right());
        assert_eq!(row_bar(&row, row_text_left(&row) + 100.0).w, 100.0);
        let (end, _) = row_text_ends(&row, true, false, None, false, false);
        assert_eq!(row_bar(&row, end).right(), row_percent(&row).x - ROW_PAD);
        // A click on the thumbnail is a click on the row.
        let mid = (p.x + p.w / 2.0, p.y + p.h / 2.0);
        assert_eq!(
            row_part(&row, mid.0, mid.1, true, Some(SHOW_IN_FOLDER), true),
            RowPart::Row
        );
    }

    #[test]
    fn a_rows_buttons_sit_at_its_right_end_each_a_target_of_its_own() {
        let l = layout(720.0, 480.0, 3);
        let (r, next) = (l.rows[0], l.rows[1]);
        let cancel = row_cancel(&r);
        let pct = row_percent(&r);
        let show = Some(SHOW_IN_FOLDER);
        let link = row_link(&r, SHOW_IN_FOLDER);
        let options = row_options(&r);
        let all = [
            cancel,
            pct,
            link,
            options,
            row_retry(&r, show),
            row_retry(&r, None),
            row_link_at(&r, SHOW_IN_FOLDER, true),
        ];
        for c in all {
            // 32-DIP subtle buttons, centred, inside the row's padding.
            assert_eq!(c.h, 32.0, "{c:?}");
            assert_eq!(c.y - r.y, r.bottom() - c.bottom(), "{c:?}");
            assert!(c.right() <= r.right() - 12.0, "{c:?}");
            assert!(apart(&c, &next));
        }
        // Cancel at the right end, the percentage just before it.
        assert_eq!(cancel.right(), r.right() - 12.0);
        assert_eq!(cancel.x - pct.right(), ROW_GAP);
        // The link at the right end; Retry before it, or alone at the end.
        assert_eq!(link.right(), r.right() - 12.0);
        assert_eq!(link.x - row_retry(&r, show).right(), ROW_GAP);
        assert_eq!(row_retry(&r, None).right(), link.right());
        // Options at the right end, the link before it.
        assert_eq!(options.right(), r.right() - 12.0);
        assert_eq!(
            options.x - row_link_at(&r, SHOW_IN_FOLDER, true).right(),
            ROW_GAP
        );
        // Each is its own target; between them is the row.
        let mid = |c: Rect| (c.x + c.w / 2.0, c.y + c.h / 2.0);
        let (x, y) = mid(cancel);
        assert_eq!(row_part(&r, x, y, true, None, false), RowPart::Cancel);
        assert_eq!(row_part(&r, x, y, false, None, false), RowPart::Row);
        let (x, y) = mid(link);
        assert_eq!(row_part(&r, x, y, false, show, false), RowPart::Link);
        assert_eq!(row_part(&r, x, y, false, None, false), RowPart::Row);
        let retry = row_retry(&r, show);
        let (x, y) = mid(retry);
        assert_eq!(
            row_part_with_retry(&r, x, y, false, show, false, true),
            RowPart::Retry
        );
        assert_eq!(
            row_part_with_retry(&r, retry.right() + 1.0, y, false, show, false, true),
            RowPart::Row
        );
        let (x, y) = mid(options);
        assert_eq!(
            row_part_full(&r, x, y, false, show, false, false, true),
            RowPart::Options
        );
        let (x, y) = mid(row_link_at(&r, SHOW_IN_FOLDER, true));
        assert_eq!(
            row_part_full(&r, x, y, false, show, false, false, true),
            RowPart::Link
        );
        // The name and the facts end short of whatever is at the right end
        // by a gap, and a row with a mark keeps the mark's room too.
        for (running, retry, link, opts) in [
            (true, false, None, false),
            (false, true, show, false),
            (false, false, show, true),
            (false, false, None, false),
        ] {
            let start = row_right_start(&r, running, retry, link, opts);
            let (name, line) = row_text_ends(&r, running, retry, link, opts, true);
            assert_eq!(name, line);
            assert_eq!(start - line, ROW_GAP + MARK_WIDTH + ROW_PAD);
            assert!(row_mark(&r, start).x >= line);
            let (bare, _) = row_text_ends(&r, running, retry, link, opts, false);
            let something = running || retry || link.is_some() || opts;
            assert_eq!(start - bare, if something { ROW_PAD } else { 0.0 });
        }
        // Even at the smallest window a row with a link has room to say
        // something.
        let (w, h) = MIN_WINDOW;
        let small = layout(w, h, 3).rows[0];
        let (end, _) = row_text_ends(&small, false, true, show, false, true);
        assert!(
            end - row_text_left(&small) >= 40.0,
            "{}",
            end - row_text_left(&small)
        );
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
                assert_grid(&l.body, "body");
                assert_grid(&l.panel, "panel");
                // The rule before the pane is the pane's own edge, so the
                // list and its rows stand on the grid (Task 18).
                assert_grid(&l.list, "list");
                for r in &l.rows {
                    assert_grid(r, "row");
                }
                assert_eq!(l.inspector.is_some(), rows > 0 && inspector_fits(w));
                if let Some(i) = &l.inspector {
                    assert_grid(i, "inspector");
                }
                if let Some(f) = &l.footer {
                    assert_grid(f, "footer");
                    for several in [false, true] {
                        let row = footer_row(f, several);
                        for b in [row.add, row.clear, row.convert] {
                            assert_grid(&b, "button");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_window_lays_out_under_a_title_bar_of_48_dips_at_any_text_size() {
        for zoom in [1.0, 1.25, 1.5, 2.25] {
            let top = TITLE_BAR / zoom;
            for rows in [0, 3] {
                let l = layout_under(720.0, 480.0, rows, 1.0, top);
                // The content layer starts right at the bar, with files or
                // without (Task 18).
                assert_eq!(l.body.y, top, "{zoom}");
                assert_eq!(480.0 - l.body.bottom(), 16.0, "{zoom}");
                assert_eq!(l.panel.y, l.body.y, "{zoom}");
                if rows > 0 {
                    assert_eq!(l.rows[0].y, l.body.y + LIST_PAD, "{zoom}");
                }
            }
        }
        // At 100% it is the plain layout.
        assert_eq!(
            layout_under(720.0, 480.0, 3, 1.0, TITLE_BAR),
            layout_with(720.0, 480.0, 3, 1.0)
        );
        // Nothing nonsensical moves the content off the window's top.
        assert_eq!(layout_under(720.0, 480.0, 3, 0.0, f32::NAN).body.y, 0.0);
        assert_eq!(layout_under(720.0, 480.0, 0, 0.0, f32::NAN).body.y, 0.0);
    }

    #[test]
    fn empty_state_is_one_drop_panel() {
        let l = layout(720.0, 480.0, 0);
        assert_eq!(l.panel, l.body);
        assert!(l.footer.is_none() && l.rows.is_empty());
    }

    #[test]
    fn the_empty_window_is_the_content_layer_under_the_title_bar() {
        // 16 from the sides and the bottom, right up to the title bar.
        let l = layout_under(720.0, 480.0, 0, 0.0, 48.0);
        assert_eq!(l.panel, Rect::new(16.0, 48.0, 688.0, 416.0));
        assert_eq!(content_layer(720.0, 480.0, 48.0), l.panel);
        // Below 600 wide it meets the window's edges.
        assert_eq!(
            content_layer(590.0, 480.0, 48.0),
            Rect::new(0.0, 48.0, 590.0, 432.0)
        );
        assert_eq!(
            content_layer(600.0, 480.0, 48.0),
            Rect::new(16.0, 48.0, 568.0, 416.0)
        );
    }

    /// Each line of a first-run card that wraps: the second and third
    /// steps in two lines, the right-click line in one.
    const WRAPPED: CardText = CardText {
        steps: [20.0, 40.0, 40.0],
        more: 16.0,
    };

    #[test]
    fn the_empty_group_reads_top_to_bottom_centred_on_the_layer() {
        for w in [600.0, 720.0, 1024.0, 1440.0, 2048.0] {
            for h in [480.0, 640.0, 900.0, 1152.0] {
                let layer = content_layer(w, h, 48.0);
                for card in [None, Some(CardText::ONE_LINE), Some(WRAPPED)] {
                    let at = format!("{w}x{h} {card:?}");
                    let e = empty_state(&layer, card);
                    assert_eq!(e.layer, layer);
                    let mut order: Vec<Rect> = e.mark.into_iter().collect();
                    order.extend([e.headline, e.body, e.button, e.status]);
                    order.extend(e.card.map(|c| c.card));
                    for pair in order.windows(2) {
                        assert!(pair[1].y >= pair[0].bottom(), "{at}: {pair:?}");
                    }
                    // Centred across the layer, and inside it.
                    let cx = layer.x + layer.w / 2.0;
                    for r in &order {
                        assert!((r.x + r.w / 2.0 - cx).abs() <= GRID, "{at}: {r:?}");
                        assert!(r.x >= layer.x && r.right() <= layer.right(), "{at}");
                    }
                    // The K is 64 across, 24 over the headline; the
                    // headline is a Subtitle line, the body a Body line, 4
                    // apart; Add files 24 under the body.
                    if let Some(m) = e.mark {
                        assert_eq!((m.w, m.h), (64.0, 64.0));
                        assert_eq!(e.headline.y, m.bottom() + 24.0, "{at}");
                    }
                    assert_eq!(e.headline.h, 28.0);
                    assert_eq!((e.body.y, e.body.h), (e.headline.bottom() + 4.0, 20.0));
                    assert_eq!(e.button.y, e.body.bottom() + 24.0);
                    assert_eq!((e.button.h, e.button.w), (32.0, ADD_FILES_EMPTY_WIDTH));
                    assert_eq!(e.status.y, e.button.bottom() + 8.0);
                    // The group is on the grid at its top, and centred
                    // up and down where it fits.
                    let top = order[0].y;
                    let bottom = order.last().unwrap().bottom();
                    assert!(on_grid(top), "{at}: {top}");
                    if bottom <= layer.bottom() - EMPTY_PAD {
                        let (above, below) = (top - layer.y, layer.bottom() - bottom);
                        assert!((above - below).abs() <= 2.0 * GRID, "{at}");
                    }
                    // The drop outline, 12 inside the layer.
                    assert_eq!(e.drop, layer.inset(12.0));
                }
            }
        }
    }

    #[test]
    fn the_first_run_card_lays_its_steps_out_as_the_mock() {
        let layer = content_layer(1100.0, 720.0, 48.0);
        let e = empty_state(&layer, Some(WRAPPED));
        let c = e.card.unwrap();
        // 32 under Add files (the status line sits in the gap), 520 wide.
        assert_eq!(c.card.y, e.button.bottom() + 32.0);
        assert_eq!(c.card.w, 520.0);
        // Padding 16 on every side; the heading's line; Dismiss 8 in
        // from the top right, and the heading stops short of it.
        assert_eq!((c.title.x, c.title.y), (c.card.x + 16.0, c.card.y + 16.0));
        assert_eq!(c.title.h, 20.0);
        assert_eq!(
            c.dismiss,
            Rect::new(c.card.right() - 48.0, c.card.y + 8.0, 40.0, 32.0)
        );
        assert!(c.title.right() <= c.dismiss.x);
        // Each step: its number's 20-DIP circle, 12 before its words, 12
        // between steps and under the heading, each as tall as its lines.
        let (step_w, more_w) = first_run_widths(layer.w);
        assert_eq!((step_w, more_w), (520.0 - 32.0 - 32.0, 520.0 - 32.0));
        assert_eq!(c.steps[0].y, c.title.bottom() + 12.0);
        for k in 0..3 {
            assert_eq!(c.numbers[k], Rect::new(c.title.x, c.steps[k].y, 20.0, 20.0));
            assert_eq!(c.steps[k].x, c.title.x + 32.0);
            assert_eq!((c.steps[k].w, c.steps[k].h), (step_w, WRAPPED.steps[k]));
            if k > 0 {
                assert_eq!(c.steps[k].y, c.steps[k - 1].bottom() + 12.0);
            }
        }
        // The right-click line 12 under the steps, and the padding under it.
        assert_eq!((c.more.x, c.more.w), (c.title.x, more_w));
        assert_eq!(c.more.y, c.steps[2].bottom() + 12.0);
        assert_eq!(c.card.bottom(), c.more.bottom() + 16.0);
        // A height that isn't a number is one line.
        let odd = CardText {
            steps: [f32::NAN, 0.0, f32::INFINITY],
            more: -3.0,
        };
        let c = empty_state(&layer, Some(odd)).card.unwrap();
        assert_eq!(c.steps.map(|r| r.h), [20.0; 3]);
        assert_eq!(c.more.h, 16.0);
    }

    #[test]
    fn a_narrow_layer_narrows_the_card_and_wraps_its_lines_sooner() {
        let layer = content_layer(480.0, 600.0, 48.0);
        let c = empty_state(&layer, Some(WRAPPED)).card.unwrap();
        assert_eq!(c.card.w, 480.0 - 48.0);
        assert!(c.card.x >= layer.x + EMPTY_PAD - 1.0);
        assert_eq!(first_run_widths(480.0).1, 480.0 - 48.0 - 32.0);
        assert_eq!(first_run_widths(10.0), (0.0, 0.0));
    }

    #[test]
    fn the_default_window_shows_the_k_with_the_first_run_card() {
        // Kiln opens at 880 x 600, where the K, the text and the first-run
        // card all fit, wrapped or not.
        assert_eq!(DEFAULT_WINDOW, (880.0, 600.0));
        let (w, h) = DEFAULT_WINDOW;
        let layer = content_layer(w, h, TITLE_BAR);
        for card in [None, Some(CardText::ONE_LINE), Some(WRAPPED)] {
            let e = empty_state(&layer, card);
            assert!(e.mark.is_some(), "{card:?}");
            if let Some(c) = e.card {
                assert!(c.card.bottom() <= layer.bottom(), "{card:?}");
            }
        }
        assert!(MIN_WINDOW.0 <= w && MIN_WINDOW.1 <= h);
    }

    #[test]
    fn a_short_layer_loses_the_k_first_then_starts_at_the_top() {
        // Tall enough for it all: the K shows.
        let tall = content_layer(1100.0, 720.0, 48.0);
        assert!(empty_state(&tall, Some(WRAPPED)).mark.is_some());
        // Without the card the old default window (720 x 480) keeps the
        // K; with it, smaller windows than the default still drop it.
        let default = content_layer(720.0, 480.0, 48.0);
        assert!(empty_state(&default, None).mark.is_some());
        // The text and card fit without the K: it goes, and the rest
        // stays centred.
        let e = empty_state(&default, Some(WRAPPED));
        assert!(e.mark.is_none());
        let c = e.card.unwrap();
        let (above, below) = (e.headline.y - default.y, default.bottom() - c.card.bottom());
        assert!((above - below).abs() <= 2.0 * GRID, "{above} {below}");
        // Shorter still: the group starts at the top, and the card runs on
        // past the layer's bottom.
        let short = content_layer(480.0, 360.0, 48.0);
        let e = empty_state(&short, Some(WRAPPED));
        assert!(e.mark.is_none());
        assert_eq!(e.headline.y, short.y + EMPTY_PAD);
        assert!(e.card.unwrap().card.bottom() > short.bottom());
        // At 200% Text size the window's smallest is the same in layout
        // units: the same holds.
        let zoom = text_zoom(2.0, min_window(2.0));
        let (w, h) = (min_window(2.0).0 / zoom, min_window(2.0).1 / zoom);
        let e = empty_state(&content_layer(w, h, 48.0), Some(WRAPPED));
        assert!(e.headline.y >= 48.0 && e.mark.is_none());
    }

    #[test]
    fn text_size_zooms_the_layout() {
        // Fonts multiply by the factor, in pixels at any display scale.
        // A 13-DIP font.
        let caption = 13.0;
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

        // The empty window, with and without the first-run card. The card
        // can run past a short window's bottom, so its parts are only
        // checked against each other there.
        let l = layout(w, h, 0);
        for card in [None, Some(CardText::ONE_LINE)] {
            let e = empty_state(&l.panel, card);
            let mut rects: Vec<(String, Rect)> = vec![
                ("headline".into(), e.headline),
                ("body".into(), e.body),
                ("button".into(), e.button),
                ("status".into(), e.status),
            ];
            rects.extend(e.mark.map(|r| ("mark".into(), r)));
            all_apart(&rects);
            if let Some(c) = e.card {
                let mut parts: Vec<(String, Rect)> = vec![
                    ("dismiss".into(), c.dismiss),
                    ("title".into(), c.title),
                    ("more".into(), c.more),
                ];
                for k in 0..3 {
                    parts.push((format!("number {k}"), c.numbers[k]));
                    parts.push((format!("step {k}"), c.steps[k]));
                }
                for (i, (a, ra)) in parts.iter().enumerate() {
                    for (b, rb) in &parts[i + 1..] {
                        assert!(apart(ra, rb), "{a} overlaps {b} at {scale}");
                    }
                }
                assert!(c.card.y >= e.status.bottom(), "at {scale}");
            }
        }

        // The list, with every row control and the footer.
        let l = layout(w, h, 4);
        let f = footer_row(&l.footer.unwrap(), true);
        let mut rects: Vec<(String, Rect)> = vec![
            ("clear".into(), f.clear),
            ("add".into(), f.add),
            ("convert".into(), f.convert),
            ("status".into(), f.status),
        ];
        let visible: Vec<&Rect> = l
            .rows
            .iter()
            .filter(|r| r.bottom() <= l.panel.bottom())
            .collect();
        assert!(visible.len() >= 2, "two rows show at {scale}");
        for (i, r) in visible.into_iter().enumerate() {
            let link = row_link(r, SHOW_IN_FOLDER);
            rects.push((format!("row {i} link"), link));
            assert!(link.bottom() <= r.bottom() && row_cancel(r).bottom() <= r.bottom());
            assert!(
                r.x >= l.list.x && r.right() <= l.list.right(),
                "row in list"
            );
            // The preview sits inside its row, with room for the chip.
            let p = row_preview(r);
            assert!(p.x >= r.x && p.y >= r.y && p.bottom() <= r.bottom());
            assert!(p.right() + PREVIEW_GAP < link.x, "row {i} preview");
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
        let hp = kit::help_dialog((w, h), 28.0, 300.0);
        all_apart(&[
            ("title".into(), hp.title),
            ("body".into(), hp.content),
            ("close".into(), hp.buttons[0]),
        ]);

        // Settings, with every Expander open and every helper missing:
        // no control on another, and no card's words under a control.
        let st = settings(
            content_layer(w, h, TITLE_BAR),
            0.0,
            &settings_tests::full_page(),
            &Estimate,
        );
        // What the page shows at its top; the rest scrolls.
        let rects: Vec<(String, Rect)> = st
            .controls
            .iter()
            .filter(|(c, r)| {
                !matches!(c, Control::Helper(_))
                    && r.y >= st.view.y
                    && r.bottom() <= st.view.bottom()
            })
            .map(|(c, r)| (format!("{c:?}"), *r))
            .collect();
        all_apart(&rects);
        for card in &st.cards {
            for (c, r) in &rects {
                assert!(apart(&card.label, r), "{:?} overlaps {c}", card.card);
            }
        }

        // The options dialog at its fullest, More options open: what its
        // content shows and its band's buttons (the rest scrolls).
        let mut shape = sheet_shape(&[Knob::Preset, Knob::Size, Knob::Fps, Knob::AudioBitrate]);
        shape.more_open = true;
        let sh = options_dialog((w, h), &shape);
        let shows = |r: &Rect| inside(r, &sh.frame.content) || sh.frame.buttons.contains(r);
        let mut rects: Vec<(String, Rect)> = sh
            .controls
            .iter()
            .filter(|(_, r)| shows(r))
            .map(|(c, r)| (format!("{c:?}"), *r))
            .collect();
        if shows(&sh.result) {
            rects.push(("result".into(), sh.result));
        }
        all_apart(&rects);

        // The wheel pointed at from each corner: it stays whole and its
        // shadow's room inside the content layer.
        use crate::wheel::{place, R_OUTER, SHADOW_GAP};
        let layer = content_layer(w, h, TITLE_BAR);
        for extent in [R_OUTER] {
            for (x, y) in [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)] {
                let ((cx, cy), s) = place(x, y, layer, extent);
                let r = extent * s;
                assert!(s > 0.0 && s <= 1.0, "wheel scale {s} at {scale}");
                for (c, lo, hi) in [(cx, layer.x, layer.right()), (cy, layer.y, layer.bottom())] {
                    assert!(c - r >= lo + SHADOW_GAP - 0.01, "wheel {extent} at {scale}");
                    assert!(c + r <= hi - SHADOW_GAP + 0.01, "wheel {extent} at {scale}");
                }
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
    fn nothing_overlaps_at_150_and_225_percent_text_size() {
        // The default window and the smallest, and the size the window
        // takes on a 2560 x 1440 screen at 125%, at both factors.
        for scale in [1.5, MAX_TEXT_SCALE] {
            assert_nothing_overlaps(scale, min_window(scale));
            assert_nothing_overlaps(scale, default_window(scale));
            assert_nothing_overlaps(scale, (1980.0, 1145.0));
        }
        // The default window is the same in units at every factor.
        for scale in [1.5, MAX_TEXT_SCALE] {
            let (w, h) = default_window(scale);
            let zoom = text_zoom(scale, (w, h));
            assert_eq!(zoom, scale);
            assert_eq!((w / zoom, h / zoom), DEFAULT_WINDOW);
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

    fn apart(a: &Rect, b: &Rect) -> bool {
        a.right() <= b.x || b.right() <= a.x || a.bottom() <= b.y || b.bottom() <= a.y
    }

    #[test]
    fn the_footer_status_ends_short_of_the_buttons() {
        // DirectWrite caption widths of the full and short idle hints.
        const FULL: f32 = 303.0;
        const SHORT: f32 = 155.0;
        let status = |w: f32, several: bool| {
            let foot = layout(w, 480.0, 2).footer.unwrap();
            let f = footer_row(&foot, several);
            assert_eq!(f.add.x - f.status.right(), 12.0);
            f.status.w
        };
        // The default window has room for the full hint, and for the short
        // one while results are held and Save all takes its share.
        assert!(status(720.0, false) >= FULL);
        assert!(status(720.0, true) >= SHORT);
        // The smallest keeps a short status beside the buttons.
        assert!(status(480.0, false) >= 100.0);
        assert!(status(480.0, true) >= 48.0);
        // A row's line ends inside the row's padding.
        let r = layout(720.0, 480.0, 1).rows[0];
        let line = row_line(&r);
        assert!(line.x >= r.x + 16.0 && line.right() <= r.right() - ROW_PAD);
        assert!(line.y >= r.y && line.bottom() <= r.bottom());
    }

    #[test]
    fn half_shown_slides_from_the_right() {
        let l = layout_with(1000.0, 600.0, 3, 0.5);
        assert_eq!(l.inspector.unwrap().w, pane_width(l.body.w) * 0.5);
    }

    #[test]
    fn nothing_jumps_while_the_inspector_slides_in() {
        // Across the slide in steps of 0.05, the column, the footer and
        // the rows move no faster than the
        // inspector grows, plus a grid step for snapping the centred
        // column. A step in the column's cap at the start would be a jump
        // of 300 DIPs.
        let most = crate::theme::fluent::space::SIDE_PANE.1 * 0.05 + GRID;
        for w in [1000.0, 1300.0, 2000.0, 3000.0] {
            let at = |i: usize| layout_with(w, 600.0, 3, i as f32 * 0.05);
            let numbers = |l: &Layout| {
                [
                    l.body.x,
                    l.body.w,
                    l.body.right(),
                    l.footer.unwrap().right(),
                    footer_row(&l.footer.unwrap(), false).clear.x,
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
        // Whatever the window is shown, no files means no inspector: the
        // empty window is the whole content layer.
        let l = layout_with(2000.0, 900.0, 0, 1.0);
        assert!(l.inspector.is_none());
        assert_eq!(
            (l.list, l.body),
            (l.panel, content_layer(2000.0, 900.0, TITLE_BAR))
        );
    }

    #[test]
    fn the_inspector_slides_in_from_the_panels_right_edge() {
        for shown in [0.25, 0.5, 0.75, 1.0] {
            let l = layout_with(1000.0, 600.0, 3, shown);
            let i = l.inspector.unwrap();
            assert_eq!(i.right(), l.panel.right());
            assert_eq!((i.y, i.h), (l.panel.y, l.panel.h));
            // The list gives up what the pane takes.
            assert_eq!(l.list.right() + LIST_PAD, i.x);
            assert_eq!(l.list.x, l.panel.x + LIST_PAD);
        }
        // Folded away it leaves the list whole; shown too far in either
        // direction is kept in range.
        let folded = layout_with(2000.0, 900.0, 3, 0.0);
        assert!(folded.inspector.is_none());
        assert_eq!(folded.list, folded.panel.inset(LIST_PAD));
        assert_eq!(folded.rows[0].w, folded.list.w);
        let w = |shown| layout_with(1000.0, 600.0, 3, shown).inspector.map(|i| i.w);
        assert_eq!(w(2.0), Some(320.0));
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

    /// How wide "More formats and tools" is as a link, near enough.
    const MORE_W: f32 = 168.0;

    /// An InfoBar with a two-line message and `n` actions of the widths a
    /// pane's buttons have.
    fn bar(n: usize) -> InfoShape {
        let widths = [112.0, 64.0, 84.0, 136.0];
        let mut actions = [0.0; 4];
        actions[..n].copy_from_slice(&widths[..n]);
        InfoShape {
            message_h: 40.0,
            titled: true,
            actions,
            count: n,
        }
    }

    #[test]
    fn a_healthy_file_has_no_kind_line_and_a_reason_keeps_the_slot() {
        // nothing under the file's name for a healthy file; the line
        // is there only for a reason.
        let waiting = |reason: Option<(String, crate::model::Tone)>| Inspector::Waiting {
            row: 0,
            facts: vec![],
            targets: vec![],
            more: false,
            helper: None,
            reason,
            smaller: false,
        };
        let measure = |_: &str, _: Wrapped| 32.0;
        let button = |_: &str| 80.0;
        let what_h = |m: Inspector| match inspector_shape(&m, "", 0.0, &measure, &button) {
            InspectorShape::Waiting { what_h, .. } => what_h,
            other => panic!("{other:?}"),
        };
        assert_eq!(what_h(waiting(None)), 0.0);
        let why = (
            "Needs FFmpeg to become MOV".to_owned(),
            crate::model::Tone::Muted,
        );
        assert_eq!(what_h(waiting(Some(why))), 32.0);
    }

    /// Every shape the side pane can take, with the most parts it has.
    fn every_shape() -> Vec<InspectorShape> {
        vec![
            InspectorShape::Nothing {
                title_h: 56.0,
                hint_h: 64.0,
            },
            InspectorShape::Waiting {
                what_h: 32.0,
                facts: 4,
                smaller: true,
                targets: 5,
                more: Some(MORE_W),
                buttons: 0,
            },
            // A file waiting on its helper app: Get where its formats
            // would be.
            InspectorShape::Waiting {
                what_h: 32.0,
                facts: 4,
                smaller: false,
                targets: 0,
                more: None,
                buttons: 1,
            },
            InspectorShape::Converting {
                figure: true,
                left: true,
            },
            InspectorShape::Converting {
                figure: false,
                left: false,
            },
            InspectorShape::Done {
                info: bar(3),
                smaller: false,
                again: 3,
            },
            InspectorShape::Failed { info: bar(3) },
            InspectorShape::Several {
                info: None,
                smaller: true,
                targets: 5,
                more: Some(MORE_W),
                buttons: 0,
            },
            InspectorShape::Several {
                info: Some(InfoShape {
                    titled: false,
                    ..bar(0)
                }),
                smaller: false,
                targets: 2,
                more: None,
                buttons: 0,
            },
        ]
    }

    /// The parts of a pane layout that stand, in reading order, one to a
    /// line: an InfoBar stands as one, and a row of buttons as its first.
    fn standing(l: &InspectorLayout) -> Vec<(String, Rect)> {
        let mut parts = vec![];
        let bar = l.info.as_ref().map(|b| ("info".to_string(), b.rect));
        parts.push(("hero".to_string(), l.hero));
        parts.push(("title".to_string(), l.title));
        let numbered = |name: &str, rs: &[Rect]| -> Vec<(String, Rect)> {
            rs.iter()
                .enumerate()
                .map(|(i, r)| (format!("{name} {i}"), *r))
                .collect()
        };
        parts.extend(numbered("line", &l.lines));
        parts.extend(numbered("fact", &l.facts));
        parts.extend(bar);
        parts.extend(l.label.map(|r| ("label".to_string(), r)));
        parts.extend(l.smaller.map(|r| ("smaller".to_string(), r)));
        parts.extend(numbered("target", &l.targets));
        parts.extend(l.more.map(|r| ("more".to_string(), r)));
        if l.info.is_none() {
            parts.extend(l.buttons.first().map(|r| ("button".to_string(), *r)));
        }
        parts.retain(|(_, r)| r.h > 0.0);
        parts
    }

    #[test]
    fn every_pane_stacks_its_parts_inside_its_padding_without_overlap() {
        for w in [320.0, 360.0, 400.0] {
            for h in [160.0, 300.0, 480.0, 640.0, 900.0] {
                let r = Rect::new(900.0, 72.0, w, h);
                let inner = r.inset(PANEL_PADDING);
                for (k, shape) in every_shape().iter().enumerate() {
                    let l = inspector_layout(&r, shape);
                    // One order for every state: the bar after the name.
                    let parts = standing(&l);
                    for pair in parts.windows(2) {
                        let ((a, ra), (b, rb)) = (&pair[0], &pair[1]);
                        assert!(rb.y >= ra.bottom(), "{w}x{h} shape {k}: {b} overlaps {a}");
                    }
                    for (name, p) in &parts {
                        assert!(
                            p.x >= inner.x
                                && p.right() <= inner.right()
                                && p.y >= inner.y
                                && p.bottom() <= inner.bottom(),
                            "{w}x{h} shape {k}: {name} {p:?} outside {inner:?}"
                        );
                    }
                    for b in &l.buttons {
                        assert!(
                            b.right() <= inner.right() && b.bottom() <= inner.bottom(),
                            "{w}x{h} shape {k}: a button sticks out"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_failed_pane_has_the_order_of_the_others_picture_name_bar() {
        // one order for every state: the picture, the name, the
        // status bar with its actions, and then the list.
        let r = Rect::new(900.0, 72.0, 320.0, 600.0);
        let l = inspector_layout(&r, &InspectorShape::Failed { info: bar(3) });
        let info = l.info.clone().unwrap();
        assert_eq!((l.hero.x, l.hero.y), (r.x + 16.0, r.y + 16.0));
        assert!(l.title.y > l.hero.bottom());
        assert_eq!(info.rect.y, l.title.bottom() + 16.0);
        // The same place a finished file's bar takes under its name.
        let done = inspector_layout(
            &r,
            &InspectorShape::Done {
                info: bar(3),
                smaller: false,
                again: 0,
            },
        );
        assert_eq!(done.info.unwrap().rect.y, info.rect.y);
        // Its actions are the pane's buttons, inside the InfoBar.
        assert_eq!(l.buttons, info.actions);
        assert_eq!(l.buttons.len(), 3);
        for b in &l.buttons {
            assert!(b.bottom() <= info.rect.bottom() && b.right() <= info.rect.right());
        }
    }

    #[test]
    fn a_long_file_name_takes_two_lines_of_the_title_and_the_rest_moves_down() {
        // the title is as tall as its name wraps, at most two
        // lines of Subtitle; a name that fits on one line is as before.
        let r = Rect::new(900.0, 72.0, 320.0, 700.0);
        let shape = InspectorShape::Waiting {
            what_h: 0.0,
            facts: 2,
            smaller: false,
            targets: 2,
            more: None,
            buttons: 0,
        };
        let one = inspector_layout(&r, &shape);
        assert_eq!(one.title.h, INSPECTOR_TITLE);
        let two = inspector_layout_titled(&r, &shape, Some(2.0 * INSPECTOR_TITLE));
        assert_eq!(two.title.h, 56.0);
        assert_eq!(two.title.y, one.title.y);
        assert_eq!(two.facts[0].y, one.facts[0].y + INSPECTOR_TITLE);
        let same = inspector_layout_titled(&r, &shape, Some(INSPECTOR_TITLE));
        assert_eq!(same, one);
        // The height is measured for a file's pane and not for several
        // files' or for nothing selected.
        let measure = |_: &str, how: Wrapped| match how {
            Wrapped::Title(n) => 28.0 * n as f32,
            _ => 20.0,
        };
        let waiting = Inspector::Waiting {
            row: 0,
            facts: vec![],
            targets: vec![],
            more: false,
            helper: None,
            reason: None,
            smaller: false,
        };
        assert_eq!(
            inspector_title_height(&waiting, "long.jpg", &measure),
            Some(56.0)
        );
        assert_eq!(
            inspector_title_height(&Inspector::Nothing, "", &measure),
            None
        );
    }

    #[test]
    fn the_facts_are_20_apart_with_a_72_label_column() {
        // off the grid at 18, and a column for "Dimensions", the widest label.
        assert_eq!(FACT_HEIGHT, 20.0);
        assert_eq!(FACT_LABEL, 72.0);
        let r = Rect::new(900.0, 72.0, 320.0, 700.0);
        let l = inspector_layout(
            &r,
            &InspectorShape::Waiting {
                what_h: 0.0,
                facts: 4,
                smaller: false,
                targets: 0,
                more: None,
                buttons: 0,
            },
        );
        assert_eq!(l.facts.len(), 4);
        for pair in l.facts.windows(2) {
            assert_eq!(pair[1].y - pair[0].y, 20.0);
        }
        assert!(l.facts.iter().all(|f| f.h == 20.0));
    }

    #[test]
    fn a_format_rows_words_start_in_one_column_after_a_52_wide_code() {
        // the code is in a column of its own whatever its width.
        assert_eq!(OFFER_CODE, 52.0);
        let b = Rect::new(916.0, 300.0, 288.0, 36.0);
        let code = offer_code_column(&b);
        assert_eq!((code.x, code.w), (b.x + 12.0, 52.0));
        assert_eq!(offer_words_x(&b), b.x + 12.0 + 52.0 + 8.0);
        // Every row of a list has them at the same offset.
        let c = Rect::new(916.0, 340.0, 288.0, 36.0);
        assert_eq!(offer_words_x(&c) - c.x, offer_words_x(&b) - b.x);
    }

    #[test]
    fn a_block_of_text_sits_in_the_middle_of_its_room() {
        let room = Rect::new(10.0, 100.0, 200.0, 44.0);
        let one = centred_block(&room, 16.0);
        assert_eq!((one.x, one.y, one.w, one.h), (10.0, 114.0, 200.0, 16.0));
        let two = centred_block(&room, 32.0);
        assert_eq!((two.y, two.h), (106.0, 32.0));
        // Taller than the room: the room's top, and no more than the room.
        let tall = centred_block(&room, 60.0);
        assert_eq!((tall.y, tall.h), (100.0, 44.0));
    }

    #[test]
    fn a_stack_of_pictures_is_inset_12_whatever_the_count() {
        // the block of cards keeps its width and place for two files or
        // three; only the cards in it are narrower.
        let hero = Rect::new(916.0, 88.0, 288.0, HERO.1);
        let two = card_stack(&hero, 2);
        let three = card_stack(&hero, 3);
        assert_eq!((two.len(), three.len()), (2, 3));
        let span = |cards: &[Rect]| {
            let left = cards.iter().map(|c| c.x).fold(f32::MAX, f32::min);
            let right = cards.iter().map(|c| c.right()).fold(f32::MIN, f32::max);
            (left, right)
        };
        assert_eq!(span(&two), (hero.x + 12.0, hero.right() - 12.0));
        assert_eq!(span(&three), span(&two));
        for cards in [&two, &three] {
            for c in cards.iter() {
                assert!(c.y >= hero.y && c.bottom() <= hero.bottom(), "{c:?}");
            }
            // Each card is a step right and down from the one behind.
            for pair in cards.windows(2) {
                assert_eq!(pair[1].x - pair[0].x, CARD_STEP.0);
                assert_eq!(pair[1].y - pair[0].y, CARD_STEP.1);
            }
        }
    }

    #[test]
    fn a_done_pane_shows_its_infobar_then_convert_again() {
        let r = Rect::new(900.0, 72.0, 320.0, 700.0);
        let l = inspector_layout(
            &r,
            &InspectorShape::Done {
                info: bar(3),
                smaller: false,
                again: 3,
            },
        );
        let info = l.info.clone().unwrap();
        assert!(info.rect.y > l.title.bottom());
        assert_eq!(l.buttons, info.actions);
        let label = l.label.unwrap();
        assert!(label.y > info.rect.bottom());
        assert_eq!(l.targets.len(), 3);
        assert!(l.targets[0].y > label.bottom());
    }

    #[test]
    fn an_infobars_actions_wrap_inside_a_narrow_pane() {
        // Three actions 112, 64 and 84 wide don't fit on one line in a
        // pane 320 wide, whose InfoBar's text starts 44 in.
        let r = Rect::new(0.0, 0.0, 320.0, 600.0);
        let l = inspector_layout(&r, &InspectorShape::Failed { info: bar(3) });
        let info = l.info.unwrap();
        let rows: std::collections::BTreeSet<u32> =
            info.actions.iter().map(|b| b.y as u32).collect();
        assert!(rows.len() > 1, "{:?}", info.actions);
        for b in &info.actions {
            assert!(b.right() <= info.rect.right() - kit::INFO_PAD_X);
            assert!(b.x >= info.title.x);
        }
        // A pane 400 wide holds Retry and Show details on one line.
        let wide = inspector_layout(
            &Rect::new(0.0, 0.0, 400.0, 600.0),
            &InspectorShape::Failed { info: bar(2) },
        );
        let a = wide.info.unwrap().actions;
        assert_eq!(a[0].y, a[1].y);
    }

    #[test]
    fn a_converting_pane_says_what_it_does_then_its_percent_bar_and_time() {
        let r = Rect::new(900.0, 72.0, 320.0, 600.0);
        let l = inspector_layout(
            &r,
            &InspectorShape::Converting {
                figure: true,
                left: true,
            },
        );
        let (doing, figure, bar, left) = (
            l.line(0).unwrap(),
            l.line(1).unwrap(),
            l.line(2).unwrap(),
            l.line(3).unwrap(),
        );
        assert!(doing.bottom() <= figure.y && figure.bottom() <= bar.y && bar.bottom() <= left.y);
        assert_eq!((figure.h, bar.h), (INSPECTOR_FIGURE, INSPECTOR_BAR));
        assert_eq!(bar.w, r.w - 32.0);
        // Progress only: the row has the Cancel.
        assert!(l.buttons.is_empty());
        // Until the engine says how far along, no figure and no time.
        let unknown = inspector_layout(
            &r,
            &InspectorShape::Converting {
                figure: false,
                left: false,
            },
        );
        assert!(unknown.line(1).is_none() && unknown.line(3).is_none());
        assert!(unknown.line(2).is_some());
    }

    #[test]
    fn make_smaller_comes_first_under_convert_to() {
        let r = Rect::new(900.0, 72.0, 400.0, 900.0);
        let l = inspector_layout(&r, &every_shape()[1]);
        let (label, smaller) = (l.label.unwrap(), l.smaller.unwrap());
        assert!(smaller.y > label.bottom());
        assert!(l.targets[0].y >= smaller.bottom());
        assert_eq!(l.targets.len(), 5);
        assert!(l.more.is_some());
        // Without it the formats come first.
        let without = inspector_layout(
            &r,
            &InspectorShape::Waiting {
                what_h: 32.0,
                facts: 4,
                smaller: false,
                targets: 5,
                more: Some(MORE_W),
                buttons: 0,
            },
        );
        assert!(without.smaller.is_none());
        assert_eq!(without.targets[0].y, smaller.y);
    }

    #[test]
    fn a_short_pane_gives_up_its_picture_and_facts_before_its_actions() {
        let shape = every_shape()[1];
        let full = inspector_layout(&Rect::new(0.0, 0.0, 320.0, 2000.0), &shape);
        assert!(full.hero.h > 0.0 && full.facts.len() == 4);
        let mut saw_no_hero = false;
        for h in (200..900).step_by(20) {
            let l = inspector_layout(&Rect::new(0.0, 0.0, 320.0, h as f32), &shape);
            // While a target is cut, the picture and the facts are gone.
            if l.targets.len() < 5 {
                assert!(l.hero.h == 0.0 && l.facts.is_empty(), "at {h}");
            }
            saw_no_hero |= l.hero.h == 0.0 && l.targets.len() == 5;
        }
        assert!(saw_no_hero);
    }

    #[test]
    fn the_text_runs_inside_the_padding_and_an_infobars_inside_its_own() {
        assert_eq!(inspector_text_width(320.0), 288.0);
        assert_eq!(inspector_text_width(400.0), 368.0);
        assert_eq!(info_text_width(320.0), 288.0 - 32.0 - 16.0 - 12.0);
        let r = Rect::new(0.0, 0.0, 320.0, 600.0);
        let l = inspector_layout(&r, &InspectorShape::Failed { info: bar(0) });
        assert_eq!(l.info.unwrap().message.w, info_text_width(320.0));
    }

    #[test]
    fn a_page_of_the_list_is_a_screenful_less_one() {
        assert_eq!(rows_per_page(ROW_HEIGHT * 8.0), 7);
        assert_eq!(rows_per_page(ROW_HEIGHT * 1.0), 1);
        assert_eq!(rows_per_page(0.0), 1);
    }

    #[test]
    fn the_options_dialog_reads_top_down_with_convert_then_cancel() {
        let knobs = [Knob::Preset, Knob::Size, Knob::Fps, Knob::AudioBitrate];
        for (w, h) in [(720.0, 600.0), (1600.0, 1000.0)] {
            for open in [false, true] {
                let mut shape = sheet_shape(&knobs);
                shape.more_open = open;
                let s = options_dialog((w, h), &shape);
                let f = &s.frame;
                // A ContentDialog, 448 wide, inside the window.
                assert_eq!(f.rect.w, kit::DIALOG_WIDTH);
                assert!(inside(&f.rect, &Rect::new(0.0, 0.0, w, h)));
                assert_eq!(s.panel, f.rect);
                // Top down: one sub-line, Quality, the card, More
                // options.
                assert_eq!(s.sub.h, 16.0);
                let hd = s.heading.unwrap();
                assert!(s.sub.bottom() < hd.y);
                assert_eq!(s.radios.len(), 3);
                assert!(hd.bottom() <= s.radios[0].row.y);
                let card = s.card.unwrap();
                assert!(s.radios[2].row.bottom() < card.y);
                assert!(inside(&s.result, &card));
                let e = s.expander.unwrap();
                assert!(card.bottom() < e.header.y);
                // The Tab order: the radios, More options and its lines,
                // then Convert and Cancel.
                let order: Vec<SheetControl> = s.controls.iter().map(|(c, _)| *c).collect();
                let mut want: Vec<SheetControl> = Preset::ALL
                    .into_iter()
                    .map(|p| SheetControl::Preset(0, p))
                    .collect();
                want.push(SheetControl::Advanced);
                if open {
                    want.extend((1..4).map(SheetControl::Step));
                }
                want.extend([SheetControl::Convert, SheetControl::Cancel]);
                assert_eq!(order, want);
                // Convert, the accent, first in the band; Cancel beside it.
                let (convert, cancel) = (f.buttons[0], f.buttons[1]);
                assert!(convert.right() < cancel.x);
                assert!(inside(&convert, &f.band) && inside(&cancel, &f.band));
                // Each line: its label left of its stepper, inside the
                // expander's content, one under another.
                let mut last = e.header.bottom();
                for &(_, label, control) in &s.lines {
                    assert!(label.right() <= control.x);
                    assert!(inside(&control, &e.content.unwrap()));
                    assert!(control.y >= last);
                    last = control.bottom();
                }
                assert_eq!(s.lines.len(), if open { 3 } else { 0 });
                if h >= 1000.0 {
                    assert_eq!(s.scroll_max, 0.0, "it fits a big window");
                }
            }
        }
    }

    #[test]
    fn a_pair_with_nothing_for_quality_has_no_radios() {
        let s = sheet(720.0, 480.0, &[Knob::Size]);
        assert!(s.heading.is_none() && s.radios.is_empty());
        let order: Vec<SheetControl> = s.controls.iter().map(|(c, _)| *c).collect();
        assert_eq!(
            order,
            [
                SheetControl::Advanced,
                SheetControl::Convert,
                SheetControl::Cancel
            ]
        );
        // Sound and video Kiln writes itself have Quality as radio buttons
        // too: Small, High, Best, and for video Original (no
        // re-encode); one Tab stop, over the estimate card.
        for (knob, n) in [(Knob::AudioTune, 3), (Knob::VideoTune, 4)] {
            let mut shape = sheet_shape(&[knob]);
            shape.tune_radios = n;
            let t = options_dialog((720.0, 600.0), &shape);
            assert_eq!(t.radios.len(), n);
            let order: Vec<SheetControl> = t.controls.iter().map(|(c, _)| *c).collect();
            let mut want: Vec<SheetControl> = (0..n).map(SheetControl::Tune).collect();
            want.extend([SheetControl::Convert, SheetControl::Cancel]);
            assert_eq!(order, want);
            assert!(order[..n].iter().all(|c| c.is_radio()));
            // Under the heading, one under another, above the card.
            assert!(t.heading.unwrap().bottom() <= t.radios[0].row.y);
            for pair in t.radios.windows(2) {
                assert!(pair[0].row.bottom() <= pair[1].row.y);
            }
            assert!(t.radios[n - 1].row.bottom() < t.card.unwrap().y);
            assert!(t.expander.is_none(), "no lines without fields");
        }
        // A pair with no steps (WAV) has no Quality at all.
        let mut none = sheet_shape(&[Knob::AudioTune]);
        none.tune_radios = 0;
        let t = options_dialog((720.0, 600.0), &none);
        assert!(t.heading.is_none() && t.radios.is_empty());
    }

    #[test]
    fn notes_push_the_content_down_a_line_each() {
        let knobs = [Knob::Preset];
        let plain = options_dialog((720.0, 600.0), &sheet_shape(&knobs));
        let mut shape = sheet_shape(&knobs);
        shape.notes = 2;
        let s = options_dialog((720.0, 600.0), &shape);
        assert_eq!(s.note_count(), 2);
        let notes = s.notes.unwrap();
        assert_eq!(notes.y, s.sub.bottom());
        for k in 0..2 {
            let line = s.note_line(k);
            assert_eq!(line.h, SHEET_NOTE);
            assert_eq!(line.y, notes.y + k as f32 * SHEET_NOTE);
        }
        assert_eq!(s.note_line(2), Rect::default());
        let below = |l: &SheetLayout| l.heading.unwrap().y - l.frame.content.y;
        let moved = below(&s) - below(&plain);
        assert!((moved - 2.0 * SHEET_NOTE).abs() < 1.0, "{moved}");
        assert_eq!(plain.note_count(), 0);
    }

    #[test]
    fn in_the_smallest_window_the_content_scrolls_and_the_buttons_stay() {
        use crate::tune_sheet::TuneField;
        let (w, h) = MIN_WINDOW;
        let fields = [
            TuneField::Resolution,
            TuneField::FrameRate,
            TuneField::Codec,
            TuneField::Sound,
            TuneField::FitUnder,
        ];
        let mut shape = sheet_shape(&[Knob::VideoTune]);
        shape.more = fields.iter().map(|f| SheetControl::Field(*f)).collect();
        shape.more_open = true;
        shape.notes = 2;
        let s = options_dialog((w, h), &shape);
        let window = Rect::new(0.0, 0.0, w, h);
        assert!(inside(&s.frame.rect, &window));
        assert!(s.scroll_max > 0.0);
        for b in &s.frame.buttons {
            assert!(inside(b, &window));
        }
        // Scrolled to its end, the last line shows whole in the content.
        shape.scroll = f32::MAX;
        let end = options_dialog((w, h), &shape);
        assert_eq!(end.scroll, end.scroll_max);
        let last = end.lines.last().unwrap().2;
        assert!(last.bottom() <= end.frame.content.bottom() + 0.5);
        assert!(last.y >= end.frame.content.y);
        // The content moves; the band doesn't.
        assert_eq!(end.frame.buttons, s.frame.buttons);
        assert!(end.sub.y < s.sub.y);
    }

    #[test]
    fn a_spin_boxs_chevrons_sit_at_its_right_end() {
        let r = Rect::new(10.0, 20.0, 168.0, 32.0);
        let (value, back, next) = kit::spin_parts(r);
        assert_eq!(next.right(), r.right());
        assert_eq!(back.right(), next.x);
        assert!(value.right() <= back.x);
        assert_eq!((back.w, next.w), (32.0, 32.0));
    }

    fn inside(a: &Rect, b: &Rect) -> bool {
        a.x >= b.x && a.y >= b.y && a.right() <= b.right() && a.bottom() <= b.bottom()
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

#[cfg(test)]
mod settings_tests {
    use super::*;
    use crate::settings::{EngineStatus, OfficeStatus, Settings};
    use crate::settings_page::{self as sp, Facts, Helper, PageSpec};

    /// The page at its fullest: every helper missing (Get it and Choose…),
    /// Office found, every Expander open, a folder chosen and the
    /// drag-wheel refused.
    pub(super) fn full_page() -> PageSpec {
        let s = Settings {
            save_to: Some(std::path::PathBuf::from(
                r"C:\Users\someone\Documents\Converted files from Kiln",
            )),
            drag_wheel: true,
            ..Settings::default()
        };
        let facts = Facts {
            engines: Some(std::array::from_fn(|_| EngineStatus::Missing)),
            office: Some(OfficeStatus {
                badge: sp::FOUND,
                apps: Some("Word, Excel and PowerPoint".into()),
                folder: Some(r"C:\Program Files\Microsoft Office\root\Office16".into()),
            }),
            ..Facts::default()
        };
        sp::page(&s, &facts, [true; 5], true)
    }

    fn default_page() -> PageSpec {
        sp::page(&Settings::default(), &Facts::default(), [false; 5], false)
    }

    fn lay(w: f32, h: f32, page: &PageSpec) -> SettingsLayout {
        settings(content_layer(w, h, TITLE_BAR), 0.0, page, &Estimate)
    }

    /// The controls that sit inside `card` (its own row and content).
    fn controls_of<'a>(l: &'a SettingsLayout, card: &CardLayout) -> Vec<&'a (Control, Rect)> {
        l.controls
            .iter()
            .filter(|(c, r)| {
                !matches!(c, Control::Helper(_))
                    && r.y >= card.rect.y - 0.5
                    && r.bottom() <= card.rect.bottom() + 0.5
            })
            .collect()
    }

    #[test]
    fn the_page_runs_title_sections_then_cards_in_order() {
        let page = default_page();
        let l = lay(1100.0, 720.0, &page);
        let order: Vec<sp::Card> = page.cards().map(|c| c.card).collect();
        assert_eq!(l.cards.iter().map(|c| c.card).collect::<Vec<_>>(), order);
        for pair in l.cards.windows(2) {
            assert!(pair[1].rect.y >= pair[0].rect.bottom() + CARD_GAP - 0.01);
        }
        // Each section's heading comes before its first card, after the
        // last card of the section before.
        let mut at = 0;
        for ((section, specs), (s2, heading)) in page.sections.iter().zip(&l.sections) {
            assert_eq!(section, s2);
            assert!(heading.y >= l.title.bottom());
            let first = &l.cards[at];
            assert_eq!(
                first.rect.y,
                heading.bottom() + SECTION_UNDER,
                "{section:?}"
            );
            if at > 0 {
                assert_eq!(heading.y, l.cards[at - 1].rect.bottom() + SECTION_GAP);
            }
            at += specs.len();
        }
        // Mock: 68 tall cards, 16 in, glyph then words 16 after it.
        let theme = l.card(sp::Card::Theme).unwrap();
        assert_eq!(theme.rect.h, CARD_MIN);
        assert_eq!(theme.glyph.x, theme.rect.x + 16.0);
        assert_eq!(theme.label.x, theme.glyph.right() + 16.0);
        let (_, combo) = l.controls[l.find(Control::Theme).unwrap()];
        assert_eq!(combo.right(), theme.rect.right() - 16.0);
        assert_eq!(combo.h, 32.0);
    }

    #[test]
    fn labels_never_run_into_the_control_column_from_600_to_2048_dips_at_any_text_size() {
        for page in [default_page(), full_page()] {
            for dips in [600.0, 720.0, 800.0, 1024.0, 1366.0, 1600.0, 2048.0] {
                for zoom in [1.0, 1.5, 2.0] {
                    let (w, h) = (dips / zoom, 900.0 / zoom);
                    let l = lay(w, h, &page);
                    for card in &l.cards {
                        let words = card.note.map_or(card.label, |n| {
                            Rect::new(
                                card.label.x,
                                card.label.y,
                                card.label.w,
                                n.bottom() - card.label.y,
                            )
                        });
                        assert!(card.label.w > 0.0, "{:?} at {w}", card.card);
                        if !card.stacked {
                            assert!(
                                card.label.w >= CARD_LABEL_MIN,
                                "{:?} at {w}: {}",
                                card.card,
                                card.label.w
                            );
                        }
                        for (c, r) in controls_of(&l, card) {
                            let beside = words.right() <= r.x - CARD_COLUMN_GAP + 0.01;
                            let under = r.y >= words.bottom() - 0.01;
                            assert!(beside || under, "{c:?} on {:?}'s words at {w}", card.card);
                            assert!(
                                r.right() <= card.rect.right() - CARD_PAD_X + 0.01,
                                "{c:?} at {w}"
                            );
                        }
                        if let Some(b) = card.badge {
                            let under = b.y >= words.bottom() - 0.01;
                            assert!(
                                words.right() <= b.x || under,
                                "{:?}'s badge at {w}",
                                card.card
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_theme_list_has_room_for_its_longest_name_and_the_three_dots() {
        let page = default_page();
        let l = lay(1000.0, 720.0, &page);
        let (_, anchor) = *l
            .controls
            .iter()
            .find(|(c, _)| *c == Control::Theme)
            .unwrap();
        let sp::Body::Combo {
            choices, swatches, ..
        } = &page.card(sp::Card::Theme).unwrap().body
        else {
            panic!()
        };
        assert_eq!(swatches.len(), choices.len());
        let list = kit::combo_flyout(anchor, choices.len(), 0, Rect::new(0.0, 0.0, 1000.0, 720.0));
        assert_eq!(list.items.len(), 9, "the whole list shows");
        let widest = choices
            .iter()
            .map(|c| Estimate.width(c, false))
            .fold(0.0, f32::max);
        for item in &list.items {
            let dots = kit::swatch_dots(*item);
            // Three 8-DIP dots, level, in a row, inside the item, at its right.
            for d in &dots {
                assert_eq!((d.w, d.h), (8.0, 8.0));
                assert_eq!(d.y, dots[0].y);
                assert!(d.x >= item.x && d.right() <= item.right() - kit::BUTTON_PAD + 0.01);
            }
            assert!(dots[0].right() <= dots[1].x && dots[1].right() <= dots[2].x);
            assert!((dots[0].y + 4.0 - (item.y + item.h / 2.0)).abs() < 0.01);
            // The longest name ends before the first dot.
            assert!(
                item.x + kit::BUTTON_PAD + widest <= dots[0].x,
                "{} vs {}",
                item.x + kit::BUTTON_PAD + widest,
                dots[0].x
            );
        }
        // The other lists on the page are as wide as before: no dots, no room.
        assert_eq!(kit::swatch_room(), 40.0);
    }

    #[test]
    fn the_page_keeps_32_at_the_sides_and_under_the_last_card() {
        // 40 was off the grid; 32 is on it.
        assert_eq!(SETTINGS_PAD_X, 32.0);
        assert_eq!(SETTINGS_PAD_BOTTOM, 32.0);
        let l = lay(1000.0, 720.0, &default_page());
        let first = &l.cards[0];
        assert_eq!(first.rect.x, l.view.x + 32.0);
        assert_eq!(first.rect.right(), l.view.right() - 32.0);
        let last = l.cards.last().unwrap();
        assert_eq!(l.content_h, last.rect.bottom() + 32.0 - l.view.y);
    }

    #[test]
    fn every_drop_down_on_the_page_is_one_width() {
        // 140 and 164 DIPs in one column; one width now, the widest
        // any of them needs.
        for page in [default_page(), full_page()] {
            let l = lay(1100.0, 900.0, &page);
            let combos: Vec<f32> = page
                .cards()
                .filter(|c| matches!(c.body, sp::Body::Combo { .. }))
                .filter_map(|c| {
                    let card = l.card(c.card)?;
                    let (_, r) = l.controls.iter().find(|(k, r)| {
                        matches!(k, Control::Theme | Control::SaveWhere | Control::Choice(_))
                            && r.y >= card.rect.y
                            && r.bottom() <= card.rect.bottom()
                    })?;
                    Some(r.w)
                })
                .collect();
            assert!(combos.len() >= 4, "{combos:?}");
            assert!(combos.iter().all(|&w| w == combos[0]), "{combos:?}");
            assert!(combos[0] >= kit::COMBO_MIN_WIDTH);
        }
    }

    #[test]
    fn an_open_helper_with_a_path_has_a_caption_label_over_it() {
        // a bare path had no label.
        let s = Settings::default();
        let facts = Facts {
            engines: Some(std::array::from_fn(|_| {
                EngineStatus::Installed(r"C:\Tools\pandoc.exe".into())
            })),
            ..Facts::default()
        };
        let page = sp::page(&s, &facts, [true; 5], false);
        let l = lay(1100.0, 900.0, &page);
        let pandoc = l.card(sp::Card::Helper(Helper::Pandoc)).unwrap();
        let label = pandoc.line_label.expect("the Location label");
        let line = pandoc.line.unwrap();
        assert_eq!((label.h, label.x), (LINE_LABEL, line.x));
        assert_eq!(line.y, label.bottom(), "the path starts under its label");
        assert!(label.y >= pandoc.content.unwrap().y);
        // The card grows by the label's line over one without.
        let bare = sp::page(&s, &Facts::default(), [true; 5], false);
        let lb = lay(1100.0, 900.0, &bare);
        let none = lb.card(sp::Card::Helper(Helper::Pandoc)).unwrap();
        assert_eq!(none.line_label, None);
        assert!(pandoc.content.unwrap().h >= none.content.unwrap().h);
        // Never on a helper that says a sentence, such as FFmpeg's included
        // line or one not found.
        let ffmpeg = l.card(sp::Card::Helper(Helper::Ffmpeg)).unwrap();
        assert!(ffmpeg.line_label.is_some(), "installed here: a path");
        let missing = Facts {
            engines: Some(std::array::from_fn(|_| EngineStatus::Missing)),
            ..Facts::default()
        };
        let lm = lay(1100.0, 900.0, &sp::page(&s, &missing, [true; 5], false));
        assert!(lm.cards.iter().all(|c| c.line_label.is_none()));
    }

    #[test]
    fn a_narrow_card_puts_its_control_under_its_words() {
        // 600 DIPs at 200% Text size: 300 units.
        let l = lay(300.0, 450.0, &default_page());
        let theme = l.card(sp::Card::Theme).unwrap();
        assert!(theme.stacked);
        let (_, combo) = l.controls[l.find(Control::Theme).unwrap()];
        assert_eq!(combo.x, theme.label.x);
        assert!(combo.y >= theme.note.unwrap().bottom());
        assert!(theme.rect.bottom() >= combo.bottom() + CARD_PAD_Y);
        // Small, High and Best go one under another where a row won't fit.
        let video = l.card(sp::Card::Video).unwrap();
        let radios: Vec<Rect> = controls_of(&l, video).iter().map(|(_, r)| *r).collect();
        assert_eq!(radios.len(), 3);
        assert!(radios
            .iter()
            .all(|r| r.right() <= video.rect.right() - CARD_PAD_X + 0.01));
        // And at 1100 they sit in a row at the card's right.
        let wide = lay(1100.0, 720.0, &default_page());
        let video = wide.card(sp::Card::Video).unwrap();
        let radios: Vec<Rect> = controls_of(&wide, video).iter().map(|(_, r)| *r).collect();
        assert!(!video.stacked);
        assert!(radios
            .windows(2)
            .all(|p| p[0].y == p[1].y && p[0].right() < p[1].x));
    }

    #[test]
    fn the_page_scrolls_and_shows_what_has_the_focus() {
        let page = full_page();
        let l = lay(720.0, 480.0, &page);
        assert!(l.content_h > l.view.h);
        assert_eq!(l.scroll, 0.0);
        let last = l.controls.len() - 1;
        let (c, r) = l.controls[last];
        assert_eq!(c, Control::Licences);
        // Out of view: the pointer finds nothing there.
        assert!(r.y > l.view.bottom());
        assert_eq!(l.hit(r.x + 1.0, r.y + 1.0), None);
        let to = l.scroll_to_show(&r);
        assert!(to > 0.0 && to <= l.max_scroll());
        let moved = settings(l.view, to, &page, &Estimate);
        let (_, r2) = moved.controls[last];
        assert_eq!(r2.y, r.y - to);
        assert!(moved.view.contains(r2.x, r2.y) && r2.bottom() <= moved.view.bottom());
        assert_eq!(moved.hit(r2.x + 1.0, r2.y + 1.0), Some(last));
        // It never scrolls past either end.
        assert_eq!(settings(l.view, -50.0, &page, &Estimate).scroll, 0.0);
        let end = settings(l.view, 1e6, &page, &Estimate);
        assert_eq!(end.scroll, l.max_scroll());
        assert_eq!(end.content_h, l.content_h);
    }

    #[test]
    fn a_wide_window_keeps_the_cards_to_their_width_in_the_middle() {
        let l = lay(2048.0, 1200.0, &default_page());
        let card = &l.cards[0];
        assert_eq!(card.rect.w, SETTINGS_MAX_WIDTH);
        let mid = l.view.x + l.view.w / 2.0;
        assert!((card.rect.x + card.rect.w / 2.0 - mid).abs() <= 1.0);
        // A narrow layer keeps 16 at the sides.
        let n = lay(500.0, 600.0, &default_page());
        assert_eq!(n.cards[0].rect.x, n.view.x + SETTINGS_PAD_X_NARROW);
    }

    #[test]
    fn an_open_expander_holds_its_line_and_its_buttons_after_its_header() {
        let page = full_page();
        let l = lay(1100.0, 720.0, &page);
        let pandoc = l.card(sp::Card::Helper(Helper::Pandoc)).unwrap();
        let content = pandoc.content.expect("open");
        assert_eq!(content.y, pandoc.header.bottom());
        assert!(pandoc.line.is_some());
        let at = l.find(Control::Helper(Helper::Pandoc)).unwrap();
        assert_eq!(l.controls[at].1, pandoc.header);
        let engine = crate::settings::Engine::Pandoc;
        assert_eq!(l.controls[at + 1].0, Control::Get(engine));
        assert_eq!(l.controls[at + 2].0, Control::Choose(engine));
        let (_, choose) = l.controls[at + 2];
        assert!(content.contains(choose.x + 1.0, choose.y + 1.0));
        // Shut, it is its header alone and has no badge until Kiln looks.
        let shut = lay(1100.0, 720.0, &default_page());
        let p = shut.card(sp::Card::Helper(Helper::Pandoc)).unwrap();
        assert_eq!(p.content, None);
        assert_eq!(p.rect, p.header);
        assert_eq!(p.badge, None);
        assert!(p.chevron.is_some());
    }

    #[test]
    fn the_estimate_wraps_at_spaces() {
        assert_eq!(Estimate.height("", false, 100.0), 0.0);
        assert_eq!(Estimate.height("one two", false, 100.0), 20.0);
        // 14 letters a line at 100: "aaaa bbbb cccc dddd" takes two.
        assert_eq!(Estimate.height("aaaa bbbb cccc dddd", false, 100.0), 40.0);
        assert_eq!(Estimate.height("aaaa", true, 12.0), 32.0);
    }
}
