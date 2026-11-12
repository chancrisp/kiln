//! What the window does with `App` let go.
//!
//! A dialog, a picker, a menu or a drag runs a message loop of its own,
//! and a call such as `SetWindowPos`, `SetFocus` or `DestroyWindow` sends
//! messages straight back to the window. Either would reach the window's
//! procedure while `App` is borrowed. So a handler never makes those calls:
//! it queues an [`Effect`], and once its message is answered the runner
//! ([`run_effects`]) makes each call with no borrow held. What the code
//! after the call did comes back as a continuation, run with a fresh
//! borrow, against the state as it is then.

use std::collections::VecDeque;
use std::path::PathBuf;

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, SetForegroundWindow, SetWindowPos, ShowWindow, SET_WINDOW_POS_FLAGS,
    SHOW_WINDOW_CMD, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SW_HIDE,
};

use super::cell::AppCell;
use super::crash::{self, Guard};
use super::tray;
use super::tray_menu;
use super::{log, system, turn, updates, App, SyncKind};
use crate::layout::Rect;
use crate::model::{Prompt, PromptButton};
use crate::settings::Engine;
use crate::theme::fluent::FluentPalette;

/// What a modal step's answer does, run with a fresh borrow of `App`.
pub(super) type Answer<T> = Box<dyn FnOnce(&mut App, T)>;

/// One thing the window does with `App` let go.
pub(super) enum Effect {
    // Each of these runs a message loop of its own.
    /// The prompt, inside the window (`surfaces::Ask`).
    Ask {
        prompt: Prompt,
        then: Answer<PromptButton>,
    },
    /// The folder picker, starting in `start` when given.
    PickFolder {
        title: String,
        start: Option<PathBuf>,
        then: Answer<Option<PathBuf>>,
    },
    /// The program (.exe) picker.
    PickProgram {
        title: String,
        then: Answer<Option<PathBuf>>,
    },
    /// The Open dialog, for files to add.
    PickFiles {
        title: String,
        start: Option<PathBuf>,
        then: Answer<Vec<PathBuf>>,
    },

    /// The tray icon's menu.
    TrayMenu {
        at: POINT,
        drag_wheel_on: bool,
        palette: Box<FluentPalette>,
        text_scale: f32,
    },

    // These can run a loop of their own too: the shell may show its own
    // message, or wait on another program.
    /// Open a file, or a page, in its usual app.
    Open {
        target: PathBuf,
        then: Answer<bool>,
    },
    /// Run `winget install -e --id <id>` in a console of its own that
    /// stays open. Nothing waits for it; the answer is whether it started.
    RunInConsole {
        winget: PathBuf,
        id: String,
        then: Answer<bool>,
    },
    /// Open a text file in Notepad, whatever app its extension has (a
    /// fresh Windows has none for `.md`), or else in its usual app.
    OpenText {
        target: PathBuf,
        then: Answer<bool>,
    },
    /// Open an engine's download page.
    OpenDownload {
        engine: Engine,
        then: Answer<bool>,
    },
    /// Open Kiln's releases page.
    OpenReleases,
    /// Open Explorer at `folder` with `files` selected.
    ShowInFolder {
        folder: PathBuf,
        files: Vec<PathBuf>,
        then: Answer<bool>,
    },
    // Each of these sends messages straight back to the window.
    /// Put text on the clipboard: emptying it tells its last owner, which
    /// may be this window.
    Clipboard {
        text: String,
        then: Answer<windows::core::Result<()>>,
    },
    /// Bring the window the keyboard focus.
    Focus,
    Show(SHOW_WINDOW_CMD),
    /// Restore the window if it's minimised, and bring it forward.
    BringForward,
    Foreground,
    /// Move or size the window.
    Place(Place),
    /// Have Windows ask for the frame again (`WM_NCCALCSIZE`), the window
    /// staying where it is: a taskbar that hides itself, or stopped,
    /// changes the room a maximised window leaves.
    FrameChanged,
    Capture,
    ReleaseCapture,
    /// Bring the tooltips' tools in line: removing the tool whose tip
    /// shows hides it, and the control tells the window.
    Tips {
        tools: Vec<(usize, Rect)>,
        dpi: f32,
        /// A tip to hold open for a capture: its words and the screen
        /// point (`KILN_TEST_TIP`). Always `None` outside a test build.
        pop: Option<(String, i32, i32)>,
    },
    Destroy,
    /// What a handler does once the effects it queued before have run,
    /// with a fresh borrow (`App::afterwards`).
    Run(Box<dyn FnOnce(&mut App)>),
}

/// `SetWindowPos` arguments: where, how big and how.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Place {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub flags: SET_WINDOW_POS_FLAGS,
}

impl Effect {
    /// Whether it runs a loop of its own, delivering the window's
    /// messages while it waits. A runner entered from inside one stops at
    /// the first of these.
    fn waits(&self) -> bool {
        matches!(
            self,
            Effect::Ask { .. }
                | Effect::PickFolder { .. }
                | Effect::PickProgram { .. }
                | Effect::PickFiles { .. }
                | Effect::TrayMenu { .. }
                | Effect::Open { .. }
                | Effect::OpenText { .. }
                | Effect::OpenDownload { .. }
                | Effect::OpenReleases
                | Effect::ShowInFolder { .. }
        )
    }

    /// Whether it is one of Kiln's dialogs, menus or drags, which
    /// `system::modal_open` counts from the moment it is queued: work
    /// that would start meanwhile waits for it, as it waited while one
    /// was open.
    fn counted(&self) -> bool {
        matches!(
            self,
            Effect::Ask { .. }
                | Effect::PickFolder { .. }
                | Effect::PickProgram { .. }
                | Effect::PickFiles { .. }
                | Effect::TrayMenu { .. }
        )
    }

    /// Its name, for the log and the tests' record of what ran.
    pub(super) fn name(&self) -> &'static str {
        match self {
            Effect::Ask { .. } => "ask",
            Effect::PickFolder { .. } => "pick folder",
            Effect::PickProgram { .. } => "pick program",
            Effect::PickFiles { .. } => "pick files",

            Effect::TrayMenu { .. } => "tray menu",

            Effect::Open { .. } => "open",
            Effect::RunInConsole { .. } => "run in console",
            Effect::OpenText { .. } => "open as text",
            Effect::OpenDownload { .. } => "open download page",
            Effect::OpenReleases => "open releases page",
            Effect::ShowInFolder { .. } => "show in folder",
            Effect::Clipboard { .. } => "clipboard",
            Effect::Focus => "focus",
            Effect::Show(_) => "show",
            Effect::BringForward => "bring forward",
            Effect::Foreground => "foreground",
            Effect::Place(_) => "place",
            Effect::FrameChanged => "frame changed",
            Effect::Capture => "capture",
            Effect::ReleaseCapture => "release capture",
            Effect::Tips { .. } => "tips",
            Effect::Destroy => "destroy",
            Effect::Run(_) => "run",
        }
    }
}

/// An effect waiting its turn, counted as a modal while it waits and
/// runs when it is one (`Effect::counted`).
struct Queued {
    effect: Effect,
    _modal: Option<system::Modal>,
}

/// The effects a handler queued, oldest first.
#[derive(Default)]
pub(super) struct Effects(VecDeque<Queued>);

impl Effects {
    pub(super) fn push(&mut self, effect: Effect) {
        let modal = effect.counted().then(system::Modal::open);
        self.0.push_back(Queued {
            effect,
            _modal: modal,
        });
    }

    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(super) fn len(&self) -> usize {
        self.0.len()
    }

    /// Drop every effect queued, unrun, with its continuation.
    pub(super) fn clear(&mut self) {
        self.0.clear();
    }

    /// The names of the effects queued, oldest first.
    #[cfg(test)]
    pub(super) fn names(&self) -> Vec<&'static str> {
        self.0.iter().map(|q| q.effect.name()).collect()
    }

    fn take(&mut self) -> Effects {
        std::mem::take(self)
    }

    fn append(&mut self, mut other: Effects) {
        self.0.append(&mut other.0);
    }
}

/// Run the effects queued on the window's `App`, with the borrow let go,
/// until none is left. Called once each message, drag event or step of
/// set-up has been answered and its borrow has ended.
pub(super) fn run_effects(hwnd: HWND, cell: &AppCell) {
    run_effects_in(&crash::WINDOW, hwnd, cell);
}

/// What the runner takes off the queue next.
enum Next {
    /// A step to make with `App` let go, and the effects queued behind it,
    /// set aside meanwhile.
    Step(Queued, Effects),
    /// A continuation (`Effect::Run`), to run with the borrow.
    Run(Box<dyn FnOnce(&mut App)>),
}

/// `run_effects`, under `guard`. Each step:
/// 1. is taken off the queue with what is queued behind it, which is set
///    aside, so a message its loop delivers queues its own effects on
///    their own;
/// 2. runs with no borrow held;
/// 3. gives its answer to its continuation with the borrow taken again;
///    what that queues comes first, then the rest.
///
/// A runner entered while a step runs (from a message the step's loop or
/// call delivers) runs the effects at the front of the queue that wait
/// for nothing, and stops at the first that does: that one runs once the
/// outer step is done. Once the guard has tripped, everything queued is
/// dropped unrun. Once the window is destroyed, so is everything still
/// queued: nothing is shown over, or done to, a window that has gone.
pub(super) fn run_effects_in(guard: &Guard, hwnd: HWND, cell: &AppCell) {
    loop {
        if guard.tripped() {
            return drop_all(cell);
        }
        let inside = cell.depth() > 0;
        let next = cell
            .with(|app| {
                if app.destroyed && !app.effects.is_empty() {
                    log(&format!(
                        "dropped {} effects queued after the window closed",
                        app.effects.len()
                    ));
                    app.effects.clear();
                }
                if inside && app.effects.0.front().is_some_and(|q| q.effect.waits()) {
                    return None;
                }
                let queued = app.effects.0.pop_front()?;
                Some(match queued.effect {
                    Effect::Run(f) => Next::Run(f),
                    effect => {
                        let rest = app.effects.take();
                        let queued = Queued {
                            effect,
                            _modal: queued._modal,
                        };
                        Next::Step(queued, rest)
                    }
                })
            })
            .flatten();
        let (queued, rest) = match next {
            None => return,
            Some(Next::Run(f)) => {
                let ran = turn(
                    cell,
                    |app| {
                        let rest = app.effects.take();
                        f(app);
                        app.effects.append(rest);
                    },
                    |_| SyncKind::Now,
                );
                unrun(ran, "run");
                continue;
            }
            Some(Next::Step(queued, rest)) => (queued, rest),
        };
        let Queued { effect, _modal } = queued;
        let waits = effect.waits();
        let name = effect.name();
        // A dialog's, picker's, menu's or drag's own loop runs the window's
        // messages meanwhile, not the message loop: the window's frames
        // come from its timer (`App::modal_frames`).
        if waits {
            cell.with(|app| app.modal_frames());
        }
        let done = {
            let _running = cell.running();
            perform(effect, hwnd, cell)
        };
        // As a dialog's own guard went once it returned: the code after it
        // ran with no dialog counted.
        drop(_modal);
        if guard.tripped() {
            drop((done, rest));
            return drop_all(cell);
        }
        let answered = done.is_some();
        let sync = move |_: bool| {
            if answered {
                SyncKind::Now
            } else {
                SyncKind::Skip
            }
        };
        let ran = turn(
            cell,
            move |app| {
                // What a message the step delivered queued and couldn't run.
                let left = app.effects.take();
                if let Some(done) = done {
                    done(app);
                }
                if waits {
                    // The step's own flow goes on first; what came in during
                    // its loop waited for it.
                    app.effects.append(rest);
                    app.effects.append(left);
                } else {
                    // As the call's own messages were handled inside it,
                    // before what followed it.
                    app.effects.append(left);
                    app.effects.append(rest);
                }
            },
            sync,
        );
        unrun(ran, name);
    }
}

/// `turn` found `App` borrowed, which the runner never is while it runs:
/// what it would have run is dropped, and the log says so.
fn unrun(ran: Option<()>, what: &str) {
    if ran.is_none() {
        log(&format!(
            "the continuation of {what} was dropped: the window's state was in use"
        ));
        debug_assert!(false, "the continuation of {what} found App borrowed");
    }
}

/// The guard has tripped: drop what is queued, unrun, never running any
/// of `App`'s code.
fn drop_all(cell: &AppCell) {
    let queued = cell.with(|app| app.effects.take());
    drop(queued);
}

/// What a step's answer does with a fresh borrow.
type Done = Option<Box<dyn FnOnce(&mut App)>>;

fn answered<T: 'static>(then: Answer<T>, answer: T) -> Done {
    Some(Box::new(move |app| then(app, answer)))
}

/// Make `effect`'s call, with `App` let go.
fn perform(effect: Effect, hwnd: HWND, cell: &AppCell) -> Done {
    #[cfg(test)]
    let effect = match script::stand_in(effect, hwnd) {
        Ok(done) => return done,
        Err(effect) => effect,
    };
    match effect {
        // Inside the window, as its top layer, answered there
        // (`App::answer_ask`); Windows' task dialog only for a window that
        // can't draw.
        Effect::Ask { prompt, then } => {
            if cell.with(|app| app.asks_inside()) == Some(true) {
                Some(Box::new(move |app| app.open_ask(prompt, then)))
            } else {
                answered(then, system::ask(hwnd, &prompt))
            }
        }
        Effect::PickFolder { title, start, then } => {
            let picked = match &start {
                Some(start) => system::pick_folder_in(hwnd, &title, start),
                None => system::pick_folder(hwnd, &title),
            };
            answered(then, picked)
        }
        Effect::PickProgram { title, then } => answered(then, system::pick_program(hwnd, &title)),
        Effect::PickFiles { title, start, then } => {
            answered(then, system::pick_files(hwnd, &title, start.as_deref()))
        }

        // Kiln's own small window, which stays up beside the window: it
        // answers by posting the window the command. Windows' menu, with
        // its own loop, only for a window that can't draw one.
        Effect::TrayMenu {
            at,
            drag_wheel_on,
            palette,
            text_scale,
        } => match tray_menu::open(tray_menu::Open {
            owner: hwnd,
            at,
            drag_wheel_on,
            palette: *palette,
            text_scale,
        }) {
            Ok(menu) => Some(Box::new(move |app| app.tray_menu = Some(menu))),
            Err(e) => {
                log(&format!(
                    "the tray menu couldn't open in its own window: {e}"
                ));
                let chosen = tray::show_menu(hwnd, at, drag_wheel_on);
                Some(Box::new(move |app| {
                    // The window went while the menu was up: nothing to do.
                    if let Some(c) = chosen.filter(|_| !app.destroyed) {
                        app.tray_command(c);
                    }
                }))
            }
        },

        Effect::Open { target, then } => answered(then, system::open_file(hwnd, &target)),
        Effect::RunInConsole { winget, id, then } => {
            answered(then, system::run_in_console(&winget, &id))
        }
        Effect::OpenText { target, then } => answered(then, system::open_text(hwnd, &target)),
        Effect::OpenDownload { engine, then } => {
            answered(then, system::open_download_page(hwnd, engine))
        }
        Effect::OpenReleases => {
            updates::open_releases_page();
            None
        }
        Effect::ShowInFolder {
            folder,
            files,
            then,
        } => answered(then, system::show_in_folder(&folder, &files).is_ok()),
        Effect::Clipboard { text, then } => answered(then, system::set_clipboard(hwnd, &text)),
        Effect::Tips { tools, dpi, pop } => {
            // Out of `App` while it syncs: the control's notice finds no
            // tooltip control and goes to DefWindowProc, as it did.
            let mut tips = cell.with(|app| app.tips.take()).flatten();
            if let Some(t) = &mut tips {
                t.sync(&tools, dpi);
                #[cfg(any(debug_assertions, feature = "test-hooks"))]
                if let Some((text, x, y)) = &pop {
                    t.pop(text, *x, *y);
                }
                #[cfg(not(any(debug_assertions, feature = "test-hooks")))]
                let _ = &pop;
            }
            Some(Box::new(move |app| {
                if app.tips.is_none() {
                    app.tips = tips;
                }
            }))
        }
        Effect::Place(p) => {
            place_now(hwnd, p);
            None
        }
        Effect::FrameChanged => {
            refresh_frame(hwnd);
            None
        }
        Effect::Run(f) => Some(f),
        // SAFETY: each is a plain call on this thread's own window, made
        // with no borrow of `App` held, so the messages it sends back are
        // answered as any other.
        Effect::Focus => unsafe {
            let _ = SetFocus(Some(hwnd));
            None
        },
        Effect::Show(cmd) => {
            // A window never shown yet (a start in the background) has
            // drawn nothing: it shows as the first start does, cloaked
            // until its first frame is on the screen.
            let fresh =
                cmd != SW_HIDE && cell.with(|app| app.renderer.presented() == 0) == Some(true);
            if fresh {
                super::show_first_frame(hwnd, cell, cmd);
            } else {
                // SAFETY: as above.
                unsafe {
                    let _ = ShowWindow(hwnd, super::placement::show_cmd(cmd));
                }
            }
            None
        }
        Effect::BringForward => {
            system::bring_forward(hwnd);
            None
        }
        Effect::Foreground => unsafe {
            // The test placement switch never takes the foreground.
            if !super::placement::quiet_on() {
                let _ = SetForegroundWindow(hwnd);
            }
            None
        },
        Effect::Capture => unsafe {
            SetCapture(hwnd);
            None
        },
        Effect::ReleaseCapture => unsafe {
            let _ = ReleaseCapture();
            None
        },
        Effect::Destroy => unsafe {
            let _ = DestroyWindow(hwnd);
            None
        },
    }
}

/// Move or size the window now. Only with `App` let go: the runner's, and
/// `run_window`'s set-up before the message loop.
pub(super) fn place_now(hwnd: HWND, p: Place) {
    // With the test placement switch on, always inside its area.
    let p = super::placement::constrain(hwnd, p);
    // SAFETY: plain call on this thread's own window; its messages find
    // `App` free.
    unsafe {
        let _ = SetWindowPos(hwnd, None, p.x, p.y, p.w, p.h, p.flags);
    }
}

/// Have Windows ask for the frame again (`WM_NCCALCSIZE`), the window
/// staying where it is. Only with `App` let go: the runner's
/// (`Effect::FrameChanged`), and `run_window`'s set-up. A client area
/// that changes size with it is told as a `WM_SIZE`, which must find
/// `App` free to reach the renderer.
pub(super) fn refresh_frame(hwnd: HWND) {
    // SAFETY: a plain call on this thread's own window; its messages find
    // `App` free.
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// Tests: modal steps and calls that would reach the desktop are answered
/// by a script instead, and every step is recorded.
#[cfg(test)]
pub(super) mod script {
    use std::cell::RefCell;
    use std::path::PathBuf;

    use windows::Win32::Foundation::HWND;

    use super::{answered, Done, Effect};
    use crate::model::PromptButton;

    /// What the script answers a step.
    pub enum Reply {
        Button(PromptButton),
        /// Let the prompt open inside the window, as it does outside the
        /// tests, to be answered there.
        InWindow,
        Path(Option<PathBuf>),
        Paths(Vec<PathBuf>),
        Ok(bool),
    }

    type StandIn = Box<dyn FnMut(HWND, &Effect) -> Reply>;

    thread_local! {
        static STAND_IN: RefCell<Option<StandIn>> = const { RefCell::new(None) };
        static RAN: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    }

    /// Answer this thread's modal steps with `f`, which may deliver
    /// messages to the window as a dialog's loop would.
    pub fn answer_with(f: impl FnMut(HWND, &Effect) -> Reply + 'static) {
        STAND_IN.with(|s| *s.borrow_mut() = Some(Box::new(f)));
    }

    /// The steps run on this thread so far, oldest first, taken.
    pub fn ran() -> Vec<&'static str> {
        RAN.with(|r| std::mem::take(&mut *r.borrow_mut()))
    }

    /// The script's answer to `effect`, or the effect back for the call
    /// itself (`Place`, `FrameChanged`, `Destroy`, `Tips`, `Run`).
    pub(super) fn stand_in(effect: Effect, hwnd: HWND) -> Result<Done, Effect> {
        RAN.with(|r| r.borrow_mut().push(effect.name()));
        match effect {
            Effect::Place(_)
            | Effect::FrameChanged
            | Effect::Destroy
            | Effect::Tips { .. }
            | Effect::Run(_) => return Err(effect),
            // Recorded only: a test never moves the focus, the capture or
            // a window on the desktop.
            Effect::Focus
            | Effect::Show(_)
            | Effect::BringForward
            | Effect::Foreground
            | Effect::Capture
            | Effect::ReleaseCapture
            | Effect::OpenReleases
            | Effect::TrayMenu { .. } => return Ok(None),
            _ => {}
        }
        // Out while it runs: a step started from inside it would find no
        // script, and the nesting rule says none is.
        let mut f = STAND_IN
            .with(|s| s.borrow_mut().take())
            .unwrap_or_else(|| panic!("no script answers {}", effect.name()));
        let reply = f(hwnd, &effect);
        STAND_IN.with(|s| {
            s.borrow_mut().get_or_insert(f);
        });
        Ok(match (effect, reply) {
            (Effect::Ask { then, .. }, Reply::Button(b)) => answered(then, b),
            (effect @ Effect::Ask { .. }, Reply::InWindow) => return Err(effect),
            (
                Effect::PickFolder { then, .. } | Effect::PickProgram { then, .. },
                Reply::Path(p),
            ) => answered(then, p),
            (Effect::PickFiles { then, .. }, Reply::Paths(p)) => answered(then, p),

            (Effect::Clipboard { then, .. }, Reply::Ok(ok)) => answered(
                then,
                if ok {
                    Ok(())
                } else {
                    Err(windows::core::Error::empty())
                },
            ),
            (
                Effect::Open { then, .. }
                | Effect::RunInConsole { then, .. }
                | Effect::OpenText { then, .. }
                | Effect::OpenDownload { then, .. }
                | Effect::ShowInFolder { then, .. },
                Reply::Ok(ok),
            ) => answered(then, ok),
            (effect, _) => panic!("the script's answer doesn't fit {}", effect.name()),
        })
    }
}
