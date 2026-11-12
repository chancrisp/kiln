//! Help as a ContentDialog over smoke, as the mock has it: Convert files,
//! Shortcuts, Show first-run help, then Close at the right of the band.
//! Closing help isn't a primary action, so Close is a standard button; the
//! keyboard starts there.

use super::*;

use crate::help::{link_place, place};
use crate::ways_in::SHOW_FIRST_RUN;

/// Help while it is open. Keys go to it, and its Close has the focus
/// until Tab moves it to Show first-run help, until it closes.
pub(in crate::win) struct Help {
    sections: Vec<HelpSection>,
    /// How far the content is scrolled, in DIPs from its top.
    scroll: f32,
    /// The button the pointer is over, and the one it went down on; that
    /// one acts on release.
    pointer: Pointer,
    /// The button with the keyboard focus.
    focus: HelpButton,
    /// When it opened: it rises into place (`anim::SHEET_MS`).
    opened: Instant,
}

/// Help laid out in a window: the dialog, how tall its content runs, and
/// Show first-run help's place in the content before scrolling.
pub(in crate::win) struct HelpLaid {
    pub frame: layout::kit::DialogLayout,
    pub content_h: f32,
    pub link: layout::Rect,
}

impl HelpLaid {
    /// Show first-run help where it is in the window, scrolled `scroll`.
    pub fn link_at(&self, scroll: f32) -> layout::Rect {
        let body = self.frame.content;
        layout::Rect::new(
            body.x + self.link.x,
            body.y + self.link.y - scroll,
            self.link.w,
            self.link.h,
        )
    }
}

/// The help button a control is.
fn button(c: ControlId) -> Option<HelpButton> {
    match c {
        ControlId::Help(b) => Some(b),
        _ => None,
    }
}

impl Help {
    /// Help showing `sections`, opened at `opened`, with focus on Close.
    pub fn new(sections: Vec<HelpSection>, opened: Instant) -> Self {
        Help {
            sections,
            scroll: 0.0,
            pointer: Pointer::default(),
            focus: HelpButton::Close,
            opened,
        }
    }

    /// The buttons the pointer is over and has pressed, for how they ease.
    pub fn buttons(&self) -> (Option<HelpButton>, Option<HelpButton>) {
        (
            self.pointer.hover.and_then(button),
            self.pointer.pressed.and_then(button),
        )
    }

    /// When it opened, for its rise.
    pub fn opened(&self) -> Instant {
        self.opened
    }

    /// Where it is in a window `cx.size` big.
    pub fn laid(&self, cx: &Ctx) -> HelpLaid {
        let (w, h) = cx.size;
        let inner = layout::kit::help_content_width(w);
        let (title_h, _) = cx.measure.dialog_heights(crate::help::TITLE, None, inner);
        let link_w = cx.measure.fit_width(SHOW_FIRST_RUN, false).ceil();
        let (placed, content_h) = place(&self.sections, inner, link_w, |s, width| {
            cx.measure.body_height(s, width)
        });
        HelpLaid {
            frame: layout::kit::help_dialog((w, h), title_h, content_h),
            content_h,
            link: link_place(&placed).unwrap_or_default(),
        }
    }

    /// Press button `b`, from a click, a key or UI Automation: Close
    /// closes it; Show first-run help closes it and brings the first-run
    /// card back to the empty window.
    pub fn act(&mut self, cx: &mut Ctx, b: HelpButton) {
        cx.out.close = true;
        if b == HelpButton::Guide {
            cx.out.follow = Some(Follow::ShowFirstRun);
        }
        cx.out.invalidate = true;
    }

    /// Move the keyboard focus to `b`, scrolling Show first-run help into
    /// view when it takes the focus.
    pub fn focus(&mut self, cx: &mut Ctx, b: HelpButton) {
        self.focus = b;
        if b == HelpButton::Guide {
            let l = self.laid(cx);
            let body = l.frame.content;
            let max = (l.content_h - body.h).max(0.0);
            let shown = l.link.bottom() + crate::help::LINK_RING - body.h;
            self.scroll = self.scroll.max(shown).min(max).max(0.0);
        }
        cx.out.invalidate = true;
    }
}

impl Surface for Help {
    type View<'a> = HelpView<'a>;
    type A11y<'a> = a11y::HelpIn<'a>;

    fn hit(&self, cx: &Ctx, x: f32, y: f32) -> Option<ControlId> {
        let l = self.laid(cx);
        if l.frame.buttons.first().is_some_and(|r| r.contains(x, y)) {
            return Some(ControlId::Help(HelpButton::Close));
        }
        // The link only where it shows inside the content.
        (l.link_at(self.scroll).contains(x, y) && l.frame.content.contains(x, y))
            .then_some(ControlId::Help(HelpButton::Guide))
    }

    fn hover(&mut self, cx: &mut Ctx, x: f32, y: f32) -> Reply {
        let hot = self.hit(cx, x, y);
        if self.pointer.hover != hot {
            self.pointer.hover = hot;
            cx.out.invalidate = true;
        }
        Reply::Taken
    }

    fn press(&mut self, cx: &mut Ctx, x: f32, y: f32) -> Reply {
        if let Some(c) = self.hit(cx, x, y) {
            self.pointer.pressed = Some(c);
            self.focus = button(c).unwrap_or(self.focus);
            cx.out.effects.push(Effect::Capture);
        } else if !self.laid(cx).frame.rect.contains(x, y) {
            // A click on the smoke closes help, as in the mock.
            cx.out.close = true;
        }
        cx.out.invalidate = true;
        Reply::Taken
    }

    fn release(&mut self, cx: &mut Ctx, x: f32, y: f32) -> Reply {
        let Some(c) = self.pointer.pressed.take() else {
            return Reply::Pass;
        };
        if self.hit(cx, x, y) == Some(c) {
            if let Some(b) = button(c) {
                self.act(cx, b);
            }
        }
        cx.out.invalidate = true;
        Reply::Taken
    }

    /// Escape closes it, and Enter and Space press the focused button.
    /// The up and down arrows, Page Up, Page Down, Home and End scroll
    /// it; Tab, either way, and the left and right arrows go between its
    /// two buttons. Every other key stays with it.
    fn key(&mut self, cx: &mut Ctx, key: VIRTUAL_KEY, _mods: Mods) -> Reply {
        let scroll = match key {
            VK_UP => Some(TextScroll::LineUp),
            VK_DOWN => Some(TextScroll::LineDown),
            VK_PRIOR => Some(TextScroll::PageUp),
            VK_NEXT => Some(TextScroll::PageDown),
            VK_HOME => Some(TextScroll::Home),
            VK_END => Some(TextScroll::End),
            _ => None,
        };
        if let Some(by) = scroll {
            return self.scroll(cx, by);
        }
        match key {
            VK_ESCAPE => cx.out.close = true,
            VK_RETURN | VK_SPACE => {
                self.act(cx, self.focus);
                return Reply::Taken;
            }
            VK_TAB | VK_LEFT | VK_RIGHT => {
                let to = match self.focus {
                    HelpButton::Guide => HelpButton::Close,
                    HelpButton::Close => HelpButton::Guide,
                };
                self.focus(cx, to);
            }
            _ => return Reply::Block,
        }
        cx.out.invalidate = true;
        Reply::Taken
    }

    /// Scroll the content, kept inside it.
    fn scroll(&mut self, cx: &mut Ctx, by: TextScroll) -> Reply {
        let l = self.laid(cx);
        let to = layout::scroll_text(by, self.scroll, l.content_h, l.frame.content.h);
        if self.scroll != to {
            self.scroll = to;
            cx.out.invalidate = true;
        }
        Reply::Taken
    }

    fn leave(&mut self) {
        self.pointer.clear();
    }

    fn view<'a>(
        &'a self,
        cx: &Ctx,
        _frame: Frame<'a>,
        button: &'a dyn Fn(ButtonId) -> (f32, f32),
    ) -> HelpView<'a> {
        let (open, rise) = sheet_rise(self.opened, cx.now, cx.animations);
        let l = self.laid(cx);
        HelpView {
            scroll: layout::clamp_text_scroll(self.scroll, l.content_h, l.frame.content.h),
            frame: l.frame,
            link_w: l.link.w,
            sections: &self.sections,
            button,
            focus: self.focus,
            open,
            rise,
        }
    }

    fn a11y<'a>(&'a self, cx: &Ctx, _frame: Frame<'a>) -> a11y::HelpIn<'a> {
        let l = self.laid(cx);
        let body = l.frame.content;
        let places = crate::help::line_places(&self.sections, body.w, |s, width| {
            cx.measure.body_height(s, width)
        })
        .into_iter()
        .map(|(section, line, r)| {
            let rect = layout::Rect::new(body.x + r.x, body.y + r.y - self.scroll, r.w, r.h);
            a11y::HelpPlace {
                section,
                line,
                rect,
                offscreen: a11y::clipped_out(&rect, &body),
            }
        })
        .collect();
        let link = l.link_at(self.scroll);
        a11y::HelpIn {
            sections: &self.sections,
            places,
            frame: l.frame.rect,
            body,
            guide: link,
            guide_offscreen: a11y::clipped_out(&link, &body),
            close: l.frame.buttons.first().copied().unwrap_or_default(),
            focus: self.focus,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One line of text per 40 units, whatever the width.
    struct Lines;
    impl Measure for Lines {
        fn mono_height(&self, text: &str, _width: f32) -> f32 {
            text.lines().count() as f32 * 10.0
        }
        fn body_height(&self, text: &str, _width: f32) -> f32 {
            text.lines().count().max(1) as f32 * 40.0
        }
    }

    /// Tall enough for all of it.
    const SIZE: (f32, f32) = (800.0, 1000.0);
    /// A window too short for all of it: it scrolls.
    const SHORT: (f32, f32) = (800.0, 360.0);

    fn help_open() -> Help {
        Help::new(
            crate::help::help_content(crate::help::Results::Beside),
            Instant::now(),
        )
    }

    fn with_size<R>(size: (f32, f32), f: impl FnOnce(&mut Ctx) -> R) -> (R, Out) {
        let settings = Settings::default();
        let mut cx = Ctx::new(&Lines, &settings, size);
        let r = f(&mut cx);
        (r, cx.out)
    }

    fn with<R>(f: impl FnOnce(&mut Ctx) -> R) -> (R, Out) {
        with_size(SIZE, f)
    }

    fn laid(h: &Help, size: (f32, f32)) -> HelpLaid {
        with_size(size, |cx| h.laid(cx)).0
    }

    fn center(r: layout::Rect) -> (f32, f32) {
        (r.x + r.w / 2.0, r.y + r.h / 2.0)
    }

    #[test]
    fn it_opens_as_a_dialog_of_the_mocks_width_on_close_with_nothing_lit() {
        let h = help_open();
        assert_eq!(h.focus, HelpButton::Close);
        assert_eq!(h.buttons(), (None, None));
        let l = laid(&h, SIZE);
        assert_eq!(l.frame.rect.w, layout::kit::HELP_WIDTH);
        assert_eq!(l.frame.buttons.len(), 1);
        // Show first-run help sits in the content, under the sections.
        assert!(l.link.y > 0.0 && l.link.w > 0.0);
        // The scrolling body runs on past it, as far as its focus ring.
        let ring = layout::kit::focus_rings(l.link, crate::theme::fluent::radius::CONTROL)[0].0;
        assert_eq!(l.content_h, ring.bottom());
    }

    #[test]
    fn a_press_and_release_on_close_closes_it() {
        let mut h = help_open();
        let (x, y) = center(laid(&h, SIZE).frame.buttons[0]);
        let (reply, out) = with(|cx| {
            assert_eq!(h.hit(cx, x, y), Some(ControlId::Help(HelpButton::Close)));
            h.press(cx, x, y)
        });
        assert_eq!(reply, Reply::Taken);
        assert_eq!(h.buttons().1, Some(HelpButton::Close));
        assert!(matches!(out.effects[..], [Effect::Capture]));
        assert!(!out.close);
        let (reply, out) = with(|cx| h.release(cx, x, y));
        assert_eq!(reply, Reply::Taken);
        assert!(out.close && out.follow.is_none());
        assert_eq!(h.buttons().1, None);
    }

    #[test]
    fn show_first_run_help_closes_it_and_brings_the_card_back() {
        let mut h = help_open();
        let (x, y) = center(laid(&h, SIZE).link_at(0.0));
        with(|cx| h.press(cx, x, y));
        assert_eq!(h.focus, HelpButton::Guide);
        let (_, out) = with(|cx| h.release(cx, x, y));
        assert!(out.close);
        assert!(matches!(out.follow, Some(Follow::ShowFirstRun)));
    }

    #[test]
    fn a_click_on_the_smoke_closes_it_and_inside_the_dialog_does_not() {
        let mut h = help_open();
        let (reply, out) = with(|cx| h.press(cx, 1.0, 1.0));
        assert_eq!(reply, Reply::Taken);
        assert!(out.close && out.effects.is_empty());
        let r = laid(&h, SIZE).frame.rect;
        let (_, out) = with(|cx| h.press(cx, r.x + 2.0, r.y + 2.0));
        assert!(!out.close);
    }

    #[test]
    fn sliding_off_before_release_takes_the_press_back() {
        let mut h = help_open();
        let (x, y) = center(laid(&h, SIZE).frame.buttons[0]);
        with(|cx| h.press(cx, x, y));
        let (_, out) = with(|cx| h.release(cx, 1.0, 1.0));
        assert!(!out.close);
        let (reply, _) = with(|cx| h.release(cx, 1.0, 1.0));
        assert_eq!(reply, Reply::Pass);
    }

    #[test]
    fn hover_follows_the_pointer_and_leaving_clears_it() {
        let mut h = help_open();
        let (x, y) = center(laid(&h, SIZE).link_at(0.0));
        let (_, out) = with(|cx| h.hover(cx, x, y));
        assert!(out.invalidate);
        assert_eq!(h.buttons().0, Some(HelpButton::Guide));
        h.leave();
        assert_eq!(h.buttons(), (None, None));
    }

    #[test]
    fn escape_closes_and_tab_moves_between_its_two_buttons() {
        let none = Mods::default();
        let mut h = help_open();
        let (reply, out) = with(|cx| h.key(cx, VK_ESCAPE, none));
        assert_eq!(reply, Reply::Taken);
        assert!(out.close);
        for (key, to) in [
            (VK_TAB, HelpButton::Guide),
            (VK_LEFT, HelpButton::Close),
            (VK_RIGHT, HelpButton::Guide),
        ] {
            let (reply, _) = with(|cx| h.key(cx, key, none));
            assert_eq!(reply, Reply::Taken);
            assert_eq!(h.focus, to);
        }
        // Enter presses the focused button: Show first-run help.
        let (_, out) = with(|cx| h.key(cx, VK_RETURN, none));
        assert!(out.close && matches!(out.follow, Some(Follow::ShowFirstRun)));
        // Another key is swallowed.
        let (reply, out) = with(|cx| h.key(cx, VK_F1, none));
        assert_eq!(reply, Reply::Block);
        assert!(!out.invalidate && !out.close);
    }

    #[test]
    fn in_a_short_window_the_content_scrolls_and_the_link_scrolls_with_it() {
        let mut h = help_open();
        let none = Mods::default();
        let l = laid(&h, SHORT);
        assert!(l.content_h > l.frame.content.h, "it must scroll here");
        // Scrolled out of the content, the link can't be pressed.
        let (x, y) = center(l.link_at(0.0));
        let (hit, _) = with_size(SHORT, |cx| h.hit(cx, x, y));
        assert_eq!(hit, None);
        let (reply, out) = with_size(SHORT, |cx| h.key(cx, VK_NEXT, none));
        assert_eq!(reply, Reply::Taken);
        assert!(out.invalidate && h.scroll > 0.0);
        let page = h.scroll;
        with_size(SHORT, |cx| h.key(cx, VK_END, none));
        let end = h.scroll;
        assert!(end >= page);
        assert_eq!(end, l.content_h - l.frame.content.h);
        // At the end, another page down changes nothing.
        let (_, out) = with_size(SHORT, |cx| h.key(cx, VK_NEXT, none));
        assert!(!out.invalidate && h.scroll == end);
        // Now it shows, and the pointer finds it.
        let (x, y) = center(l.link_at(end));
        let (hit, _) = with_size(SHORT, |cx| h.hit(cx, x, y));
        assert_eq!(hit, Some(ControlId::Help(HelpButton::Guide)));
        with_size(SHORT, |cx| h.key(cx, VK_HOME, none));
        assert_eq!(h.scroll, 0.0);
        let (_, out) = with_size(SHORT, |cx| h.scroll(cx, TextScroll::Wheel(-1.0)));
        assert!(out.invalidate && h.scroll > 0.0);
    }

    #[test]
    fn tab_to_the_link_scrolls_it_into_view() {
        let mut h = help_open();
        let l = laid(&h, SHORT);
        with_size(SHORT, |cx| h.key(cx, VK_TAB, Mods::default()));
        assert_eq!(h.focus, HelpButton::Guide);
        let link = l.link_at(h.scroll);
        let body = l.frame.content;
        assert!(link.y >= body.y && link.bottom() <= body.bottom() + 0.5);
    }

    #[test]
    fn the_last_link_scrolls_fully_into_view_with_its_focus_ring() {
        let mut h = help_open();
        let l = laid(&h, SHORT);
        assert!(l.content_h > l.frame.content.h, "it must scroll here");
        with_size(SHORT, |cx| h.key(cx, VK_TAB, Mods::default()));
        assert_eq!(h.focus, HelpButton::Guide);
        let body = l.frame.content;
        let ring =
            layout::kit::focus_rings(l.link_at(h.scroll), crate::theme::fluent::radius::CONTROL)[0]
                .0;
        assert!(ring.y >= body.y - 0.5, "{ring:?} in {body:?}");
        assert!(ring.bottom() <= body.bottom() + 0.5, "{ring:?} in {body:?}");
        // The end of the body shows it just as fully.
        let end = (l.content_h - body.h).max(0.0);
        let ring =
            layout::kit::focus_rings(l.link_at(end), crate::theme::fluent::radius::CONTROL)[0].0;
        assert!(ring.bottom() <= body.bottom() + 0.5, "{ring:?} in {body:?}");
    }

    #[test]
    fn its_lines_and_buttons_are_placed_for_ui_automation() {
        let mut h = help_open();
        let (a, _) = with_size(SHORT, |cx| {
            let a = h.a11y(cx, Frame::default());
            (a.places.clone(), a.guide_offscreen, a.close, a.frame)
        });
        let (places, guide_off, close, frame) = a;
        assert!(!places.is_empty());
        assert!(guide_off, "the link is below the content at first");
        assert!(close.w > 0.0 && frame.contains(close.x + 1.0, close.y + 1.0));
        with_size(SHORT, |cx| h.key(cx, VK_END, Mods::default()));
        let (after, _) = with_size(SHORT, |cx| {
            let a = h.a11y(cx, Frame::default());
            assert!(!a.guide_offscreen);
            a.places.clone()
        });
        assert_eq!(places.len(), after.len());
        assert!(after[0].rect.y < places[0].rect.y);
        assert!(after[0].offscreen);
    }
}
