//! Settings as one page of cards (mock screen 6), in the content layer
//! under the title bar, whose Back closes it.
//!
//! The page owns where the pointer and the keyboard are, how far it is
//! scrolled, which helper apps' Expanders are open and an open ComboBox's
//! list. A control that changes a setting is handed to `App` through
//! `cx.out.changes`, because the change reaches past the page: it is
//! saved, the engines are looked for again, the theme and the drag-wheel
//! follow. A folder or a program is chosen through the `PickFolder` and
//! `PickProgram` effects there.

use super::*;

use kiln_core::Preset;

use crate::controls::{ComboEvent, ComboState, Key, Press};
use crate::layout::Rect;
use crate::layout::{kit, Fit};
use crate::quality::DefaultSetting;
use crate::settings_page::{self as sp, Body, Card, Facts, Helper, PageSpec, Set};

/// How far one wheel notch scrolls the page.
const WHEEL_STEP: f32 = 48.0;
/// How far an arrow key's line scrolls it.
const LINE_STEP: f32 = 40.0;

/// The renderer's measures as the page's layout asks for them.
struct Fitted<'a>(&'a dyn Measure);

impl Fit for Fitted<'_> {
    fn width(&self, s: &str, caption: bool) -> f32 {
        self.0.fit_width(s, caption)
    }

    fn height(&self, s: &str, caption: bool, width: f32) -> f32 {
        self.0.fit_height(s, caption, width)
    }
}

/// A ComboBox whose list is open: which, and where its highlight is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::win) struct OpenCombo {
    pub control: Control,
    pub state: ComboState,
}

/// Settings while it is open.
pub(in crate::win) struct SettingsPanel {
    facts: Facts,
    /// Which helper apps' Expanders are open, in `Helper::ALL` order.
    open: [bool; 5],
    scroll: f32,
    hover: Option<Control>,
    pressed: Option<Control>,
    /// The ComboBox list item the pointer is on.
    hover_item: Option<usize>,
    focus: Option<Control>,
    /// The keyboard has been used since it opened: the focus visual shows.
    keys: bool,
    /// Tab put the keyboard on Back, on the title bar: it comes before
    /// the page's first stop and after its last. `focus` is then `None`.
    on_back: bool,
    combo: Option<OpenCombo>,
    /// A closed list's choice an arrow key moved onto that asks for
    /// something first (`sp::asks_first`): shown, and chosen only by
    /// Enter or Space. Any other key or a click lets it go.
    shown: Option<(Control, usize)>,
    /// When it opened: it rises into place.
    opened: Instant,
    /// When it last scrolled: the scroll indicator shows for a while.
    scrolled: Option<Instant>,
    /// Each Expander's chevron, 0 shut to 1 open, as it turns.
    chevrons: [Tween; 5],
    /// The drag-wheel's knob, 0 off to 1 on, as it moves.
    knobs: [Tween; 1],
}

/// The ToggleSwitches whose knobs move, in `SettingsPanel::knobs` order.
const KNOBS: [Control; 1] = [Control::DragWheel];

impl SettingsPanel {
    /// The page at its top, opened at `opened`, saying what `facts` hold.
    /// Nothing has the focus until a key is pressed: Tab starts at the
    /// first control.
    pub fn new(facts: Facts, settings: &Settings, opened: Instant) -> Self {
        let on = |_: Control| settings.drag_wheel;
        SettingsPanel {
            facts,
            open: [false; 5],
            scroll: 0.0,
            hover: None,
            pressed: None,
            hover_item: None,
            focus: None,
            keys: false,
            on_back: false,
            combo: None,
            shown: None,
            opened,
            scrolled: None,
            chevrons: std::array::from_fn(|_| Tween::new(0.0, opened)),
            knobs: KNOBS.map(|c| Tween::new(if on(c) { 1.0 } else { 0.0 }, opened)),
        }
    }

    /// The page's words and controls as they are now.
    pub fn page(&self, cx: &Ctx) -> PageSpec {
        let mut page = sp::page(cx.settings, &self.facts, self.open, cx.hooks_refused);
        if let Some((c, i)) = self.shown {
            for card in page.sections.iter_mut().flat_map(|(_, cards)| cards) {
                if let Body::Combo { control, value, .. } = &mut card.body {
                    if *control == c {
                        *value = i;
                    }
                }
            }
        }
        page
    }

    /// The page laid out in the window's content layer.
    pub fn layout(&self, cx: &Ctx) -> SettingsLayout {
        let (w, h) = cx.size;
        let view = layout::content_layer(w, h, layout::TITLE_BAR);
        layout::settings(view, self.scroll, &self.page(cx), &Fitted(cx.measure))
    }

    /// What Kiln found out changed: the page says the new facts. The
    /// focus stays on the control it was on, if it is still there.
    pub fn set_facts(&mut self, facts: Facts) {
        // A note about something that went wrong stays until the next
        // press.
        let alert = self.facts.alert.take();
        self.facts = Facts { alert, ..facts };
    }

    /// What it says now.
    #[cfg(test)]
    pub fn facts(&self) -> &Facts {
        &self.facts
    }

    /// Say `text` in `card`'s note, as something that went wrong, or take
    /// it back.
    pub fn set_alert(&mut self, alert: Option<(Card, String)>) {
        self.facts.alert = alert;
    }

    /// The control the keyboard is on.
    pub fn focus(&self) -> Option<Control> {
        self.focus
    }

    /// The control the pointer is over, and the one it has pressed.
    pub fn pointer(&self) -> (Option<Control>, Option<Control>) {
        (self.hover, self.pressed)
    }

    /// Whether Tab put the keyboard on Back, on the title bar.
    pub fn on_back(&self) -> bool {
        self.on_back
    }

    /// Whether helper `h`'s Expander is open.
    #[cfg(test)]
    pub fn is_open(&self, h: Helper) -> bool {
        self.open[h.index()]
    }

    /// The open ComboBox's list, if one is open.
    #[cfg(test)]
    pub fn combo(&self) -> Option<OpenCombo> {
        self.combo
    }

    /// Put the keyboard on `c` and scroll it into view.
    pub fn focus_on(&mut self, cx: &Ctx, c: Control) {
        self.shown = None;
        self.on_back = false;
        self.focus = Some(c);
        self.show(cx, c);
    }

    /// Scroll so that `c` shows whole.
    fn show(&mut self, cx: &Ctx, c: Control) {
        let l = self.layout(cx);
        if let Some(i) = l.find(c) {
            let to = l.scroll_to_show(&l.controls[i].1);
            self.scroll_to(cx, to);
        }
    }

    fn scroll_to(&mut self, cx: &Ctx, to: f32) {
        if (to - self.scroll).abs() > 0.01 {
            self.scroll = to;
            self.scrolled = Some(cx.now);
        }
    }

    /// Whether anything on it moves at `now`.
    pub fn animating(&self, now: Instant, animations: bool) -> bool {
        anim::within(self.opened, anim::SHEET_MS, now, animations)
            || self.chevrons.iter().any(|t| t.running(now))
            || self.knobs.iter().any(|t| t.running(now))
            || self.scrolled.is_some_and(|t| {
                now.saturating_duration_since(t)
                    < Duration::from_millis(u64::from(anim::SCROLL_SHOW_MS) + 200)
            })
    }

    /// Open or close helper `h`'s Expander.
    pub fn set_open(&mut self, cx: &mut Ctx, h: Helper, open: bool) {
        if self.open[h.index()] == open {
            return;
        }
        self.open[h.index()] = open;
        self.chevrons[h.index()].set_fluent(
            if open { 1.0 } else { 0.0 },
            theme::fluent::motion::SMALL,
            cx.now,
            cx.animations,
        );
        // Shut, it may leave the page shorter than its scroll.
        let l = self.layout(cx);
        self.scroll = l.scroll;
        cx.out.animate = true;
        cx.out.invalidate = true;
    }

    /// The card body a control belongs to, as the page has it now.
    fn body_of(page: &PageSpec, c: Control) -> Option<&Body> {
        page.cards().map(|k| &k.body).find(|b| match b {
            Body::Combo { control, .. } | Body::Toggle { control, .. } => *control == c,
            Body::Radios { setting, .. } => matches!(c, Control::Preset(s, _) if s == *setting),
            Body::Buttons(b) => b.iter().any(|(k, _)| *k == c),
            Body::Expander(e) => {
                c == Control::Helper(e.helper) || e.buttons.iter().any(|(k, _)| *k == c)
            }
        })
    }

    /// Press, pick or turn control `c`, from a click, a key or UI
    /// Automation. An Expander and a ComboBox's list are the page's own;
    /// everything else changes a setting, which `App` carries out.
    pub fn act(&mut self, cx: &mut Ctx, c: Control, set: Set) {
        cx.out.invalidate = true;
        // Something new was pressed: an old note of what went wrong goes.
        self.shown = None;
        self.facts.alert = None;
        match (c, set) {
            (Control::Helper(h), _) => {
                let open = !self.open[h.index()];
                self.set_open(cx, h, open);
            }
            (Control::DragWheel, Set::Turn(on)) => {
                if let Some(k) = KNOBS.iter().position(|k| *k == c) {
                    self.knobs[k].set_fluent(
                        if on { 1.0 } else { 0.0 },
                        theme::fluent::motion::SMALL,
                        cx.now,
                        cx.animations,
                    );
                    cx.out.animate = true;
                }
                cx.out.changes.push((c, set));
            }
            _ => cx.out.changes.push((c, set)),
        }
    }

    /// Open `c`'s list, or close it as it was.
    fn open_combo(&mut self, cx: &mut Ctx, c: Control) {
        let page = self.page(cx);
        let Some(Body::Combo { choices, value, .. }) = Self::body_of(&page, c) else {
            return;
        };
        let mut state = ComboState::new(choices.len(), *value);
        if state.click() == ComboEvent::Opened {
            self.combo = Some(OpenCombo { control: c, state });
            self.hover_item = None;
        }
        cx.out.invalidate = true;
    }

    /// Close an open list as it was.
    pub fn close_combo(&mut self) {
        self.combo = None;
    }

    /// Open (`open`) or close `c`'s list, as UI Automation's
    /// ExpandCollapse asks.
    pub fn expand_combo(&mut self, cx: &mut Ctx, c: Control, open: bool) {
        let is_open = self.combo.is_some_and(|o| o.control == c);
        if open && !is_open {
            self.focus = Some(c);
            self.open_combo(cx, c);
        } else if !open && is_open {
            self.combo = None;
            cx.out.invalidate = true;
        }
    }

    /// The open list laid out, anchored to its ComboBox.
    pub fn combo_list(&self, cx: &Ctx) -> Option<kit::FlyoutLayout> {
        let open = self.combo?;
        let l = self.layout(cx);
        let anchor = l.controls[l.find(open.control)?].1;
        let (w, h) = cx.size;
        Some(kit::combo_flyout(
            anchor,
            open.state.len,
            // Placed by the value, so the list holds still as the
            // highlight moves.
            open.state.value,
            Rect::new(0.0, layout::TITLE_BAR, w, (h - layout::TITLE_BAR).max(0.0)),
        ))
    }

    /// A ComboBox's key or click answered: a new value goes to `App`.
    fn combo_event(&mut self, cx: &mut Ctx, e: ComboEvent) {
        let Some(open) = self.combo else {
            return;
        };
        match e {
            ComboEvent::Changed(i) => {
                self.combo = None;
                self.act(cx, open.control, Set::Pick(i));
            }
            ComboEvent::Closed => self.combo = None,
            ComboEvent::Moved(_) | ComboEvent::Opened | ComboEvent::Ignored => {}
        }
        cx.out.invalidate = true;
    }

    /// The Tab stops in order: every control, but a group of radio
    /// buttons is one stop, landing on the one picked.
    fn stops(page: &PageSpec, l: &SettingsLayout) -> Vec<Control> {
        let mut out: Vec<Control> = vec![];
        for (c, _) in &l.controls {
            if let Control::Preset(s, _) = c {
                if matches!(out.last(), Some(Control::Preset(t, _)) if t == s) {
                    continue;
                }
                let picked = match Self::body_of(page, *c) {
                    Some(Body::Radios { value: Some(p), .. }) => *p,
                    _ => Preset::ALL[0],
                };
                out.push(Control::Preset(*s, picked));
            } else {
                out.push(*c);
            }
        }
        out
    }

    /// Tab, or Shift+Tab with `back`: the next stop, with Back on the
    /// title bar before the first and after the last.
    fn tab(&mut self, cx: &Ctx, back: bool) {
        let l = self.layout(cx);
        let page = self.page(cx);
        let stops = Self::stops(&page, &l);
        if stops.is_empty() {
            return;
        }
        // Where the focus is among the stops (a radio button counts as
        // its group's stop).
        let at = self.focus.and_then(|f| {
            stops.iter().position(|s| match (s, f) {
                (Control::Preset(a, _), Control::Preset(b, _)) => *a == b,
                _ => *s == f,
            })
        });
        let next = match (self.on_back, at, back) {
            (true, _, false) | (false, None, false) => Some(0),
            (true, _, true) => Some(stops.len() - 1),
            (false, None, true) => None,
            (false, Some(i), false) => (i + 1 < stops.len()).then_some(i + 1),
            (false, Some(i), true) => i.checked_sub(1),
        };
        match next {
            Some(i) => self.focus_on(cx, stops[i]),
            None => {
                self.on_back = true;
                self.focus = None;
            }
        }
    }

    /// Up or Down off a ComboBox or radio buttons: the control before or
    /// after.
    fn step_focus(&mut self, cx: &Ctx, back: bool) {
        let l = self.layout(cx);
        let n = l.controls.len();
        if n == 0 {
            return;
        }
        let at = self.focus.and_then(|f| l.find(f));
        let to = match (at, back) {
            (None, false) => 0,
            (None, true) => n - 1,
            (Some(i), false) => (i + 1).min(n - 1),
            (Some(i), true) => i.saturating_sub(1),
        };
        let c = l.controls[to].0;
        self.focus_on(cx, c);
    }

    /// The radio button `delta` along from the focused one, in its group:
    /// it takes the focus and is picked. The ends hold.
    fn step_radio(&mut self, cx: &mut Ctx, s: DefaultSetting, p: Preset, delta: isize) {
        let i = Preset::ALL.iter().position(|x| *x == p).unwrap_or(0) as isize + delta;
        if !(0..Preset::ALL.len() as isize).contains(&i) {
            return;
        }
        let next = Preset::ALL[i as usize];
        let c = Control::Preset(s, next);
        self.focus_on(cx, c);
        self.act(cx, c, Set::Pick(i as usize));
    }

    /// Enter or Space on control `c`.
    fn press_key(&mut self, cx: &mut Ctx, c: Control) {
        let page = self.page(cx);
        match Self::body_of(&page, c) {
            Some(Body::Combo { .. }) => self.open_combo(cx, c),
            Some(Body::Toggle { on, .. }) => {
                let on = !*on;
                self.act(cx, c, Set::Turn(on));
            }
            Some(Body::Radios { .. }) => {
                if let Control::Preset(_, p) = c {
                    let i = Preset::ALL.iter().position(|x| *x == p).unwrap_or(0);
                    self.act(cx, c, Set::Pick(i));
                }
            }
            Some(_) => self.act(cx, c, Set::Press),
            None => {}
        }
    }

    /// The pointer acted on control `c` (released over it).
    fn click(&mut self, cx: &mut Ctx, c: Control) {
        self.shown = None;
        self.focus = Some(c);
        self.on_back = false;
        self.press_key(cx, c);
    }
}

impl Surface for SettingsPanel {
    type View<'a> = Option<SettingsView<'a>>;
    type A11y<'a> = Option<a11y::SettingsIn>;

    fn hit(&self, cx: &Ctx, x: f32, y: f32) -> Option<ControlId> {
        self.layout(cx).hit(x, y).map(ControlId::Setting)
    }

    /// The page takes every move.
    fn hover(&mut self, cx: &mut Ctx, x: f32, y: f32) -> Reply {
        if self.combo.is_some() {
            let item = self.combo_list(cx).and_then(|l| {
                l.items
                    .iter()
                    .position(|r| r.contains(x, y))
                    .map(|k| l.first + k)
            });
            if item != self.hover_item {
                self.hover_item = item;
                if let (Some(i), Some(open)) = (item, self.combo.as_mut()) {
                    open.state.highlight = i;
                }
                cx.out.invalidate = true;
            }
            return Reply::Taken;
        }
        let l = self.layout(cx);
        let hover = l.hit(x, y).map(|i| l.controls[i].0);
        if self.hover != hover {
            self.hover = hover;
            cx.out.invalidate = true;
        }
        Reply::Taken
    }

    /// The page takes every press. With a ComboBox's list open, a press
    /// off the list closes it and does nothing else.
    fn press(&mut self, cx: &mut Ctx, x: f32, y: f32) -> Reply {
        self.keys = false;
        if self.combo.is_some() {
            let on_list = self.combo_list(cx).is_some_and(|l| l.rect.contains(x, y));
            if !on_list {
                self.combo = None;
                self.pressed = None;
            }
            cx.out.invalidate = true;
            return Reply::Taken;
        }
        let l = self.layout(cx);
        if let Some(i) = l.hit(x, y) {
            self.pressed = Some(l.controls[i].0);
            cx.out.effects.push(Effect::Capture);
        }
        cx.out.invalidate = true;
        Reply::Taken
    }

    /// What was pressed acts if the pointer is still on it; a list item
    /// released on is picked.
    fn release(&mut self, cx: &mut Ctx, x: f32, y: f32) -> Reply {
        if let Some(open) = self.combo {
            let item = self.combo_list(cx).and_then(|l| {
                l.items
                    .iter()
                    .position(|r| r.contains(x, y))
                    .map(|k| l.first + k)
            });
            if let Some(i) = item {
                let mut state = open.state;
                let e = state.pick(i);
                self.combo = Some(OpenCombo { state, ..open });
                self.combo_event(cx, e);
            }
            return Reply::Taken;
        }
        let Some(c) = self.pressed.take() else {
            return Reply::Pass;
        };
        let l = self.layout(cx);
        if l.hit(x, y).map(|i| l.controls[i].0) == Some(c) {
            self.click(cx, c);
        }
        cx.out.invalidate = true;
        Reply::Taken
    }

    /// Tab and Shift+Tab go from stop to stop, Back on the title bar
    /// between the last and the first. Up and Down step a ComboBox's
    /// value or the radio buttons, and elsewhere move to the control
    /// before or after; Left and Right step radio buttons. Enter and
    /// Space press; F4 opens a ComboBox. Page Up, Page Down, Home and End
    /// scroll. Esc closes an open list, else Settings.
    fn key(&mut self, cx: &mut Ctx, key: VIRTUAL_KEY, mods: Mods) -> Reply {
        self.keys = true;
        cx.out.invalidate = true;
        // A choice an arrow only showed: Enter or Space chooses it, an
        // arrow moves on from it, anything else lets it go.
        let shown = self.shown.take();
        let press = |k: Key| Press {
            key: k,
            alt: false,
            ctrl: mods.ctrl,
            shift: mods.shift,
        };
        let as_key = match key {
            VK_UP => Some(Key::Up),
            VK_DOWN => Some(Key::Down),
            VK_HOME => Some(Key::Home),
            VK_END => Some(Key::End),
            VK_RETURN => Some(Key::Enter),
            VK_SPACE => Some(Key::Space),
            VK_ESCAPE => Some(Key::Esc),
            VK_TAB => Some(Key::Tab),
            VK_F4 => Some(Key::F4),
            _ => None,
        };
        // An open list keeps the keys.
        if let Some(mut open) = self.combo {
            if let Some(k) = as_key {
                if mods.repeat && matches!(k, Key::Enter | Key::Space) {
                    return Reply::Taken;
                }
                let e = open.state.key(press(k));
                self.combo = Some(open);
                self.combo_event(cx, e);
            }
            return Reply::Taken;
        }
        // A held Enter or Space acts once.
        if mods.repeat && matches!(key, VK_RETURN | VK_SPACE) {
            return Reply::Taken;
        }
        let page = self.page(cx);
        let focused = self.focus.filter(|_| !self.on_back);
        let body = focused.and_then(|c| Self::body_of(&page, c).cloned());
        let shown = shown.filter(|(c, _)| focused == Some(*c));
        match key {
            VK_ESCAPE => cx.out.close = true,
            VK_TAB => self.tab(cx, mods.shift),
            VK_RETURN | VK_SPACE if self.on_back => cx.out.close = true,
            VK_RETURN | VK_SPACE if shown.is_some() => {
                if let Some((c, i)) = shown {
                    self.act(cx, c, Set::Pick(i));
                }
            }
            VK_RETURN | VK_SPACE | VK_F4 => {
                if let Some(c) = focused {
                    if key != VK_F4 || matches!(body, Some(Body::Combo { .. })) {
                        self.press_key(cx, c);
                    }
                }
            }
            VK_UP | VK_DOWN | VK_LEFT | VK_RIGHT => {
                let back = matches!(key, VK_UP | VK_LEFT);
                match (focused, &body) {
                    (Some(c), Some(Body::Combo { choices, value, .. }))
                        if matches!(key, VK_UP | VK_DOWN) =>
                    {
                        // From the choice shown, if an arrow showed one.
                        let from = shown.map_or(*value, |(_, i)| i);
                        let mut state = ComboState::new(choices.len(), from);
                        match state.key(press(if back { Key::Up } else { Key::Down })) {
                            // Back to what the setting already is.
                            ComboEvent::Changed(i) if i == *value => {}
                            ComboEvent::Changed(i) if sp::asks_first(c, i) => {
                                self.shown = Some((c, i));
                            }
                            ComboEvent::Changed(i) => self.act(cx, c, Set::Pick(i)),
                            // At the end already: a shown choice stays.
                            _ => self.shown = shown,
                        }
                    }
                    (Some(Control::Preset(s, p)), _) => {
                        self.step_radio(cx, s, p, if back { -1 } else { 1 })
                    }
                    _ if matches!(key, VK_UP | VK_DOWN) => self.step_focus(cx, back),
                    _ => {}
                }
            }
            VK_PRIOR | VK_NEXT | VK_HOME | VK_END => {
                let by = match key {
                    VK_PRIOR => TextScroll::PageUp,
                    VK_NEXT => TextScroll::PageDown,
                    VK_HOME => TextScroll::Home,
                    _ => TextScroll::End,
                };
                self.scroll(cx, by);
            }
            _ => cx.out.invalidate = false,
        }
        Reply::Taken
    }

    /// The page scrolls; nothing beneath it does. An open list closes.
    fn scroll(&mut self, cx: &mut Ctx, by: TextScroll) -> Reply {
        self.combo = None;
        let l = self.layout(cx);
        let page = (l.view.h - LINE_STEP).max(LINE_STEP);
        let to = match by {
            TextScroll::LineUp => l.scroll - LINE_STEP,
            TextScroll::LineDown => l.scroll + LINE_STEP,
            TextScroll::PageUp => l.scroll - page,
            TextScroll::PageDown => l.scroll + page,
            TextScroll::Home => 0.0,
            TextScroll::End => l.max_scroll(),
            TextScroll::Wheel(n) => l.scroll - n * WHEEL_STEP,
        }
        .clamp(0.0, l.max_scroll());
        if (to - self.scroll).abs() > 0.01 {
            self.scroll_to(cx, to);
            self.hover = None;
            cx.out.invalidate = true;
            cx.out.animate = true;
        }
        Reply::Block
    }

    fn leave(&mut self) {
        self.hover = None;
        self.pressed = None;
        self.hover_item = None;
    }

    fn view<'a>(
        &'a self,
        cx: &Ctx,
        _frame: Frame<'a>,
        button: &'a dyn Fn(ButtonId) -> (f32, f32),
    ) -> Option<SettingsView<'a>> {
        let (open, rise) = sheet_rise(self.opened, cx.now, cx.animations);
        let knob = |k: usize, on: bool| {
            let t = &self.knobs[k];
            let target = if on { 1.0 } else { 0.0 };
            // A setting changed elsewhere (the tray) snaps to its place.
            if (t.target() - target).abs() > 0.01 {
                target
            } else {
                t.value(cx.now)
            }
        };
        let s = cx.settings;
        let idle = self.scrolled.map_or(f32::INFINITY, |t| {
            cx.now.saturating_duration_since(t).as_secs_f32() * 1000.0
        });
        Some(SettingsView {
            layout: self.layout(cx),
            page: self.page(cx),
            open,
            rise,
            hover: self.hover,
            pressed: self.pressed,
            focus: self.focus.filter(|_| self.keys && !self.on_back),
            button,
            combo: self
                .combo
                .and_then(|o| Some((o.control, o.state, self.combo_list(cx)?))),
            chevrons: std::array::from_fn(|i| self.chevrons[i].value(cx.now)),
            knobs: [knob(0, s.drag_wheel)],
            scroll_shown: anim::scroll_indicator(idle, false, cx.animations),
        })
    }

    fn a11y<'a>(&'a self, cx: &Ctx, _frame: Frame<'a>) -> Option<a11y::SettingsIn> {
        Some(a11y::SettingsIn {
            layout: self.layout(cx),
            page: self.page(cx),
            focus: self.focus.filter(|_| !self.on_back),
            combo: self
                .combo
                .and_then(|o| Some((o.control, o.state, self.combo_list(cx)?))),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::EngineStatus;

    struct Nothing;
    impl Measure for Nothing {
        fn mono_height(&self, _text: &str, _width: f32) -> f32 {
            0.0
        }
        fn body_height(&self, _text: &str, _width: f32) -> f32 {
            0.0
        }
    }

    const SIZE: (f32, f32) = (900.0, 600.0);

    fn open() -> SettingsPanel {
        SettingsPanel::new(Facts::default(), &Settings::default(), Instant::now())
    }

    fn with<R>(f: impl FnOnce(&mut Ctx) -> R) -> (R, Out) {
        with_settings(&Settings::default(), f)
    }

    fn with_settings<R>(settings: &Settings, f: impl FnOnce(&mut Ctx) -> R) -> (R, Out) {
        let mut cx = Ctx::new(&Nothing, settings, SIZE);
        let r = f(&mut cx);
        (r, cx.out)
    }

    /// Where `c` is, scrolled into view.
    fn rect_of(p: &mut SettingsPanel, c: Control) -> Rect {
        with(|cx| p.show(cx, c));
        let (l, _) = with(|cx| p.layout(cx));
        l.controls[l.find(c).expect("the control is shown")].1
    }

    fn center(r: Rect) -> (f32, f32) {
        (r.x + r.w / 2.0, r.y + r.h / 2.0)
    }

    fn click(p: &mut SettingsPanel, c: Control) -> Out {
        let (x, y) = center(rect_of(p, c));
        with(|cx| p.press(cx, x, y));
        with(|cx| p.release(cx, x, y)).1
    }

    fn key(p: &mut SettingsPanel, k: VIRTUAL_KEY) -> Out {
        with(|cx| p.key(cx, k, Mods::default())).1
    }

    fn shift_tab(p: &mut SettingsPanel) -> Out {
        with(|cx| p.key(cx, VK_TAB, Mods::held(true, false))).1
    }

    #[test]
    fn it_opens_at_the_top_with_nothing_focused() {
        let p = open();
        assert_eq!(p.focus(), None);
        assert_eq!(p.pointer(), (None, None));
        assert_eq!(p.scroll, 0.0);
        assert!(Helper::ALL.iter().all(|h| !p.is_open(*h)));
    }

    #[test]
    fn tab_goes_through_every_control_in_order_with_back_between_the_ends() {
        let mut p = open();
        let mut seen = vec![];
        for _ in 0..64 {
            key(&mut p, VK_TAB);
            if p.on_back() {
                break;
            }
            seen.push(p.focus().unwrap());
        }
        assert!(p.on_back(), "Tab never reached Back");
        use DefaultSetting as D;
        assert_eq!(
            seen,
            [
                Control::Theme,
                Control::SaveWhere,
                // Small, High and Best are one stop, on the one picked.
                Control::Preset(D::Video, Preset::High),
                Control::Choice(D::Resolution),
                Control::Preset(D::Images, Preset::High),
                Control::Choice(D::ImageSize),
                Control::Choice(D::Audio),
                Control::Choice(D::PageDpi),
                Control::DragWheel,
                Control::Helper(Helper::Ffmpeg),
                // No Office card while Office to PDF is off (D14).
                Control::Helper(Helper::LibreOffice),
                Control::Helper(Helper::Pandoc),
                Control::Helper(Helper::Calibre),
                Control::CheckUpdates,
                Control::CopyDiagnostics,
                Control::OpenLogFolder,
                Control::Donate,
                Control::Licences,
            ]
        );
        // Past Back, the first again; Shift+Tab goes back the same way.
        key(&mut p, VK_TAB);
        assert_eq!(p.focus(), Some(Control::Theme));
        shift_tab(&mut p);
        assert!(p.on_back());
        shift_tab(&mut p);
        assert_eq!(p.focus(), Some(Control::Licences));
        // Enter on Back closes Settings.
        let mut p = open();
        shift_tab(&mut p);
        assert!(p.on_back());
        assert!(key(&mut p, VK_RETURN).close);
    }

    #[test]
    fn tab_scrolls_the_focus_into_view() {
        let mut p = open();
        // 18 controls, the last Licences (no Office card while Office to
        // PDF is off, D14).
        for _ in 0..18 {
            key(&mut p, VK_TAB);
        }
        assert_eq!(p.focus(), Some(Control::Licences));
        assert!(p.scroll > 0.0);
        let (l, _) = with(|cx| p.layout(cx));
        let r = l.controls[l.find(Control::Licences).unwrap()].1;
        assert!(r.y >= l.view.y && r.bottom() <= l.view.bottom());
    }

    #[test]
    fn escape_closes_it() {
        let mut p = open();
        let out = key(&mut p, VK_ESCAPE);
        assert!(out.close);
    }

    #[test]
    fn a_combo_box_opens_steps_and_picks() {
        let mut p = open();
        // A click opens the Theme list; nothing changes yet.
        let out = click(&mut p, Control::Theme);
        assert!(out.changes.is_empty());
        let open_list = p.combo().expect("open");
        assert_eq!(open_list.control, Control::Theme);
        assert_eq!(open_list.state.value, 0);
        // Down and Enter pick Light.
        key(&mut p, VK_DOWN);
        let out = key(&mut p, VK_RETURN);
        assert_eq!(out.changes, [(Control::Theme, Set::Pick(1))]);
        assert_eq!(p.combo(), None);
        // Esc closes a list without a change, and leaves Settings open.
        click(&mut p, Control::Theme);
        let out = key(&mut p, VK_ESCAPE);
        assert!(out.changes.is_empty() && !out.close);
        assert_eq!(p.combo(), None);
        // Closed and focused, Down steps the value at once (WinUI).
        p.focus = Some(Control::Choice(DefaultSetting::PageDpi));
        let out = key(&mut p, VK_DOWN);
        assert_eq!(
            out.changes,
            [(Control::Choice(DefaultSetting::PageDpi), Set::Pick(2))]
        );
    }

    #[test]
    fn the_keyboard_reaches_every_theme_and_each_applies_at_once() {
        let last = crate::settings_page::THEMES.len() - 1;
        // Closed and focused, Down steps to the next at once.
        let mut p = open();
        p.focus = Some(Control::Theme);
        let out = key(&mut p, VK_DOWN);
        assert_eq!(out.changes, [(Control::Theme, Set::Pick(1))]);
        // Opened with F4, Down goes through the nine, one at a time; the
        // dots are not stops of their own. Enter picks the one lit.
        let mut p = open();
        p.focus = Some(Control::Theme);
        key(&mut p, VK_F4);
        assert_eq!(p.combo().unwrap().control, Control::Theme);
        for i in 1..=last {
            key(&mut p, VK_DOWN);
            assert_eq!(p.combo().unwrap().state.highlight, i);
        }
        let out = key(&mut p, VK_RETURN);
        assert_eq!(out.changes, [(Control::Theme, Set::Pick(last))]);
        assert_eq!(p.combo(), None);
    }

    #[test]
    fn arrowing_onto_a_folder_i_choose_shows_it_and_only_enter_picks_it() {
        let shown = |p: &SettingsPanel| {
            let (page, _) = with(|cx| p.page(cx));
            match page.card(Card::SaveWhere).unwrap().body {
                Body::Combo { value, .. } => value,
                _ => unreachable!(),
            }
        };
        let mut p = open();
        p.focus = Some(Control::SaveWhere);
        // Down shows the choice, and asks for no folder.
        let out = key(&mut p, VK_DOWN);
        assert!(out.changes.is_empty(), "{:?}", out.changes);
        assert_eq!(shown(&p), 1);
        // Down again: the last choice already.
        assert!(key(&mut p, VK_DOWN).changes.is_empty());
        assert_eq!(shown(&p), 1);
        // Enter chooses it: the picker.
        let out = key(&mut p, VK_RETURN);
        assert_eq!(out.changes, [(Control::SaveWhere, Set::Pick(1))]);
        assert_eq!(p.combo(), None, "no list opened");
        // Up from a shown choice goes back to Next to the original, which
        // is what the setting says: nothing to change.
        key(&mut p, VK_DOWN);
        let out = key(&mut p, VK_UP);
        assert!(out.changes.is_empty());
        assert_eq!(shown(&p), 0);
        // Moving on drops a shown choice.
        key(&mut p, VK_DOWN);
        key(&mut p, VK_TAB);
        assert_eq!(shown(&p), 0);
        assert_eq!(
            p.focus(),
            Some(Control::Preset(DefaultSetting::Video, Preset::High))
        );
        // A folder chosen already: Up goes back to Next to the original at
        // once, as any other list does.
        let chosen = Settings {
            save_to: Some(std::path::PathBuf::from(r"D:\Converted")),
            ..Settings::default()
        };
        let mut p = open();
        p.focus = Some(Control::SaveWhere);
        let (_, out) = with_settings(&chosen, |cx| p.key(cx, VK_UP, Mods::default()));
        assert_eq!(out.changes, [(Control::SaveWhere, Set::Pick(0))]);
    }

    #[test]
    fn a_click_on_a_list_item_picks_it_and_a_click_off_it_only_closes_it() {
        let mut p = open();
        click(&mut p, Control::SaveWhere);
        let (list, _) = with(|cx| p.combo_list(cx));
        let list = list.expect("open");
        let (x, y) = center(list.items[1 - list.first]);
        with(|cx| p.hover(cx, x, y));
        let (_, out) = with(|cx| p.press(cx, x, y));
        assert!(out.changes.is_empty());
        let (_, out) = with(|cx| p.release(cx, x, y));
        assert_eq!(out.changes, [(Control::SaveWhere, Set::Pick(1))]);
        // Off the list: closed, and the radio under it isn't picked.
        click(&mut p, Control::SaveWhere);
        let keep = rect_of(
            &mut p,
            Control::Preset(DefaultSetting::Video, Preset::Small),
        );
        let (x, y) = center(keep);
        let (_, out) = with(|cx| p.press(cx, x, y));
        assert!(out.changes.is_empty());
        assert_eq!(p.combo(), None);
        let (_, out) = with(|cx| p.release(cx, x, y));
        assert!(out.changes.is_empty());
    }

    #[test]
    fn a_toggle_turns_and_its_knob_moves() {
        let mut p = open();
        let out = click(&mut p, Control::DragWheel);
        assert_eq!(out.changes, [(Control::DragWheel, Set::Turn(true))]);
        assert!(out.animate);
        // Space turns the focused one; Enter too.
        p.focus = Some(Control::DragWheel);
        let out = key(&mut p, VK_SPACE);
        assert_eq!(out.changes, [(Control::DragWheel, Set::Turn(true))]);
        // A held Space acts once.
        let (_, out) = with(|cx| p.key(cx, VK_SPACE, Mods::with_repeat(false, false, true)));
        assert!(out.changes.is_empty());
    }

    #[test]
    fn radio_buttons_pick_with_a_click_or_the_arrows() {
        use DefaultSetting as D;
        let mut p = open();
        let out = click(&mut p, Control::Preset(D::Video, Preset::Best));
        assert_eq!(
            out.changes,
            [(Control::Preset(D::Video, Preset::Best), Set::Pick(2))]
        );
        p.focus = Some(Control::Preset(D::Images, Preset::High));
        let out = key(&mut p, VK_LEFT);
        assert_eq!(
            out.changes,
            [(Control::Preset(D::Images, Preset::Small), Set::Pick(0))]
        );
        assert_eq!(p.focus(), Some(Control::Preset(D::Images, Preset::Small)));
        // The ends hold.
        let out = key(&mut p, VK_LEFT);
        assert!(out.changes.is_empty());
    }

    #[test]
    fn an_expander_opens_and_shows_its_buttons() {
        let mut p = open();
        p.set_facts(Facts {
            engines: Some(std::array::from_fn(|_| EngineStatus::Missing)),
            ..Facts::default()
        });
        let pandoc = Control::Helper(Helper::Pandoc);
        let (l, _) = with(|cx| p.layout(cx));
        assert_eq!(
            l.find(Control::Choose(crate::settings::Engine::Pandoc)),
            None
        );
        let out = click(&mut p, pandoc);
        assert!(out.changes.is_empty() && out.animate);
        assert!(p.is_open(Helper::Pandoc));
        let (l, _) = with(|cx| p.layout(cx));
        assert!(l
            .find(Control::Choose(crate::settings::Engine::Pandoc))
            .is_some());
        // Enter on its header closes it again.
        p.focus = Some(pandoc);
        key(&mut p, VK_RETURN);
        assert!(!p.is_open(Helper::Pandoc));
    }

    #[test]
    fn buttons_go_to_app() {
        let mut p = open();
        let out = click(&mut p, Control::CheckUpdates);
        assert_eq!(out.changes, [(Control::CheckUpdates, Set::Press)]);
        let out = click(&mut p, Control::Licences);
        assert_eq!(out.changes, [(Control::Licences, Set::Press)]);
    }

    #[test]
    fn the_wheel_scrolls_the_page_and_nothing_beneath() {
        let mut p = open();
        let (reply, out) = with(|cx| p.scroll(cx, TextScroll::Wheel(-2.0)));
        assert_eq!(reply, Reply::Block);
        assert!(out.invalidate);
        assert_eq!(p.scroll, 2.0 * WHEEL_STEP);
        with(|cx| p.scroll(cx, TextScroll::End));
        let (l, _) = with(|cx| p.layout(cx));
        assert_eq!(p.scroll, l.max_scroll());
        key(&mut p, VK_HOME);
        assert_eq!(p.scroll, 0.0);
    }

    #[test]
    fn a_note_of_what_went_wrong_shows_until_the_next_press() {
        let mut p = open();
        p.set_alert(Some((Card::Licences, sp::LICENCES_DIDNT_OPEN.into())));
        let (page, _) = with(|cx| p.page(cx));
        assert!(page.card(Card::Licences).unwrap().alert);
        // New facts keep it.
        p.set_facts(Facts::default());
        assert!(p.facts().alert.is_some());
        click(&mut p, Control::CheckUpdates);
        assert_eq!(p.facts().alert, None);
    }
}
