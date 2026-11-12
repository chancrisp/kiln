//! The inspector beside the list: the page built for the selection, the
//! one it replaces while that fades out, the clock that keeps a converting
//! file's time left, and the pointer over its controls.
//!
//! It is no layer of its own: it sits in the list's panel and takes the
//! pointer from the list. What it asks of `App` (opening a sheet,
//! converting, exporting) is `App::panel_act`.

use super::*;

/// The inspector's state: what is shown, what it replaced, whether its
/// clock runs and where the pointer is on it.
#[derive(Default)]
pub(in crate::win) struct InspectorUi {
    /// The inspector beside the list, as last built: see
    /// `App::refresh_panel`.
    panel: Option<Panel>,
    /// What the inspector showed before its content last changed, and how
    /// far it had faded in then. It fades out over half the cross-fade as
    /// the new content rises in (`Motion::inspector_fading`), then goes;
    /// at once if rows come or go under it (`Panel::still_listed`).
    prev: Option<(Panel, f32)>,
    /// Whether `CLOCK_TIMER` is running: while the inspector shows a file
    /// converting.
    clock: bool,
    /// The control under the pointer, and the one pressed, which acts when
    /// the pointer is released over it.
    pointer: Pointer,
    /// What the inspector was last built from, so a message that changed
    /// none of it builds nothing: see `App::refresh_panel`.
    built: Option<PanelInputs>,
    /// Moves when something the inspector is built from changes outside
    /// the list: Settings, a result's size, the Text size.
    rev: u64,
}

/// The inspector control a control is.
fn control(c: ControlId) -> Option<InspectorControl> {
    match c {
        ControlId::Inspector(c) => Some(c),
        _ => None,
    }
}

impl InspectorUi {
    /// The inspector as last built, shown or folding away.
    pub fn panel(&self) -> Option<&Panel> {
        self.panel.as_ref()
    }

    /// What it showed before, with how far that had faded in.
    pub fn prev(&self) -> Option<&(Panel, f32)> {
        self.prev.as_ref()
    }

    /// The rows it pictures at its own size: none while it doesn't show.
    pub fn pictured(&self) -> Vec<usize> {
        self.panel
            .as_ref()
            .map(|p| p.pictured().to_vec())
            .unwrap_or_default()
    }

    /// What it showed before is gone: Animation effects went off, or its
    /// fade ended.
    pub fn drop_prev(&mut self) {
        self.prev = None;
    }

    /// Drop what it showed before once its rows no longer hold the files
    /// it showed in `entries`. Returns whether it did.
    pub fn drop_moved_prev(&mut self, entries: &[model::Entry]) -> bool {
        let moved = self
            .prev
            .as_ref()
            .is_some_and(|(p, _)| !p.still_listed(entries));
        if moved {
            self.prev = None;
        }
        moved
    }

    /// Something it is built from changed outside the list.
    pub fn bump(&mut self) {
        self.rev = self.rev.wrapping_add(1);
    }

    /// The window's own revision, one of what it is built from.
    pub fn rev(&self) -> u64 {
        self.rev
    }

    /// Note that it is being built from `inputs`. Returns whether it
    /// already was: then there is nothing to build.
    pub fn built_from(&mut self, inputs: PanelInputs) -> bool {
        if self.built == Some(inputs) {
            return true;
        }
        self.built = Some(inputs);
        false
    }

    /// Whether a page for these words and files, laid out on `rect`, is
    /// the one that shows already.
    pub fn is_page(
        &self,
        rect: layout::Rect,
        nothing: &str,
        model: &insp::Inspector,
        shows: &Shows,
    ) -> bool {
        self.panel.as_ref().is_some_and(|p| {
            p.rect == rect
                && p.nothing == nothing
                && same_page((&p.model, &p.shows), (model, shows))
        })
    }

    /// Whether a page that shows `shows` is another page than the one
    /// showing, so that it cross-fades.
    pub fn is_fresh(&self, shows: &Shows) -> bool {
        self.panel
            .as_ref()
            .is_none_or(|p| new_content(&p.shows, shows))
    }

    /// Show `page`; the page it replaces is returned.
    pub fn show(&mut self, page: Panel) -> Option<Panel> {
        self.panel.replace(page)
    }

    /// Settle the cross-fade after `show` of new content: what `motion`
    /// says fades out, from where it is.
    pub fn fade(
        &mut self,
        old: Option<Panel>,
        how: Option<(Fading, f32)>,
        entries: &[model::Entry],
    ) {
        match how {
            // What was fading out still shows more than what was rising
            // in: it carries on fading from where it is, and the other
            // goes.
            Some((Fading::Before, from)) => {
                if let Some((_, a)) = &mut self.prev {
                    *a = from;
                }
            }
            // What was there fades out, unless its rows moved under it.
            Some((Fading::Now, from)) => {
                self.prev = old.filter(|p| p.still_listed(entries)).map(|p| (p, from));
            }
            None => self.prev = None,
        }
    }

    /// Nothing is shown any more: it goes, and so does what it replaced.
    /// Returns whether anything was there.
    pub fn clear(&mut self) -> bool {
        let had = self.panel.is_some() || self.prev.is_some();
        self.panel = None;
        self.prev = None;
        had
    }

    /// Whether rows came or went under the page, so that its rows no
    /// longer hold its files in `entries`.
    pub fn moved(&self, entries: &[model::Entry]) -> bool {
        self.panel
            .as_ref()
            .is_some_and(|p| !p.still_listed(entries))
    }

    /// It isn't carrying the actions: nothing is built for it, and the
    /// pointer lets go.
    pub fn folded(&mut self) {
        self.built = None;
        self.pointer.clear();
    }

    /// Whether the clock is wanted.
    pub fn clock(&self) -> bool {
        self.clock
    }

    /// Whether the clock should change to `on`: it should when it isn't
    /// already so. Returns whether it changed.
    pub fn set_clock(&mut self, on: bool) -> bool {
        if on == self.clock {
            return false;
        }
        self.clock = on;
        true
    }

    // The pointer.

    /// The control the pointer is on.
    pub fn hover(&self) -> Option<InspectorControl> {
        self.pointer.hover.and_then(control)
    }

    /// The control the button went down on.
    pub fn pressed(&self) -> Option<InspectorControl> {
        self.pointer.pressed.and_then(control)
    }

    /// The pointer is on `c` now. Returns whether that changed.
    pub fn point(&mut self, c: Option<InspectorControl>) -> bool {
        let to = c.map(ControlId::Inspector);
        let changed = to != self.pointer.hover;
        self.pointer.hover = to;
        changed
    }

    /// The button went down on `c`.
    pub fn press(&mut self, c: InspectorControl) {
        self.pointer.pressed = Some(ControlId::Inspector(c));
    }

    /// The button came up: the control it was down on, which acts if the
    /// pointer is still on it.
    pub fn release(&mut self) -> Option<InspectorControl> {
        self.pointer.pressed.take().and_then(control)
    }

    /// The pointer left the window.
    pub fn leave(&mut self) {
        self.pointer.clear();
    }

    /// The page changed or went: the pointer lets go of a control it
    /// doesn't place any more.
    pub fn lost(&mut self) {
        let there = |c: Option<ControlId>| {
            c.and_then(control)
                .is_none_or(|c| self.panel.as_ref().is_some_and(|p| p.rect_of(c).is_some()))
        };
        let (hover, pressed) = (there(self.pointer.hover), there(self.pointer.pressed));
        if !hover {
            self.pointer.hover = None;
        }
        if !pressed {
            self.pointer.pressed = None;
        }
    }
}

/// The name of the one file the inspector shows, which its buttons are
/// named with: empty for several files or none.
pub(in crate::win) fn panel_file<'a>(
    model: &insp::Inspector,
    entries: &'a [model::Entry],
) -> &'a str {
    match model {
        insp::Inspector::Waiting { row, .. }
        | insp::Inspector::Converting { row, .. }
        | insp::Inspector::Done { row, .. }
        | insp::Inspector::Failed { row, .. } => entries.get(*row).map_or("", |e| e.name.as_str()),
        insp::Inspector::Nothing | insp::Inspector::Several { .. } => "",
    }
}

/// The whole path of the folder of the one file the inspector shows: what
/// its Folder link opens. Empty for anything else.
pub(in crate::win) fn panel_folder<'a>(
    model: &insp::Inspector,
    entries: &'a [model::Entry],
) -> &'a str {
    match model {
        insp::Inspector::Waiting { row, .. } => entries.get(*row).map_or("", |e| e.folder.as_str()),
        _ => "",
    }
}

/// The inspector as last built for the selection, while it shows or is
/// folding away.
pub(in crate::win) struct Panel {
    /// Where it was laid out: its whole place at the right of the list's
    /// panel, as it is once it shows in full. The fold slides it across
    /// from there (`App::inspector_shift`).
    pub(in crate::win) rect: layout::Rect,
    /// What it says.
    pub(in crate::win) model: insp::Inspector,
    /// What Nothing says under its title as it was built with: the key
    /// hint.
    nothing: String,
    pub(in crate::win) layout: layout::InspectorLayout,
    /// Its controls that are placed, in reading order, with where each is.
    /// The pointer, Tab, the drawing and the tree all go by these.
    pub(in crate::win) controls: Vec<(InspectorControl, layout::Rect)>,
    /// What it shows, by the files themselves: whether new content is
    /// another page (`new_content`), and whether its rows still hold them.
    shows: Shows,
}

impl Panel {
    /// Whether the rows it draws by their place in the list still hold the
    /// files it shows in `entries`. Once rows came or went above them, it
    /// would show other files.
    pub fn still_listed(&self, entries: &[model::Entry]) -> bool {
        self.shows.still_at(self.pictured(), |i| {
            entries.get(i).map(|e| e.path.as_path())
        })
    }

    /// The rows it pictures at its own size: the one file, or the first
    /// three of several.
    pub fn pictured(&self) -> &[usize] {
        match &self.model {
            insp::Inspector::Nothing => &[],
            insp::Inspector::Waiting { row, .. }
            | insp::Inspector::Converting { row, .. }
            | insp::Inspector::Done { row, .. }
            | insp::Inspector::Failed { row, .. } => std::slice::from_ref(row),
            insp::Inspector::Several { rows, .. } => &rows[..rows.len().min(3)],
        }
    }

    /// Where control `c` is, if it is placed.
    pub fn rect_of(&self, c: InspectorControl) -> Option<layout::Rect> {
        self.controls.iter().find(|(k, _)| *k == c).map(|(_, r)| *r)
    }

    /// The control under a point.
    pub fn control_at(&self, x: f32, y: f32) -> Option<InspectorControl> {
        self.controls
            .iter()
            .find(|(_, r)| r.contains(x, y))
            .map(|(c, _)| *c)
    }
}

/// How `p` shows in a frame: at `alpha`, still `rise` DIPs from its
/// place, its controls lit by the pointer only while `live`.
pub(in crate::win) fn page(p: &Panel, alpha: f32, rise: f32, live: bool) -> InspectorPage<'_> {
    InspectorPage {
        laid: p.rect,
        model: &p.model,
        layout: &p.layout,
        controls: &p.controls,
        nothing: &p.nothing,
        alpha,
        rise,
        live,
    }
}

/// What the inspector is built from: the list's revision, the window's
/// own (`InspectorUi::bump`), the rows' notes' and the window's size in DIPs.
pub(in crate::win) type PanelInputs = (u64, u64, u64, (f32, f32));

impl App {
    // The inspector.

    /// Whether the inspector carries the selection's actions: files are
    /// listed and the fold is heading to show it, however far it has slid
    /// in yet. Only then do its controls take the pointer, Tab and
    /// assistive tools, and only then do rows show no links of their own
    /// (`row_links`). Folding away, it is drawn sliding out but takes
    /// nothing, and UI Automation's tree leaves it out.
    pub(in crate::win) fn inspector_live(&self) -> bool {
        !self.files.is_empty() && self.motion.inspector_staying()
    }

    /// Where the Folder link is, while the inspector carries the actions.
    pub(in crate::win) fn folder_link(&self) -> Option<layout::Rect> {
        self.live_panel()
            .and_then(|p| p.rect_of(InspectorControl::Folder))
    }

    /// The inspector while it carries the actions (`inspector_live`).
    pub(in crate::win) fn live_panel(&self) -> Option<&Panel> {
        self.inspector_ui.panel().filter(|_| self.inspector_live())
    }

    /// The inspector's whole place, as it is once it shows in full, which
    /// its content is laid out on. `None` with no files listed.
    pub(in crate::win) fn full_inspector(&self) -> Option<layout::Rect> {
        if self.files.is_empty() {
            return None;
        }
        self.renderer.layout(1, 1.0).inspector
    }

    /// Where the inspector is drawn at `now`, and how far its content is
    /// moved from where it was laid out: across by the fold's slide (as
    /// far as `fold` has it), and down by new content's rise. `None` while
    /// none of it shows. The pointer goes by it, so it hits what is drawn.
    pub(in crate::win) fn inspector_shift(&self, now: Instant) -> Option<(layout::Rect, f32, f32)> {
        let p = self.inspector_ui.panel()?;
        if self.files.is_empty() {
            return None;
        }
        let drawn = self.renderer.layout(1, self.motion.held_fold()).inspector?;
        let (_, rise) = self.inspector_rise(now);
        Some((drawn, drawn.x - p.rect.x, rise))
    }

    /// The inspector's content's way in at `now`: see
    /// `Motion::inspector_rise`.
    pub(in crate::win) fn inspector_rise(&self, now: Instant) -> (f32, f32) {
        self.motion
            .inspector_rise(now, theme::fluent::motion::animations())
    }

    /// How far what the inspector showed before still shows at `now`: see
    /// `Motion::inspector_fading`. `None` once it has gone, or with
    /// nothing fading.
    pub(in crate::win) fn inspector_fading(&self, now: Instant) -> Option<f32> {
        let (_, from) = self.inspector_ui.prev()?;
        self.motion.inspector_fading(*from, now)
    }

    /// A page fading out draws its rows by their place in the list: once
    /// rows came or went above them, it would show other files, so it goes
    /// at once (`Panel::still_listed`).
    pub(in crate::win) fn drop_moved_page(&mut self) {
        if self.inspector_ui.drop_moved_prev(self.files.entries()) {
            self.invalidate();
        }
    }

    /// Whether `(x, y)` is on the inspector as it is drawn, live or
    /// folding away: no part of the list.
    pub(in crate::win) fn on_inspector(&self, x: f32, y: f32) -> bool {
        self.inspector_shift(Instant::now())
            .is_some_and(|(drawn, ..)| drawn.contains(x, y))
    }

    /// The inspector control drawn at `(x, y)`, while it carries the
    /// actions.
    pub(in crate::win) fn panel_control_at(&self, x: f32, y: f32) -> Option<InspectorControl> {
        let p = self.live_panel()?;
        let (drawn, dx, dy) = self.inspector_shift(Instant::now())?;
        if !drawn.contains(x, y) {
            return None;
        }
        p.control_at(x - dx, y - dy)
    }

    /// Build the inspector again for the selection as it is now, when what
    /// it is built from has changed: the list's revision (the selection, a
    /// row's job, its facts or its results), the window's own (Settings,
    /// a result's size, the Text size) or the window's size.
    /// Otherwise it does nothing at all. Its words are measured only when
    /// they change. A control the keyboard or the pointer was on that has
    /// gone hands the focus back to the list; one still there keeps it.
    /// New content cross-fades with what it showed before
    /// (`new_content`). Run after every message and before every frame.
    pub(in crate::win) fn refresh_panel(&mut self) {
        if self.destroyed {
            return;
        }
        self.drop_moved_page();
        // Minimised, the inspector stays as it was built, so it comes back
        // as it went.
        if self.minimised() {
            return;
        }
        if !self.inspector_live() {
            self.panel_folded();
            return;
        }
        // Nothing it is built from has changed: nothing to do, and nothing
        // allocated to find that out.
        let inputs = (
            self.files.revision(),
            self.inspector_ui.rev(),
            self.notes.revision(),
            self.renderer.size_dips(),
        );
        if self.inspector_ui.built_from(inputs) {
            return;
        }
        let Some(rect) = self.full_inspector() else {
            self.panel_folded();
            return;
        };
        let model = {
            let kiln = &self.kiln;
            let sizes = &self.work.result_sizes;
            let notes = &self.notes;
            let kind = |e: &model::Entry| kind_of(kiln, e);
            let size = |p: &Path| sizes.get(p).copied();
            let note = |p: &Path| notes.get(p).cloned();
            let saving = self.saving_rows();
            let saving_of = |p: &Path| saving.contains(p);
            // Make smaller as the wheel offers it.
            let available = self.available();
            let smaller_of = |e: &model::Entry| {
                crate::wheel::offered(kiln, &[e.path.as_path()], available)
                    .tools
                    .contains(&kiln_core::Tool::Smaller)
            };
            let look = insp::Lookups {
                kind_of: &kind,
                size_of: &size,
                note_of: &note,
                saving_of: &saving_of,
                smaller_of: &smaller_of,
            };
            insp::inspect(&self.files, &look)
        };
        // While a file converts, its time left is worked out again each
        // second, as the row's own line is.
        self.keep_clock(matches!(model, insp::Inspector::Converting { .. }));
        let nothing = crate::inspector::NOTHING_HINT.to_owned();
        let entries = self.files.entries();
        let selected = self.files.selected_rows().iter();
        let shows = Shows::of(
            &model,
            selected.filter_map(|&i| entries.get(i).map(|e| e.path.as_path())),
        );
        // The same words for the same files, the same size: nothing to do.
        if self.inspector_ui.is_page(rect, &nothing, &model, &shows) {
            return;
        }
        let renderer = &self.renderer;
        let measure = |s: &str, how: layout::Wrapped| {
            let width = match how {
                layout::Wrapped::Info(_) => layout::info_text_width(rect.w),
                _ => layout::inspector_text_width(rect.w),
            };
            renderer.inspector_text_height(s, how, width)
        };
        let button_w = |s: &str| renderer.inspector_button_width(s);
        let shape = layout::inspector_shape(
            &model,
            &nothing,
            renderer.inspector_button_width(insp::MORE_FORMATS),
            &measure,
            &button_w,
        );
        // A file's name takes a second line of the title when it needs one.
        let name = model
            .file_row()
            .and_then(|row| entries.get(row))
            .map_or("", |e| e.name.as_str());
        let title = layout::inspector_title_height(&model, name, &measure);
        let il = layout::inspector_layout_titled(&rect, &shape, title);
        let controls = a11y::inspector_controls(&model, &il);
        let fresh = self.inspector_ui.is_fresh(&shows);
        let old = self.inspector_ui.show(Panel {
            rect,
            model,
            nothing,
            layout: il,
            controls,
            shows,
        });
        self.panel_lost();
        // New content rises in as what was there fades out, from as far as
        // it had come in itself. The same files' content changing (their
        // facts, a converting file's figures, a result saved), a row moving
        // in the list, or a new size, is laid out again in place.
        if !fresh {
            self.invalidate();
            return;
        }
        let now = Instant::now();
        let fading = self.inspector_fading(now);
        let how = self
            .motion
            .content_changed(fading, now, theme::fluent::motion::animations());
        self.inspector_ui.fade(old, how, self.files.entries());
        self.animate();
    }

    /// The inspector isn't carrying the actions (`inspector_live`): no
    /// control of it keeps the keyboard, the pointer or a press, and
    /// nothing is built for it. What it showed slides out with the fold
    /// and goes once it is out of sight; at once with no files listed, or
    /// once rows came or went under it (`Panel::still_listed`).
    pub(in crate::win) fn panel_folded(&mut self) {
        self.inspector_ui.folded();
        self.keep_clock(false);
        if let Some(FocusButton::Inspector(_)) = self.button_focus {
            self.button_focus = None;
        }
        let moved = self.inspector_ui.moved(self.files.entries());
        let out = self.files.is_empty() || moved || !self.motion.folding(Instant::now());
        if out && self.inspector_ui.clear() {
            self.invalidate();
        }
    }

    /// Run `CLOCK_TIMER` while `on` and the window is on screen, and only
    /// then.
    pub(in crate::win) fn keep_clock(&mut self, on: bool) {
        if self.inspector_ui.set_clock(on) {
            self.sync_clock(self.on_screen());
        }
    }

    /// Set or clear `CLOCK_TIMER` to match the clock being wanted and the
    /// window being on screen (`visible`): hidden or minimised, no timer
    /// fires. Shown again, the time left is built again.
    pub(in crate::win) fn sync_clock(&mut self, visible: bool) {
        let run = self.inspector_ui.clock() && visible;
        if run == self.clock_timer {
            return;
        }
        self.clock_timer = run;
        unsafe {
            if run {
                SetTimer(Some(self.hwnd), CLOCK_TIMER, CLOCK_MS, None);
            } else {
                let _ = KillTimer(Some(self.hwnd), CLOCK_TIMER);
            }
        }
        if run {
            self.panel_stale();
        }
    }

    /// Something the inspector is built from changed outside the list:
    /// build it again after this message.
    pub(in crate::win) fn panel_stale(&mut self) {
        self.inspector_ui.bump();
    }

    /// The inspector changed or went: a control the keyboard, the pointer
    /// or a press was on that isn't there any more lets go of it, and the
    /// keyboard goes back to the list.
    pub(in crate::win) fn panel_lost(&mut self) {
        let there = |c: InspectorControl| {
            self.inspector_ui
                .panel()
                .is_some_and(|p| p.rect_of(c).is_some())
        };
        if let Some(FocusButton::Inspector(c)) = self.button_focus {
            if !there(c) {
                self.button_focus = None;
            }
        }
        self.inspector_ui.lost();
    }

    /// Do what inspector control `c` is for, from a click, a key
    /// (`keyboard`) or an assistive tool. A format converts the files the
    /// inspector shows; More
    /// opens the format choice for them; a kind narrows the selection to
    /// its files; Get opens the download page of the helper app they wait
    /// on; the buttons do what the row's own would.
    pub(in crate::win) fn panel_act(&mut self, c: InspectorControl, keyboard: bool) {
        use insp::Inspector as I;
        use InspectorControl as C;
        let Some(p) = self.live_panel() else {
            return;
        };
        let at = p.rect_of(c);
        let model = p.model.clone();
        let path = |app: &App, i: usize| app.files.entries().get(i).map(|e| e.path.clone());
        match (c, &model) {
            // A format converts at once, the one file or every
            // selected file, at Settings' quality.
            (C::Target(_), _) => {
                if let Some((ext, picks)) = model.format_pick(c) {
                    let paths = match picks {
                        insp::Picks::Row(row) => path(self, row).into_iter().collect(),
                        insp::Picks::Selection => self.selected_paths(),
                    };
                    self.choose_format(ext, paths);
                }
            }
            // Make smaller runs at once on the file or the selection.
            (C::Smaller, I::Waiting { row, .. }) => {
                if let Some(file) = path(self, *row) {
                    self.make_smaller(vec![file]);
                }
            }
            (C::Smaller, I::Done { row, .. }) => {
                if let Some(file) = path(self, *row) {
                    self.make_smaller(vec![file]);
                }
            }
            (C::Smaller, I::Several { .. }) => {
                let paths = self.selected_paths();
                self.make_smaller(paths);
            }
            // More opens the choice for the file.
            (C::More, I::Waiting { row, .. }) => {
                if let Some(file) = path(self, *row) {
                    let from = at.map_or_else(
                        || self.row_centre(*row),
                        |r| (r.x + r.w / 2.0, r.y + r.h / 2.0),
                    );
                    self.open_choice(WheelKind::Convert, vec![file], from, keyboard);
                }
            }
            // Every selected file, as their shared formats are.
            (C::More, I::Several { rows, .. }) => {
                let from = at
                    .map(|r| (r.x + r.w / 2.0, r.y + r.h / 2.0))
                    .or_else(|| rows.first().map(|&i| self.row_centre(i)));
                if let Some(from) = from {
                    let paths = self.selected_paths();
                    self.open_choice(WheelKind::Convert, paths, from, keyboard);
                }
            }
            (C::Kind(k), I::Several { offered, .. }) => {
                if let Some((code, _)) = offered.get(k) {
                    self.select_kind(code, keyboard);
                }
            }
            // The row's own way to get its helper app. Several files are
            // offered Get only when they all wait on the same one.
            (C::GetHelper, I::Waiting { .. } | I::Several { .. }) => {
                let row = match &model {
                    I::Waiting { row, .. } => Some(*row),
                    I::Several { rows, .. } => rows.first().copied(),
                    _ => None,
                };
                let e = row.and_then(|i| self.files.entries().get(i));
                let backend = e
                    .and_then(|e| {
                        e.waits_on
                            .as_deref()
                            .or(e.needs_helper())
                            .or(e.partly_needs())
                    })
                    .map(str::to_owned);
                let product = e.and_then(|e| e.store_product());
                if let Some(backend) = backend {
                    self.get_helper(&backend);
                } else if let Some(product) = product {
                    self.get_extension(product);
                }
            }

            (C::Options, I::Done { row, .. }) => self.options_row(*row, keyboard),
            (C::ShowInFolder, I::Done { row, .. }) => self.show_row(*row),
            (C::Retry, I::Failed { row, .. }) => self.retry_row(*row),
            (C::ShowDetails, I::Failed { row, .. }) => self.show_details(*row),
            (C::Open, I::Done { row, .. }) => self.open_result(*row),
            // The folder the file is in, with the file selected in it.
            (C::Folder, I::Waiting { row, .. }) => self.show_row(*row),
            _ => {}
        }
        self.invalidate();
    }

    /// Convert `paths` to `ext` at once, as a format in the side pane
    /// does, unless one of them is converting already: then the
    /// footer says so, as the wheel would.
    pub(in crate::win) fn choose_format(&mut self, ext: &str, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        match self.files.convert_busy(&paths) {
            Some(why) => self.alert(why.to_owned()),
            None => self.start(Action::Convert(ext.to_owned()), paths, None),
        }
    }

    /// Make `paths` smaller at once, as the side pane's and the row
    /// menu's Make smaller do, when they share a kind and none of them is
    /// converting.
    pub(in crate::win) fn make_smaller(&mut self, paths: Vec<PathBuf>) {
        let Some(kind) = crate::wheel::source_kind(&self.kiln, &paths) else {
            return;
        };
        match self.files.convert_busy(&paths) {
            Some(why) => self.alert(why.to_owned()),
            None => self.start(Action::Tool(Tool::Smaller, kind), paths, None),
        }
    }

    /// Narrow the selection to its files of type `code`, as the inspector
    /// offers when several files share no format.
    pub(in crate::win) fn select_kind(&mut self, code: &str, keyboard: bool) {
        let entries = self.files.entries();
        let rows: Vec<usize> = self
            .files
            .selection()
            .into_iter()
            .filter(|&i| entries.get(i).is_some_and(|e| insp::type_code(e) == code))
            .collect();
        let Some(&first) = rows.first() else {
            return;
        };
        self.files.select_only(&rows);
        if keyboard {
            self.files.keyboard_used();
        }
        self.scroll_to_row(first);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page for `model` over the files at `paths`, laid out in a
    /// 300 x 500 place with every text a line tall.
    fn page_of(model: insp::Inspector, paths: &[&Path]) -> Panel {
        let rect = layout::Rect::new(0.0, 0.0, 300.0, 500.0);
        let nothing = "keys".to_owned();
        let shape = layout::inspector_shape(&model, &nothing, 40.0, &|_, _| 20.0, &|_| 80.0);
        let laid = layout::inspector_layout(&rect, &shape);
        let controls = a11y::inspector_controls(&model, &laid);
        let shows = Shows::of(&model, paths.iter().copied());
        Panel {
            rect,
            model,
            nothing,
            layout: laid,
            controls,
            shows,
        }
    }

    fn failed(row: usize) -> insp::Inspector {
        insp::Inspector::Failed {
            row,
            title: "Couldn't convert this file".to_owned(),
            sentence: "It did not convert".to_owned(),
            tone: model::Tone::Crack,
            retry: true,
            details: false,
            cancelled: false,
        }
    }

    fn converting(row: usize, percent: u32) -> insp::Inspector {
        insp::Inspector::Converting {
            row,
            doing: "Converting to WEBP".to_owned(),
            percent: Some(percent),
            left: Some("About 1 min left".to_owned()),
        }
    }

    #[test]
    fn a_page_replaces_the_one_before_it() {
        let mut ui = InspectorUi::default();
        assert!(ui.panel().is_none());
        let a = Path::new("a.png");
        assert!(ui.show(page_of(failed(0), &[a])).is_none());
        let old = ui.show(page_of(converting(0, 10), &[a])).unwrap();
        assert!(matches!(old.model, insp::Inspector::Failed { .. }));
        assert!(matches!(
            ui.panel().unwrap().model,
            insp::Inspector::Converting { .. }
        ));
    }

    #[test]
    fn the_same_words_for_the_same_files_are_the_page_already() {
        let mut ui = InspectorUi::default();
        let a = Path::new("a.png");
        let b = Path::new("b.png");
        ui.show(page_of(converting(0, 10), &[a]));
        let rect = layout::Rect::new(0.0, 0.0, 300.0, 500.0);
        let same = converting(0, 10);
        let shows = Shows::of(&same, [a]);
        assert!(ui.is_page(rect, "keys", &same, &shows));
        // Other figures, other files, other hint or another place are not.
        let more = converting(0, 11);
        assert!(!ui.is_page(rect, "keys", &more, &Shows::of(&more, [a])));
        assert!(!ui.is_page(rect, "keys", &same, &Shows::of(&same, [b])));
        assert!(!ui.is_page(rect, "other", &same, &shows));
        let wider = layout::Rect::new(0.0, 0.0, 320.0, 500.0);
        assert!(!ui.is_page(wider, "keys", &same, &shows));
    }

    #[test]
    fn only_another_page_for_the_files_is_new_content() {
        let mut ui = InspectorUi::default();
        let a = Path::new("a.png");
        let b = Path::new("b.png");
        // Nothing showing: whatever comes is new.
        assert!(ui.is_fresh(&Shows::of(&failed(0), [a])));
        ui.show(page_of(converting(0, 10), &[a]));
        // The same file converting further is laid out again in place.
        let later = converting(0, 50);
        assert!(!ui.is_fresh(&Shows::of(&later, [a])));
        // The same file failed is another page, and so is another file.
        assert!(ui.is_fresh(&Shows::of(&failed(0), [a])));
        assert!(ui.is_fresh(&Shows::of(&later, [b])));
    }

    #[test]
    fn what_fades_out_is_kept_only_while_its_rows_hold_its_files() {
        let (_dir, files) = super::super::listed(2);
        let entries = files.entries();
        let (a, b) = (entries[0].path.as_path(), entries[1].path.as_path());
        let mut ui = InspectorUi::default();
        ui.show(page_of(failed(0), &[a]));
        let old = ui.show(page_of(converting(1, 0), &[b]));
        // The page before fades out from where it had come to.
        ui.fade(old, Some((Fading::Now, 0.5)), entries);
        let (before, from) = ui.prev().unwrap();
        assert!(matches!(before.model, insp::Inspector::Failed { .. }));
        assert_eq!(*from, 0.5);
        // The fade that was running carries on from where it is.
        let old = ui.show(page_of(failed(1), &[b]));
        ui.fade(old, Some((Fading::Before, 0.25)), entries);
        assert_eq!(ui.prev().unwrap().1, 0.25);
        assert!(matches!(
            ui.prev().unwrap().0.model,
            insp::Inspector::Failed { .. }
        ));
        // With no cross-fade there is nothing left to fade.
        let old = ui.show(page_of(converting(0, 5), &[a]));
        ui.fade(old, None, entries);
        assert!(ui.prev().is_none());
        // Rows that moved under the old page: it goes at once.
        ui.show(page_of(failed(0), &[b]));
        let old = ui.show(page_of(converting(0, 7), &[a]));
        ui.fade(old, Some((Fading::Now, 1.0)), entries);
        assert!(ui.prev().is_none());
    }

    #[test]
    fn a_page_showing_goes_stale_when_rows_move_under_it() {
        let (_dir, files) = super::super::listed(2);
        let entries = files.entries();
        let (a, b) = (entries[0].path.as_path(), entries[1].path.as_path());
        let mut ui = InspectorUi::default();
        ui.show(page_of(failed(0), &[a]));
        assert!(!ui.moved(entries));
        // The page is about a file that row 0 holds no more.
        ui.show(page_of(failed(0), &[b]));
        assert!(ui.moved(entries));
    }

    #[test]
    fn what_fades_out_goes_when_rows_move_under_it() {
        let (_dir, files) = super::super::listed(2);
        let entries = files.entries();
        let b = entries[1].path.as_path();
        let mut ui = InspectorUi::default();
        ui.show(page_of(failed(1), &[b]));
        let old = ui.show(page_of(converting(1, 0), &[b]));
        ui.fade(old, Some((Fading::Now, 1.0)), entries);
        assert!(ui.prev().is_some());
        assert!(!ui.drop_moved_prev(entries));
        // Other files in the same rows: the old page would show those.
        let (_other, moved) = super::super::listed(2);
        assert!(ui.drop_moved_prev(moved.entries()));
        assert!(ui.prev().is_none());
    }

    #[test]
    fn folding_clears_the_pointer_and_what_it_was_built_from() {
        let mut ui = InspectorUi::default();
        let a = Path::new("a.png");
        ui.show(page_of(failed(0), &[a]));
        assert!(!ui.built_from((1, 0, 0, (10.0, 10.0))));
        assert!(ui.built_from((1, 0, 0, (10.0, 10.0))));
        assert!(!ui.built_from((2, 0, 0, (10.0, 10.0))));
        ui.point(Some(InspectorControl::Retry));
        ui.press(InspectorControl::Retry);
        ui.folded();
        assert_eq!((ui.hover(), ui.pressed()), (None, None));
        assert!(!ui.built_from((2, 0, 0, (10.0, 10.0))));
        assert!(ui.clear());
        assert!(ui.panel().is_none());
        assert!(!ui.clear());
    }

    #[test]
    fn the_window_changing_what_it_is_built_from_bumps_its_revision() {
        let mut ui = InspectorUi::default();
        let before = ui.rev();
        ui.bump();
        assert_ne!(ui.rev(), before);
    }

    #[test]
    fn the_clock_runs_once_and_stops_once() {
        let mut ui = InspectorUi::default();
        assert!(!ui.set_clock(false));
        assert!(ui.set_clock(true));
        assert!(!ui.set_clock(true));
        assert!(ui.set_clock(false));
    }

    #[test]
    fn the_pointer_lights_a_control_and_a_press_acts_on_release() {
        let mut ui = InspectorUi::default();
        ui.show(page_of(failed(0), &[Path::new("a.png")]));
        let retry = InspectorControl::Retry;
        assert!(ui.point(Some(retry)));
        assert!(!ui.point(Some(retry)), "no change, nothing to draw again");
        assert_eq!(ui.hover(), Some(retry));
        ui.press(retry);
        assert_eq!(ui.pressed(), Some(retry));
        // The release hands the press over once.
        assert_eq!(ui.release(), Some(retry));
        assert_eq!(ui.release(), None);
        assert!(ui.point(None));
        assert_eq!(ui.hover(), None);
    }

    #[test]
    fn a_control_the_new_page_does_not_place_is_let_go() {
        let mut ui = InspectorUi::default();
        let a = Path::new("a.png");
        ui.show(page_of(failed(0), &[a]));
        ui.point(Some(InspectorControl::Retry));
        ui.press(InspectorControl::Retry);
        // Converting has no Retry.
        ui.show(page_of(converting(0, 10), &[a]));
        ui.lost();
        assert_eq!((ui.hover(), ui.pressed()), (None, None));
        // One it does place is kept.
        ui.show(page_of(failed(0), &[a]));
        ui.point(Some(InspectorControl::Retry));
        ui.press(InspectorControl::Retry);
        ui.lost();
        assert_eq!(ui.hover(), Some(InspectorControl::Retry));
        assert_eq!(ui.pressed(), Some(InspectorControl::Retry));
    }

    #[test]
    fn the_pointer_leaving_lets_go_of_everything() {
        let mut ui = InspectorUi::default();
        ui.show(page_of(failed(0), &[Path::new("a.png")]));
        ui.point(Some(InspectorControl::Retry));
        ui.press(InspectorControl::Retry);
        ui.leave();
        assert_eq!((ui.hover(), ui.pressed()), (None, None));
    }

    #[test]
    fn a_page_pictures_its_rows_at_their_own_size() {
        let mut ui = InspectorUi::default();
        assert!(ui.pictured().is_empty());
        let a = Path::new("a.png");
        ui.show(page_of(failed(4), &[a]));
        assert_eq!(ui.pictured(), [4]);
        ui.show(page_of(insp::Inspector::Nothing, &[]));
        assert!(ui.pictured().is_empty());
    }

    #[test]
    fn a_control_is_found_by_where_the_page_placed_it() {
        let p = page_of(failed(0), &[Path::new("a.png")]);
        let r = p.rect_of(InspectorControl::Retry).unwrap();
        assert_eq!(
            p.control_at(r.x + r.w / 2.0, r.y + r.h / 2.0),
            Some(InspectorControl::Retry)
        );
        assert_eq!(p.control_at(-5.0, -5.0), None);
        assert!(p.rect_of(InspectorControl::ShowInFolder).is_none());
    }
}
