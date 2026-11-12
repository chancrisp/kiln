//! The file list and the buttons around it: rows, selection, scrolling,
//! the press that may become a drag out, and the header's and footer's
//! buttons. It is the layer under every overlay, and always there.
//!
//! `ListUi` holds the state and the choices that need nothing but the
//! list itself (what a press, a key or a wheel notch does to the
//! selection and the scroll). What needs the rest of the window (a row's
//! links, the buttons' places, Convert) is in the `impl App` at
//! the end, which asks it.

use super::*;

/// A button in the window's header or footer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::win) enum WindowButton {
    Clear,
    Add,
    /// The first-run card's Dismiss.
    Dismiss,
    /// The Dismiss of the InfoBar at the top of the list.
    InfoDismiss,
    Convert,
    Settings,
    Help,
    /// The credit's link.
    Credit,
}

/// What releasing a list press without a drag does to the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::win) enum Release {
    /// The press already did all it does.
    Nothing,
    /// A plain press inside a selection of two or more rows kept it: the
    /// release is the click.
    Click,
    /// A Ctrl-press on a selected row kept it: the release is the
    /// Ctrl-click, which takes the row out.
    CtrlClick,
}

/// A press on a list row, until the button is released.
#[derive(Debug, Clone, Copy)]
pub(in crate::win) struct ListPress {
    row: usize,
    /// What the release does.
    release: Release,
}

/// The row under the pointer and the part of it the pointer is on.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(in crate::win) struct RowHit {
    pub row: Option<usize>,
    /// On a running row's cancel cross.
    pub cancel: bool,
    /// On the row's link ("Show details", "Export", or an exported row's
    /// "Saved to" line).
    pub link: bool,
    /// On the row's Retry.
    pub retry: bool,
    /// On a finished drag-wheel row's Options.
    pub options: bool,
}

/// What a press on a row asks `App` to do once the selection has taken it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::win) enum Tap {
    /// The press only selected.
    Select,
    /// Options on a finished drag-wheel row.
    Options(usize),
    /// A running row's cancel cross.
    Cancel(usize),
    /// A failed row's Retry.
    Retry(usize),
    /// The row's link.
    Link(usize),
}

/// The list's state: how far it is scrolled, the part of a row the
/// pointer is on, the buttons' pointer and the press the button is still
/// down on.
#[derive(Default)]
pub(in crate::win) struct ListUi {
    /// How far the rows are scrolled, in DIPs from the top.
    scroll: f32,
    /// The row the pointer is on, and the part of it.
    hover: RowHit,
    /// The header or footer button the pointer is over, and the one it
    /// went down on, which acts on release.
    pointer: Pointer,
    /// The press on a list row the button is still down on.
    press: Option<ListPress>,
}

/// The window button a control is.
fn button(c: ControlId) -> Option<WindowButton> {
    match c {
        ControlId::Button(b) => Some(b),
        _ => None,
    }
}

impl ListUi {
    /// How far the rows are scrolled.
    pub fn scroll(&self) -> f32 {
        self.scroll
    }

    /// The first row in view, wholly or partly.
    pub fn first_row(&self) -> usize {
        (self.scroll / ROW_HEIGHT).floor().max(0.0) as usize
    }

    /// Back to the top.
    pub fn rewind(&mut self) {
        self.scroll = 0.0;
    }

    /// Keep the scroll inside the `rows` rows of a panel `panel_h` tall.
    pub fn clamp(&mut self, rows: usize, panel_h: f32) {
        self.scroll = self.scroll.clamp(0.0, layout::max_scroll(rows, panel_h));
    }

    /// The wheel turned by `notches`: the rows move up for a positive
    /// number, within the list.
    pub fn scroll_by(&mut self, notches: f32, rows: usize, panel_h: f32) {
        self.scroll -= notches * ROW_HEIGHT;
        self.clamp(rows, panel_h);
    }

    /// Scroll just far enough to show row `i` whole.
    pub fn show_row(&mut self, i: usize, rows: usize, panel_h: f32) {
        self.scroll = layout::scroll_to_show(i, self.scroll, panel_h);
        self.clamp(rows, panel_h);
    }

    // The pointer.

    /// The part of a row the pointer is on.
    pub fn hover(&self) -> RowHit {
        self.hover
    }

    /// The pointer is on `hit` now. Returns whether that changed.
    pub fn point_row(&mut self, hit: RowHit) -> bool {
        let changed = hit != self.hover;
        self.hover = hit;
        changed
    }

    /// The button the pointer is over.
    pub fn over_button(&self) -> Option<WindowButton> {
        self.pointer.hover.and_then(button)
    }

    /// The pointer is over `b` now. Returns whether that changed.
    pub fn point_button(&mut self, b: Option<WindowButton>) -> bool {
        let to = b.map(ControlId::Button);
        let changed = to != self.pointer.hover;
        self.pointer.hover = to;
        changed
    }

    /// The button went down on `b`.
    pub fn press_button(&mut self, b: WindowButton) {
        self.pointer.pressed = Some(ControlId::Button(b));
    }

    /// The button came up: the one it was down on, which acts if the
    /// pointer is still on it.
    pub fn release_button(&mut self) -> Option<WindowButton> {
        self.pointer.pressed.take().and_then(button)
    }

    /// Whether `b` is lit by the pointer, and whether it is pressed.
    pub fn state(&self, b: WindowButton) -> (bool, bool) {
        let is = |c: Option<ControlId>| c.and_then(button) == Some(b);
        (is(self.pointer.hover), is(self.pointer.pressed))
    }

    /// Nothing on the list stays lit or held by the pointer: a panel is
    /// over it, or the pointer left. A press on a row stays until the
    /// button is released.
    pub fn quiet(&mut self) {
        self.hover = RowHit::default();
        self.pointer.clear();
    }

    /// `b` is no longer lit by the pointer, as when its panel opens.
    pub fn unlight(&mut self, b: WindowButton) {
        if self.over_button() == Some(b) {
            self.pointer.hover = None;
        }
    }

    // The press.

    /// Whether a press on a row is held.
    pub fn pressing(&self) -> bool {
        self.press.is_some()
    }

    /// The press is over: the button came up elsewhere, or another window
    /// took the mouse.
    pub fn drop_press(&mut self) {
        self.press = None;
    }

    /// A press on the list with Shift or Ctrl held. The selection takes it
    /// as Explorer's does, and what the row's own parts ask of `App` is
    /// returned. A press that keeps the selection is held until the button
    /// comes up (`release_press`).
    pub fn row_down(
        &mut self,
        files: &mut FileList,
        hit: RowHit,
        (shift, ctrl): (bool, bool),
    ) -> Tap {
        // A click in the list hides the focus ring until a key.
        files.pointer_used();
        let mut tap = Tap::Select;
        let release = match hit.row {
            // As in Explorer, a Ctrl-press on a selected row keeps it
            // until release, so the selection can be dragged out.
            Some(i) if ctrl => Some(if files.ctrl_press(i) {
                Release::CtrlClick
            } else {
                Release::Nothing
            }),
            Some(i) if shift => {
                files.shift_click(i);
                Some(Release::Nothing)
            }
            Some(i) if hit.options => {
                files.click(i);
                tap = Tap::Options(i);
                None
            }
            Some(i) if hit.cancel || hit.link || hit.retry => {
                files.click(i);
                tap = if hit.cancel {
                    Tap::Cancel(i)
                } else if hit.retry {
                    Tap::Retry(i)
                } else {
                    Tap::Link(i)
                };
                None
            }
            // A press inside a selection keeps it, so the whole
            // selection can be dragged out.
            Some(i) => Some(if files.press(i) {
                Release::Click
            } else {
                Release::Nothing
            }),
            // A plain click off the rows clears the selection.
            None if !ctrl && !shift => {
                files.clear_selection();
                None
            }
            None => None,
        };
        // Held until release.
        if let (Some(i), Some(release)) = (hit.row, release) {
            self.press = Some(ListPress { row: i, release });
        }
        tap
    }

    /// The button came up. A press that kept the selection does the
    /// click, or the Ctrl-click, it held back.
    /// Returns whether a press was held.
    pub fn release_press(&mut self, files: &mut FileList) -> bool {
        let Some(p) = self.press.take() else {
            return false;
        };
        match p.release {
            Release::Nothing => {}
            Release::Click => files.click(p.row),
            Release::CtrlClick => files.ctrl_click(p.row),
        }
        true
    }

    /// A key that moves through the selection: the arrows, Page Up and
    /// Down, Home and End (Shift extends it), Ctrl+A and Escape. `page` is
    /// how many rows a screenful is, asked for only by Page Up and Down.
    /// Returns whether it was one.
    pub fn select_key(
        files: &mut FileList,
        key: VIRTUAL_KEY,
        shift: bool,
        ctrl: bool,
        page: impl FnOnce() -> isize,
    ) -> bool {
        match key {
            // Shift with an arrow, Home or End extends the selection.
            VK_UP if shift => files.extend(-1),
            VK_DOWN if shift => files.extend(1),
            VK_UP => files.step(-1),
            VK_DOWN => files.step(1),
            // A screenful at a time; Shift extends the selection.
            VK_PRIOR | VK_NEXT => {
                let page = page();
                let by = if key == VK_PRIOR { -page } else { page };
                if shift {
                    files.extend(by);
                } else {
                    files.step(by);
                }
            }
            VK_HOME | VK_END => {
                let i = match key {
                    VK_HOME => 0,
                    _ => files.len().saturating_sub(1),
                };
                if shift {
                    files.shift_click(i);
                } else {
                    files.click(i);
                }
            }
            VK_A if ctrl => files.select_all(),
            VK_ESCAPE => files.clear_selection(),
            _ => return false,
        }
        true
    }
}

impl App {
    /// Row `i`'s rectangle in the window, as scrolled.
    pub(in crate::win) fn row_rect(&self, i: usize) -> Option<layout::Rect> {
        let mut r = *self.layout().rows.get(i)?;
        r.y -= self.list.scroll();
        Some(r)
    }

    /// Whether `(x, y)` is on a row's link, with nothing over the list.
    pub(in crate::win) fn on_link(&self, x: f32, y: f32) -> bool {
        self.over().list_live() && {
            let hit = self.hit_row(x, y);
            hit.link || hit.retry
        }
    }

    /// The row under a point and whether the point is on its cancel cross,
    /// its link ("Show details", "Export", or an exported row's "Saved
    /// to" line) or its Retry.
    pub(in crate::win) fn hit_row(&self, x: f32, y: f32) -> RowHit {
        let l = self.layout();
        let scroll = self.list.scroll();
        let Some(i) = layout::row_at(&l, x, y, scroll) else {
            return RowHit::default();
        };
        let mut r = l.rows[i];
        r.y -= scroll;
        let e = &self.files.entries()[i];
        // No row has a link, a Cancel or a "Saved to" line to click while
        // the inspector carries the actions, and a row being exported has
        // no link until it's done.
        // Cancel is always on a running row, and Show in folder on a saved
        // one (`model::link_on_row`).
        let links = self.row_links();
        let running = matches!(e.job, JobState::Running { .. });
        let saving = self.row_saving(i);
        let link = e
            .link(false)
            .filter(|l| !saving && crate::model::link_on_row(l, links));
        let saved = false;
        let retry = links && !saving && e.retryable();
        let options = links && !saving && e.options_for().is_some();
        let part = layout::row_part_full(&r, x, y, running, link, saved, retry, options);
        RowHit {
            row: Some(i),
            cancel: part == layout::RowPart::Cancel,
            link: part == layout::RowPart::Link,
            retry: part == layout::RowPart::Retry,
            options: false,
        }
    }

    /// The row whose Options is under a point, if any: a finished
    /// drag-wheel conversion's, right after its link.
    fn hit_options(&self, x: f32, y: f32) -> Option<usize> {
        let l = self.layout();
        let scroll = self.list.scroll();
        let i = layout::row_at(&l, x, y, scroll)?;
        let mut r = l.rows[i];
        r.y -= scroll;
        let e = &self.files.entries()[i];
        let saving = self.row_saving(i);
        let links = self.row_links();
        if saving || e.options_for().is_none() || !links {
            return None;
        }
        let link = e.link(false);
        let part = layout::row_part_full(&r, x, y, false, link, false, false, true);
        (part == layout::RowPart::Options).then_some(i)
    }

    /// The row under a point and every part of it the point is on.
    fn hit(&self, x: f32, y: f32) -> RowHit {
        let hit = self.hit_row(x, y);
        RowHit {
            options: hit.row.is_some() && self.hit_options(x, y) == hit.row,
            ..hit
        }
    }

    // Input.

    /// The pointer moved to `(x, y)`, with the buttons and keys `keys`
    /// (the message's MK_ flags) says.
    pub(in crate::win) fn mouse_move(&mut self, x: f32, y: f32, keys: u32) {
        self.dispatch(Input::Move { x, y, keys });
    }

    /// A move over the list and its buttons, with nothing over them.
    pub(in crate::win) fn list_move(&mut self, x: f32, y: f32, keys: u32) -> Reply {
        let _ = keys;
        let hit = self.hit(x, y);
        let bar = self.title_bar();
        let at = |r: Option<layout::Rect>| r.is_some_and(|b| b.contains(x, y));
        let convert = self.convert_block().is_none() && at(self.convert_rect());
        let button = [
            (WindowButton::Clear, at(self.clear_rect())),
            (WindowButton::Convert, convert),
            (WindowButton::Add, at(self.add_button())),
            (WindowButton::Dismiss, at(self.dismiss_button())),
            (WindowButton::InfoDismiss, at(self.top_info_dismiss())),
            (WindowButton::Credit, at(self.credit().map(|c| c.name))),
            (WindowButton::Settings, at(bar.settings)),
            (WindowButton::Help, at(bar.help)),
        ]
        .into_iter()
        .find_map(|(b, over)| over.then_some(b));
        if self.list.point_button(button) {
            self.invalidate();
        }
        let hot = self.panel_control_at(x, y);
        if self.inspector_ui.point(hot) {
            self.invalidate();
        }
        if self.list.point_row(hit) {
            self.invalidate();
        }
        Reply::Taken
    }

    /// A pressed button acts when the pointer is released over it, so a
    /// press can be taken back by sliding off.
    pub(in crate::win) fn mouse_up(&mut self, x: f32, y: f32) {
        let rel = Released {
            button: self.list.release_button(),
            panel: self.inspector_ui.release(),
        };
        self.effect(Effect::ReleaseCapture);
        // A press that kept the selection: the click, or the Ctrl-click,
        // it held back.
        if self.list.release_press(&mut self.files) {
            self.invalidate();
        }
        self.dispatch(Input::Up { x, y, rel });
    }

    /// The pointer left the window: nothing on the list stays lit or held.
    pub(in crate::win) fn list_leave(&mut self) {
        self.list.quiet();
        self.inspector_ui.leave();
    }

    /// The list scrolls by `notches` wheel notches.
    pub(in crate::win) fn list_scroll(&mut self, notches: f32) {
        let panel_h = self.layout().list.h;
        self.list.scroll_by(notches, self.files.len(), panel_h);
        self.invalidate();
    }

    /// A release on the list's buttons and the inspector's controls: each
    /// pressed one acts if the pointer is still on it.
    pub(in crate::win) fn list_up(&mut self, x: f32, y: f32, rel: &Released) -> Reply {
        let Released { button, panel } = *rel;
        if button.is_none() && panel.is_none() {
            return Reply::Pass;
        }
        if let Some(c) = panel {
            if self.panel_control_at(x, y) == Some(c) {
                self.panel_act(c, false);
            }
        }
        let on = |r: Option<layout::Rect>| r.is_some_and(|b| b.contains(x, y));
        match button {
            Some(WindowButton::Convert) if on(self.convert_rect()) => self.convert_selection(false),
            Some(WindowButton::Add) if on(self.add_button()) => self.add_files(false),
            Some(WindowButton::Dismiss) if on(self.dismiss_button()) => self.dismiss_first_run(),
            Some(WindowButton::InfoDismiss) if on(self.top_info_dismiss()) => {
                self.set_top_info(None)
            }
            Some(WindowButton::Credit) if on(self.credit().map(|c| c.name)) => self.open_credit(),

            Some(WindowButton::Clear) if on(self.clear_rect()) => self.clear(),
            _ => {}
        }
        self.invalidate();
        Reply::Taken
    }

    /// The left button went down at `(x, y)`, with Shift or Ctrl held as
    /// `keys` (the message's MK_ flags) says.
    pub(in crate::win) fn mouse_down(&mut self, x: f32, y: f32, keys: u32) {
        // The pointer takes over from the keyboard: no button keeps a ring.
        self.button_focus = None;
        self.list.drop_press();
        self.dispatch(Input::Down { x, y, keys });
    }

    /// A press on the list and its buttons, with nothing over them.
    pub(in crate::win) fn list_down(&mut self, x: f32, y: f32, keys: u32) -> Reply {
        let (shift, ctrl) = (keys & MK_SHIFT_FLAG != 0, keys & MK_CONTROL_FLAG != 0);
        let bar = self.title_bar();
        let on = |r: Option<layout::Rect>| r.is_some_and(|b| b.contains(x, y));
        if on(bar.settings) {
            self.open_settings();
        } else if on(bar.help) {
            self.open_help();
        } else if self.clear_rect().is_some_and(|b| b.contains(x, y)) {
            self.list.press_button(WindowButton::Clear);
            self.effect(Effect::Capture);
        } else if self.undo_link_rect().is_some_and(|b| b.contains(x, y)) {
            self.undo_remove();
        } else if self.convert_rect().is_some_and(|b| b.contains(x, y)) {
            // Disabled, a press says why and nothing else.
            match self.convert_block() {
                None => {
                    self.list.press_button(WindowButton::Convert);
                    self.effect(Effect::Capture);
                }
                Some(why) => {
                    self.alert(why.to_owned());
                    self.invalidate();
                }
            }
        } else if self.add_button().is_some_and(|b| b.contains(x, y)) {
            self.list.press_button(WindowButton::Add);
            self.effect(Effect::Capture);
        } else if self.dismiss_button().is_some_and(|b| b.contains(x, y)) {
            self.list.press_button(WindowButton::Dismiss);
            self.effect(Effect::Capture);
        } else if self.top_info_dismiss().is_some_and(|b| b.contains(x, y)) {
            self.list.press_button(WindowButton::InfoDismiss);
            self.effect(Effect::Capture);
        } else if self.credit().is_some_and(|c| c.name.contains(x, y)) {
            self.list.press_button(WindowButton::Credit);
            self.effect(Effect::Capture);
        } else if self.on_inspector(x, y) {
            // A control in the inspector acts on release, as a button does.
            // The rest of it, and all of it while it folds away, is no part
            // of the list: the selection stays.
            if let Some(c) = self.panel_control_at(x, y) {
                self.inspector_ui.press(c);
                self.effect(Effect::Capture);
            }
        } else {
            let hit = self.hit(x, y);
            match self.list.row_down(&mut self.files, hit, (shift, ctrl)) {
                Tap::Select => {}
                Tap::Options(i) => self.options_row(i, false),
                Tap::Cancel(i) => self.cancel_row(i),
                Tap::Retry(i) => self.retry_row(i),
                Tap::Link(i) => self.follow_link(i, false),
            }
            // Held until release.
            if self.list.pressing() {
                self.effect(Effect::Capture);
            }
        }
        self.invalidate();
        Reply::Taken
    }

    pub(in crate::win) fn key(&mut self, key: VIRTUAL_KEY) -> bool {
        let down = |k: VIRTUAL_KEY| unsafe { GetKeyState(k.0 as i32) } < 0;
        let (shift, ctrl, alt) = (down(VK_SHIFT), down(VK_CONTROL), down(VK_MENU));
        self.dispatch(Input::Key {
            key,
            shift,
            ctrl,
            alt,
        }) != Reply::Pass
    }

    /// A key with nothing over the list.
    pub(in crate::win) fn list_key(
        &mut self,
        key: VIRTUAL_KEY,
        shift: bool,
        ctrl: bool,
        alt: bool,
    ) -> Reply {
        // F1 opens help from the list or a wheel.
        if key == VK_F1 {
            self.open_help();
            return Reply::Taken;
        }
        // Tab and Shift+Tab go round the list and the header and footer
        // buttons. On a button, Enter or Space presses it, and Escape or an
        // arrow key goes back to the list.
        if key == VK_TAB && !ctrl && !alt {
            // The inspector's controls that are drawn, in its reading order,
            // while it carries the actions.
            let inspector: Vec<InspectorControl> = self
                .live_panel()
                .map(|p| p.controls.iter().map(|(c, _)| *c).collect())
                .unwrap_or_default();
            self.button_focus = if self.files.is_empty() {
                model::empty_tab_stop_all(
                    self.focused_button(),
                    shift,
                    self.first_run().is_some(),
                    self.top_info_dismiss().is_some(),
                    self.credit().is_some(),
                )
            } else {
                model::tab_stop_all(
                    self.focused_button(),
                    shift,
                    true,
                    self.convert_block().is_none(),
                    self.top_info_bar().and_then(|b| b.dismiss).is_some(),
                    self.credit().is_some(),
                    &inspector,
                )
            };
            self.files.keyboard_used();
            self.invalidate();
            return Reply::Taken;
        }
        if let Some(b) = self.focused_button() {
            match key {
                VK_RETURN | VK_SPACE => {
                    self.press_button(b);
                    self.invalidate();
                    return Reply::Taken;
                }
                VK_ESCAPE | VK_UP | VK_DOWN | VK_LEFT | VK_RIGHT | VK_HOME | VK_END | VK_PRIOR
                | VK_NEXT => {
                    self.button_focus = None;
                    self.files.keyboard_used();
                    self.invalidate();
                    return Reply::Taken;
                }
                // Delete acts on the list's selection, so the focus goes
                // back to it; shortcuts leave it where it is.
                VK_DELETE | VK_BACK => self.button_focus = None,
                _ => {}
            }
        }
        // Keys that move through or act on the list show its focus ring.
        if matches!(
            key,
            VK_UP
                | VK_DOWN
                | VK_HOME
                | VK_END
                | VK_PRIOR
                | VK_NEXT
                | VK_DELETE
                | VK_BACK
                | VK_ESCAPE
        ) || (key == VK_A && ctrl)
        {
            self.files.keyboard_used();
        }
        let page = if matches!(key, VK_PRIOR | VK_NEXT) {
            layout::rows_per_page(self.layout().list.h)
        } else {
            0
        };
        let by_selection = ListUi::select_key(&mut self.files, key, shift, ctrl, || page);
        if !by_selection {
            match key {
                VK_DELETE | VK_BACK => self.remove_selected(),
                // Enter does what the footer's Convert does; with Ctrl, the
                // tools wheel opens.
                VK_RETURN if ctrl => self.open_wheel_for_selection(WheelKind::Tools),
                VK_RETURN => self.convert_selection(true),
                VK_OEM_COMMA if ctrl => self.open_settings(),
                VK_Z if ctrl => self.undo_remove(),
                VK_O if ctrl => self.add_files(true),
                VK_V if ctrl => self.paste(true),

                // Space follows the selected row's link, with one row
                // selected, while rows show their links; it stops a
                // running row as the footer's Cancel does, whose key it is.
                VK_SPACE => {
                    if let [i] = self.files.selection()[..] {
                        let shown = self.files.entries()[i]
                            .link(false)
                            .is_some_and(|l| crate::model::link_on_row(l, self.row_links()));
                        if self.row_state(i) == RowState::Running {
                            self.cancel_row(i);
                        } else if shown {
                            self.follow_link(i, true);
                        }
                    }
                }
                _ => return Reply::Pass,
            }
        }
        self.scroll_to_selection();
        self.invalidate();
        Reply::Taken
    }

    pub(in crate::win) fn clamp_scroll(&mut self) {
        let panel_h = self.layout().list.h;
        self.list.clamp(self.files.len(), panel_h);
    }

    /// Scroll the focus row into view.
    pub(in crate::win) fn scroll_to_selection(&mut self) {
        if let Some(i) = self.files.focus() {
            self.scroll_to_row(i);
        }
    }

    pub(in crate::win) fn scroll_to_row(&mut self, i: usize) {
        let panel_h = self.layout().list.h;
        self.list.show_row(i, self.files.len(), panel_h);
    }

    /// The rows in view, or partly in view.
    pub(in crate::win) fn visible_rows(&self) -> std::ops::Range<usize> {
        let panel = self.layout().panel;
        let first = self.list.first_row();
        let visible = (panel.h / ROW_HEIGHT).ceil() as usize + 1;
        first.min(self.files.len())..(first + visible).min(self.files.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(row: usize) -> RowHit {
        RowHit {
            row: Some(row),
            ..RowHit::default()
        }
    }

    const PLAIN: (bool, bool) = (false, false);
    const SHIFT: (bool, bool) = (true, false);
    const CTRL: (bool, bool) = (false, true);

    /// A press on `hit` with the modifiers `keys`. A new press lets go of the old one, as `App::mouse_down` does.
    fn press(ui: &mut ListUi, files: &mut FileList, hit: RowHit, keys: (bool, bool)) -> Tap {
        ui.drop_press();
        ui.row_down(files, hit, keys)
    }

    // Selection by the pointer.

    #[test]
    fn a_plain_press_selects_only_that_row() {
        let (_dir, mut files) = super::super::listed(4);
        let mut ui = ListUi::default();
        assert_eq!(press(&mut ui, &mut files, on(1), PLAIN), Tap::Select);
        assert_eq!(files.selection(), [1]);
        assert_eq!(press(&mut ui, &mut files, on(2), PLAIN), Tap::Select);
        assert_eq!(files.selection(), [2]);
        assert_eq!(files.focus(), Some(2));
    }

    #[test]
    fn a_click_off_the_rows_clears_the_selection_only_without_a_modifier() {
        let (_dir, mut files) = super::super::listed(4);
        let mut ui = ListUi::default();
        press(&mut ui, &mut files, on(1), PLAIN);
        let off = RowHit::default();
        press(&mut ui, &mut files, off, SHIFT);
        press(&mut ui, &mut files, off, CTRL);
        assert_eq!(files.selection(), [1]);
        press(&mut ui, &mut files, off, PLAIN);
        assert!(files.selection().is_empty());
        assert!(!ui.pressing(), "nothing was pressed on");
    }

    #[test]
    fn shift_selects_from_the_anchor_to_the_row() {
        let (_dir, mut files) = super::super::listed(6);
        let mut ui = ListUi::default();
        press(&mut ui, &mut files, on(1), PLAIN);
        press(&mut ui, &mut files, on(4), SHIFT);
        assert_eq!(files.selection(), [1, 2, 3, 4]);
        press(&mut ui, &mut files, on(0), SHIFT);
        assert_eq!(files.selection(), [0, 1]);
    }

    #[test]
    fn ctrl_toggles_a_row_in_and_out() {
        let (_dir, mut files) = super::super::listed(4);
        let mut ui = ListUi::default();
        press(&mut ui, &mut files, on(0), PLAIN);
        // An unselected row goes in at once.
        press(&mut ui, &mut files, on(2), CTRL);
        assert_eq!(files.selection(), [0, 2]);
        ui.release_press(&mut files);
        // A selected row stays until the release, so the selection can be
        // dragged out; the release is the Ctrl-click that takes it out.
        press(&mut ui, &mut files, on(0), CTRL);
        assert_eq!(files.selection(), [0, 2]);
        assert!(ui.release_press(&mut files));
        assert_eq!(files.selection(), [2]);
    }

    #[test]
    fn a_press_inside_a_selection_keeps_it_until_the_release() {
        let (_dir, mut files) = super::super::listed(5);
        let mut ui = ListUi::default();
        press(&mut ui, &mut files, on(1), PLAIN);
        press(&mut ui, &mut files, on(3), SHIFT);
        ui.release_press(&mut files);
        assert_eq!(files.selection(), [1, 2, 3]);
        press(&mut ui, &mut files, on(2), PLAIN);
        assert_eq!(files.selection(), [1, 2, 3], "the whole selection can drag");
        assert!(ui.release_press(&mut files));
        assert_eq!(files.selection(), [2], "without a drag it is the click");
        assert!(!ui.release_press(&mut files), "nothing is held any more");
    }

    #[test]
    fn a_rows_own_parts_act_after_selecting_it() {
        let (_dir, mut files) = super::super::listed(4);
        let mut ui = ListUi::default();
        let hit = |f: fn(&mut RowHit)| {
            let mut h = on(2);
            f(&mut h);
            h
        };
        let cases = [
            (hit(|h| h.cancel = true), Tap::Cancel(2)),
            (hit(|h| h.retry = true), Tap::Retry(2)),
            (hit(|h| h.link = true), Tap::Link(2)),
            (hit(|h| h.options = true), Tap::Options(2)),
        ];
        for (h, tap) in cases {
            press(&mut ui, &mut files, on(0), PLAIN);
            assert_eq!(press(&mut ui, &mut files, h, PLAIN), tap);
            assert_eq!(files.selection(), [2]);
            assert!(!ui.pressing(), "these act at once and hold nothing");
        }
    }

    #[test]
    fn a_modifier_press_on_a_parts_selects_instead_of_acting() {
        let (_dir, mut files) = super::super::listed(4);
        let mut ui = ListUi::default();
        press(&mut ui, &mut files, on(0), PLAIN);
        let on_link = RowHit {
            link: true,
            ..on(2)
        };
        assert_eq!(press(&mut ui, &mut files, on_link, SHIFT), Tap::Select);
        assert_eq!(files.selection(), [0, 1, 2]);
    }

    // Selection by keys.

    fn key(files: &mut FileList, k: VIRTUAL_KEY, shift: bool, ctrl: bool) -> bool {
        ListUi::select_key(files, k, shift, ctrl, || 3)
    }

    #[test]
    fn the_arrows_move_the_one_selected_row() {
        let (_dir, mut files) = super::super::listed(4);
        assert!(key(&mut files, VK_DOWN, false, false));
        assert_eq!(files.selection(), [0]);
        key(&mut files, VK_DOWN, false, false);
        key(&mut files, VK_DOWN, false, false);
        assert_eq!(files.selection(), [2]);
        key(&mut files, VK_UP, false, false);
        assert_eq!(files.selection(), [1]);
        // The ends hold.
        for _ in 0..9 {
            key(&mut files, VK_DOWN, false, false);
        }
        assert_eq!(files.selection(), [3]);
    }

    #[test]
    fn shift_with_an_arrow_extends_the_selection() {
        let (_dir, mut files) = super::super::listed(5);
        key(&mut files, VK_DOWN, false, false);
        key(&mut files, VK_DOWN, true, false);
        key(&mut files, VK_DOWN, true, false);
        assert_eq!(files.selection(), [0, 1, 2]);
        key(&mut files, VK_UP, true, false);
        assert_eq!(files.selection(), [0, 1]);
    }

    #[test]
    fn home_and_end_go_to_the_ends_and_shift_extends_to_them() {
        let (_dir, mut files) = super::super::listed(5);
        key(&mut files, VK_END, false, false);
        assert_eq!(files.selection(), [4]);
        key(&mut files, VK_HOME, false, false);
        assert_eq!(files.selection(), [0]);
        key(&mut files, VK_END, true, false);
        assert_eq!(files.selection(), [0, 1, 2, 3, 4]);
    }

    #[test]
    fn page_keys_move_a_screenful_and_ask_for_its_size_only_then() {
        let (_dir, mut files) = super::super::listed(10);
        key(&mut files, VK_HOME, false, false);
        assert!(ListUi::select_key(&mut files, VK_NEXT, false, false, || 4));
        assert_eq!(files.selection(), [4]);
        assert!(ListUi::select_key(&mut files, VK_NEXT, true, false, || 4));
        assert_eq!(files.selection(), [4, 5, 6, 7, 8]);
        assert!(ListUi::select_key(&mut files, VK_PRIOR, false, false, || 2));
        assert_eq!(files.selection(), [6]);
        // Other keys never ask.
        assert!(ListUi::select_key(&mut files, VK_UP, false, false, || {
            panic!("asked")
        }));
    }

    #[test]
    fn ctrl_a_selects_all_and_escape_clears_the_selection() {
        let (_dir, mut files) = super::super::listed(4);
        assert!(key(&mut files, VK_A, false, true));
        assert_eq!(files.selection(), [0, 1, 2, 3]);
        assert!(key(&mut files, VK_ESCAPE, false, false));
        assert!(files.selection().is_empty());
        // A on its own is not a selection key.
        assert!(!key(&mut files, VK_A, false, false));
        assert!(files.selection().is_empty());
    }

    #[test]
    fn keys_the_list_does_not_select_with_are_left_to_it() {
        let (_dir, mut files) = super::super::listed(4);
        for k in [VK_DELETE, VK_RETURN, VK_SPACE, VK_TAB, VK_F1] {
            assert!(!key(&mut files, k, false, false));
        }
        assert!(files.selection().is_empty());
    }

    // Scrolling.

    const PANEL: f32 = ROW_HEIGHT * 5.0;

    #[test]
    fn the_wheel_scrolls_a_row_a_notch_and_stops_at_both_ends() {
        let mut ui = ListUi::default();
        // 20 rows in a panel five tall scroll 15 rows.
        ui.scroll_by(-2.0, 20, PANEL);
        assert_eq!(ui.scroll(), 2.0 * ROW_HEIGHT);
        ui.scroll_by(1.0, 20, PANEL);
        assert_eq!(ui.scroll(), ROW_HEIGHT);
        ui.scroll_by(5.0, 20, PANEL);
        assert_eq!(ui.scroll(), 0.0, "never above the first row");
        ui.scroll_by(-99.0, 20, PANEL);
        assert_eq!(ui.scroll(), 15.0 * ROW_HEIGHT, "never past the last");
    }

    #[test]
    fn a_short_list_does_not_scroll() {
        let mut ui = ListUi::default();
        ui.scroll_by(-3.0, 4, PANEL);
        assert_eq!(ui.scroll(), 0.0);
    }

    #[test]
    fn fewer_rows_pull_the_scroll_back_in() {
        let mut ui = ListUi::default();
        ui.scroll_by(-99.0, 20, PANEL);
        ui.clamp(8, PANEL);
        assert_eq!(ui.scroll(), 3.0 * ROW_HEIGHT);
        ui.rewind();
        assert_eq!(ui.scroll(), 0.0);
        assert_eq!(ui.first_row(), 0);
    }

    #[test]
    fn a_row_out_of_view_scrolls_just_into_it() {
        let mut ui = ListUi::default();
        ui.show_row(2, 20, PANEL);
        assert_eq!(ui.scroll(), 0.0, "already in view");
        ui.show_row(7, 20, PANEL);
        assert_eq!(ui.scroll(), 3.0 * ROW_HEIGHT, "its bottom at the panel's");
        assert_eq!(ui.first_row(), 3);
        ui.show_row(1, 20, PANEL);
        assert_eq!(ui.scroll(), ROW_HEIGHT, "its top at the panel's");
        ui.show_row(19, 20, PANEL);
        assert_eq!(ui.scroll(), 15.0 * ROW_HEIGHT);
    }

    // The buttons' pointer.

    #[test]
    fn the_pointer_lights_one_button_and_a_press_acts_on_release() {
        let mut ui = ListUi::default();
        assert!(ui.point_button(Some(WindowButton::Add)));
        assert!(!ui.point_button(Some(WindowButton::Add)));
        assert_eq!(ui.state(WindowButton::Add), (true, false));
        assert_eq!(ui.state(WindowButton::Clear), (false, false));
        ui.press_button(WindowButton::Add);
        assert_eq!(ui.state(WindowButton::Add), (true, true));
        assert_eq!(ui.release_button(), Some(WindowButton::Add));
        assert_eq!(ui.release_button(), None);
        assert!(ui.point_button(None));
        assert_eq!(ui.over_button(), None);
    }

    #[test]
    fn a_panel_over_the_list_puts_everything_on_it_out() {
        let mut ui = ListUi::default();
        ui.point_button(Some(WindowButton::Add));
        ui.press_button(WindowButton::Add);
        ui.point_row(on(3));
        ui.quiet();
        assert_eq!(ui.state(WindowButton::Add), (false, false));
        assert_eq!(ui.hover(), RowHit::default());
    }

    #[test]
    fn a_panel_opening_unlights_only_its_own_button() {
        let mut ui = ListUi::default();
        ui.point_button(Some(WindowButton::Help));
        ui.unlight(WindowButton::Settings);
        assert_eq!(ui.over_button(), Some(WindowButton::Help));
        ui.unlight(WindowButton::Help);
        assert_eq!(ui.over_button(), None);
    }

    #[test]
    fn a_quiet_list_keeps_the_press_a_row_is_held_by() {
        let (_dir, mut files) = super::super::listed(3);
        let mut ui = ListUi::default();
        press(&mut ui, &mut files, on(1), PLAIN);
        ui.quiet();
        assert!(ui.pressing());
    }

    #[test]
    fn the_row_hover_says_when_it_changes() {
        let mut ui = ListUi::default();
        assert!(!ui.point_row(RowHit::default()));
        assert!(ui.point_row(on(2)));
        assert!(!ui.point_row(on(2)));
        let on_cancel = RowHit {
            cancel: true,
            ..on(2)
        };
        assert!(ui.point_row(on_cancel));
        assert_eq!(ui.hover(), on_cancel);
    }
}
