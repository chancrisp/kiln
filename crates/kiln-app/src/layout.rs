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
            SettingsTab::Engines => "Engines",
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
    /// One segment of a quality preset control.
    Preset(DefaultSetting, Preset),
    /// A value that steps through its choices: "1080p", "30 fps".
    Step(DefaultSetting),
    EngineChoose(Engine),
    EngineReset(Engine),
    CheckUpdates,
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
pub const SETTINGS_LABEL: f32 = 112.0;
/// A label in the second column of the quality page.
const SETTINGS_LABEL_2: f32 = 80.0;
/// Segments of the theme and preset controls.
pub const SEGMENT_WIDTH: f32 = 64.0;
/// The tabs at the top of the settings panel.
pub const TAB_WIDTH: f32 = 80.0;
/// A control that steps through values.
pub const STEP_WIDTH: f32 = 96.0;
/// The left end of a stepping control, where a click steps back.
pub const CHEVRON_ZONE: f32 = 28.0;
/// The Check for updates button.
pub const UPDATE_BUTTON_WIDTH: f32 = 136.0;
/// The Forget picks button, wider than Choose to fit its label.
pub const FORGET_BUTTON_WIDTH: f32 = 104.0;
/// Rows on the fullest page. Every page keeps this height, so switching
/// pages never moves the panel.
const SETTINGS_ROWS: usize = 5;

/// What the settings panel needs to know to lay itself out: the page, and
/// which values are set, since those rows grow a Reset button.
#[derive(Debug, Clone, Copy, Default)]
pub struct SettingsShape {
    pub tab: SettingsTab,
    pub output_custom: bool,
    pub engine_custom: [bool; 4],
}

pub fn settings(w: f32, h: f32, shape: SettingsShape) -> SettingsLayout {
    let p = PANEL_PADDING;
    let ph = p + 32.0 + 8.0 + SETTINGS_ROWS as f32 * SETTINGS_ROW + p + BUTTON_HEIGHT + p;
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
                rows.push(SettingsRow {
                    label,
                    label_rect: Rect::new(lx, row_y(i) + 8.0, x - 8.0 - lx, 24.0),
                    value: None,
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
            stepper(c, r, ("PDF pages", 3, true), DefaultSetting::PageDpi);
            stepper(c, r, ("Audio", 4, false), DefaultSetting::Audio);
        }
        SettingsTab::Engines => {
            for (k, e) in Engine::ALL.into_iter().enumerate() {
                let v = chooser(
                    &mut controls,
                    k,
                    Control::EngineChoose(e),
                    shape.engine_custom[k].then_some(Control::EngineReset(e)),
                );
                rows.push(row(e.label(), k, Some(v)));
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
    /// What the source is, under the title.
    pub context: Rect,
    /// One label per knob.
    pub labels: Vec<Rect>,
    /// Every control, in Tab order. Convert comes last.
    pub controls: Vec<(SheetControl, Rect)>,
    /// The result's size and a rough file size, beside the buttons.
    pub result: Rect,
}

pub const SHEET_WIDTH: f32 = 432.0;
pub const SHEET_LABEL: f32 = 96.0;
/// The sheet's stepping controls, wider than the settings page's since
/// each row holds only one.
pub const SHEET_STEP_WIDTH: f32 = 136.0;
pub const CONVERT_WIDTH: f32 = 88.0;

/// Lay out the sheet for `knobs`, centred in a `w` x `h` window.
pub fn sheet(w: f32, h: f32, knobs: &[Knob]) -> SheetLayout {
    let p = PANEL_PADDING;
    let n = knobs.len() as f32;
    let ph = p + 32.0 + 20.0 + 12.0 + n * SETTINGS_ROW + 12.0 + BUTTON_HEIGHT + p;
    let ph = (ph / GRID).ceil() * GRID;
    let pw = snap((w - 32.0).clamp(0.0, SHEET_WIDTH));
    let panel = Rect::new(
        snap(((w - pw) / 2.0).max(0.0)),
        snap(((h - ph) / 2.0).max(8.0)),
        pw,
        ph,
    );
    let title = Rect::new(panel.x + p, panel.y + p, panel.w - 2.0 * p, 32.0);
    let context = Rect::new(title.x, title.bottom(), title.w, 20.0);
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
        context,
        labels,
        controls,
        result,
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
    fn settings_pages_hold_their_rows() {
        let general = page(SettingsTab::General);
        let labels: Vec<_> = general.rows.iter().map(|r| r.label).collect();
        assert_eq!(
            labels,
            ["Output folder", "Theme", "Wheel keys", "Suggestions"]
        );
        assert_eq!(general.controls.len(), 3 + 1 + 3 + 3 + 2 + 1 + 1 + 1);
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
        assert_eq!(forget.w, 104.0);
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
                "PDF pages",
                "Audio"
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
