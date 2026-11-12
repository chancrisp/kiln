//! The details dialog: what happened to a failed job in plain words, then
//! what its engine said, with Copy and Close, as a ContentDialog over
//! smoke (mock screen 8). Close is its accent, and the
//! keyboard starts there.

use super::*;

/// The details dialog while it's open.
pub(in crate::win) struct Details {
    /// "What happened".
    title: String,
    /// The plain line over the card: the row's sentence.
    lead: String,
    /// The engine's own words, in the card; empty when it had none.
    text: String,
    /// How far the text is scrolled, in DIPs from its top.
    scroll: f32,
    /// The button with keyboard focus. It starts on Close, the accent.
    focus: DialogButton,
    /// The button the pointer is over, and the one it went down on; that
    /// one acts on release.
    pointer: Pointer,
    /// Whether the last copy worked and when it was, for the button's
    /// "Copied" or "Couldn't copy".
    last_copy: Option<(bool, Instant)>,
}

/// The dialog's button a control is.
fn button(c: ControlId) -> Option<DialogButton> {
    match c {
        ControlId::Dialog(b) => Some(b),
        _ => None,
    }
}

impl Details {
    /// A dialog with `title` over its plain `lead` line and the engine's
    /// `text`, focus on Close.
    pub fn new(title: String, lead: String, text: String) -> Self {
        Details {
            title,
            lead,
            text,
            scroll: 0.0,
            focus: DialogButton::Close,
            pointer: Pointer::default(),
            last_copy: None,
        }
    }

    /// The buttons the pointer is over and has pressed, for how they ease.
    pub fn buttons(&self) -> (Option<DialogButton>, Option<DialogButton>) {
        (
            self.pointer.hover.and_then(button),
            self.pointer.pressed.and_then(button),
        )
    }

    /// Press button `b`, from a click, a key or UI Automation: Copy puts
    /// all of the text on the clipboard and says "Copied", or "Couldn't
    /// copy", for a moment; Close closes.
    pub fn act(&mut self, cx: &mut Ctx, b: DialogButton) {
        match b {
            DialogButton::Close => cx.out.close = true,
            DialogButton::Copy => {
                let all = if self.text.is_empty() {
                    self.lead.clone()
                } else {
                    format!(
                        "{}

{}",
                        self.lead, self.text
                    )
                };
                let text = model::clipboard_lines(&all);
                // The button itself says how it went, in front of the
                // scrim, without Windows' own error text.
                cx.out.effects.push(Effect::Clipboard {
                    text,
                    then: Box::new(|app, copied| {
                        if let Some(d) = &mut app.details {
                            d.last_copy = Some((copied.is_ok(), Instant::now()));
                        }
                        let ms = model::COPIED_FOR.as_millis() as u32 + 20;
                        // SAFETY: plain call on this thread's own window;
                        // it sends nothing.
                        unsafe {
                            SetTimer(Some(app.hwnd), COPIED_TIMER, ms, None);
                        }
                        app.invalidate();
                    }),
                });
            }
        }
        cx.out.invalidate = true;
    }

    /// Where it is in a window `cx.size` big: a ContentDialog, its text
    /// in a card.
    pub fn layout(&self, cx: &Ctx) -> layout::kit::DetailsDialog {
        let (w, h) = cx.size;
        let wide = layout::kit::details_lead_width(w);
        let inner = layout::kit::details_text_width(w);
        let (title_h, _) = cx.measure.dialog_heights(&self.title, None, wide);
        let lead_h = cx.measure.body_height(&self.lead, wide);
        let text_h = if self.text.is_empty() {
            0.0
        } else {
            cx.measure.mono_height(&self.text, inner)
        };
        layout::kit::details_dialog((w, h), title_h, lead_h, text_h)
    }

    /// Move the keyboard focus to `b`.
    pub fn focus(&mut self, cx: &mut Ctx, b: DialogButton) {
        self.focus = b;
        cx.out.invalidate = true;
    }
}

impl Surface for Details {
    type View<'a> = DetailsView<'a>;
    type A11y<'a> = a11y::DetailsIn<'a>;

    fn hit(&self, cx: &Ctx, x: f32, y: f32) -> Option<ControlId> {
        let l = self.layout(cx);
        [DialogButton::Copy, DialogButton::Close]
            .into_iter()
            .zip(&l.frame.buttons)
            .find_map(|(b, r)| r.contains(x, y).then_some(ControlId::Dialog(b)))
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

    /// Escape closes it. Tab and the left and right arrows move focus
    /// between Copy and Close, and Enter and Space press the focused one,
    /// which starts as Close.
    /// Ctrl+C copies. The other arrows, Page Up, Page Down, Home and End
    /// scroll the text. Every other key stays with it.
    fn key(&mut self, cx: &mut Ctx, key: VIRTUAL_KEY, mods: Mods) -> Reply {
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
            VK_C if mods.ctrl => {
                self.act(cx, DialogButton::Copy);
                return Reply::Taken;
            }
            VK_SPACE | VK_RETURN => {
                self.act(cx, self.focus);
                return Reply::Taken;
            }
            VK_TAB | VK_LEFT | VK_RIGHT => {
                self.focus = match self.focus {
                    DialogButton::Copy => DialogButton::Close,
                    DialogButton::Close => DialogButton::Copy,
                };
            }
            _ => return Reply::Block,
        }
        cx.out.invalidate = true;
        Reply::Taken
    }

    /// Scroll the text, kept inside the text.
    fn scroll(&mut self, cx: &mut Ctx, by: TextScroll) -> Reply {
        let body = self.layout(cx).text;
        let content = cx.measure.mono_height(&self.text, body.w);
        let to = layout::scroll_text(by, self.scroll, content, body.h);
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
    ) -> DetailsView<'a> {
        DetailsView {
            layout: self.layout(cx),
            title: &self.title,
            lead: &self.lead,
            text: &self.text,
            scroll: self.scroll,
            copy_label: model::copy_label(self.last_copy.map(|(ok, t)| (ok, t.elapsed()))),
            focus: self.focus,
            button,
        }
    }

    fn a11y<'a>(&'a self, cx: &Ctx, _frame: Frame<'a>) -> a11y::DetailsIn<'a> {
        a11y::DetailsIn {
            layout: self.layout(cx),
            title: &self.title,
            lead: &self.lead,
            text: &self.text,
            copy: model::copy_label(self.last_copy.map(|(ok, t)| (ok, t.elapsed()))),
            focus: self.focus,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One line of text per 10 units, whatever the width.
    struct Lines;
    impl Measure for Lines {
        fn mono_height(&self, text: &str, _width: f32) -> f32 {
            text.lines().count() as f32 * 10.0
        }
        fn body_height(&self, text: &str, _width: f32) -> f32 {
            text.lines().count() as f32 * 10.0
        }
    }

    const SIZE: (f32, f32) = (800.0, 600.0);

    fn dialog(lines: usize) -> Details {
        let text = (0..lines)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        Details::new(
            "What happened".to_owned(),
            "Kiln couldn't read a.pdf.".to_owned(),
            text,
        )
    }

    fn with<R>(f: impl FnOnce(&mut Ctx) -> R) -> (R, Out) {
        let settings = Settings::default();
        let mut cx = Ctx::new(&Lines, &settings, SIZE);
        let r = f(&mut cx);
        (r, cx.out)
    }

    fn center(r: layout::Rect) -> (f32, f32) {
        (r.x + r.w / 2.0, r.y + r.h / 2.0)
    }

    /// Where the dialog's buttons are, Copy then Close, as `d` lays out.
    fn buttons_of(d: &Details) -> (layout::Rect, layout::Rect) {
        let (l, _) = with(|cx| d.layout(cx));
        (l.frame.buttons[0], l.frame.buttons[1])
    }

    #[test]
    fn it_opens_on_close_the_accent_with_nothing_lit() {
        // A ContentDialog's focus starts on its accent button: Close (it
        // started on Copy before phase 3).
        let d = dialog(3);
        assert_eq!(d.focus, DialogButton::Close);
        assert_eq!(d.buttons(), (None, None));
        let (v, _) = with(|cx| {
            d.view(cx, Frame::default(), &|_| (0.0, 0.0))
                .title
                .to_owned()
        });
        assert_eq!(v, "What happened");
    }

    #[test]
    fn a_press_and_release_on_close_closes_it() {
        let mut d = dialog(3);
        let (x, y) = center(buttons_of(&d).1);
        let (reply, out) = with(|cx| {
            assert_eq!(
                d.hit(cx, x, y),
                Some(ControlId::Dialog(DialogButton::Close))
            );
            d.press(cx, x, y)
        });
        assert_eq!(reply, Reply::Taken);
        assert_eq!(d.focus, DialogButton::Close);
        assert_eq!(d.buttons().1, Some(DialogButton::Close));
        assert!(matches!(out.effects[..], [Effect::Capture]));
        assert!(!out.close);
        let (reply, out) = with(|cx| d.release(cx, x, y));
        assert_eq!(reply, Reply::Taken);
        assert!(out.close);
        assert_eq!(d.buttons().1, None);
    }

    #[test]
    fn sliding_off_before_release_takes_the_press_back() {
        let mut d = dialog(3);
        let (x, y) = center(buttons_of(&d).1);
        with(|cx| d.press(cx, x, y));
        let (_, out) = with(|cx| d.release(cx, 1.0, 1.0));
        assert!(!out.close && out.effects.is_empty());
    }

    #[test]
    fn a_release_with_nothing_pressed_passes() {
        let mut d = dialog(3);
        let (reply, out) = with(|cx| d.release(cx, 5.0, 5.0));
        assert_eq!(reply, Reply::Pass);
        assert!(!out.invalidate);
    }

    #[test]
    fn hover_follows_the_pointer_and_leaving_clears_it() {
        let mut d = dialog(3);
        let (x, y) = center(buttons_of(&d).0);
        let (_, out) = with(|cx| d.hover(cx, x, y));
        assert!(out.invalidate);
        assert_eq!(d.buttons().0, Some(DialogButton::Copy));
        let (_, out) = with(|cx| d.hover(cx, x, y));
        assert!(!out.invalidate);
        d.leave();
        assert_eq!(d.buttons(), (None, None));
    }

    #[test]
    fn escape_and_enter_close_and_tab_moves_between_buttons() {
        let none = Mods::default();
        for key in [VK_ESCAPE, VK_RETURN] {
            let mut d = dialog(3);
            let (reply, out) = with(|cx| d.key(cx, key, none));
            assert_eq!(reply, Reply::Taken);
            assert!(out.close);
        }
        let mut d = dialog(3);
        for (key, to) in [
            (VK_TAB, DialogButton::Copy),
            (VK_TAB, DialogButton::Close),
            (VK_LEFT, DialogButton::Copy),
            (VK_RIGHT, DialogButton::Close),
        ] {
            let (reply, _) = with(|cx| d.key(cx, key, none));
            assert_eq!(reply, Reply::Taken);
            assert_eq!(d.focus, to);
        }
        // Space presses the focused button: Close.
        let (_, out) = with(|cx| d.key(cx, VK_SPACE, none));
        assert!(out.close);
        // Enter presses the focused button too: on Copy, it copies and
        // the dialog stays (Enter closed it from any button before).
        let mut d = dialog(3);
        with(|cx| d.key(cx, VK_TAB, none));
        let (_, out) = with(|cx| d.key(cx, VK_RETURN, none));
        assert!(!out.close);
        assert!(matches!(out.effects[..], [Effect::Clipboard { .. }]));
        // Esc closes from anywhere.
        let (_, out) = with(|cx| d.key(cx, VK_ESCAPE, none));
        assert!(out.close);
    }

    #[test]
    fn the_text_sits_in_the_dialogs_card_and_scrolls_there() {
        let d = dialog(3);
        let (l, _) = with(|cx| d.layout(cx));
        let c = l.frame.content;
        assert!(l.text.x > c.x && l.text.bottom() < c.bottom());
        // The plain line, one line of 10 units, sits above the card.
        assert_eq!(l.lead.h, 10.0);
        assert!(l.lead.bottom() < l.card.y && l.card.y < l.text.y);
        // Three lines of 10 units: the card holds them and its padding.
        assert_eq!(l.text.h, 30.0);
        let tall = dialog(500);
        let (l, _) = with(|cx| tall.layout(cx));
        assert!(l.text.h < 5000.0);
    }

    #[test]
    fn ctrl_c_copies_and_other_keys_are_blocked() {
        let mut d = dialog(3);
        let ctrl = Mods {
            ctrl: true,
            ..Mods::default()
        };
        let (reply, out) = with(|cx| d.key(cx, VK_C, ctrl));
        assert_eq!(reply, Reply::Taken);
        assert!(matches!(out.effects[..], [Effect::Clipboard { .. }]));
        assert!(!out.close);
        let (reply, out) = with(|cx| d.key(cx, VK_C, Mods::default()));
        assert_eq!(reply, Reply::Block);
        assert!(!out.invalidate);
    }

    #[test]
    fn the_keys_and_the_wheel_scroll_the_text() {
        // 100 lines of 10 units is far taller than the body.
        let mut d = dialog(100);
        let none = Mods::default();
        with(|cx| d.key(cx, VK_END, none));
        let end = d.scroll;
        assert!(end > 0.0);
        with(|cx| d.key(cx, VK_HOME, none));
        assert_eq!(d.scroll, 0.0);
        let (reply, out) = with(|cx| d.key(cx, VK_NEXT, none));
        assert_eq!(reply, Reply::Taken);
        assert!(out.invalidate && d.scroll > 0.0);
        with(|cx| d.key(cx, VK_PRIOR, none));
        assert_eq!(d.scroll, 0.0);
        let (_, out) = with(|cx| d.scroll(cx, TextScroll::Wheel(-1.0)));
        assert!(out.invalidate && d.scroll > 0.0);
        // Already at the top, scrolling up changes nothing.
        with(|cx| d.key(cx, VK_HOME, none));
        let (_, out) = with(|cx| d.scroll(cx, TextScroll::Wheel(1.0)));
        assert!(!out.invalidate);
    }
}
