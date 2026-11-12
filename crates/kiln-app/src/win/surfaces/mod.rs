//! The overlays over the file list, and the one place that says which of
//! them gets an input first.
//!
//! Every input path asks `App::dispatch`: mouse down, up and move, a key,
//! the wheel and the pointer leaving the window. It offers the input to
//! each open layer from the top down, and the layer answers with a
//! `Reply`. The order is fixed: one of Kiln's prompts, a sheet, Settings,
//! the details dialog, Help, a wheel, then the list.

use super::*;

mod details;
mod dialog;
mod help;
mod inspector;
mod list;
mod menu;
mod settings;
mod sheet;
mod smaller;
mod wheel;

pub(super) use details::Details;
pub(super) use dialog::Ask;
pub(super) use help::Help;
pub(super) use inspector::{page, panel_file, panel_folder, InspectorUi};
pub(super) use list::{ListUi, WindowButton};
pub(super) use menu::{menu_rows, MenuDid, RowMenu};
pub(super) use settings::SettingsPanel;
pub(super) use sheet::{look, CardsFound, Opening, Probed, Sheet, SheetFrame};
pub(super) use smaller::{Stop as ToolStop, ToolPart, ToolUi, ToolView};
pub(super) use wheel::{Opening as WheelOpening, Tick, WheelUi};

/// The surfaces that can be over the list, from the top down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Layer {
    /// The right-click menu, over everything while it is up.
    Menu,
    /// One of Kiln's prompts (`Effect::Ask`), over everything.
    Dialog,
    /// The Make smaller or Resize dialog.
    Tool,
    Sheet,
    Settings,
    Details,
    Help,
    Wheel,
    /// The file list and its buttons: always there, always last.
    List,
}

/// What a layer did with an input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Reply {
    /// The layer acted on it. Nothing below sees it.
    Taken,
    /// The layer has no use for it. The next layer down is offered it.
    Pass,
    /// The layer is modal: it does nothing with the input, and nothing
    /// below sees it either.
    Block,
}

/// One input, as the router offers it.
#[derive(Debug, Clone, Copy)]
pub(super) enum Input {
    /// The left button went down at `(x, y)` in layout units, with the
    /// message's MK_ flags.
    Down { x: f32, y: f32, keys: u32 },
    /// The left button came up; `rel` is what each layer had pressed.
    Up { x: f32, y: f32, rel: Released },
    /// The pointer moved.
    Move { x: f32, y: f32, keys: u32 },
    /// A key, with the modifiers held.
    Key {
        key: VIRTUAL_KEY,
        shift: bool,
        ctrl: bool,
        alt: bool,
    },
    /// The wheel turned by this many notches.
    Scroll(f32),
    /// The pointer left the window. Every layer hears it.
    Leave,
}

/// What each layer had pressed when the button came up, taken from them
/// all before any acts.
#[derive(Debug, Clone, Copy)]
pub(super) struct Released {
    /// The header or footer button the list had pressed.
    pub button: Option<WindowButton>,
    /// The inspector control it had pressed.
    pub panel: Option<InspectorControl>,
}

/// Which surfaces are open.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Open {
    pub menu: bool,
    pub dialog: bool,
    pub tool: bool,
    pub sheet: bool,
    pub settings: bool,
    pub details: bool,
    pub help: bool,
    pub wheel: bool,
}

impl Open {
    /// The open layers, top-down, ending with the list.
    pub fn stack(self) -> Vec<Layer> {
        [
            (self.menu, Layer::Menu),
            (self.dialog, Layer::Dialog),
            (self.tool, Layer::Tool),
            (self.sheet, Layer::Sheet),
            (self.settings, Layer::Settings),
            (self.details, Layer::Details),
            (self.help, Layer::Help),
            (self.wheel, Layer::Wheel),
        ]
        .into_iter()
        .filter_map(|(open, layer)| open.then_some(layer))
        .chain([Layer::List])
        .collect()
    }
}

/// A control a pointer can be over or pressing, on any surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ControlId {
    /// A button of one of Kiln's prompts, by its place.
    Ask(usize),
    /// A part of the Make smaller or Resize dialog.
    Tool(ToolPart),
    /// The details dialog's Copy or Close.
    Dialog(DialogButton),
    /// Help's Show first-run help or Close.
    Help(HelpButton),
    /// A settings control, by its place among the layout's controls.
    Setting(usize),
    /// An options sheet control, by its place among the layout's controls.
    Sheet(usize),
    /// A slice or More on the wheel.
    Wheel(Spot),
    /// A control of the inspector.
    Inspector(InspectorControl),
    /// A button in the window's header or footer.
    Button(WindowButton),
}

/// Where the pointer is on a surface: the control it is over, and the one
/// it went down on, which acts on release.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Pointer {
    pub hover: Option<ControlId>,
    pub pressed: Option<ControlId>,
}

impl Pointer {
    /// The pointer is gone: nothing is lit or held.
    pub fn clear(&mut self) {
        *self = Pointer::default();
    }
}

/// Text sizes, as the renderer measures them, so a surface can work out
/// how far it scrolls without a window.
pub(super) trait Measure {
    /// How tall `text` runs set as engine output, wrapped to `width`.
    fn mono_height(&self, text: &str, width: f32) -> f32;
    /// How tall `text` runs set as body text, wrapped to `width`.
    fn body_height(&self, text: &str, width: f32) -> f32;
    /// How tall a ContentDialog's title (Subtitle) and text (Body) run,
    /// wrapped to `width`. Without a renderer, one line each.
    fn dialog_heights(&self, _title: &str, text: Option<&str>, _width: f32) -> (f32, f32) {
        use crate::theme::fluent::type_ramp::{BODY, SUBTITLE};
        (SUBTITLE.line, text.map_or(0.0, |_| BODY.line))
    }
    /// How wide a button's label runs in Body. Without a renderer, 7
    /// units a letter.
    fn label_width(&self, label: &str) -> f32 {
        label.chars().count() as f32 * 7.0
    }
    /// How wide `s` runs on one line in Body (Caption when `caption`).
    /// Without a renderer, `layout::Estimate`'s.
    fn fit_width(&self, s: &str, caption: bool) -> f32 {
        layout::Fit::width(&layout::Estimate, s, caption)
    }
    /// How tall `s` runs wrapped to `width` in Body (Caption when
    /// `caption`). Without a renderer, `layout::Estimate`'s.
    fn fit_height(&self, s: &str, caption: bool, width: f32) -> f32 {
        layout::Fit::height(&layout::Estimate, s, caption, width)
    }
}

impl Measure for Renderer {
    fn mono_height(&self, text: &str, width: f32) -> f32 {
        self.details_height(text, width)
    }

    fn body_height(&self, text: &str, width: f32) -> f32 {
        self.help_line_height(text, width)
    }

    fn dialog_heights(&self, title: &str, text: Option<&str>, width: f32) -> (f32, f32) {
        self.dialog_text_heights(title, text, width)
    }

    fn label_width(&self, label: &str) -> f32 {
        self.body_label_width(label)
    }

    fn fit_width(&self, s: &str, caption: bool) -> f32 {
        self.run_width(s, caption)
    }

    fn fit_height(&self, s: &str, caption: bool, width: f32) -> f32 {
        self.wrapped_height(s, caption, width)
    }
}

/// What a surface asks of `App` once it has answered: the router carries
/// it out after the surface is let go.
#[derive(Default)]
pub(super) struct Out {
    /// The window needs drawing again.
    pub invalidate: bool,
    /// Effects to queue, in order.
    pub effects: Vec<Effect>,
    /// The surface closes.
    pub close: bool,
    /// What happens once it has closed.
    pub follow: Option<Follow>,
    /// Settings controls pressed, each with the step it takes, for `App`
    /// to carry out: they change a setting.
    pub changes: Vec<(Control, crate::settings_page::Set)>,
    /// Something on the surface starts to move: the animation timer runs.
    pub animate: bool,
    /// A job to start, as the options sheet's Convert asks.
    pub start: Option<Start>,
    /// One of Kiln's prompts was answered with this button: it closes,
    /// and what asked it hears the answer (`App::answer_ask`).
    pub answer: Option<crate::model::PromptButton>,
    /// A tool to start, as Make smaller's or Resize's dialog asks.
    pub tool_start: Option<ToolStart>,
}

/// A tool the Make smaller or Resize dialog starts once it has closed.
#[derive(Debug)]
pub(super) struct ToolStart {
    pub action: Action,
    pub paths: Vec<PathBuf>,
    pub tool: kiln_core::ToolOptions,
    pub via: Via,
}

/// A conversion the options sheet starts once it has closed.
pub(super) struct Start {
    pub action: Action,
    pub paths: Vec<PathBuf>,
    pub quality: Quality,
    /// Whether it came from Options picked on the drag-wheel.
    pub via: Via,
    /// Redo a finished drag-wheel row instead of starting a new job.
    pub redo: bool,
}

/// What `App` does after a surface closes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Follow {
    /// Bring the first-run card back, as Help's Show first-run help does.
    ShowFirstRun,
    /// Open help, as F1 does on a wheel.
    Help,
}

impl Out {
    /// Carry it out on `app`; `close` takes the surface away.
    fn apply(self, app: &mut App, close: impl FnOnce(&mut App)) {
        for effect in self.effects {
            app.effect(effect);
        }
        for (c, set) in self.changes {
            app.apply_setting(c, set);
        }
        if self.close {
            close(app);
        }
        match self.follow {
            Some(Follow::ShowFirstRun) => app.show_first_run(),
            Some(Follow::Help) => app.open_help(),
            None => {}
        }
        if let Some(b) = self.answer {
            app.answer_ask(b);
        }
        if let Some(t) = self.tool_start {
            app.start_tool(t.action, t.paths, t.tool, t.via);
        }
        if let Some(s) = self.start {
            if s.redo {
                app.redo(s.action, s.paths, s.quality);
            } else {
                app.start_as(s.action, s.paths, Some(s.quality), s.via);
            }
        }
        if self.animate {
            app.animate();
        }
        if self.invalidate {
            app.invalidate();
        }
    }
}

/// What `App` works out once for a frame or a tree that the surfaces which
/// draw it read: the settings and the options sheet's layout and words.
/// It is apart from `Ctx` so that a view can hold it while the
/// renderer, which `Ctx` borrows, draws.
#[derive(Default, Clone, Copy)]
pub(super) struct Frame<'a> {
    /// The options sheet's layout and words.
    pub sheet: Option<&'a SheetFrame>,
}

/// What a surface reads from `App` while it answers an input: the window's
/// size in layout units, the renderer's text measures, the palette, the
/// time, the settings.
pub(super) struct Ctx<'a> {
    /// The client area in layout units.
    pub size: (f32, f32),
    pub measure: &'a dyn Measure,
    pub now: Instant,
    pub settings: &'a Settings,
    pub animations: bool,
    /// Whether Windows refused the drag-wheel's hooks.
    pub hooks_refused: bool,
    pub out: Out,
}

impl<'a> Ctx<'a> {
    pub fn new(measure: &'a dyn Measure, settings: &'a Settings, size: (f32, f32)) -> Self {
        Ctx {
            size,
            measure,
            now: Instant::now(),
            settings,
            animations: theme::fluent::motion::animations(),
            hooks_refused: false,
            out: Out::default(),
        }
    }

    /// With whether Windows refused the drag-wheel's hooks.
    pub fn hooks_refused(mut self, refused: bool) -> Self {
        self.hooks_refused = refused;
        self
    }
}

/// Modifier keys held with a key, as the surfaces read them. Alt reaches
/// only the list (`App::list_key`).
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    /// The key is held and Windows repeats it (bit 30 of `WM_KEYDOWN`'s
    /// lParam): a prompt's Enter and Space wait for a fresh press.
    pub repeat: bool,
}

impl Mods {
    /// Shift and Ctrl as held, and whether Windows repeats the key.
    pub fn with_repeat(shift: bool, ctrl: bool, repeat: bool) -> Self {
        Mods {
            shift,
            ctrl,
            repeat,
        }
    }

    /// Shift and Ctrl as held, on a fresh press.
    #[cfg(test)]
    pub fn held(shift: bool, ctrl: bool) -> Self {
        Mods {
            shift,
            ctrl,
            repeat: false,
        }
    }
}

/// One surface over the file list. It answers each input with a `Reply`
/// and leaves what `App` must do in `cx.out`.
pub(super) trait Surface {
    /// What it hands the renderer.
    type View<'a>
    where
        Self: 'a;
    /// What it hands UI Automation.
    type A11y<'a>
    where
        Self: 'a;

    /// The control under a point.
    fn hit(&self, cx: &Ctx, x: f32, y: f32) -> Option<ControlId>;
    /// The pointer moved over the surface.
    fn hover(&mut self, cx: &mut Ctx, x: f32, y: f32) -> Reply;
    /// The button went down.
    fn press(&mut self, cx: &mut Ctx, x: f32, y: f32) -> Reply;
    /// The button came up: what went down acts if the pointer is still on it.
    fn release(&mut self, cx: &mut Ctx, x: f32, y: f32) -> Reply;
    /// A key.
    fn key(&mut self, cx: &mut Ctx, key: VIRTUAL_KEY, mods: Mods) -> Reply;
    /// Its text scrolls.
    fn scroll(&mut self, cx: &mut Ctx, by: TextScroll) -> Reply;
    /// The pointer left the window.
    fn leave(&mut self);
    /// What the renderer draws this frame. `button` is each button's hover
    /// and press as they ease.
    fn view<'a>(
        &'a self,
        cx: &Ctx,
        frame: Frame<'a>,
        button: &'a dyn Fn(ButtonId) -> (f32, f32),
    ) -> Self::View<'a>;
    /// What UI Automation reads.
    fn a11y<'a>(&'a self, cx: &Ctx, frame: Frame<'a>) -> Self::A11y<'a>;
}

/// The context for a surface that is taken from `App` to answer: built from
/// the fields it reads, so `App`'s surface stays free to borrow.
fn cx_of<'a>(renderer: &'a Renderer, settings: &'a Settings, hooks_refused: bool) -> Ctx<'a> {
    Ctx::new(renderer, settings, renderer.size_dips()).hooks_refused(hooks_refused)
}

/// Offer `input` to each of `layers` from the top down, as `route` has
/// each answer it, until one takes it or blocks it. A pointer leaving is
/// told to all of them.
fn offer(layers: &[Layer], input: Input, mut route: impl FnMut(Layer, Input) -> Reply) -> Reply {
    if matches!(input, Input::Leave) {
        for &layer in layers {
            route(layer, input);
        }
        return Reply::Pass;
    }
    for &layer in layers {
        match route(layer, input) {
            Reply::Pass => {}
            reply => return reply,
        }
    }
    Reply::Pass
}

impl App {
    /// The open layers, top-down, ending with the list.
    pub(super) fn layers(&self) -> Vec<Layer> {
        Open {
            menu: self.row_menu.is_some(),
            dialog: self.prompts.ask_ui.is_some(),
            tool: self.prompts.tool_ui.is_some(),
            sheet: self.sheet.is_some(),
            settings: self.settings_ui.is_some(),
            details: self.details.is_some(),
            help: self.help.is_some(),
            wheel: self.wheel.is_some(),
        }
        .stack()
    }

    /// The context a surface reads from, for a view or a tree.
    pub(super) fn surface_cx(&self) -> Ctx<'_> {
        Ctx::new(
            &self.renderer,
            &self.prefs.settings,
            self.renderer.size_dips(),
        )
        .hooks_refused(self.hooks_refused)
    }

    /// Run `f` on the options sheet, if it is open, and carry out what it
    /// asked for. It fades out by itself (`App::tick` drops it), so
    /// nothing here takes it away.
    pub(super) fn drive_sheet<R>(
        &mut self,
        f: impl FnOnce(&mut Sheet, &mut Ctx) -> R,
    ) -> Option<R> {
        let mut cx = cx_of(&self.renderer, &self.prefs.settings, self.hooks_refused);
        let surface = self.sheet.as_mut()?;
        let reply = f(surface, &mut cx);
        let out = cx.out;
        out.apply(self, |_| {});
        Some(reply)
    }

    /// Run `f` on the open prompt, if there is one, and carry out what it
    /// asked for; an answer closes it.
    pub(super) fn drive_ask<R>(&mut self, f: impl FnOnce(&mut Ask, &mut Ctx) -> R) -> Option<R> {
        let mut cx = cx_of(&self.renderer, &self.prefs.settings, self.hooks_refused);
        let surface = self.prompts.ask_ui.as_mut()?;
        let reply = f(surface, &mut cx);
        let out = cx.out;
        out.apply(self, |_| {});
        Some(reply)
    }

    /// Run `f` on the Make smaller or Resize dialog, if it is open, and
    /// carry out what it asked for; Cancel or its accent button closes it.
    /// An estimate it now wants is asked for.
    pub(super) fn drive_tool<R>(
        &mut self,
        f: impl FnOnce(&mut ToolUi, &mut Ctx) -> R,
    ) -> Option<R> {
        let mut cx = cx_of(&self.renderer, &self.prefs.settings, self.hooks_refused);
        let surface = self.prompts.tool_ui.as_mut()?;
        let reply = f(surface, &mut cx);
        let out = cx.out;
        out.apply(self, |app| app.close_tool());
        self.ask_estimate();
        Some(reply)
    }

    /// Run `f` on the details dialog, if it is open, and carry out what it
    /// asked for.
    pub(super) fn drive_details<R>(
        &mut self,
        f: impl FnOnce(&mut Details, &mut Ctx) -> R,
    ) -> Option<R> {
        let mut cx = cx_of(&self.renderer, &self.prefs.settings, self.hooks_refused);
        let surface = self.details.as_mut()?;
        let reply = f(surface, &mut cx);
        let out = cx.out;
        out.apply(self, |app| app.details = None);
        Some(reply)
    }

    /// Run `f` on the help overlay, if it is open, and carry out what it
    /// asked for.
    pub(super) fn drive_help<R>(&mut self, f: impl FnOnce(&mut Help, &mut Ctx) -> R) -> Option<R> {
        let mut cx = cx_of(&self.renderer, &self.prefs.settings, self.hooks_refused);
        let surface = self.help.as_mut()?;
        let reply = f(surface, &mut cx);
        let out = cx.out;
        out.apply(self, |app| app.help = None);
        Some(reply)
    }

    /// Run `f` on the settings panel, if it is open, and carry out what
    /// it asked for.
    pub(super) fn drive_settings<R>(
        &mut self,
        f: impl FnOnce(&mut SettingsPanel, &mut Ctx) -> R,
    ) -> Option<R> {
        let mut cx = cx_of(&self.renderer, &self.prefs.settings, self.hooks_refused);
        let surface = self.settings_ui.as_mut()?;
        let reply = f(surface, &mut cx);
        let out = cx.out;
        out.apply(self, |app| app.close_settings());
        Some(reply)
    }

    /// Run `f` on the wheel, if it is open, and carry out what it asked
    /// for. It fades out by itself (`App::tick` drops it), so nothing here
    /// takes it away.
    pub(super) fn drive_wheel<R>(
        &mut self,
        f: impl FnOnce(&mut WheelUi, &mut Ctx) -> R,
    ) -> Option<R> {
        let mut cx = cx_of(&self.renderer, &self.prefs.settings, self.hooks_refused);
        let surface = self.wheel.as_mut()?;
        let reply = f(surface, &mut cx);
        let out = cx.out;
        out.apply(self, |_| {});
        Some(reply)
    }

    /// Whether a sheet, Settings or the details dialog is up: none of
    /// them lets help open over it.
    pub(super) fn dialog_up(&self) -> bool {
        self.layers().iter().any(|l| {
            matches!(
                l,
                Layer::Dialog | Layer::Tool | Layer::Sheet | Layer::Settings | Layer::Details
            )
        })
    }

    /// Offer `input` to each open layer from the top down, until one takes
    /// it or blocks it. A pointer leaving is told to all of them.
    pub(super) fn dispatch(&mut self, input: Input) -> Reply {
        let layers = self.layers();
        offer(&layers, input, |layer, input| self.route(layer, input))
    }

    /// What `layer` makes of `input`.
    fn route(&mut self, layer: Layer, input: Input) -> Reply {
        match (layer, input) {
            // The right-click menu takes every input while it is up: a
            // press off it closes it and goes no further.
            (Layer::Menu, Input::Down { x, y, .. }) => self.menu_down(x, y),
            (Layer::Menu, Input::Up { x, y, .. }) => self.menu_up(x, y),
            (Layer::Menu, Input::Move { x, y, .. }) => self.menu_move(x, y),
            (Layer::Menu, Input::Key { key, .. }) => self.menu_key(key),
            (Layer::Menu, Input::Scroll(_)) => {
                self.close_menu();
                Reply::Block
            }
            (Layer::Menu, Input::Leave) => {
                if let Some(m) = &mut self.row_menu {
                    if !m.keyboard && m.hover(-1.0, -1.0) == MenuDid::Lit {
                        self.invalidate();
                    }
                }
                Reply::Pass
            }
            (Layer::Dialog, Input::Down { x, y, .. }) => self
                .drive_ask(|s, cx| s.press(cx, x, y))
                .unwrap_or(Reply::Pass),
            (Layer::Dialog, Input::Up { x, y, .. }) => self
                .drive_ask(|s, cx| s.release(cx, x, y))
                .unwrap_or(Reply::Pass),
            (Layer::Dialog, Input::Move { x, y, .. }) => self
                .drive_ask(|s, cx| s.hover(cx, x, y))
                .unwrap_or(Reply::Pass),
            (
                Layer::Dialog,
                Input::Key {
                    key, shift, ctrl, ..
                },
            ) => {
                let mods = Mods {
                    shift,
                    ctrl,
                    repeat: self.key_repeat,
                };
                self.drive_ask(|s, cx| s.key(cx, key, mods))
                    .unwrap_or(Reply::Pass)
            }
            (Layer::Dialog, Input::Scroll(n)) => self
                .drive_ask(|s, cx| s.scroll(cx, TextScroll::Wheel(n)))
                .unwrap_or(Reply::Pass),
            (Layer::Dialog, Input::Leave) => {
                if let Some(a) = &mut self.prompts.ask_ui {
                    a.leave();
                }
                Reply::Pass
            }
            (Layer::Tool, Input::Down { x, y, .. }) => self
                .drive_tool(|s, cx| s.press(cx, x, y))
                .unwrap_or(Reply::Pass),
            (Layer::Tool, Input::Up { x, y, .. }) => self
                .drive_tool(|s, cx| s.release(cx, x, y))
                .unwrap_or(Reply::Pass),
            (Layer::Tool, Input::Move { x, y, .. }) => self
                .drive_tool(|s, cx| s.hover(cx, x, y))
                .unwrap_or(Reply::Pass),
            (
                Layer::Tool,
                Input::Key {
                    key, shift, ctrl, ..
                },
            ) => {
                let mods = Mods {
                    shift,
                    ctrl,
                    repeat: self.key_repeat,
                };
                // Ctrl+V on Custom's box pastes the clipboard's text.
                let on_custom = self
                    .prompts
                    .tool_ui
                    .as_ref()
                    .is_some_and(|t| t.focus() == crate::smaller::Stop::Custom);
                if ctrl && key == VIRTUAL_KEY(b'V' as u16) && on_custom {
                    let text = system::clipboard_text(self.hwnd);
                    return self
                        .drive_tool(|s, cx| s.paste(cx, &text))
                        .map_or(Reply::Pass, |_| Reply::Taken);
                }
                self.drive_tool(|s, cx| s.key(cx, key, mods))
                    .unwrap_or(Reply::Pass)
            }
            (Layer::Tool, Input::Scroll(n)) => self
                .drive_tool(|s, cx| s.scroll(cx, TextScroll::Wheel(n)))
                .unwrap_or(Reply::Pass),
            (Layer::Tool, Input::Leave) => {
                if let Some(t) = &mut self.prompts.tool_ui {
                    t.leave();
                }
                Reply::Pass
            }
            (Layer::Sheet, Input::Down { x, y, .. }) => self
                .drive_sheet(|s, cx| s.press(cx, x, y))
                .unwrap_or(Reply::Pass),
            (Layer::Settings, Input::Down { x, y, .. }) => self
                .drive_settings(|s, cx| s.press(cx, x, y))
                .unwrap_or(Reply::Pass),
            (Layer::Details, Input::Down { x, y, .. }) => self
                .drive_details(|s, cx| s.press(cx, x, y))
                .unwrap_or(Reply::Pass),
            (Layer::Help, Input::Down { x, y, .. }) => self
                .drive_help(|s, cx| s.press(cx, x, y))
                .unwrap_or(Reply::Pass),
            (Layer::Wheel, Input::Down { x, y, .. }) => self
                .drive_wheel(|s, cx| s.press(cx, x, y))
                .unwrap_or(Reply::Pass),
            (Layer::List, Input::Down { x, y, keys }) => self.list_down(x, y, keys),

            (Layer::Sheet, Input::Up { x, y, .. }) => self
                .drive_sheet(|s, cx| s.release(cx, x, y))
                .unwrap_or(Reply::Pass),
            (Layer::Settings, Input::Up { x, y, .. }) => self
                .drive_settings(|s, cx| s.release(cx, x, y))
                .unwrap_or(Reply::Pass),
            (Layer::Details, Input::Up { x, y, .. }) => self
                .drive_details(|s, cx| s.release(cx, x, y))
                .unwrap_or(Reply::Pass),
            (Layer::Help, Input::Up { x, y, .. }) => self
                .drive_help(|s, cx| s.release(cx, x, y))
                .unwrap_or(Reply::Pass),
            (Layer::Wheel, Input::Up { x, y, .. }) => self
                .drive_wheel(|s, cx| s.release(cx, x, y))
                .unwrap_or(Reply::Pass),
            (Layer::List, Input::Up { x, y, rel }) => self.list_up(x, y, &rel),

            (Layer::Sheet, Input::Move { x, y, keys }) => self
                .drive_sheet(|s, cx| s.moved(cx, x, y, keys & MK_LBUTTON_FLAG != 0))
                .unwrap_or(Reply::Pass),
            (Layer::Settings, Input::Move { x, y, .. }) => self
                .drive_settings(|s, cx| s.hover(cx, x, y))
                .unwrap_or(Reply::Pass),
            (Layer::Details, Input::Move { x, y, .. }) => self
                .drive_details(|s, cx| s.hover(cx, x, y))
                .unwrap_or(Reply::Pass),
            (Layer::Help, Input::Move { x, y, .. }) => self
                .drive_help(|s, cx| s.hover(cx, x, y))
                .unwrap_or(Reply::Pass),
            (Layer::Wheel, Input::Move { x, y, .. }) => self
                .drive_wheel(|s, cx| s.hover(cx, x, y))
                .unwrap_or(Reply::Pass),
            (Layer::List, Input::Move { x, y, keys }) => self.list_move(x, y, keys),

            (
                Layer::Sheet,
                Input::Key {
                    key, shift, ctrl, ..
                },
            ) => {
                let mods = Mods::with_repeat(shift, ctrl, self.key_repeat);
                self.drive_sheet(|s, cx| s.key(cx, key, mods))
                    .unwrap_or(Reply::Pass)
            }
            (
                Layer::Settings,
                Input::Key {
                    key, shift, ctrl, ..
                },
            ) => {
                let mods = Mods::with_repeat(shift, ctrl, self.key_repeat);
                self.drive_settings(|s, cx| s.key(cx, key, mods))
                    .unwrap_or(Reply::Pass)
            }
            (
                Layer::Details,
                Input::Key {
                    key, shift, ctrl, ..
                },
            ) => {
                let mods = Mods::with_repeat(shift, ctrl, self.key_repeat);
                self.drive_details(|s, cx| s.key(cx, key, mods))
                    .unwrap_or(Reply::Pass)
            }
            (
                Layer::Help,
                Input::Key {
                    key, shift, ctrl, ..
                },
            ) => {
                let mods = Mods::with_repeat(shift, ctrl, self.key_repeat);
                self.drive_help(|s, cx| s.key(cx, key, mods))
                    .unwrap_or(Reply::Pass)
            }
            (
                Layer::Wheel,
                Input::Key {
                    key, shift, ctrl, ..
                },
            ) => self.wheel_key(key, Mods::with_repeat(shift, ctrl, self.key_repeat)),
            (
                Layer::List,
                Input::Key {
                    key,
                    shift,
                    ctrl,
                    alt,
                },
            ) => self.list_key(key, shift, ctrl, alt),

            // Nothing under a sheet, Settings or a wheel scrolls.
            (Layer::Sheet, Input::Scroll(n)) => self
                .drive_sheet(|s, cx| s.scroll(cx, TextScroll::Wheel(n)))
                .unwrap_or(Reply::Pass),
            (Layer::Wheel, Input::Scroll(n)) => self
                .drive_wheel(|s, cx| s.scroll(cx, TextScroll::Wheel(n)))
                .unwrap_or(Reply::Pass),
            (Layer::Settings, Input::Scroll(n)) => self
                .drive_settings(|s, cx| s.scroll(cx, TextScroll::Wheel(n)))
                .unwrap_or(Reply::Pass),
            (Layer::Details, Input::Scroll(n)) => self
                .drive_details(|s, cx| s.scroll(cx, TextScroll::Wheel(n)))
                .unwrap_or(Reply::Pass),
            (Layer::Help, Input::Scroll(n)) => self
                .drive_help(|s, cx| s.scroll(cx, TextScroll::Wheel(n)))
                .unwrap_or(Reply::Pass),
            (Layer::List, Input::Scroll(n)) => {
                self.list_scroll(n);
                Reply::Taken
            }

            (Layer::List, Input::Leave) => {
                self.list_leave();
                Reply::Pass
            }
            (Layer::Sheet, Input::Leave) => {
                if let Some(s) = &mut self.sheet {
                    s.leave();
                }
                Reply::Pass
            }
            (Layer::Details, Input::Leave) => {
                if let Some(d) = &mut self.details {
                    d.leave();
                }
                Reply::Pass
            }
            (Layer::Help, Input::Leave) => {
                if let Some(h) = &mut self.help {
                    h.leave();
                }
                Reply::Pass
            }
            (Layer::Settings, Input::Leave) => {
                if let Some(s) = &mut self.settings_ui {
                    s.leave();
                }
                Reply::Pass
            }
            (_, Input::Leave) => Reply::Pass,
        }
    }
}

/// A list of `n` rows for tests, "0.png", "1.png"…, on disk in a folder
/// that lives as long as the guard.
#[cfg(test)]
pub(super) fn listed(n: usize) -> (tempfile::TempDir, FileList) {
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<PathBuf> = (0..n)
        .map(|i| {
            let p = dir.path().join(format!("{i}.png"));
            std::fs::write(&p, b"x").unwrap();
            p
        })
        .collect();
    let mut list = FileList::default();
    list.add(&Kiln::new(), &kiln_core::Availability::all(), paths);
    (dir, list)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_run_in_todays_order() {
        let all = Open {
            menu: true,
            dialog: true,
            tool: true,
            sheet: true,
            settings: true,
            details: true,
            help: true,
            wheel: true,
        };
        assert_eq!(
            all.stack(),
            [
                Layer::Menu,
                Layer::Dialog,
                Layer::Tool,
                Layer::Sheet,
                Layer::Settings,
                Layer::Details,
                Layer::Help,
                Layer::Wheel,
                Layer::List
            ]
        );
        assert_eq!(Open::default().stack(), [Layer::List]);
        let some = Open {
            help: true,
            wheel: true,
            ..Open::default()
        };
        assert_eq!(some.stack(), [Layer::Help, Layer::Wheel, Layer::List]);
    }

    fn up() -> Input {
        Input::Up {
            x: 0.0,
            y: 0.0,
            rel: Released {
                button: None,
                panel: None,
            },
        }
    }

    /// Offer `input` to `layers`, answering each as `answer` says, and
    /// return the reply with the layers that heard it.
    fn heard(
        layers: &[Layer],
        input: Input,
        answer: impl Fn(Layer) -> Reply,
    ) -> (Reply, Vec<Layer>) {
        let mut seen = Vec::new();
        let reply = offer(layers, input, |layer, _| {
            seen.push(layer);
            answer(layer)
        });
        (reply, seen)
    }

    #[test]
    fn a_block_stops_the_input_and_nothing_below_hears_it() {
        let stack = Open {
            settings: true,
            help: true,
            ..Open::default()
        }
        .stack();
        let (reply, seen) = heard(&stack, Input::Scroll(1.0), |l| match l {
            Layer::Settings => Reply::Block,
            _ => Reply::Taken,
        });
        assert_eq!(reply, Reply::Block);
        assert_eq!(seen, [Layer::Settings]);
    }

    #[test]
    fn a_pointer_leaving_reaches_every_layer_even_when_one_blocks() {
        let stack = Open {
            sheet: true,
            help: true,
            wheel: true,
            ..Open::default()
        }
        .stack();
        let (reply, seen) = heard(&stack, Input::Leave, |_| Reply::Block);
        assert_eq!(reply, Reply::Pass);
        assert_eq!(seen, stack);
    }

    #[test]
    fn a_release_falls_through_a_pass_to_the_layer_below() {
        let stack = Open {
            details: true,
            help: true,
            ..Open::default()
        }
        .stack();
        // Details and Help had nothing pressed: the list gets it.
        let (reply, seen) = heard(&stack, up(), |l| match l {
            Layer::List => Reply::Taken,
            _ => Reply::Pass,
        });
        assert_eq!(reply, Reply::Taken);
        assert_eq!(seen, [Layer::Details, Layer::Help, Layer::List]);
        // Help taking it keeps it from the list.
        let (reply, seen) = heard(&stack, up(), |l| match l {
            Layer::Help => Reply::Taken,
            _ => Reply::Pass,
        });
        assert_eq!(reply, Reply::Taken);
        assert_eq!(seen, [Layer::Details, Layer::Help]);
        // Nobody wanting it leaves it unanswered.
        let (reply, seen) = heard(&stack, up(), |_| Reply::Pass);
        assert_eq!(reply, Reply::Pass);
        assert_eq!(seen, stack);
    }
}
