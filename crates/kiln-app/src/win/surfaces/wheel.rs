//! The format wheel over Kiln's window (mock screens 3 and 7): which
//! segment is lit, More's list and the fade.
//!
//! The wheel owns its motion (`WheelMotion`, shared with the drag-wheel's
//! floating wheel) and what it was opened for. What reaches past it goes
//! through `cx.out`: the animation timer and Help on F1. A pick (R1: a
//! format converts at once) is handed back by `tick` as the wheel starts
//! to fade, for `App` to carry out. The wheel fades by itself (`App::tick`
//! drops it), so closing it needs nothing from `App`.

use std::collections::BTreeMap;

use super::*;

use crate::layout::Rect;
use crate::wheel::{self as ring, FlyoutPlace, FlyoutRow, HubLines, WheelMotion};

/// How far one notch of the mouse wheel scrolls More's list, in DIPs.
const SCROLL_NOTCH: f32 = 48.0;

/// What `App` works out before the wheel opens.
pub(in crate::win) struct Opening {
    pub wheel: Wheel,
    /// The files it acts on.
    pub paths: Vec<PathBuf>,
    /// Opened from the keyboard rather than by the pointer.
    pub keyboard: bool,
    /// What Kiln suggests on this wheel, once every file's facts are in.
    pub suggestion: Option<Suggestion>,
    /// The segment lit as it opens: the keyboard starts on one.
    pub hover: Option<Spot>,
    /// Where More's list may go: the window under the title bar.
    pub bounds: Rect,
    /// One file's likely size as each format, by extension, for the hub.
    pub estimates: BTreeMap<String, u64>,
}

/// The wheel while it is on screen.
pub(in crate::win) struct WheelUi {
    wheel: Wheel,
    /// The files it acts on.
    paths: Vec<PathBuf>,
    /// What is lit, More's list and the opening.
    motion: WheelMotion,
    closing: Option<Instant>,
    /// What was chosen, handed to `App` at the next tick.
    chosen: Option<Picked>,
    keyboard: bool,
    suggestion: Option<Suggestion>,
    bounds: Rect,
    /// How far More's list is scrolled.
    scroll: f32,
    estimates: BTreeMap<String, u64>,
}

/// What a tick of the clock did to the wheel.
pub(in crate::win) enum Tick {
    /// Nothing for `App` to do.
    Idle,
    /// It has faded out: `App` drops it.
    Gone,
    /// Something was chosen and the wheel has started to fade. `App`
    /// carries the pick out for `paths`.
    Picked {
        picked: Option<Picked>,
        paths: Vec<PathBuf>,
        keyboard: bool,
    },
}

/// What UI Automation reads of the wheel.
pub(in crate::win) struct WheelSeen<'a> {
    pub wheel: &'a Wheel,
    pub paths: &'a [PathBuf],
    pub hover: Option<Spot>,
    pub hub: HubLines,
    /// More's list, while it is open: its lines and where they are.
    pub flyout: Option<(Vec<FlyoutRow<'a>>, FlyoutPlace)>,
}

impl WheelUi {
    /// A wheel opened at `now`.
    pub fn new(opening: Opening, now: Instant) -> Self {
        let motion = WheelMotion::starting_on(&opening.wheel, opening.hover, now);
        let mut ui = WheelUi {
            wheel: opening.wheel,
            paths: opening.paths,
            motion,
            closing: None,
            chosen: None,
            keyboard: opening.keyboard,
            suggestion: opening.suggestion,
            bounds: opening.bounds,
            scroll: 0.0,
            estimates: opening.estimates,
        };
        // The list alone is open from the start; from the keyboard it
        // starts on the suggestion, or the first line.
        if ui.wheel.list_only {
            ui.open_more(now, ui.keyboard);
        }
        ui
    }

    /// It takes input: it is not fading.
    pub fn is_open(&self) -> bool {
        self.closing.is_none()
    }

    /// Whether it is the list alone, with no ring.
    #[cfg(test)]
    pub fn list_only(&self) -> bool {
        self.wheel.list_only
    }

    /// The wheel's kind: Convert or Tools.
    pub fn kind(&self) -> WheelKind {
        self.wheel.kind
    }

    /// What is lit now.
    pub fn lit(&self) -> Option<Spot> {
        self.motion.hover
    }

    /// Whether More's list is open.
    pub fn flyout_open(&self) -> bool {
        self.motion.flyout_open()
    }

    /// Whether it is open and waiting on `path`'s facts for a suggestion.
    pub fn awaits(&self, path: &Path) -> bool {
        self.is_open() && self.suggestion.is_none() && self.paths.iter().any(|p| p == path)
    }

    /// The wheel's kind and files, to work out what it suggests.
    pub fn subject(&self) -> (WheelKind, Vec<PathBuf>) {
        (self.wheel.kind, self.paths.clone())
    }

    /// Where the wheel's centre is, in DIPs.
    #[cfg(test)]
    pub fn center(&self) -> (f32, f32) {
        self.wheel.center
    }

    /// The files it acts on.
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// The ring and More as they are now.
    #[cfg(test)]
    pub fn ring(&self) -> &Wheel {
        &self.wheel
    }

    /// What it offers changed while it was open, as when a video turns out
    /// to have no sound: the ring and More hold `items` and `flyout` from
    /// now on. More shuts and nothing stays lit, unless it was opened from
    /// the keyboard, which starts on the first segment again.
    pub fn restock(&mut self, items: Vec<ring::Item>, flyout: Vec<ring::Group>) {
        // The list alone holds everything in its groups.
        let (items, flyout) = if self.wheel.list_only {
            (
                vec![],
                ring::all_groups(items, flyout, self.wheel.source_kind),
            )
        } else {
            (items, flyout)
        };
        if self.wheel.items == items && self.wheel.flyout == flyout {
            return;
        }
        self.wheel.items = items;
        self.wheel.flyout = flyout;
        // Only formats still offered can be the footer's.
        let offered: Vec<String> = self
            .wheel
            .items
            .iter()
            .chain(self.wheel.more_items())
            .filter_map(|i| match &i.action {
                ring::Action::Convert(ext) => Some(ext.clone()),
                ring::Action::Tool(..) => None,
            })
            .collect();
        self.wheel.settable.retain(|e| offered.contains(e));
        self.motion.close_flyout();
        self.scroll = 0.0;
        self.motion.format = None;
        let start = (self.keyboard && self.wheel.segments() > 0).then_some(Spot::Inner(0));
        self.motion.set_hover(&self.wheel, start);
    }

    /// The suggestion arrived while the wheel was open. Nothing on it
    /// changes: it is where the keyboard starts in More.
    pub fn suggest(&mut self, s: Suggestion) {
        // The footer's options are for the suggestion when nothing is lit.
        if let kiln_core::Suggested::Convert(ext) = &s.action {
            if let Some(at) = self.wheel.settable.iter().position(|e| e == ext) {
                let ext = self.wheel.settable.remove(at);
                self.wheel.settable.insert(0, ext);
            }
        }
        self.suggestion = Some(s);
    }

    /// Where the suggestion is: its segment, or More and its line.
    fn marks(&self) -> Vec<Spot> {
        self.suggestion
            .as_ref()
            .map_or_else(Vec::new, |s| ring::marks(&self.wheel, &s.action))
    }

    /// Whether anything on it is moving at `now` and needs full frame
    /// rate: the opening, More's list, a fade or a pick.
    pub fn animating(&self, now: Instant, animations: bool) -> bool {
        self.motion.animating(now, animations) || self.closing.is_some() || self.chosen.is_some()
    }

    /// The options dialog's format for `spot`, as Tab or "Options…" opens
    /// it: its extension, the files and whether the wheel was opened from
    /// the keyboard. `None` when `spot` is no format. The footer row is for
    /// the format last lit if it has settings, else the suggested or the
    /// first that has.
    pub fn options_for(&self, spot: Option<Spot>) -> Option<(String, Vec<PathBuf>, bool)> {
        let ext = match spot {
            // The footer row: the format last lit, or else the one it falls
            // back to.
            Some(Spot::Options) => self
                .wheel
                .options_subject(self.motion.format)
                .map(str::to_owned),
            other => match ring::picked_options(&self.wheel, other)? {
                Picked::Options(ext) => Some(ext),
                _ => None,
            },
        }?;
        Some((ext, self.paths.clone(), self.keyboard))
    }

    /// More's lines as the list shows them: "Options…" last for the format
    /// last lit.
    fn rows(&self) -> Vec<FlyoutRow<'_>> {
        self.wheel.flyout_rows(self.motion.format)
    }

    /// Where More's list is, scrolled as it is.
    fn place(&self, rows: &[FlyoutRow]) -> FlyoutPlace {
        ring::flyout_place(&self.wheel, rows, self.bounds, self.scroll)
    }

    /// What is under `(x, y)`: a line of More's list while it is open, or
    /// else a segment of the ring.
    pub fn spot_at(&self, x: f32, y: f32) -> Option<Spot> {
        if self.flyout_open() {
            let rows = self.rows();
            let place = self.place(&rows);
            if place.rect.contains(x, y) {
                return ring::flyout_hit(&place, &rows, x, y);
            }
        }
        ring::hit(&self.wheel, x, y)
    }

    /// Whether `(x, y)` is on More's open list, a line or not.
    fn on_flyout(&self, x: f32, y: f32) -> bool {
        self.flyout_open() && {
            let rows = self.rows();
            self.place(&rows).rect.contains(x, y)
        }
    }

    /// Start to fade it out. True when that is new.
    pub fn close(&mut self, now: Instant) -> bool {
        if self.closing.is_some() {
            return false;
        }
        self.closing = Some(now);
        true
    }

    /// Open More's list, from the keyboard lighting its first line, or
    /// the suggestion's when it is in it.
    fn open_more(&mut self, now: Instant, by_key: bool) {
        self.motion.open_flyout(now);
        self.scroll = 0.0;
        if by_key {
            let rows = self.rows();
            let first = self
                .marks()
                .into_iter()
                .find(|m| matches!(m, Spot::Flyout(_)))
                .or_else(|| ring::flyout_step(&rows, None, 1));
            drop(rows);
            self.light_in_list(first);
        }
    }

    /// Light `spot` in More's list and scroll it into view.
    fn light_in_list(&mut self, spot: Option<Spot>) {
        self.motion.set_hover(&self.wheel, spot);
        let rows = self.rows();
        let place = self.place(&rows);
        if let Some(i) = rows
            .iter()
            .position(|r| ring::row_spot(r) == spot && spot.is_some())
        {
            self.scroll = place.scroll_to(i);
        }
    }

    /// Choose `spot`: More opens its list; anything else is picked, and
    /// the wheel starts to fade. `by_key` when Enter or Space chose it.
    /// True when something changed.
    pub fn choose(&mut self, spot: Spot, now: Instant, by_key: bool) -> bool {
        if !self.is_open() {
            return false;
        }
        if self.wheel.is_more_spot(spot) {
            self.open_more(now, by_key);
            return true;
        }
        let picked = match spot {
            Spot::Options => self
                .options_for(Some(spot))
                .map(|(ext, ..)| Picked::Options(ext)),
            _ => ring::picked(&self.wheel, spot),
        };
        let Some(picked) = picked else {
            return false;
        };
        self.motion.set_hover(&self.wheel, Some(spot));
        self.chosen = Some(picked);
        self.closing = Some(now);
        true
    }

    /// Point at `spot`. Returns whether anything changed.
    pub fn point(&mut self, spot: Option<Spot>, now: Instant) -> bool {
        self.motion.point(Some(&self.wheel), spot, now)
    }

    /// Hand back what was chosen, and drop a faded wheel.
    pub fn tick(&mut self, now: Instant, animations: bool) -> Tick {
        if let Some(picked) = self.chosen.take() {
            return Tick::Picked {
                picked: Some(picked),
                paths: self.paths.clone(),
                keyboard: self.keyboard,
            };
        }
        if self
            .closing
            .is_some_and(|t| anim::wheel_close(anim::elapsed_ms(t, now), animations) <= 0.0)
        {
            return Tick::Gone;
        }
        Tick::Idle
    }

    /// The footer's hint while it is open: one sentence for the lit
    /// format ("JPG. Photo that opens anywhere, about 85 KB."), or nothing,
    /// and the footer says what it did before.
    pub fn hint(&self, kiln: &Kiln) -> Option<String> {
        if !self.is_open() {
            return None;
        }
        // "(text only)" when every file becomes it by way of its text.
        let text_only = |ext: &str| model::text_only_for(kiln, &self.paths, ext);
        let one = self.paths.len() == 1;
        let estimate = |ext: &str| one.then(|| self.estimates.get(ext).copied()).flatten();
        ring::lit_footer(
            self.motion.hover.and_then(|spot| self.wheel.item(spot)),
            &text_only,
            &estimate,
        )
    }

    /// What the hub says now.
    fn hub(&self) -> HubLines {
        let one = self.paths.len() == 1;
        let estimate = |ext: &str| one.then(|| self.estimates.get(ext).copied()).flatten();
        self.wheel
            .hub_lines(self.motion.hover, self.paths.len(), &estimate)
    }

    /// Keys while More's list is open: Up and Down move through it, Enter
    /// and Space choose, Esc and Left close it and go back to More.
    fn list_key(&mut self, key: VIRTUAL_KEY, now: Instant) -> bool {
        let delta = match key {
            VK_DOWN => 1,
            VK_UP => -1,
            _ => 0,
        };
        let more = Spot::Inner(self.wheel.items.len());
        match key {
            _ if delta != 0 => {
                let rows = self.rows();
                let from = self.motion.hover.filter(|s| !matches!(s, Spot::Inner(_)));
                let next = ring::flyout_step(&rows, from, delta);
                drop(rows);
                self.light_in_list(next);
            }
            VK_ESCAPE | VK_LEFT if self.wheel.list_only => {
                self.close(now);
            }
            VK_ESCAPE | VK_LEFT => {
                self.motion.close_flyout();
                self.motion.set_hover(&self.wheel, Some(more));
            }
            VK_RETURN | VK_SPACE => {
                if let Some(spot) = self.motion.hover {
                    self.choose(spot, now, true);
                }
            }
            _ => return false,
        }
        true
    }
}

impl Surface for WheelUi {
    type View<'a> = WheelView<'a>;
    type A11y<'a> = WheelSeen<'a>;

    fn hit(&self, _cx: &Ctx, x: f32, y: f32) -> Option<ControlId> {
        self.spot_at(x, y).map(ControlId::Wheel)
    }

    /// Point the open wheel at whatever is under `(x, y)`. The hub
    /// selects nothing, so a drop there converts nothing. It takes every
    /// move.
    fn hover(&mut self, cx: &mut Ctx, x: f32, y: f32) -> Reply {
        if self.is_open() {
            let spot = self.spot_at(x, y);
            if self.point(spot, cx.now) {
                cx.out.animate = true;
            }
        }
        Reply::Taken
    }

    /// A press while a wheel is up, which takes every press. Off the ring
    /// and the list, it closes the list if it is open, or else the wheel.
    fn press(&mut self, cx: &mut Ctx, x: f32, y: f32) -> Reply {
        if self.is_open() {
            let changed = match self.spot_at(x, y) {
                Some(spot) => self.choose(spot, cx.now, false),
                // A heading in the list is a label, not a way out.
                None if self.on_flyout(x, y) => false,
                None if self.flyout_open() && !self.wheel.list_only => {
                    self.motion.close_flyout();
                    true
                }
                None => self.close(cx.now),
            };
            cx.out.animate |= changed;
        }
        Reply::Taken
    }

    /// Nothing acts on release: a choice is made by the press.
    fn release(&mut self, _cx: &mut Ctx, _x: f32, _y: f32) -> Reply {
        Reply::Pass
    }

    /// A key while a wheel is up. A fading wheel no longer takes keys.
    fn key(&mut self, cx: &mut Ctx, key: VIRTUAL_KEY, mods: Mods) -> Reply {
        // F1 opens help from the list or a wheel.
        if key == VK_F1 {
            cx.out.follow = Some(Follow::Help);
            return Reply::Taken;
        }
        if !self.is_open() {
            return Reply::Pass;
        }
        // A pick converts at once: an Enter or Space held down from
        // whatever opened the wheel picks nothing until pressed again.
        if mods.repeat && matches!(key, VK_RETURN | VK_SPACE) {
            return Reply::Taken;
        }
        let now = cx.now;
        let list_keys = self.wheel.list_only || (key != VK_TAB && key != VK_RIGHT);
        if self.flyout_open() && list_keys {
            if self.list_key(key, now) {
                cx.out.animate = true;
            }
            return Reply::Taken;
        }
        let inner = self.wheel.segments();
        let delta = match key {
            VK_TAB if mods.shift => -1,
            VK_RIGHT | VK_DOWN | VK_TAB => 1,
            VK_LEFT | VK_UP => -1,
            _ => 0,
        };
        match (key, self.motion.hover) {
            // Right on More opens its list, as a submenu opens.
            (VK_RIGHT, Some(s)) if self.wheel.is_more_spot(s) => {
                self.open_more(now, true);
            }
            _ if self.flyout_open() => return Reply::Taken,
            (_, h) if delta != 0 => {
                let i = match h {
                    Some(Spot::Inner(i)) => Some(i),
                    _ => None,
                };
                let next = ring::step(i, delta, inner).map(Spot::Inner);
                self.point(next, now);
            }
            (VK_ESCAPE, _) => {
                self.close(now);
            }
            (VK_RETURN | VK_SPACE, Some(spot)) => {
                self.choose(spot, now, true);
            }
            _ => return Reply::Taken,
        }
        cx.out.animate = true;
        Reply::Taken
    }

    /// The mouse wheel scrolls More's list; nothing under a wheel scrolls.
    fn scroll(&mut self, _cx: &mut Ctx, by: TextScroll) -> Reply {
        if let (true, TextScroll::Wheel(notches)) = (self.flyout_open(), by) {
            let rows = self.rows();
            let max = self.place(&rows).max_scroll();
            drop(rows);
            self.scroll = (self.scroll - notches * SCROLL_NOTCH).clamp(0.0, max);
        }
        Reply::Block
    }

    /// The wheel doesn't hear the pointer leave: it keeps what is lit.
    fn leave(&mut self) {}

    fn view<'a>(
        &'a self,
        cx: &Ctx,
        _frame: Frame<'a>,
        _button: &'a dyn Fn(ButtonId) -> (f32, f32),
    ) -> WheelView<'a> {
        let (now, animations) = (cx.now, cx.animations);
        let m = &self.motion;
        let (open, grow) = m.opening(now, animations);
        let fade = self.closing.map_or(1.0, |t| {
            anim::wheel_close(anim::elapsed_ms(t, now), animations)
        });
        let flyout = self.flyout_open().then(|| {
            let rows = self.rows();
            let place = self.place(&rows);
            render::FlyoutView {
                rows,
                place,
                lit: m.hover,
                shown: m.flyout_shown(now, animations),
            }
        });
        WheelView {
            wheel: &self.wheel,
            hover: m.hover,
            hub: self.hub(),
            opacity: open * fade,
            grow,
            smoke: true,
            flyout,
            thumb: None,
            kind: self.wheel.source_kind,
        }
    }

    fn a11y<'a>(&'a self, _cx: &Ctx, _frame: Frame<'a>) -> WheelSeen<'a> {
        let flyout = self.flyout_open().then(|| {
            let rows = self.rows();
            let place = self.place(&rows);
            (rows, place)
        });
        WheelSeen {
            wheel: &self.wheel,
            paths: &self.paths,
            hover: self.motion.hover,
            hub: self.hub(),
            flyout,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Duration;

    use crate::wheel::{Action, Item, Origin};
    use kiln_core::{Kind, Tool};

    struct Lines;
    impl Measure for Lines {
        fn mono_height(&self, _text: &str, _width: f32) -> f32 {
            10.0
        }
        fn body_height(&self, _text: &str, _width: f32) -> f32 {
            10.0
        }
    }

    const SIZE: (f32, f32) = (1100.0, 720.0);

    fn item(i: usize) -> Item {
        Item {
            label: format!("F{i}"),
            is_code: true,
            action: Action::Convert(format!("f{i}")),
            kind: Some(Kind::Image),
        }
    }

    fn opening(items: usize, more: usize, keyboard: bool, hover: Option<Spot>) -> Opening {
        Opening {
            wheel: Wheel {
                kind: WheelKind::Convert,
                origin: Origin::Window,
                items: (0..items).map(item).collect(),
                flyout: ring::grouped((100..100 + more).map(item).collect()),
                center: (400.0, 300.0),
                scale: 1.0,
                source_code: "PNG".into(),
                source_kind: None,
                list_only: false,
                settable: (0..items)
                    .chain(100..100 + more)
                    .map(|i| format!("f{i}"))
                    .collect(),
            },
            paths: vec![PathBuf::from("a.png")],
            keyboard,
            suggestion: None,
            hover,
            bounds: Rect::new(0.0, 48.0, SIZE.0, SIZE.1 - 48.0),
            estimates: BTreeMap::from([("f1".to_owned(), 1_100_000)]),
        }
    }

    fn make(items: usize, more: usize, keyboard: bool, hover: Option<Spot>) -> WheelUi {
        WheelUi::new(opening(items, more, keyboard, hover), Instant::now())
    }

    fn open() -> WheelUi {
        make(4, 0, false, None)
    }

    fn with<R>(f: impl FnOnce(&mut Ctx) -> R) -> (R, Out) {
        let settings = Settings::default();
        let mut cx = Ctx::new(&Lines, &settings, SIZE);
        let r = f(&mut cx);
        (r, cx.out)
    }

    /// The middle of segment `i` of `n`.
    fn slice(i: usize, n: usize) -> (f32, f32) {
        let (x, y) = ring::label_point(i, n);
        (400.0 + x, 300.0 + y)
    }

    fn none() -> Mods {
        Mods::default()
    }

    /// The middle of More's list's line `i`.
    fn line(w: &WheelUi, i: usize) -> (f32, f32) {
        let rows = w.rows();
        let r = w.place(&rows).rows[i];
        (r.x + r.w / 2.0, r.y + r.h / 2.0)
    }

    #[test]
    fn it_opens_lit_on_the_spot_it_is_given_and_otherwise_on_nothing() {
        assert_eq!(open().lit(), None);
        let w = make(4, 0, true, Some(Spot::Inner(2)));
        assert_eq!(w.lit(), Some(Spot::Inner(2)));
        assert!(w.is_open());
    }

    #[test]
    fn the_pointer_lights_the_segment_under_it_and_a_move_takes_the_event() {
        let mut w = open();
        let (x, y) = slice(1, 4);
        let (reply, out) = with(|cx| {
            assert_eq!(w.hit(cx, x, y), Some(ControlId::Wheel(Spot::Inner(1))));
            w.hover(cx, x, y)
        });
        assert_eq!(reply, Reply::Taken);
        assert!(out.animate);
        assert_eq!(w.lit(), Some(Spot::Inner(1)));
        // The same segment again changes nothing, and still takes the move.
        let (reply, out) = with(|cx| w.hover(cx, x, y));
        assert_eq!(reply, Reply::Taken);
        assert!(!out.animate);
        // The hub selects nothing.
        let (_, out) = with(|cx| w.hover(cx, 400.0, 300.0));
        assert!(out.animate);
        assert_eq!(w.lit(), None);
    }

    #[test]
    fn a_press_on_a_format_converts_it_at_once_and_the_wheel_fades() {
        // no pulse and no options sheet in between.
        let mut w = open();
        let (x, y) = slice(2, 4);
        let (reply, out) = with(|cx| w.press(cx, x, y));
        assert_eq!(reply, Reply::Taken);
        assert!(out.animate);
        assert!(!w.is_open(), "fading, it takes no more");
        assert_eq!(w.lit(), Some(Spot::Inner(2)));
        let now = Instant::now();
        match w.tick(now, true) {
            Tick::Picked {
                picked,
                paths,
                keyboard,
            } => {
                assert_eq!(picked, Some(Picked::Run(Action::Convert("f2".into()))));
                assert_eq!(paths, [PathBuf::from("a.png")]);
                assert!(!keyboard);
            }
            _ => panic!("the pick comes back at the next tick"),
        }
        // It fades out on the exit curve, and then is gone.
        assert!(matches!(w.tick(now, true), Tick::Idle));
        assert!(matches!(
            w.tick(now + Duration::from_millis(200), true),
            Tick::Gone
        ));
    }

    #[test]
    fn a_press_off_the_wheel_closes_it() {
        let mut w = open();
        let (reply, out) = with(|cx| w.press(cx, 5.0, 60.0));
        assert_eq!(reply, Reply::Taken);
        assert!(out.animate && w.closing.is_some());
        // A fading wheel takes the press and does nothing.
        let (reply, out) = with(|cx| w.press(cx, 5.0, 60.0));
        assert_eq!(reply, Reply::Taken);
        assert!(!out.animate);
    }

    #[test]
    fn more_opens_a_list_beside_the_ring_whose_lines_are_picked() {
        let mut w = make(3, 3, false, None);
        let more = slice(3, 4);
        with(|cx| w.press(cx, more.0, more.1));
        assert!(
            w.flyout_open() && w.is_open(),
            "More does not close the wheel"
        );
        let rows = w.rows();
        let place = w.place(&rows);
        // Anchored beside the ring, on the side with room.
        assert_eq!(place.rect.x, 400.0 + ring::R_OUTER + ring::FLYOUT_GAP);
        assert!(matches!(rows[0], FlyoutRow::Heading("Photos")));
        drop(rows);
        // A heading is a label: a press on it leaves everything open.
        let head = line(&w, 0);
        let (_, out) = with(|cx| w.press(cx, head.0, head.1));
        assert!(w.flyout_open() && w.is_open() && !out.animate);
        // The pointer lights a line, and a press converts it.
        let at = line(&w, 2);
        with(|cx| w.hover(cx, at.0, at.1));
        assert_eq!(w.lit(), Some(Spot::Flyout(1)));
        with(|cx| w.press(cx, at.0, at.1));
        match w.tick(Instant::now(), true) {
            Tick::Picked { picked, .. } => {
                assert_eq!(picked, Some(Picked::Run(Action::Convert("f101".into()))));
            }
            _ => panic!("picked"),
        }
    }

    #[test]
    fn a_press_beside_the_open_list_closes_only_the_list() {
        let mut w = make(3, 3, false, None);
        w.choose(Spot::Inner(3), Instant::now(), false);
        let (_, out) = with(|cx| w.press(cx, 5.0, 60.0));
        assert!(!w.flyout_open() && w.is_open() && out.animate);
        with(|cx| w.press(cx, 5.0, 60.0));
        assert!(!w.is_open());
    }

    #[test]
    fn enter_on_more_opens_the_list_on_its_first_line_and_keys_move_through_it() {
        let mut w = make(3, 3, true, Some(Spot::Inner(2)));
        // The format last lit is what Options… is for.
        with(|cx| w.key(cx, VK_RIGHT, none()));
        assert_eq!(w.lit(), Some(Spot::Inner(3)));
        with(|cx| w.key(cx, VK_RETURN, none()));
        assert!(w.flyout_open());
        assert_eq!(w.lit(), Some(Spot::Flyout(0)));
        // Down past the formats to Options… for the one last lit.
        with(|cx| w.key(cx, VK_DOWN, none()));
        with(|cx| w.key(cx, VK_DOWN, none()));
        assert_eq!(w.lit(), Some(Spot::Flyout(2)));
        with(|cx| w.key(cx, VK_DOWN, none()));
        assert_eq!(w.lit(), Some(Spot::Options));
        assert!(matches!(w.rows().last(), Some(FlyoutRow::Options("f102"))));
        // Up wraps from the first line to the last.
        with(|cx| w.key(cx, VK_DOWN, none()));
        assert_eq!(w.lit(), Some(Spot::Flyout(0)));
        with(|cx| w.key(cx, VK_UP, none()));
        assert_eq!(w.lit(), Some(Spot::Options));
        // Esc closes the list and goes back to More; the next Esc closes.
        with(|cx| w.key(cx, VK_ESCAPE, none()));
        assert!(!w.flyout_open() && w.is_open());
        assert_eq!(w.lit(), Some(Spot::Inner(3)));
        with(|cx| w.key(cx, VK_ESCAPE, none()));
        assert!(!w.is_open());
    }

    #[test]
    fn options_in_the_list_opens_the_options_for_the_format_last_lit() {
        let mut w = make(3, 2, true, Some(Spot::Inner(1)));
        assert_eq!(
            w.options_for(Some(Spot::Options)).map(|o| o.0),
            Some("f1".to_owned())
        );
        w.choose(Spot::Inner(3), Instant::now(), true);
        // Down to Options…, then Enter.
        for _ in 0..2 {
            with(|cx| w.key(cx, VK_DOWN, none()));
        }
        assert_eq!(w.lit(), Some(Spot::Options));
        assert!(matches!(w.rows().last(), Some(FlyoutRow::Options("f101"))));
        with(|cx| w.key(cx, VK_RETURN, none()));
        match w.tick(Instant::now(), true) {
            Tick::Picked { picked, .. } => {
                assert_eq!(picked, Some(Picked::Options("f101".into())));
            }
            _ => panic!("picked"),
        }
    }

    #[test]
    fn arrows_step_round_the_ring() {
        let mut w = make(3, 2, true, None);
        let (reply, out) = with(|cx| w.key(cx, VK_RIGHT, none()));
        assert_eq!(reply, Reply::Taken);
        assert!(out.animate);
        assert_eq!(w.lit(), Some(Spot::Inner(0)));
        with(|cx| w.key(cx, VK_LEFT, none()));
        // Back round past the first: More, lit but not open.
        assert_eq!(w.lit(), Some(Spot::Inner(3)));
        assert!(!w.flyout_open());
        with(|cx| w.key(cx, VK_DOWN, none()));
        assert_eq!(w.lit(), Some(Spot::Inner(0)));
    }

    #[test]
    fn shift_tab_steps_back_round_the_ring() {
        let mut w = make(3, 2, true, Some(Spot::Inner(1)));
        let back = Mods::held(true, false);
        with(|cx| w.key(cx, VK_TAB, back));
        assert_eq!(w.lit(), Some(Spot::Inner(0)));
        with(|cx| w.key(cx, VK_TAB, none()));
        assert_eq!(w.lit(), Some(Spot::Inner(1)));
    }

    #[test]
    fn a_repeated_enter_or_space_chooses_nothing() {
        let mut w = make(4, 0, true, Some(Spot::Inner(1)));
        let held = Mods::with_repeat(false, false, true);
        for key in [VK_RETURN, VK_SPACE] {
            let (reply, _) = with(|cx| w.key(cx, key, held));
            assert_eq!(reply, Reply::Taken);
            assert!(w.is_open() && w.chosen.is_none());
        }
        with(|cx| w.key(cx, VK_RETURN, none()));
        assert!(w.chosen.is_some(), "a fresh press chooses");
    }

    #[test]
    fn enter_chooses_the_lit_segment_and_does_nothing_with_none() {
        let mut w = make(4, 0, true, None);
        with(|cx| w.key(cx, VK_RETURN, none()));
        assert!(w.is_open());
        let mut w = make(4, 0, true, Some(Spot::Inner(1)));
        with(|cx| w.key(cx, VK_SPACE, none()));
        assert!(!w.is_open() && w.chosen.is_some());
    }

    #[test]
    fn f1_asks_for_help_even_while_fading_and_other_keys_pass_then() {
        let mut w = open();
        w.close(Instant::now());
        let (reply, out) = with(|cx| w.key(cx, VK_F1, none()));
        assert_eq!(reply, Reply::Taken);
        assert_eq!(out.follow, Some(Follow::Help));
        let (reply, _) = with(|cx| w.key(cx, VK_RIGHT, none()));
        assert_eq!(reply, Reply::Pass);
    }

    #[test]
    fn another_key_is_taken_and_nothing_scrolls_under_it() {
        let mut w = open();
        let (reply, out) = with(|cx| w.key(cx, VK_F2, none()));
        assert_eq!(reply, Reply::Taken);
        assert!(!out.animate);
        let (reply, _) = with(|cx| w.scroll(cx, TextScroll::Wheel(1.0)));
        assert_eq!(reply, Reply::Block);
        let (reply, _) = with(|cx| w.release(cx, 1.0, 1.0));
        assert_eq!(reply, Reply::Pass);
    }

    #[test]
    fn a_long_list_scrolls_with_the_mouse_wheel_and_follows_the_keys() {
        let mut o = opening(3, 30, true, Some(Spot::Inner(3)));
        o.bounds = Rect::new(0.0, 48.0, 1100.0, 400.0);
        let mut w = WheelUi::new(o, Instant::now());
        with(|cx| w.key(cx, VK_RETURN, none()));
        assert_eq!(w.scroll, 0.0);
        with(|cx| w.scroll(cx, TextScroll::Wheel(-1.0)));
        assert_eq!(w.scroll, SCROLL_NOTCH);
        with(|cx| w.scroll(cx, TextScroll::Wheel(-100.0)));
        let rows = w.rows();
        let max = w.place(&rows).max_scroll();
        drop(rows);
        assert_eq!(w.scroll, max);
        // Up from the first line goes to the footer row, which never
        // scrolls, and once more to the last line, which is in view.
        with(|cx| w.scroll(cx, TextScroll::Wheel(100.0)));
        with(|cx| w.key(cx, VK_UP, none()));
        assert_eq!(w.lit(), Some(Spot::Options));
        assert_eq!(w.scroll, 0.0);
        with(|cx| w.key(cx, VK_UP, none()));
        assert_eq!(w.scroll, max);
    }

    #[test]
    fn options_are_a_format_and_not_more() {
        let w = make(3, 2, true, None);
        let (ext, paths, keyboard) = w.options_for(Some(Spot::Inner(1))).unwrap();
        assert_eq!((ext.as_str(), paths.len(), keyboard), ("f1", 1, true));
        assert!(w.options_for(Some(Spot::Inner(3))).is_none());
        assert!(w.options_for(None).is_none());
    }

    #[test]
    fn the_hub_reads_what_is_lit_with_the_files_estimate() {
        let mut w = make(3, 0, true, Some(Spot::Inner(1)));
        assert_eq!(w.hub().pair, "PNG \u{2192} F1");
        assert_eq!(w.hub().hint, "about 1.1 MB");
        w.motion.hover = None;
        assert_eq!(w.hub().hint, "Pick a format");
        // Several files: how many, never one file's estimate.
        w.paths.push(PathBuf::from("b.png"));
        w.motion.hover = Some(Spot::Inner(1));
        assert_eq!(w.hub().hint, "2 files");
    }

    #[test]
    fn make_smaller_runs_as_a_tool() {
        let mut o = opening(2, 0, true, Some(Spot::Inner(0)));
        o.wheel.items[0] = Item {
            label: "Make smaller".into(),
            is_code: false,
            action: Action::Tool(Tool::Smaller, Kind::Image),
            kind: None,
        };
        let mut w = WheelUi::new(o, Instant::now());
        assert_eq!(w.hub().pair, "Make smaller");
        assert_eq!(w.hub().hint, "Keeps PNG");
        with(|cx| w.key(cx, VK_RETURN, none()));
        match w.tick(Instant::now(), true) {
            Tick::Picked { picked, .. } => assert_eq!(
                picked,
                Some(Picked::Run(Action::Tool(Tool::Smaller, Kind::Image)))
            ),
            _ => panic!("picked"),
        }
    }

    #[test]
    fn a_suggestion_that_comes_late_marks_where_the_keyboard_starts_and_nothing_moves() {
        let mut w = open();
        let t = Instant::now();
        assert!(w.awaits(Path::new("a.png")) && !w.awaits(Path::new("z.png")));
        let s = Suggestion {
            action: kiln_core::Suggested::Convert("f1".into()),
            reason: "why".into(),
            estimate: None,
        };
        w.suggest(s);
        assert!(!w.awaits(Path::new("a.png")));
        assert_eq!(w.marks(), [Spot::Inner(1)]);
        // no dot to fade in, so nothing is moving once it has opened.
        let settled = t + Duration::from_millis(5000);
        assert!(!w.animating(settled, true));
    }

    #[test]
    fn the_options_row_falls_back_to_the_suggestion_then_the_first_format_with_settings() {
        // Of f0..f3 on the ring and f100, f101 in More, f1 and f101 have
        // nothing to set.
        let mut w = make(4, 2, false, None);
        w.wheel.settable = ["f0", "f2", "f3", "f100"].map(String::from).to_vec();
        let subject = |w: &WheelUi, lit| w.wheel.options_subject(lit).map(str::to_owned);
        // Nothing lit, and the format lit having nothing: the first.
        assert_eq!(subject(&w, None).as_deref(), Some("f0"));
        assert_eq!(subject(&w, Some(Spot::Inner(1))).as_deref(), Some("f0"));
        assert_eq!(subject(&w, Some(Spot::Flyout(1))).as_deref(), Some("f0"));
        // A lit format with settings: itself.
        assert_eq!(subject(&w, Some(Spot::Inner(2))).as_deref(), Some("f2"));
        assert_eq!(subject(&w, Some(Spot::Flyout(0))).as_deref(), Some("f100"));
        // The suggestion, once it arrives, is the fall-back; one with
        // nothing to set changes nothing.
        w.suggest(Suggestion {
            action: kiln_core::Suggested::Convert("f3".into()),
            reason: String::new(),
            estimate: None,
        });
        assert_eq!(subject(&w, None).as_deref(), Some("f3"));
        assert_eq!(subject(&w, Some(Spot::Inner(1))).as_deref(), Some("f3"));
        w.suggest(Suggestion {
            action: kiln_core::Suggested::Convert("f1".into()),
            reason: String::new(),
            estimate: None,
        });
        assert_eq!(subject(&w, None).as_deref(), Some("f3"));
        // Nothing can be picked from the row for a format with nothing to
        // set; the row is for one that has.
        assert_eq!(
            w.options_for(Some(Spot::Options)).map(|o| o.0),
            Some("f3".to_owned())
        );
    }

    #[test]
    fn restocking_drops_a_format_no_longer_offered_from_the_options_row() {
        let mut w = make(3, 1, false, None);
        w.wheel.settable = ["f1", "f0", "f101"].map(String::from).to_vec();
        // f1 goes: the row is for the next.
        let items = vec![item(0), item(2)];
        let flyout = ring::grouped(vec![item(100)]);
        w.restock(items, flyout);
        assert_eq!(w.wheel.settable, ["f0"]);
        assert_eq!(w.wheel.options_subject(None), Some("f0"));
    }

    #[test]
    fn it_is_animating_while_it_opens_or_fades() {
        let mut w = make(4, 0, true, Some(Spot::Inner(1)));
        let settled = Instant::now() + Duration::from_millis(5000);
        assert!(w.animating(Instant::now(), true));
        assert!(!w.animating(settled, true));
        w.close(settled);
        assert!(w.animating(settled, true));
    }

    #[test]
    fn what_ui_automation_reads_is_the_wheel_as_it_is() {
        let mut w = make(3, 2, false, Some(Spot::Inner(1)));
        let (seen, _) = with(|cx| {
            let s = w.a11y(cx, Frame::default());
            (
                s.hover,
                s.flyout.is_some(),
                s.paths.len(),
                s.wheel.segments(),
                s.hub.pair,
            )
        });
        assert_eq!(
            seen,
            (
                Some(Spot::Inner(1)),
                false,
                1,
                4,
                "PNG \u{2192} F1".to_owned()
            )
        );
        w.choose(Spot::Inner(3), Instant::now(), false);
        let (open, _) = with(|cx| w.a11y(cx, Frame::default()).flyout.map(|f| f.0.len()));
        assert_eq!(open, Some(w.rows().len()));
    }
}
