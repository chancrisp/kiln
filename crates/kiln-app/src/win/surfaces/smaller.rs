//! The Make smaller and Resize dialogs (mock "4 Make smaller", R12), as a
//! ContentDialog over smoke: the title, a sub-line, the choices as
//! RadioButtons with Custom's box, the estimate card, More options, and
//! the accent button and Cancel.
//!
//! It is modal while it is open (`system::modal_open` counts it), as one
//! of Kiln's prompts is: drops wait and nothing starts under it. Focus
//! starts on the accent button; Tab and Shift+Tab go round its stops; the
//! arrows pick a choice, skipping those that would do nothing; Esc is
//! Cancel. Its estimate is worked out on a worker (`App::ask_estimate`)
//! and handed back (`ToolUi::estimated`).

use super::*;

use crate::controls::{ComboEvent, Key as CKey, Press};
use crate::layout::{kit, Rect};
#[cfg(test)]
use crate::smaller::MoreLine;
use crate::smaller::{Estimate, Saving, ToolDialog};
pub(in crate::win) use crate::smaller::{LineParts, Stop, ToolLayout};
use kiln_core::ToolOptions;

/// A part of the dialog a pointer can be over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::win) enum ToolPart {
    /// Choice `n`, Custom last.
    Radio(usize),
    /// Custom's box.
    Custom,
    /// More options' header.
    Expander,
    /// The control of More options' line `n`.
    Line(usize),
    /// Item `n` of an open ComboBox's list.
    Item(usize),
    Accent,
    Cancel,
}

/// The sub-line, the heading, the gaps.
const SUB: f32 = 16.0;
const GAP: f32 = 12.0;
const HEADING: f32 = 20.0;
const CARD_PAD: (f32, f32) = crate::layout::SHEET_CARD_PAD;
/// A More options line with a one-line note, or none.
const LINE: f32 = 56.0;
/// Where a line's note starts under its top, and the room under it:
/// a note that wraps to more lines makes its row taller.
const NOTE_TOP: f32 = 28.0;
const NOTE_BOTTOM: f32 = 12.0;
const NOTE_LINE: f32 = 16.0;

/// How tall More options' line with a note `note_h` tall is: `LINE` for a
/// line of one, and each line more adds its own.
fn line_height(note_h: f32) -> f32 {
    if note_h > 0.0 {
        (NOTE_TOP + note_h + NOTE_BOTTOM).max(LINE)
    } else {
        LINE
    }
}
/// The hint under the choices: a gap, then one Caption line.
const HINT_GAP: f32 = 4.0;
const HINT_H: f32 = 16.0;
const CUSTOM_BOX: f32 = 88.0;
/// A ToggleSwitch's target: its word and track.
const TOGGLE_HIT: f32 = 96.0;

/// What the layout needs measured, from the renderer.
pub(in crate::win) struct Measured {
    pub title_h: f32,
    pub labels: Vec<f32>,
    pub tails: Vec<f32>,
    pub detail_h: f32,
    pub combo_w: f32,
    /// How tall each More options line's note runs wrapped to the room
    /// beside its control (0 for a line with no note).
    pub note_hs: Vec<f32>,
}

/// Lay the dialog `d` out in a window `size` big, its content scrolled by
/// `scroll`.
pub(in crate::win) fn layout_of(
    d: &ToolDialog,
    m: &Measured,
    size: (f32, f32),
    scroll: f32,
) -> ToolLayout {
    let lines = d.more_lines();
    let n = d.labels().len() as f32;
    let radios_h = n * kit::RADIO_ROW + (n - 1.0) * kit::RADIO_ROW_GAP;
    let card_h = d
        .estimate_lines(Saving::NextToOriginal)
        .map(|_| 2.0 * CARD_PAD.1 + kit::LINE + m.detail_h);
    let more_open = d.smaller().is_some_and(|s| s.more.open);
    let height_of = |k: usize| line_height(m.note_hs.get(k).copied().unwrap_or(0.0));
    let lines_h: f32 = (0..lines.len()).map(height_of).sum::<f32>() + 8.0;
    let more_h = if lines.is_empty() {
        0.0
    } else {
        kit::EXPANDER_HEADER + if more_open { lines_h } else { 0.0 }
    };
    let content_h = SUB
        + GAP
        + HEADING
        + 6.0
        + radios_h
        + d.hint().map_or(0.0, |_| HINT_GAP + HINT_H)
        + card_h.map_or(0.0, |h| GAP + h)
        + if more_h > 0.0 { GAP + more_h } else { 0.0 };
    let frame = kit::dialog(size, m.title_h, content_h, 2);
    let c = frame.content;
    let scroll_max = (content_h - c.h).max(0.0);
    let scroll = scroll.clamp(0.0, scroll_max);
    let mut y = c.y - scroll;
    let sub = Rect::new(c.x, y, c.w, SUB);
    y += SUB + GAP;
    let heading = Rect::new(c.x, y, c.w, HEADING);
    y += HEADING + 6.0;
    let radios = kit::radio_rows(c.x, y, &m.labels);
    let tails = radios
        .iter()
        .zip(&m.tails)
        .map(|(p, &w)| Rect::new(p.label.right() + 8.0, p.row.y, w, p.row.h))
        .collect();
    let custom = d.custom_picked().then(|| {
        let p = radios[radios.len() - 1];
        let b = Rect::new(
            p.label.right() + GAP,
            p.row.y,
            CUSTOM_BOX,
            kit::CONTROL_HEIGHT,
        );
        (b, Rect::new(b.right() + 8.0, b.y, 32.0, b.h))
    });
    y += radios_h;
    // Under Custom's box, plain: a hint and not an estimate.
    let hint = d.hint().map(|_| {
        let x = radios.last().map_or(c.x, |p| p.label.x);
        let r = Rect::new(x, y + HINT_GAP, (c.right() - x).max(0.0), HINT_H);
        y += HINT_GAP + HINT_H;
        r
    });
    let card = card_h.map(|h| {
        y += GAP;
        let r = Rect::new(c.x, y, c.w, h);
        y += h;
        let inner = r.w - 2.0 * CARD_PAD.0;
        let bold = Rect::new(r.x + CARD_PAD.0, r.y + CARD_PAD.1, inner, kit::LINE);
        let detail = Rect::new(bold.x, bold.bottom(), inner, m.detail_h);
        (r, bold, detail)
    });
    let mut parts = vec![];
    let expander = (!lines.is_empty()).then(|| {
        y += GAP;
        let content = if more_open { lines_h } else { 0.0 };
        let e = kit::expander_parts(c.x, y, c.w, content, if more_open { 1.0 } else { 0.0 });
        if let Some(ct) = e.content {
            let mut next = ct.y + 4.0;
            for (k, &line) in lines.iter().enumerate() {
                let note_h = m.note_hs.get(k).copied().unwrap_or(0.0);
                let top = next;
                next += line_height(note_h);
                let x = ct.x + 16.0;
                let right = ct.right() - 16.0;
                let control = if line.is_toggle() {
                    Rect::new(
                        right - TOGGLE_HIT,
                        top + 12.0,
                        TOGGLE_HIT,
                        kit::CONTROL_HEIGHT,
                    )
                } else {
                    let w = m.combo_w;
                    Rect::new(right - w, top + 12.0, w, kit::CONTROL_HEIGHT)
                };
                let text_w = (control.x - 12.0 - x).max(0.0);
                // A line with no note has its label level with its control.
                let label_y = if line.note().is_some() {
                    top + 8.0
                } else {
                    control.y + (control.h - 20.0) / 2.0
                };
                parts.push(LineParts {
                    line,
                    label: Rect::new(x, label_y, text_w, 20.0),
                    note: Rect::new(
                        x,
                        top + NOTE_TOP,
                        text_w,
                        if note_h > 0.0 { note_h } else { NOTE_LINE },
                    ),
                    control,
                });
            }
        }
        e
    });
    // An open ComboBox's list, over everything in the window.
    let list = d.smaller().and_then(|s| {
        parts.iter().find_map(|p| {
            let combo = s.combo(p.line).filter(|c| c.open)?;
            let bounds = Rect::new(0.0, 0.0, size.0, size.1);
            Some(kit::combo_flyout(p.control, combo.len, combo.value, bounds))
        })
    });
    ToolLayout {
        frame,
        sub,
        heading,
        radios,
        tails,
        custom,
        card,
        hint,
        expander,
        lines: parts,
        list,
        scroll_max,
        scroll,
    }
}

/// A Make smaller or Resize dialog open in the window.
pub(in crate::win) struct ToolUi {
    model: ToolDialog,
    kind: kiln_core::Kind,
    via: Via,
    saving: Saving,
    focus: Stop,
    pointer: Pointer,
    scroll: f32,
    opened: Instant,
    /// Counted as a modal for as long as it is open.
    _open: system::InWindow,
}

/// The part a control is.
fn part(c: ControlId) -> Option<ToolPart> {
    match c {
        ControlId::Tool(p) => Some(p),
        _ => None,
    }
}

/// A key as the controls read it.
fn press_of(key: VIRTUAL_KEY, mods: Mods) -> Option<Press> {
    let k = match key {
        VK_UP => CKey::Up,
        VK_DOWN => CKey::Down,
        VK_LEFT => CKey::Left,
        VK_RIGHT => CKey::Right,
        VK_HOME => CKey::Home,
        VK_END => CKey::End,
        VK_RETURN => CKey::Enter,
        VK_SPACE => CKey::Space,
        VK_ESCAPE => CKey::Esc,
        VK_TAB => CKey::Tab,
        VK_BACK => CKey::Backspace,
        VK_DELETE => CKey::Delete,
        VK_F4 => CKey::F4,
        _ if mods.ctrl && key == VIRTUAL_KEY(b'A' as u16) => CKey::Char('a'),
        _ => return None,
    };
    Some(Press {
        key: k,
        alt: false,
        ctrl: mods.ctrl,
        shift: mods.shift,
    })
}

impl ToolUi {
    /// `model` open, for a job of files of `kind` started as `via` says,
    /// whose results go as `saving` says.
    pub fn new(model: ToolDialog, kind: kiln_core::Kind, via: Via, saving: Saving) -> Self {
        // On the accent button, as every dialog starts (spec 5.4); when
        // it can't be pressed yet as Custom is the only choice, on
        // Custom's box, which needs a size first.
        let focus = if !model.can_run() && model.custom_picked() {
            Stop::Custom
        } else {
            Stop::Accent
        };
        ToolUi {
            model,
            kind,
            via,
            saving,
            focus,
            pointer: Pointer::default(),
            scroll: 0.0,
            opened: Instant::now(),
            _open: system::InWindow::open(),
        }
    }

    pub fn model(&self) -> &ToolDialog {
        &self.model
    }

    pub fn focus(&self) -> Stop {
        self.focus
    }

    pub fn saving(&self) -> Saving {
        self.saving
    }

    /// Its opacity and scale as it comes in, as a prompt's.
    pub fn rise_at(&self, now: Instant, animations: bool) -> (f32, f32) {
        crate::anim::dialog_in(crate::anim::elapsed_ms(self.opened, now), animations)
    }

    pub fn rising(&self, now: Instant, animations: bool) -> bool {
        let ms = crate::theme::fluent::motion::PANE.ms;
        crate::anim::within(self.opened, ms, now, animations)
    }

    /// The estimate the worker should work out now, if one is wanted. The
    /// dialog takes it as asked.
    pub fn take_estimate_wanted(&mut self) -> Option<(PathBuf, ToolOptions)> {
        let s = self.model.smaller_mut()?;
        let (path, opts) = s.wants_estimate()?;
        s.asked(opts.clone());
        Some((path, opts))
    }

    /// Ctrl+V on Custom's box: `text` pasted over its selection, or
    /// refused whole when the box wouldn't take it.
    pub fn paste(&mut self, cx: &mut Ctx, text: &str) {
        if self.focus == Stop::Custom && self.model.custom_mut().paste(text.trim()) {
            cx.out.invalidate = true;
        }
    }

    /// A file's picture size and frame rate came in: the dialog follows.
    pub fn facts_arrived(
        &mut self,
        cx: &mut Ctx,
        path: &std::path::Path,
        picture: Option<(u32, u32)>,
        fps: Option<f64>,
    ) {
        if self.model.facts_arrived(path, picture, fps) {
            cx.out.invalidate = true;
        }
    }

    /// The worker's answer for `opts`.
    pub fn estimated(&mut self, cx: &mut Ctx, opts: ToolOptions, e: Estimate) {
        if let Some(s) = self.model.smaller_mut() {
            s.estimated(opts, e);
            cx.out.invalidate = true;
        }
    }

    /// What the layout needs measured.
    fn measured(&self, cx: &Ctx) -> Measured {
        let m = cx.measure;
        let pad = crate::theme::fluent::space::DIALOG_PADDING;
        let inner =
            (kit::DIALOG_WIDTH.min(cx.size.0 - 2.0 * kit::DIALOG_MARGIN) - 2.0 * pad).max(0.0);
        let (title_h, _) = m.dialog_heights(self.model.title(), None, inner);
        let labels = self.model.labels();
        let detail = self
            .model
            .estimate_lines(self.saving)
            .map_or(0.0, |(_, d)| {
                if d.is_empty() {
                    0.0
                } else {
                    m.body_height(&d, inner - 2.0 * CARD_PAD.0).max(16.0)
                }
            });
        let combo_w = self
            .model
            .smaller()
            .map(|s| {
                let widest = self
                    .model
                    .more_lines()
                    .iter()
                    .flat_map(|&l| s.line_choices(l))
                    .map(|c| m.label_width(&c))
                    .fold(0.0, f32::max);
                kit::combo_width(widest)
            })
            .unwrap_or(kit::COMBO_MIN_WIDTH);
        // Each note wraps to the room beside its control: the content's
        // width less the line's padding, the control and the gap.
        let note_hs = self
            .model
            .more_lines()
            .iter()
            .map(|&l| {
                let Some(note) = l.note() else {
                    return 0.0;
                };
                let control = if l.is_toggle() { TOGGLE_HIT } else { combo_w };
                let room = (inner - 32.0 - control - 12.0).max(0.0);
                m.fit_height(note, true, room).ceil().max(NOTE_LINE)
            })
            .collect();
        Measured {
            title_h,
            labels: labels.iter().map(|l| m.label_width(l)).collect(),
            tails: (0..labels.len())
                .map(|i| self.model.tail(i).map_or(0.0, |t| m.label_width(t)))
                .collect(),
            detail_h: detail,
            combo_w,
            note_hs,
        }
    }

    pub fn layout(&self, cx: &Ctx) -> ToolLayout {
        layout_of(&self.model, &self.measured(cx), cx.size, self.scroll)
    }

    /// The stops Tab goes round, in order.
    fn stops(&self) -> Vec<Stop> {
        let mut s = vec![Stop::Radios];
        if self.model.custom_picked() {
            s.push(Stop::Custom);
        }
        if !self.model.more_lines().is_empty() {
            s.push(Stop::Expander);
            if self.model.smaller().is_some_and(|d| d.more.open) {
                s.extend((0..self.model.more_lines().len()).map(Stop::Line));
            }
        }
        s.extend([Stop::Accent, Stop::Cancel]);
        s
    }

    /// Move the focus to `to`, scrolling it into view.
    fn focus_on(&mut self, cx: &mut Ctx, to: Stop) {
        self.focus = to;
        self.reveal(cx);
        cx.out.invalidate = true;
    }

    /// Scroll so the focused stop shows in the content.
    fn reveal(&mut self, cx: &Ctx) {
        let l = self.layout(cx);
        let c = l.frame.content;
        let r = match self.focus {
            Stop::Radios => {
                let i = self.model.radios().focus;
                l.radios.get(i).map(|p| p.row)
            }
            Stop::Custom => l.custom.map(|(b, _)| b),
            Stop::Expander => l.expander.map(|e| e.header),
            Stop::Line(k) => l.lines.get(k).map(|p| p.control),
            Stop::Accent | Stop::Cancel => None,
        };
        if let Some(r) = r {
            if r.y < c.y {
                self.scroll -= c.y - r.y;
            } else if r.bottom() > c.bottom() {
                self.scroll += r.bottom() - c.bottom();
            }
            self.scroll = self.scroll.clamp(0.0, l.scroll_max);
        }
    }

    /// Tab or Shift+Tab.
    fn tab(&mut self, cx: &mut Ctx, back: bool) {
        let stops = self.stops();
        let at = stops.iter().position(|s| *s == self.focus).unwrap_or(0);
        let n = stops.len();
        let to = if back { (at + n - 1) % n } else { (at + 1) % n };
        self.focus_on(cx, stops[to]);
    }

    /// Press the accent button: the job starts once the dialog closes.
    pub fn run(&mut self, cx: &mut Ctx) {
        if !self.model.can_run() {
            return;
        }
        let Some(tool) = self.model.tool_options() else {
            return;
        };
        cx.out.tool_start = Some(ToolStart {
            action: Action::Tool(self.model.tool(), self.kind),
            paths: self.model.paths(),
            tool,
            via: self.via,
        });
        cx.out.close = true;
        cx.out.invalidate = true;
    }

    /// Cancel, Esc, or UI Automation closing it.
    pub fn cancel(&mut self, cx: &mut Ctx) {
        cx.out.close = true;
        cx.out.invalidate = true;
    }

    /// Pick choice `i`, from a click or UI Automation.
    pub fn pick(&mut self, cx: &mut Ctx, i: usize) {
        if self.model.pick(i) {
            self.focus = Stop::Radios;
            cx.out.invalidate = true;
        }
    }

    /// Open or close More options.
    pub fn expand(&mut self, cx: &mut Ctx, open: bool) {
        if let Some(s) = self.model.smaller_mut() {
            if s.more.open != open {
                s.more.open = open;
                cx.out.invalidate = true;
            }
        }
        if open {
            self.reveal_more(cx);
        }
    }

    /// Scroll so More options' lines show, as far as its header lets.
    fn reveal_more(&mut self, cx: &Ctx) {
        let l = self.layout(cx);
        let (Some(e), Some(last)) = (l.expander, l.lines.last()) else {
            return;
        };
        let c = l.frame.content;
        let down = (last.control.bottom() + 12.0 - c.bottom()).max(0.0);
        let room = (e.header.y - c.y).max(0.0);
        self.scroll = (self.scroll + down.min(room)).clamp(0.0, l.scroll_max);
    }

    /// Flip More options' ToggleSwitch on line `k`.
    pub fn toggle_line(&mut self, cx: &mut Ctx, k: usize) {
        let Some(&line) = self.model.more_lines().get(k) else {
            return;
        };
        if let Some(s) = self.model.smaller_mut() {
            s.toggle(line);
            cx.out.invalidate = true;
        }
    }

    /// Set More options' ComboBox on line `k` to choice `v`.
    pub fn set_line(&mut self, cx: &mut Ctx, k: usize, v: usize) {
        let Some(&line) = self.model.more_lines().get(k) else {
            return;
        };
        if let Some(c) = self.model.smaller_mut().and_then(|s| s.combo_mut(line)) {
            if v < c.len {
                c.value = v;
                c.highlight = v;
                c.open = false;
                cx.out.invalidate = true;
            }
        }
    }

    /// Open or close the ComboBox on line `k`'s list, as UI Automation's
    /// ExpandCollapse pattern does.
    pub fn open_line_list(&mut self, cx: &mut Ctx, k: usize, open: bool) {
        let Some(&line) = self.model.more_lines().get(k) else {
            return;
        };
        if let Some(c) = self.model.smaller_mut().and_then(|s| s.combo_mut(line)) {
            c.open = open;
            c.highlight = c.value;
            cx.out.invalidate = true;
        }
    }

    /// Set Custom's text, as UI Automation's Value pattern does; picks
    /// Custom. Whether it was taken.
    pub fn set_custom(&mut self, cx: &mut Ctx, text: &str) -> bool {
        let i = self.model.custom_index();
        self.model.pick(i);
        let taken = self.model.custom_mut().set(text);
        cx.out.invalidate = true;
        taken
    }

    /// UI Automation moves the focus to `to`.
    pub fn focus_stop(&mut self, cx: &mut Ctx, to: Stop) {
        if self.stops().contains(&to) {
            self.focus_on(cx, to);
        }
    }

    /// The open ComboBox's line, if one is open.
    fn open_line(&self) -> Option<usize> {
        let s = self.model.smaller()?;
        self.model
            .more_lines()
            .iter()
            .position(|&l| s.combo(l).is_some_and(|c| c.open))
    }

    /// A key for the ComboBox on line `k`.
    fn combo_key(&mut self, cx: &mut Ctx, k: usize, p: Press) -> bool {
        let Some(&line) = self.model.more_lines().get(k) else {
            return false;
        };
        let Some(c) = self.model.smaller_mut().and_then(|s| s.combo_mut(line)) else {
            return false;
        };
        let e = c.key(p);
        cx.out.invalidate = true;
        e != ComboEvent::Ignored
    }

    /// A typed character, for Custom's box.
    pub fn char(&mut self, cx: &mut Ctx, c: char) -> Reply {
        if self.focus == Stop::Custom && !c.is_control() {
            self.model.custom_mut().key(Press::key(CKey::Char(c)));
            cx.out.invalidate = true;
        }
        Reply::Taken
    }

    /// The parts the layout has, for the hit test.
    fn parts(&self, l: &ToolLayout) -> Vec<(ToolPart, Rect)> {
        let c = l.frame.content;
        let mut v: Vec<(ToolPart, Rect)> = vec![];
        if let Some(list) = &l.list {
            v.extend(
                list.items
                    .iter()
                    .enumerate()
                    .map(|(k, r)| (ToolPart::Item(list.first + k), *r)),
            );
            // An open list takes the press, wherever it is.
            return v;
        }
        let shown = |r: &Rect| r.bottom() > c.y && r.y < c.bottom();
        for (i, p) in l.radios.iter().enumerate() {
            let row = Rect::new(p.row.x, p.row.y, p.row.w + l.tails[i].w + 8.0, p.row.h);
            if shown(&row) {
                v.push((ToolPart::Radio(i), row));
            }
        }
        if let Some((b, _)) = l.custom.filter(|(b, _)| shown(b)) {
            v.push((ToolPart::Custom, b));
        }
        if let Some(e) = l.expander.filter(|e| shown(&e.header)) {
            v.push((ToolPart::Expander, e.header));
        }
        for (k, p) in l.lines.iter().enumerate() {
            if shown(&p.control) {
                v.push((ToolPart::Line(k), p.control));
            }
        }
        v.push((ToolPart::Accent, l.frame.buttons[0]));
        v.push((ToolPart::Cancel, l.frame.buttons[1]));
        v
    }

    /// Act on a click on `p`.
    fn click(&mut self, cx: &mut Ctx, p: ToolPart) {
        match p {
            ToolPart::Radio(i) => {
                self.pick(cx, i);
                if self.model.custom_picked() && i == self.model.custom_index() {
                    self.focus_on(cx, Stop::Custom);
                }
            }
            ToolPart::Custom => {
                let i = self.model.custom_index();
                self.model.pick(i);
                self.focus_on(cx, Stop::Custom);
            }
            ToolPart::Expander => {
                let open = self.model.smaller().is_some_and(|s| s.more.open);
                self.expand(cx, !open);
                self.focus_on(cx, Stop::Expander);
            }
            ToolPart::Line(k) => {
                let Some(&line) = self.model.more_lines().get(k) else {
                    return;
                };
                if line.is_toggle() {
                    self.toggle_line(cx, k);
                } else if let Some(c) = self.model.smaller_mut().and_then(|s| s.combo_mut(line)) {
                    c.click();
                }
                self.focus_on(cx, Stop::Line(k));
            }
            ToolPart::Item(i) => {
                if let Some(k) = self.open_line() {
                    self.set_line(cx, k, i);
                }
            }
            ToolPart::Accent => self.run(cx),
            ToolPart::Cancel => self.cancel(cx),
        }
        cx.out.invalidate = true;
    }

    /// Whether part `p` is drawn disabled.
    pub fn part_disabled(&self, p: ToolPart) -> bool {
        match p {
            ToolPart::Radio(i) => self.model.disabled(i),
            ToolPart::Accent => !self.model.can_run(),
            _ => false,
        }
    }

    pub fn parts_lit(&self) -> (Option<ToolPart>, Option<ToolPart>) {
        (
            self.pointer.hover.and_then(part),
            self.pointer.pressed.and_then(part),
        )
    }
}

impl Surface for ToolUi {
    type View<'a> = ToolView<'a>;
    type A11y<'a> = a11y::ToolIn<'a>;

    fn hit(&self, cx: &Ctx, x: f32, y: f32) -> Option<ControlId> {
        let l = self.layout(cx);
        self.parts(&l)
            .into_iter()
            .find(|(_, r)| r.contains(x, y))
            .map(|(p, _)| ControlId::Tool(p))
    }

    fn hover(&mut self, cx: &mut Ctx, x: f32, y: f32) -> Reply {
        let hot = self.hit(cx, x, y);
        if self.pointer.hover != hot {
            self.pointer.hover = hot;
            cx.out.invalidate = true;
        }
        Reply::Taken
    }

    /// A press on a part holds it; anywhere else it does nothing, but an
    /// open list closes.
    fn press(&mut self, cx: &mut Ctx, x: f32, y: f32) -> Reply {
        match self.hit(cx, x, y) {
            Some(c) => {
                self.pointer.pressed = Some(c);
                cx.out.effects.push(Effect::Capture);
            }
            None => {
                if let Some(k) = self.open_line() {
                    let line = self.model.more_lines()[k];
                    if let Some(c) = self.model.smaller_mut().and_then(|s| s.combo_mut(line)) {
                        c.open = false;
                        c.highlight = c.value;
                    }
                }
            }
        }
        cx.out.invalidate = true;
        Reply::Taken
    }

    fn release(&mut self, cx: &mut Ctx, x: f32, y: f32) -> Reply {
        let Some(c) = self.pointer.pressed.take() else {
            return Reply::Block;
        };
        if self.hit(cx, x, y) == Some(c) {
            if let Some(p) = part(c).filter(|&p| !self.part_disabled(p)) {
                self.click(cx, p);
            }
        }
        cx.out.invalidate = true;
        Reply::Taken
    }

    fn key(&mut self, cx: &mut Ctx, key: VIRTUAL_KEY, mods: Mods) -> Reply {
        let Some(p) = press_of(key, mods) else {
            return Reply::Block;
        };
        // An open list has the keys.
        if let Some(k) = self.open_line() {
            self.combo_key(cx, k, p);
            return Reply::Taken;
        }
        if mods.repeat && matches!(p.key, CKey::Enter | CKey::Space) {
            return Reply::Taken;
        }
        if p.key == CKey::Esc {
            self.cancel(cx);
            return Reply::Taken;
        }
        if p.key == CKey::Tab {
            self.tab(cx, mods.shift);
            return Reply::Taken;
        }
        match self.focus {
            Stop::Radios => {
                let moved = match p.key {
                    CKey::Up | CKey::Left => self.model.step(-1),
                    CKey::Down | CKey::Right => self.model.step(1),
                    CKey::Space => {
                        let i = self.model.radios().focus;
                        self.model.pick(i)
                    }
                    CKey::Enter => {
                        self.run(cx);
                        false
                    }
                    _ => false,
                };
                if moved {
                    self.reveal(cx);
                }
            }
            Stop::Custom => match p.key {
                CKey::Enter => self.run(cx),
                _ => {
                    self.model.custom_mut().key(p);
                }
            },
            Stop::Expander => {
                if matches!(p.key, CKey::Enter | CKey::Space) {
                    let open = self.model.smaller().is_some_and(|s| s.more.open);
                    self.expand(cx, !open);
                }
            }
            Stop::Line(k) => {
                let line = self.model.more_lines().get(k).copied();
                match line {
                    Some(l) if l.is_toggle() => match p.key {
                        CKey::Space => self.toggle_line(cx, k),
                        CKey::Enter => self.run(cx),
                        _ => {}
                    },
                    Some(_) => {
                        self.combo_key(cx, k, p);
                    }
                    None => {}
                }
            }
            Stop::Accent => {
                if matches!(p.key, CKey::Enter | CKey::Space) {
                    self.run(cx);
                } else if matches!(p.key, CKey::Right) {
                    self.focus_on(cx, Stop::Cancel);
                }
            }
            Stop::Cancel => match p.key {
                CKey::Enter | CKey::Space => self.cancel(cx),
                CKey::Left => self.focus_on(cx, Stop::Accent),
                _ => {}
            },
        }
        cx.out.invalidate = true;
        Reply::Taken
    }

    /// The content scrolls when it is taller than the window lets it be;
    /// nothing under it scrolls.
    fn scroll(&mut self, cx: &mut Ctx, by: TextScroll) -> Reply {
        if let TextScroll::Wheel(n) = by {
            let max = self.layout(cx).scroll_max;
            let to = (self.scroll - n * 48.0).clamp(0.0, max);
            if to != self.scroll {
                self.scroll = to;
                cx.out.invalidate = true;
            }
        }
        Reply::Block
    }

    fn leave(&mut self) {
        self.pointer.clear();
    }

    fn view<'a>(
        &'a self,
        cx: &Ctx,
        _frame: Frame<'a>,
        _button: &'a dyn Fn(ButtonId) -> (f32, f32),
    ) -> ToolView<'a> {
        let (hover, pressed) = self.parts_lit();
        ToolView {
            layout: self.layout(cx),
            ui: self,
            estimate: self.model.estimate_lines(self.saving),
            hover,
            pressed,
            rise: self.rise_at(cx.now, crate::theme::fluent::motion::animations()),
        }
    }

    fn a11y<'a>(&'a self, cx: &Ctx, _frame: Frame<'a>) -> a11y::ToolIn<'a> {
        a11y::ToolIn {
            layout: self.layout(cx),
            model: &self.model,
            focus: self.focus,
            estimate: self.model.estimate_lines(self.saving),
        }
    }
}

/// The dialog as the renderer draws it this frame.
pub(in crate::win) struct ToolView<'a> {
    pub layout: ToolLayout,
    pub ui: &'a ToolUi,
    pub estimate: Option<(String, String)>,
    pub hover: Option<ToolPart>,
    pub pressed: Option<ToolPart>,
    pub rise: (f32, f32),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::smaller::File;

    struct Lines;
    impl Measure for Lines {
        fn mono_height(&self, _text: &str, _width: f32) -> f32 {
            0.0
        }
        fn body_height(&self, _text: &str, _width: f32) -> f32 {
            20.0
        }
    }

    const SIZE: (f32, f32) = (900.0, 800.0);

    fn with<R>(size: (f32, f32), f: impl FnOnce(&mut Ctx) -> R) -> (R, Out) {
        let settings = Settings::default();
        let mut cx = Ctx::new(&Lines, &settings, size);
        let r = f(&mut cx);
        (r, cx.out)
    }

    fn video(bytes: u64) -> ToolUi {
        let f = File {
            path: PathBuf::from("C:\\v\\clip.mp4"),
            name: "clip.mp4".into(),
            ext: "mp4".into(),
            bytes,
            picture: Some((1920, 800)),
            fps: Some(24.0),
        };
        ToolUi::new(
            ToolDialog::new(Tool::Smaller, vec![f]).unwrap(),
            kiln_core::Kind::Video,
            Via::Window,
            Saving::NextToOriginal,
        )
    }

    fn photo() -> ToolUi {
        let f = File {
            path: PathBuf::from("C:\\p\\p.jpg"),
            name: "p.jpg".into(),
            ext: "jpg".into(),
            bytes: 3_000_000,
            picture: Some((4032, 3024)),
            fps: None,
        };
        ToolUi::new(
            ToolDialog::new(Tool::Resize, vec![f]).unwrap(),
            kiln_core::Kind::Image,
            Via::Window,
            Saving::NextToOriginal,
        )
    }

    fn key(t: &mut ToolUi, k: VIRTUAL_KEY) -> Out {
        with(SIZE, |cx| t.key(cx, k, Mods::default())).1
    }

    fn shift_tab(t: &mut ToolUi) {
        with(SIZE, |cx| t.key(cx, VK_TAB, Mods::held(true, false)));
    }

    fn center(r: Rect) -> (f32, f32) {
        (r.x + r.w / 2.0, r.y + r.h / 2.0)
    }

    fn click_in(t: &mut ToolUi, size: (f32, f32), r: Rect) -> Out {
        let (x, y) = center(r);
        with(size, |cx| {
            t.press(cx, x, y);
            t.release(cx, x, y)
        })
        .1
    }

    fn click(t: &mut ToolUi, r: Rect) -> Out {
        let (x, y) = center(r);
        with(SIZE, |cx| {
            t.press(cx, x, y);
            t.release(cx, x, y)
        })
        .1
    }

    #[test]
    fn it_counts_as_a_modal_and_focus_starts_on_the_accent_button() {
        assert!(!system::modal_open());
        let t = video(31_000_000);
        assert!(system::modal_open());
        assert_eq!(t.focus(), Stop::Accent);
        drop(t);
        assert!(!system::modal_open());
    }

    #[test]
    fn with_every_preset_disabled_focus_starts_on_customs_box() {
        // A 5 MB video: 10, 25 and 50 MB would all do nothing.
        let t = video(5_000_000);
        assert!(!t.model().can_run());
        assert_eq!(t.focus(), Stop::Custom);
    }

    #[test]
    fn a_paste_into_custom_takes_a_size_and_refuses_anything_else() {
        let mut t = video(5_000_000);
        with(SIZE, |cx| t.paste(cx, "2.5"));
        assert_eq!(t.model().custom().text(), "2.5");
        with(SIZE, |cx| t.paste(cx, "abc"));
        assert_eq!(t.model().custom().text(), "2.5");
        assert!(t.model().can_run());
    }

    #[test]
    fn enter_on_the_accent_starts_make_smaller_with_its_size_and_closes() {
        let mut t = video(31_000_000);
        let out = key(&mut t, VK_RETURN);
        assert!(out.close);
        let s = out.tool_start.expect("a job");
        assert_eq!(
            s.action,
            Action::Tool(Tool::Smaller, kiln_core::Kind::Video)
        );
        assert_eq!(s.paths, [PathBuf::from("C:\\v\\clip.mp4")]);
        assert_eq!(s.tool.max_bytes, Some(25_000_000));
        assert_eq!(s.tool.picture_lines, Some(720));
        assert_eq!(s.via, Via::Window);
    }

    #[test]
    fn esc_is_cancel_and_starts_nothing() {
        let mut t = video(31_000_000);
        let out = key(&mut t, VK_ESCAPE);
        assert!(out.close);
        assert!(out.tool_start.is_none());
    }

    #[test]
    fn tab_goes_round_the_choices_more_options_and_the_buttons() {
        let mut t = video(31_000_000);
        key(&mut t, VK_TAB);
        assert_eq!(t.focus(), Stop::Cancel);
        key(&mut t, VK_TAB);
        assert_eq!(t.focus(), Stop::Radios);
        key(&mut t, VK_TAB);
        assert_eq!(t.focus(), Stop::Expander);
        // Opening More options adds its lines to the round.
        key(&mut t, VK_SPACE);
        key(&mut t, VK_TAB);
        assert_eq!(t.focus(), Stop::Line(0));
        key(&mut t, VK_TAB);
        assert_eq!(t.focus(), Stop::Line(1));
        // Space turns Sound off.
        key(&mut t, VK_SPACE);
        assert!(!t.model().smaller().unwrap().sound);
        key(&mut t, VK_TAB);
        assert_eq!(t.focus(), Stop::Accent);
        shift_tab(&mut t);
        assert_eq!(t.focus(), Stop::Line(1));
    }

    #[test]
    fn arrows_pick_and_skip_presets_that_would_do_nothing() {
        // 20 MB: 25 and 50 MB are disabled.
        let mut t = video(20_000_000);
        key(&mut t, VK_TAB);
        key(&mut t, VK_TAB);
        assert_eq!(t.focus(), Stop::Radios);
        assert_eq!(t.model().radios().selected, Some(0));
        key(&mut t, VK_DOWN);
        assert_eq!(t.model().radios().selected, Some(3), "Custom");
        // Custom's box joins the round, and takes digits and a point.
        key(&mut t, VK_TAB);
        assert_eq!(t.focus(), Stop::Custom);
        for c in "7.5x".chars() {
            with(SIZE, |cx| t.char(cx, c));
        }
        assert_eq!(t.model().custom().text(), "7.5");
        let out = key(&mut t, VK_RETURN);
        assert_eq!(out.tool_start.unwrap().tool.max_bytes, Some(7_500_000));
    }

    #[test]
    fn a_click_on_a_disabled_preset_does_nothing_and_the_accent_waits_without_a_size() {
        let mut t = video(20_000_000);
        let l = with(SIZE, |cx| t.layout(cx)).0;
        click(&mut t, l.radios[2].row);
        assert_eq!(t.model().radios().selected, Some(0));
        // Custom, empty: Make smaller can't be pressed.
        click(&mut t, l.radios[3].row);
        assert_eq!(t.focus(), Stop::Custom);
        assert!(t.part_disabled(ToolPart::Accent));
        let l = with(SIZE, |cx| t.layout(cx)).0;
        assert!(l.custom.is_some());
        // Waiting for a size: a plain hint under the choices, no card.
        assert!(l.card.is_none(), "no estimate card for a hint");
        let hint = l.hint.expect("the hint");
        assert!(hint.y >= l.radios[3].row.bottom());
        assert!(t.model().hint().is_some());
        let out = click(&mut t, l.frame.buttons[0]);
        assert!(out.tool_start.is_none() && !out.close);
        let out = click(&mut t, l.frame.buttons[1]);
        assert!(out.close && out.tool_start.is_none());
    }

    #[test]
    fn the_layout_reads_top_down_and_more_options_opens_its_lines() {
        let mut t = video(31_000_000);
        let l = with(SIZE, |cx| t.layout(cx)).0;
        assert!(l.sub.y < l.heading.y && l.heading.y < l.radios[0].row.y);
        let (card, ..) = l.card.unwrap();
        assert!(card.y > l.radios[3].row.bottom());
        let e = l.expander.unwrap();
        assert!(e.header.y > card.bottom());
        assert!(l.lines.is_empty());
        assert_eq!(l.scroll_max, 0.0);
        click(&mut t, e.header);
        let l = with(SIZE, |cx| t.layout(cx)).0;
        assert_eq!(l.lines.len(), 2);
        assert_eq!(l.lines[0].line, MoreLine::PictureSize);
        // Sound has no note: its label is level with its switch.
        let sound = l.lines[1];
        assert_eq!(
            sound.label.y + sound.label.h / 2.0,
            sound.control.y + sound.control.h / 2.0
        );
        // The picture size's list opens over it, and a pick closes it.
        click(&mut t, l.lines[0].control);
        let l = with(SIZE, |cx| t.layout(cx)).0;
        let list = l.list.clone().expect("open");
        click(&mut t, list.items[0]);
        assert_eq!(t.model().smaller().unwrap().picture_lines(), None, "Keep");
        assert!(with(SIZE, |cx| t.layout(cx)).0.list.is_none());
        // A PDF has no More options.
        let pdf = File {
            path: PathBuf::from("C:\\d.pdf"),
            name: "d.pdf".into(),
            ext: "pdf".into(),
            bytes: 9_000_000,
            picture: None,
            fps: None,
        };
        let t = ToolUi::new(
            ToolDialog::new(Tool::Smaller, vec![pdf]).unwrap(),
            kiln_core::Kind::Document,
            Via::Window,
            Saving::NextToOriginal,
        );
        assert!(with(SIZE, |cx| t.layout(cx)).0.expander.is_none());
    }

    #[test]
    fn in_a_small_window_the_content_scrolls_to_what_has_the_focus() {
        let small = (720.0, 480.0);
        let mut t = video(31_000_000);
        with(small, |cx| t.expand(cx, true));
        let l = with(small, |cx| t.layout(cx)).0;
        assert!(
            l.scroll_max > 0.0,
            "the open dialog is taller than the window"
        );
        // The frame stays in the window.
        assert!(l.frame.rect.bottom() <= small.1);
        with(small, |cx| t.focus_stop(cx, Stop::Line(1)));
        let l = with(small, |cx| t.layout(cx)).0;
        let c = l.frame.content;
        let r = l.lines[1].control;
        assert!(
            r.y >= c.y && r.bottom() <= c.bottom() + 0.5,
            "{r:?} in {c:?}"
        );
        with(small, |cx| t.scroll(cx, TextScroll::Wheel(10.0)));
        let l = with(small, |cx| t.layout(cx)).0;
        assert_eq!(l.sub.y, l.frame.content.y, "back at the top");
    }

    #[test]
    fn opening_more_options_scrolls_its_lines_into_view_and_keeps_its_header() {
        let small = (1100.0, 600.0);
        let mut t = video(31_000_000);
        let l = with(small, |cx| t.layout(cx)).0;
        click_in(&mut t, small, l.expander.unwrap().header);
        let l = with(small, |cx| t.layout(cx)).0;
        assert!(l.scroll_max > 0.0);
        let c = l.frame.content;
        let last = l.lines.last().unwrap().control;
        assert!(last.bottom() <= c.bottom() + 0.5, "{last:?} in {c:?}");
        let header = l.expander.unwrap().header;
        assert!(header.y >= c.y - 0.5, "{header:?} in {c:?}");
        assert!(l.scroll > 0.0);
    }

    #[test]
    fn resize_starts_with_the_long_side_picked() {
        let mut t = photo();
        let l = with(SIZE, |cx| t.layout(cx)).0;
        assert_eq!(l.radios.len(), 5);
        assert!(l.card.is_none() && l.expander.is_none());
        click(&mut t, l.radios[3].row);
        let out = key(&mut t, VK_RETURN);
        let s = out.tool_start.unwrap();
        assert_eq!(s.action, Action::Tool(Tool::Resize, kiln_core::Kind::Image));
        assert_eq!(s.tool.long_side, Some(1280));
        assert!(!s.tool.remove_location);
    }

    #[test]
    fn the_estimate_is_asked_once_for_what_is_picked() {
        let mut t = video(31_000_000);
        let (path, opts) = t.take_estimate_wanted().unwrap();
        assert_eq!(path, PathBuf::from("C:\\v\\clip.mp4"));
        assert_eq!(t.take_estimate_wanted(), None);
        assert_eq!(
            t.model().estimate_lines(t.saving()).unwrap().0,
            "Working out the size…"
        );
        with(SIZE, |cx| {
            t.estimated(cx, opts, Estimate::About(24_100_000))
        });
        assert_eq!(
            t.model().estimate_lines(t.saving()).unwrap().0,
            "About 24 MB"
        );
        assert!(photo().take_estimate_wanted().is_none(), "Resize has none");
    }

    #[test]
    fn the_estimate_card_has_16_by_12_padding() {
        // as every other card.
        let t = video(31_000_000);
        let l = with(SIZE, |cx| t.layout(cx)).0;
        let (card, bold, detail) = l.card.unwrap();
        assert_eq!(CARD_PAD, (16.0, 12.0));
        assert_eq!(bold.x - card.x, 16.0);
        assert_eq!(bold.y - card.y, 12.0);
        assert_eq!(card.right() - bold.right(), 16.0);
        assert_eq!(card.bottom() - detail.bottom(), 12.0);
    }

    #[test]
    fn a_note_that_wraps_to_two_lines_makes_its_row_taller() {
        // a one-line note keeps the 56-DIP row; two lines add 16.
        let mut t = video(31_000_000);
        with(SIZE, |cx| t.expand(cx, true));
        let d = t.model();
        let m = |note_hs: Vec<f32>| Measured {
            title_h: 28.0,
            labels: vec![60.0; d.labels().len()],
            tails: vec![0.0; d.labels().len()],
            detail_h: 16.0,
            combo_w: 140.0,
            note_hs,
        };
        let one = layout_of(d, &m(vec![16.0, 0.0]), SIZE, 0.0);
        let two = layout_of(d, &m(vec![32.0, 0.0]), SIZE, 0.0);
        assert_eq!(one.lines.len(), 2);
        // The first row grows by a line, and the one under it moves down.
        assert_eq!(two.lines[0].note.h, 32.0);
        let gap = |l: &ToolLayout| l.lines[1].control.y - l.lines[0].control.y;
        assert_eq!(gap(&two) - gap(&one), 16.0);
        // The note ends 12 above the row's end, as a one-line note does.
        let row = |l: &ToolLayout, k: usize| {
            let next = l.lines.get(k + 1).map(|p| p.control.y - 12.0);
            next.unwrap_or(f32::MAX)
        };
        assert_eq!(row(&two, 0) - two.lines[0].note.bottom(), 12.0);
        assert_eq!(row(&one, 0) - one.lines[0].note.bottom(), 12.0);
        // The expander holds all of it.
        let e = two.expander.unwrap();
        let last = two.lines.last().unwrap();
        assert!(e.content.unwrap().bottom() >= last.control.bottom());
        assert_eq!(
            e.content.unwrap().h - one.expander.unwrap().content.unwrap().h,
            16.0
        );
    }

    #[test]
    fn the_dialog_stays_under_the_title_bar_and_in_the_layer() {
        // in an 1100 x 720 window with More options open.
        let size = (1100.0, 720.0);
        let mut t = video(31_000_000);
        with(size, |cx| t.expand(cx, true));
        let l = with(size, |cx| t.layout(cx)).0;
        let layer = crate::layout::content_layer(size.0, size.1, crate::layout::TITLE_BAR);
        assert!(l.frame.rect.y >= layer.y + kit::DIALOG_MARGIN);
        assert!(l.frame.rect.bottom() <= layer.bottom() - kit::DIALOG_MARGIN + 0.5);
        assert_eq!(l.frame.rect.w, 480.0);
    }
}
