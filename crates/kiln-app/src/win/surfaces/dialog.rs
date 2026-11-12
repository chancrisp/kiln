//! Kiln's prompts as a ContentDialog inside the window: the leave prompt,
//! the offer of what last time left and the big-folder question. Each is
//! an `Effect::Ask`, answered here instead of in a task dialog
//! (`system::ask` stays only for a window that can't draw).
//!
//! The dialog is the top layer and modal: it takes every key and press,
//! and nothing under it hears them. Focus starts on the accent button;
//! Tab and Shift+Tab go round its buttons; Enter and Space press the one
//! focused; Esc is Cancel. While it is open, `system::modal_open` counts
//! it, so drops wait (`adds_wait`), a close brings the window forward
//! (`close_window`) and nothing starts under it.

use super::*;

use crate::layout::kit::{self, DialogLayout};
use crate::model::{Prompt, PromptButton};

use super::super::effects::Answer;

/// A prompt open in the window.
pub(in crate::win) struct Ask {
    prompt: Prompt,
    /// What its answer does; `None` once answered.
    then: Option<Answer<PromptButton>>,
    /// The button with the keyboard focus, as an index into the prompt's
    /// buttons.
    focus: usize,
    /// The button the pointer is over, and the one it went down on; that
    /// one acts on release.
    pointer: Pointer,
    /// When it opened: it rises into place (`anim::dialog_in`).
    opened: std::time::Instant,
    /// Counted as a modal for as long as it is open.
    _open: system::InWindow,
}

/// The button a control is.
fn button(c: ControlId) -> Option<usize> {
    match c {
        ControlId::Ask(i) => Some(i),
        _ => None,
    }
}

impl Ask {
    /// `prompt`, asked, with the focus on its accent button; `then` gets
    /// the answer.
    pub fn new(prompt: Prompt, then: Answer<PromptButton>) -> Self {
        let focus = prompt.accent().unwrap_or(0);
        Ask {
            prompt,
            then: Some(then),
            focus,
            pointer: Pointer::default(),
            opened: std::time::Instant::now(),
            _open: system::InWindow::open(),
        }
    }

    /// When it opened.
    #[cfg(test)]
    pub fn opened(&self) -> std::time::Instant {
        self.opened
    }

    /// Its opacity and scale about its centre at `now`: fading in and
    /// settling from 1.05 over 250 ms, as the look spec's Motion has a
    /// dialog come in; there at once with animations off.
    pub fn rise_at(&self, now: std::time::Instant, animations: bool) -> (f32, f32) {
        crate::anim::dialog_in(crate::anim::elapsed_ms(self.opened, now), animations)
    }

    /// Whether it is still rising at `now`, so frames are drawn.
    pub fn rising(&self, now: std::time::Instant, animations: bool) -> bool {
        let ms = crate::theme::fluent::motion::PANE.ms;
        crate::anim::within(self.opened, ms, now, animations)
    }

    /// The button with the keyboard focus.
    #[cfg(test)]
    pub fn focus(&self) -> usize {
        self.focus
    }

    /// The buttons the pointer is over and has pressed.
    pub fn buttons(&self) -> (Option<usize>, Option<usize>) {
        (
            self.pointer.hover.and_then(button),
            self.pointer.pressed.and_then(button),
        )
    }

    /// What it asks and its continuation, if it wasn't answered: to be
    /// asked somewhere else.
    pub fn into_parts(mut self) -> (Prompt, Option<Answer<PromptButton>>) {
        let then = self.then.take();
        (self.prompt, then)
    }

    /// Its continuation, taken: the dialog has been answered.
    pub fn take_then(&mut self) -> Option<Answer<PromptButton>> {
        self.then.take()
    }

    /// Where it is in a window `cx.size` big.
    pub fn layout(&self, cx: &Ctx) -> DialogLayout {
        layout_of(&self.prompt, cx.size, cx.measure)
    }

    /// Answer with button `b`: the router closes the dialog and hands the
    /// answer on.
    fn answer(&self, cx: &mut Ctx, b: PromptButton) {
        cx.out.answer = Some(b);
        cx.out.invalidate = true;
    }

    /// Press button `i`, from a click, a key or UI Automation.
    pub fn press_button(&mut self, cx: &mut Ctx, i: usize) {
        if let Some(&b) = self.prompt.buttons.get(i) {
            self.focus = i;
            self.answer(cx, b);
        }
    }

    /// Esc, or UI Automation closing the dialog: Cancel, which keeps
    /// everything as it was.
    pub fn cancel(&mut self, cx: &mut Ctx) {
        self.answer(cx, PromptButton::Cancel);
    }

    /// Move the keyboard focus to button `i`.
    pub fn focus_on(&mut self, cx: &mut Ctx, i: usize) {
        if i < self.prompt.buttons.len() {
            self.focus = i;
            cx.out.invalidate = true;
        }
    }
}

/// The dialog for `prompt` in a window `size` big: its title and text
/// measured by `measure`, one button each in the band.
pub(in crate::win) fn layout_of(
    prompt: &Prompt,
    size: (f32, f32),
    measure: &dyn Measure,
) -> DialogLayout {
    let pad = crate::theme::fluent::space::DIALOG_PADDING;
    let inner = (kit::DIALOG_WIDTH.min(size.0 - 2.0 * kit::DIALOG_MARGIN) - 2.0 * pad).max(0.0);
    let (title_h, text_h) =
        measure.dialog_heights(prompt.dialog_title(), prompt.dialog_text(), inner);
    let labels: Vec<f32> = prompt
        .buttons
        .iter()
        .map(|&b| measure.label_width(prompt.label(b)))
        .collect();
    kit::fitting_dialog(size, title_h, text_h, &labels)
}

impl Surface for Ask {
    type View<'a> = AskView<'a>;
    type A11y<'a> = a11y::AskIn<'a>;

    fn hit(&self, cx: &Ctx, x: f32, y: f32) -> Option<ControlId> {
        self.layout(cx)
            .buttons
            .iter()
            .position(|r| r.contains(x, y))
            .map(ControlId::Ask)
    }

    fn hover(&mut self, cx: &mut Ctx, x: f32, y: f32) -> Reply {
        let hot = self.hit(cx, x, y);
        if self.pointer.hover != hot {
            self.pointer.hover = hot;
            cx.out.invalidate = true;
        }
        Reply::Taken
    }

    /// A press on a button holds it; a press anywhere else, on the smoke
    /// too, does nothing: the dialog waits for an answer.
    fn press(&mut self, cx: &mut Ctx, x: f32, y: f32) -> Reply {
        if let Some(c) = self.hit(cx, x, y) {
            self.pointer.pressed = Some(c);
            self.focus = button(c).unwrap_or(self.focus);
            cx.out.effects.push(Effect::Capture);
            cx.out.invalidate = true;
        }
        Reply::Taken
    }

    fn release(&mut self, cx: &mut Ctx, x: f32, y: f32) -> Reply {
        let Some(c) = self.pointer.pressed.take() else {
            return Reply::Block;
        };
        if self.hit(cx, x, y) == Some(c) {
            if let Some(i) = button(c) {
                self.press_button(cx, i);
            }
        }
        cx.out.invalidate = true;
        Reply::Taken
    }

    /// Esc is Cancel; Enter and Space press the focused button, which
    /// starts as the accent, but not as a held key repeats: the press that
    /// opened the prompt never answers it too. Tab, Shift+Tab and the left
    /// and right arrows go round the buttons, and Up and Down too while
    /// they stand one under another. Every other key stays with it.
    fn key(&mut self, cx: &mut Ctx, key: VIRTUAL_KEY, mods: Mods) -> Reply {
        let n = self.prompt.buttons.len();
        let stacked = || self.layout(cx).buttons.windows(2).any(|p| p[1].y > p[0].y);
        match key {
            VK_ESCAPE => self.cancel(cx),
            VK_RETURN | VK_SPACE if mods.repeat => {}
            VK_RETURN | VK_SPACE => self.press_button(cx, self.focus),
            VK_UP | VK_DOWN if n > 0 && !stacked() => return Reply::Block,
            VK_TAB | VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN if n > 0 => {
                let back = matches!(key, VK_LEFT | VK_UP) || (key == VK_TAB && mods.shift);
                self.focus = if back {
                    (self.focus + n - 1) % n
                } else {
                    (self.focus + 1) % n
                };
                cx.out.invalidate = true;
            }
            _ => return Reply::Block,
        }
        Reply::Taken
    }

    /// Nothing under it scrolls.
    fn scroll(&mut self, _cx: &mut Ctx, _by: TextScroll) -> Reply {
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
    ) -> AskView<'a> {
        let (hover, pressed) = self.buttons();
        AskView {
            layout: self.layout(cx),
            title: self.prompt.dialog_title(),
            text: self.prompt.dialog_text(),
            buttons: self
                .prompt
                .buttons
                .iter()
                .map(|&b| self.prompt.label(b))
                .collect(),
            accent: self.prompt.accent(),
            focus: self.focus,
            hover,
            pressed,
            rise: self.rise_at(cx.now, crate::theme::fluent::motion::animations()),
        }
    }

    fn a11y<'a>(&'a self, cx: &Ctx, _frame: Frame<'a>) -> a11y::AskIn<'a> {
        a11y::AskIn {
            layout: self.layout(cx),
            title: self.prompt.dialog_title(),
            text: self.prompt.dialog_text(),
            buttons: self
                .prompt
                .buttons
                .iter()
                .map(|&b| self.prompt.label(b))
                .collect(),
            focus: self.focus,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{leave_prompt, Leave, Prompt};
    use std::cell::RefCell;
    use std::rc::Rc;

    struct Lines;
    impl Measure for Lines {
        fn mono_height(&self, _text: &str, _width: f32) -> f32 {
            0.0
        }
        fn body_height(&self, _text: &str, _width: f32) -> f32 {
            0.0
        }
    }

    const SIZE: (f32, f32) = (800.0, 600.0);

    fn with<R>(f: impl FnOnce(&mut Ctx) -> R) -> (R, Out) {
        let settings = Settings::default();
        let mut cx = Ctx::new(&Lines, &settings, SIZE);
        let r = f(&mut cx);
        (r, cx.out)
    }

    /// The close prompt for one running job: Keep converting, Close.
    fn held() -> Ask {
        Ask::new(leave_prompt(1, Leave::Close).unwrap(), Box::new(|_, _| {}))
    }

    /// A prompt with more buttons than share the band side by side.
    fn tall() -> Ask {
        Ask::new(
            Prompt {
                heading: None,
                title: "x".into(),
                buttons: vec![
                    PromptButton::Stop(Leave::Remove),
                    PromptButton::Stop(Leave::Clear),
                    PromptButton::AddAll,
                    PromptButton::Cancel,
                ],
            },
            Box::new(|_, _| {}),
        )
    }

    fn center(r: crate::layout::Rect) -> (f32, f32) {
        (r.x + r.w / 2.0, r.y + r.h / 2.0)
    }

    #[test]
    fn it_counts_as_a_modal_while_it_is_open() {
        assert!(!system::modal_open());
        let a = held();
        assert!(system::modal_open());
        // It runs no loop of its own: the window's frames come as ever.
        assert!(!system::modal_loop_open());
        drop(a);
        assert!(!system::modal_open());
    }

    #[test]
    fn focus_starts_on_the_accent_and_enter_presses_it() {
        let none = Mods::default();
        let mut a = held();
        assert_eq!(a.focus(), 0);
        // A stop prompt's accent is Keep converting, first.
        let (reply, out) = with(|cx| a.key(cx, VK_RETURN, none));
        assert_eq!(reply, Reply::Taken);
        assert_eq!(out.answer, Some(PromptButton::Cancel));
        // The big folder's accent is Cancel, second.
        let a = Ask::new(model::big_folder_prompt(2000), Box::new(|_, _| {}));
        assert_eq!(a.focus(), 1);
    }

    #[test]
    fn esc_is_cancel_even_where_cancel_isnt_a_button() {
        let none = Mods::default();
        let mut a = held();
        let (reply, out) = with(|cx| a.key(cx, VK_ESCAPE, none));
        assert_eq!(reply, Reply::Taken);
        assert_eq!(out.answer, Some(PromptButton::Cancel));
        // The big folder's: Esc is its Cancel, which adds nothing.
        let mut a = Ask::new(model::big_folder_prompt(2000), Box::new(|_, _| {}));
        let (_, out) = with(|cx| a.key(cx, VK_ESCAPE, none));
        assert_eq!(out.answer, Some(PromptButton::Cancel));
    }

    #[test]
    fn tab_goes_round_the_buttons_inside_the_dialog() {
        let none = Mods::default();
        let shift = Mods {
            shift: true,
            ..Mods::default()
        };
        let mut a = held();
        let mut seen = vec![a.focus()];
        for _ in 0..2 {
            let (reply, out) = with(|cx| a.key(cx, VK_TAB, none));
            assert_eq!(reply, Reply::Taken);
            assert!(out.invalidate && out.answer.is_none());
            seen.push(a.focus());
        }
        // Round and back to the start, never leaving the dialog.
        assert_eq!(seen, [0, 1, 0]);
        with(|cx| a.key(cx, VK_TAB, shift));
        assert_eq!(a.focus(), 1);
        with(|cx| a.key(cx, VK_LEFT, none));
        assert_eq!(a.focus(), 0);
        with(|cx| a.key(cx, VK_RIGHT, none));
        assert_eq!(a.focus(), 1);
        // Space presses the focused button: Close.
        let (_, out) = with(|cx| a.key(cx, VK_SPACE, none));
        assert_eq!(out.answer, Some(PromptButton::Stop(Leave::Close)));
        // Enter on Close, once Tab is on it, presses Close.
        let mut a = held();
        with(|cx| a.key(cx, VK_TAB, none));
        let (_, out) = with(|cx| a.key(cx, VK_RETURN, none));
        assert_eq!(out.answer, Some(PromptButton::Stop(Leave::Close)));
    }

    #[test]
    fn it_rises_into_place_over_250_ms_and_at_once_with_animations_off() {
        let a = held();
        let t = a.opened();
        let ms = |n: u64| t + std::time::Duration::from_millis(n);
        let (o, s) = a.rise_at(t, true);
        assert!(o < 0.01, "{o}");
        assert!((s - crate::anim::DIALOG_SCALE_FROM).abs() < 1e-3, "{s}");
        let (o, s) = a.rise_at(ms(125), true);
        assert!(o > 0.5 && o < 1.0 && s > 1.0 && s < 1.05, "{o} {s}");
        assert_eq!(a.rise_at(ms(250), true), (1.0, 1.0));
        assert_eq!(a.rise_at(t, false), (1.0, 1.0));
        // Frames are asked for while it rises, and only then.
        assert!(a.rising(ms(100), true));
        assert!(!a.rising(ms(250), true));
        assert!(!a.rising(t, false));
    }

    #[test]
    fn a_held_enter_or_space_presses_nothing_until_pressed_again() {
        let repeat = Mods {
            repeat: true,
            ..Mods::default()
        };
        let mut a = held();
        for key in [VK_RETURN, VK_SPACE] {
            let (reply, out) = with(|cx| a.key(cx, key, repeat));
            assert_eq!(reply, Reply::Taken, "kept from what is under it");
            assert_eq!(out.answer, None);
        }
        // Tab still moves while held; a fresh press answers.
        with(|cx| a.key(cx, VK_TAB, repeat));
        assert_eq!(a.focus(), 1);
        let (_, out) = with(|cx| a.key(cx, VK_RETURN, Mods::default()));
        assert_eq!(out.answer, Some(PromptButton::Stop(Leave::Close)));
    }

    #[test]
    fn up_and_down_move_between_buttons_that_stand_one_under_another() {
        let none = Mods::default();
        let mut a = tall();
        assert_eq!(a.focus(), 3, "Cancel is the accent");
        with(|cx| a.key(cx, VK_DOWN, none));
        assert_eq!(a.focus(), 0, "round, as Tab goes");
        let (reply, _) = with(|cx| a.key(cx, VK_DOWN, none));
        assert_eq!(reply, Reply::Taken);
        assert_eq!(a.focus(), 1);
        with(|cx| a.key(cx, VK_DOWN, none));
        assert_eq!(a.focus(), 2);
        with(|cx| a.key(cx, VK_UP, none));
        assert_eq!(a.focus(), 1);
        with(|cx| a.key(cx, VK_UP, none));
        with(|cx| a.key(cx, VK_UP, none));
        assert_eq!(a.focus(), 3, "round, as Tab goes");
        // Side by side, Up and Down stay with the dialog and move nothing.
        let mut a = held();
        let (reply, _) = with(|cx| a.key(cx, VK_DOWN, none));
        assert_eq!(reply, Reply::Block);
        assert_eq!(a.focus(), 0);
    }

    #[test]
    fn other_keys_presses_and_the_wheel_go_no_further() {
        let mut a = held();
        let (reply, out) = with(|cx| a.key(cx, VK_DELETE, Mods::default()));
        assert_eq!(reply, Reply::Block);
        assert!(out.answer.is_none());
        let (reply, out) = with(|cx| a.press(cx, 2.0, 2.0));
        assert_eq!(reply, Reply::Taken);
        assert!(out.answer.is_none() && out.effects.is_empty());
        let (reply, _) = with(|cx| a.release(cx, 2.0, 2.0));
        assert_eq!(reply, Reply::Block);
        let (reply, _) = with(|cx| a.scroll(cx, TextScroll::Wheel(1.0)));
        assert_eq!(reply, Reply::Block);
    }

    #[test]
    fn a_click_on_a_button_answers_on_release_over_it() {
        let mut a = held();
        let (l, _) = with(|cx| a.layout(cx));
        assert_eq!(l.buttons.len(), 2);
        let (x, y) = center(l.buttons[1]);
        let (_, out) = with(|cx| a.press(cx, x, y));
        assert!(matches!(out.effects[..], [Effect::Capture]));
        assert_eq!(a.buttons().1, Some(1));
        assert_eq!(a.focus(), 1);
        let (_, out) = with(|cx| a.release(cx, x, y));
        assert_eq!(out.answer, Some(PromptButton::Stop(Leave::Close)));
        // Slid off before release: nothing.
        let mut a = held();
        let (x, y) = center(l.buttons[0]);
        with(|cx| a.press(cx, x, y));
        let (_, out) = with(|cx| a.release(cx, 2.0, 2.0));
        assert_eq!(out.answer, None);
    }

    #[test]
    fn long_button_words_stand_one_under_another_uncut() {
        // Four buttons can't share the band.
        let a = tall();
        let (l, _) = with(|cx| a.layout(cx));
        assert_eq!(l.buttons.len(), 4);
        assert!(l.buttons.windows(2).all(|p| p[1].y > p[0].y));
        assert!(l.buttons.iter().all(|b| b.w == l.buttons[0].w));
        // The close prompt's two short words share it.
        let (l, _) = with(|cx| held().layout(cx));
        assert!(l.buttons.windows(2).all(|p| p[1].y == p[0].y));
    }

    #[test]
    fn its_answer_goes_once_to_what_asked() {
        let got = Rc::new(RefCell::new(None));
        let to = got.clone();
        let mut a = Ask::new(
            leave_prompt(1, Leave::Close).unwrap(),
            Box::new(move |_, b| *to.borrow_mut() = Some(b)),
        );
        assert!(a.take_then().is_some());
        assert!(a.take_then().is_none());
        // (Run by App::answer_ask, with the dialog gone; see flow_tests.)
        assert_eq!(*got.borrow(), None);
    }

    #[test]
    fn the_view_names_the_buttons_in_the_prompts_words() {
        let a = Ask::new(leave_prompt(2, Leave::Close).unwrap(), Box::new(|_, _| {}));
        let (v, _) = with(|cx| {
            let v = a.view(cx, Frame::default(), &|_| (0.0, 0.0));
            (
                v.title.to_owned(),
                v.buttons.clone(),
                v.accent,
                v.text.map(str::to_owned),
            )
        });
        assert_eq!(v.0, "Stop converting and close?");
        assert_eq!(v.1, ["Keep converting", "Close"]);
        assert_eq!(v.2, Some(0));
        assert!(v.3.unwrap().starts_with("2 files are still converting."));
    }
}
