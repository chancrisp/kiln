//! In-process tests of the flows that run through effects: the window's
//! own procedure answers real messages sent to a hidden window, which is
//! never shown, and a script answers each dialog, picker and menu, so
//! nothing reaches the desktop.

use super::arrive::{Walk, WalkAsk, WALK_ADD_AT_ONCE};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use windows::core::w;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, IsWindow, PeekMessageW, RegisterClassW, SendMessageW, MSG,
    PM_REMOVE, WINDOW_EX_STYLE, WM_CLOSE, WM_COMMAND, WM_ENDSESSION, WM_GETOBJECT, WM_PAINT,
    WM_TIMER, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};

use super::cell::{self, AppCell, Installed};
use super::effects::script::{self, Reply};
use super::effects::{run_effects, run_effects_in, Effect};
use super::tray::TrayCommand;
use super::*;
use crate::autostart::RunStore;
use crate::model::{Leave, Prompt, PromptButton};

/// A hidden window answered by Kiln's own procedure, with an `App` whose
/// holding folder holds one result, `a.jpg`, not yet saved.
struct Rig {
    hwnd: HWND,
    installed: Option<Installed>,
    dir: tempfile::TempDir,
    /// The start-up entry, in memory: Kiln's own, naming [`RIG_EXE`].
    run: autostart::MemoryRun,
}

/// The `kiln-app.exe` the rig's start-up entry is for.
const RIG_EXE: &str = r"C:\Program Files\Kiln\kiln-app.exe";

impl Rig {
    fn new() -> Rig {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: the module handle of this test binary.
        let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }.unwrap().into();
        let class = w!("KilnEffectsTest");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: class,
            ..Default::default()
        };
        // SAFETY: a plain registration; a second one fails harmlessly.
        unsafe { RegisterClassW(&wc) };
        // SAFETY: a plain window of this thread, never shown.
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class,
                w!("Kiln test"),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                640,
                480,
                None,
                None,
                Some(instance),
                None,
            )
        }
        .unwrap();
        let holding = Held::open(
            kiln_core::Places::new(dir.path()),
            Box::new(kiln_core::SystemVolumes),
        )
        .unwrap();
        let held = holding.local().join("a.jpg");
        std::fs::write(&held, b"first").unwrap();
        // No hooks in a test.
        let settings = Settings {
            drag_wheel: false,
            ..Settings::default()
        };
        let (wake, inbox) = channel::channel(hwnd);
        let run =
            autostart::MemoryRun::holding(&autostart::run_value(std::path::Path::new(RIG_EXE)));
        let kiln = Arc::new(Kiln::new());
        let runner = Runner::new(kiln.clone(), 1, |_| {});
        let inspect_kiln = Kiln::new();
        let inspector = Inspector::new(
            move |path, opts, cancel| kiln_core::inspect(&inspect_kiln, path, opts, cancel),
            |_, _| {},
        );
        let text_scale = 1.0;
        let mut app = new_app(
            hwnd,
            Start {
                palette: theme::fluent::DARK,
                contrast: None,
                system_light: false,
                settings,
                settings_store: settings::Store::new(Some(dir.path().join("settings.toml"))),
                dpi: 96.0,
                wake,
                inbox,
                text_scale_source: system::TextScale::new(|| {}),
                text_scale,
                zoom: layout::text_zoom(text_scale, layout::default_window(text_scale)),
                held: Some(holding),
                places: None,
                sticky: None,
                kiln: kiln.clone(),
                runner,
                inspector,
                autostart: Autostart::new(run.clone(), Some(PathBuf::from(RIG_EXE))),
            },
        )
        .unwrap();
        // The engines thread isn't asked in a test: they are known, every
        // one found, so no notice of a missing helper app speaks.
        let look = app.engines.ask();
        app.engines.found(look, Availability::all());
        let mut e = model::entry_for(
            &kiln,
            &Availability::default(),
            PathBuf::from(r"C:\in\a.png"),
            1,
            None,
        );
        e.job = JobState::Done {
            action: Action::Convert("jpg".into()),
            outputs: vec![held],
            saved: None,
        };
        e.last = Some(Action::Convert("jpg".into()));
        app.files.insert(0, e);
        app.motion.rows_added(0, 1, Instant::now());
        let installed = cell::install(hwnd, AppCell::new(hwnd, app));
        Rig {
            hwnd,
            installed: Some(installed),
            dir,
            run,
        }
    }

    fn cell(&self) -> &AppCell {
        self.installed.as_ref().unwrap().cell()
    }

    fn app<R>(&self, f: impl FnOnce(&mut App) -> R) -> R {
        self.cell().with(f).expect("App is free between messages")
    }

    fn held_file(&self) -> PathBuf {
        self.app(|app| app.held.as_ref().unwrap().local().join("a.jpg"))
    }

    /// Send `msg` to the window, as Windows or a dialog's loop would.
    fn send(&self, msg: u32, wparam: usize) -> isize {
        send(self.hwnd, msg, wparam)
    }

    fn alive(&self) -> bool {
        // SAFETY: asks only whether the handle names a window.
        unsafe { IsWindow(Some(self.hwnd)) }.as_bool()
    }
}

fn send(hwnd: HWND, msg: u32, wparam: usize) -> isize {
    // SAFETY: a plain send to this thread's own window, with no pointer
    // arguments.
    unsafe { SendMessageW(hwnd, msg, Some(WPARAM(wparam)), Some(LPARAM(0))) }.0
}

impl Drop for Rig {
    fn drop(&mut self) {
        // Whatever the window's going asks is turned down, unshown.
        script::answer_with(|_, effect| match effect {
            Effect::Ask { .. } => Reply::Button(PromptButton::Cancel),
            Effect::PickFiles { .. } => Reply::Paths(vec![]),
            Effect::PickFolder { .. } | Effect::PickProgram { .. } => Reply::Path(None),
            _ => Reply::Ok(false),
        });
        if self.alive() {
            // SAFETY: this thread's own window.
            unsafe {
                let _ = DestroyWindow(self.hwnd);
            }
        }
        if let Some(i) = self.installed.take() {
            drop(i.finish());
        }
        // What the window posted to this thread, the quit included.
        let mut msg = MSG::default();
        // SAFETY: this thread's own queue.
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {}
        let _ = &self.dir;
    }
}

/// The close prompt for one running job, byte for byte as it is.
fn close_prompt() -> Prompt {
    leave_prompt(1, Leave::Close).expect("one running job asks")
}

/// Row 0 converting, so a close asks first.
fn converting(rig: &Rig) {
    rig.app(|app| {
        app.files.entries_mut()[0].job = JobState::Running {
            action: Action::Convert("jpg".into()),
            fraction: None,
            started: None,
        };
    });
}

/// How many results the rows hold that are still there, as far as Kiln
/// knows.
fn held_present(app: &App) -> usize {
    let present = present_in(app.held.as_ref(), &app.gone_seen);
    app.files
        .held_files(|_| true)
        .iter()
        .filter(|p| present(p))
        .count()
}

fn title(effect: &Effect) -> Option<String> {
    match effect {
        Effect::Ask { prompt, .. } => Some(prompt.title.clone()),
        _ => None,
    }
}

#[test]
fn turning_the_drag_wheel_off_removes_kilns_start_up_entry() {
    let rig = Rig::new();
    // On, as from the Settings card, without arming hooks in a test.
    rig.app(|app| app.prefs.settings.drag_wheel = true);
    rig.app(|app| {
        app.set_drag_wheel(false);
        assert!(app.autostart.finish(Duration::from_secs(10)));
    });
    assert_eq!(rig.run.value(), None, "Kiln's own entry goes");
    assert!(!rig.app(|app| app.prefs.settings.drag_wheel));
}

#[test]
fn turning_the_drag_wheel_on_writes_kilns_start_up_entry() {
    let rig = Rig::new();
    rig.run.delete().unwrap();
    rig.app(|app| {
        app.set_drag_wheel(true);
        assert!(app.autostart.finish(Duration::from_secs(10)));
    });
    assert_eq!(
        rig.run.value(),
        Some(autostart::run_value(std::path::Path::new(RIG_EXE)))
    );
    assert!(rig.app(|app| app.prefs.settings.drag_wheel));
}

#[test]
fn a_drag_wheel_turned_off_at_load_is_told_once_and_saved() {
    let rig = Rig::new();
    let path = rig.dir.path().join("settings.toml");
    // A file from before phase 3, with the drag-wheel on.
    std::fs::write(&path, "drag_wheel = true\n").unwrap();
    let (loaded, err) = Settings::load(&path);
    assert_eq!(err, None);
    assert_eq!(loaded.pending_notice, Some(settings::Notice::DragWheelOff));
    rig.app(|app| {
        app.prefs.settings = loaded;
        app.status.text.clear();
        app.status.alert = None;
        app.set_top_info(None);
        app.settings_loaded(None);
        // Said once, in an InfoBar at the top of the window, not in the
        // footer.
        assert_eq!(
            app.top_info,
            Some((String::new(), settings::DRAG_WHEEL_NOW_OFF.to_owned()))
        );
        assert_ne!(app.status.text, settings::DRAG_WHEEL_NOW_OFF);
        assert_eq!(app.prefs.settings.pending_notice, None);
        // The tree reads it, and its Dismiss takes it away.
        let tree = app.a11y_tree(&[]);
        let bar = tree.find(&a11y::Id::TopInfo).expect("the InfoBar");
        assert_eq!(bar.name, settings::DRAG_WHEEL_NOW_OFF);
        app.prefs.store.flush().unwrap();
    });
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\ndrag_wheel_reset = true"), "{text}");
    assert!(text.contains("\ndrag_wheel = false"), "{text}");
    // The next start has nothing to say.
    let (again, err) = Settings::load(&path);
    assert_eq!(err, None);
    assert_eq!(again.pending_notice, None);
    rig.app(|app| {
        app.status.alert = None;
        app.say("");
        app.set_top_info(None);
        app.prefs.settings = again;
        app.settings_loaded(None);
        assert_eq!(app.status.text, "");
        assert_eq!(app.status.alert, None);
        assert_eq!(app.top_info, None);
    });
}

#[test]
fn windows_refusing_the_drag_wheels_hooks_is_said_aloud_once() {
    let rig = Rig::new();
    rig.app(|app| {
        app.prefs.settings.drag_wheel = true;
        app.status.sets.clear();
        app.hooks_now_refused(false);
        assert!(app.hooks_refused);
        assert_eq!(
            app.status.alert.as_deref(),
            Some(settings::DRAG_WHEEL_REFUSED)
        );
        assert_eq!(app.status.sets, [settings::DRAG_WHEEL_REFUSED]);
        // Refused again on a later re-arm: not said again.
        app.status.sets.clear();
        app.hooks_now_refused(true);
        assert!(app.status.sets.is_empty());
    });
}

#[test]
fn turning_the_drag_wheel_off_leaves_another_programs_entry() {
    let rig = Rig::new();
    let theirs = r#""D:\Portable\Kiln\kiln-app.exe" --background"#;
    rig.run.write(theirs).unwrap();
    rig.app(|app| app.prefs.settings.drag_wheel = true);
    rig.app(|app| {
        app.set_drag_wheel(false);
        assert!(app.autostart.finish(Duration::from_secs(10)));
    });
    assert_eq!(rig.run.value().as_deref(), Some(theirs));
}

#[test]
fn close_with_a_held_result_asks_nothing_and_never_removes_it() {
    let rig = Rig::new();
    let held = rig.held_file();
    assert!(held.exists());
    script::answer_with(|_, effect| match effect {
        Effect::Ask { .. } | Effect::PickFolder { .. } => {
            panic!("nothing is asked about results: {}", effect.name())
        }
        _ => Reply::Ok(false),
    });
    rig.send(WM_CLOSE, 0);
    let ran = script::ran();
    assert!(!ran.contains(&"ask"), "{ran:?}");
    // Kiln quits (the drag-wheel is off); the result stays, whole, for the
    // next start to deliver.
    assert!(!rig.alive());
    assert_eq!(std::fs::read(&held).unwrap(), b"first");
    rig.app(|app| assert!(app.destroyed));
    #[cfg(debug_assertions)]
    assert_eq!(rig.cell().nested(), 0);
}

#[test]
fn a_close_waiting_on_a_save_that_keeps_its_result_says_why_kiln_stays_open() {
    let rig = Rig::new();
    let held = rig.held_file();
    rig.app(|app| {
        // A save under way, which a close waits for.
        app.exports.insert(
            7,
            Export {
                files: vec![held.clone()],
                beside: None,
                cancel: CancelToken::new(),
                thread: None,
            },
        );
        app.proceed(Then::Close);
        assert!(app.close_when_saved);
        assert_eq!(app.status.text, "Saving… Kiln will close when it's done.");
        // It couldn't save the result anywhere: Kiln stays open, keeps the
        // result, and the footer says why instead of still saying it will
        // close.
        app.on_exported(Exported {
            id: 7,
            folder: PathBuf::from(r"E:\DCIM"),
            results: vec![(held.clone(), None)],
            cancelled: vec![],
        });
        assert!(!app.close_when_saved);
        assert!(!app.destroyed);
        assert_eq!(
            app.status.text,
            "Couldn't save a.jpg. Kiln tries again the next time it starts."
        );
        assert!(app.status.alerting());
    });
    assert!(rig.alive());
    assert_eq!(std::fs::read(&held).unwrap(), b"first");
}

#[test]
fn a_close_waiting_on_a_save_that_was_cancelled_stops_saying_kiln_will_close() {
    let rig = Rig::new();
    let held = rig.held_file();
    rig.app(|app| {
        app.exports.insert(
            7,
            Export {
                files: vec![held.clone()],
                beside: None,
                cancel: CancelToken::new(),
                thread: None,
            },
        );
        app.proceed(Then::Close);
        assert!(app.close_when_saved);
        // The save was cancelled: the result is kept, nothing went wrong,
        // and Kiln stays open, so the footer no longer says it will close.
        app.on_exported(Exported {
            id: 7,
            folder: PathBuf::from(r"E:\DCIM"),
            results: vec![(held.clone(), None)],
            cancelled: vec![held.clone()],
        });
        assert!(!app.close_when_saved);
        assert!(!app.destroyed);
        assert_ne!(app.status.text, "Saving… Kiln will close when it's done.");
        assert!(
            !app.status.text.contains("will close"),
            "{}",
            app.status.text
        );
        assert_eq!(app.status.text, app.plain_status());
    });
    assert!(rig.alive());
    assert_eq!(std::fs::read(&held).unwrap(), b"first");
}

#[test]
fn a_close_queues_the_ask_and_keep_converting_changes_nothing() {
    let rig = Rig::new();
    converting(&rig);
    // The close's own handler only queues the prompt: nothing is shown
    // with `App` borrowed.
    rig.app(|app| {
        app.close_window();
        assert_eq!(app.effects.names(), ["ask"]);
        // A queued prompt counts as open, as a shown one did.
        assert!(system::modal_open());
        app.effects.clear();
        app.leave_answered(PromptButton::Cancel, (Rows::All, Then::Close));
        assert!(app.effects.is_empty());
        assert!(!app.destroyed);
    });
    assert!(!system::modal_open());
}

#[test]
fn the_session_ending_during_the_close_prompt_keeps_what_is_held() {
    let rig = Rig::new();
    let held = rig.held_file();
    converting(&rig);
    script::answer_with(|hwnd, effect| match effect {
        Effect::Ask { .. } => {
            // Windows ends the session while the prompt is up; its loop
            // delivers it.
            send(hwnd, WM_ENDSESSION, 1);
            Reply::Button(PromptButton::Stop(Leave::Close))
        }
        other => panic!("unexpected {}", other.name()),
    });
    rig.send(WM_CLOSE, 0);
    // The window went at once, inside the prompt's loop; the answer came
    // too late to do anything.
    assert_eq!(script::ran(), ["ask", "destroy"]);
    assert!(!rig.alive());
    assert!(held.exists());
    rig.app(|app| {
        assert!(app.destroyed);
        assert!(app.keep_holding);
        assert!(app.effects.is_empty());
    });
    #[cfg(debug_assertions)]
    assert_eq!(rig.cell().nested(), 0);
}

#[test]
fn the_session_ending_with_a_prompt_queued_drops_it_unasked() {
    let rig = Rig::new();
    let held = rig.held_file();
    let answered = Rc::new(RefCell::new(false));
    let then = answered.clone();
    rig.app(|app| {
        app.ask(close_prompt(), move |_, _| *then.borrow_mut() = true);
    });
    script::answer_with(|_, effect| panic!("nothing is asked: {}", effect.name()));
    rig.send(WM_ENDSESSION, 1);
    assert_eq!(script::ran(), ["destroy"]);
    assert!(!*answered.borrow());
    assert!(!rig.alive());
    assert!(held.exists());
    rig.app(|app| assert!(app.keep_holding));
    assert!(!system::modal_open());
}

#[test]
fn messages_a_prompts_loop_delivers_find_the_app_free() {
    let rig = Rig::new();
    script::answer_with(|hwnd, effect| match effect {
        Effect::Ask { .. } => {
            // As a task dialog's loop would: a paint and a timer for the
            // window under it.
            send(hwnd, WM_PAINT, 0);
            send(hwnd, WM_KILN_FRAME, 0);
            send(hwnd, WM_TIMER, MODAL_FRAME_TIMER);
            Reply::Button(PromptButton::Cancel)
        }
        other => panic!("unexpected {}", other.name()),
    });
    converting(&rig);
    rig.send(WM_CLOSE, 0);
    assert_eq!(script::ran(), ["ask"]);
    assert!(rig.alive());
    #[cfg(debug_assertions)]
    assert_eq!(rig.cell().nested(), 0);
}

#[test]
fn a_prompt_queued_inside_another_waits_for_it_to_close() {
    let rig = Rig::new();
    let order = Rc::new(RefCell::new(vec![]));
    let log = order.clone();
    script::answer_with(move |hwnd, effect| {
        let t = title(effect).unwrap();
        log.borrow_mut().push(format!("open {t}"));
        if t == "outer" {
            // A message the outer prompt's loop delivers queues a prompt
            // of its own, and its runner runs.
            let cell = AppCell::of(hwnd).unwrap();
            let inner = Prompt {
                title: "inner".to_owned(),
                ..close_prompt()
            };
            cell.with(|app| app.ask(inner, |_, _| {})).unwrap();
            run_effects(hwnd, cell);
            // It waits: one modal at a time.
            assert_eq!(cell.with(|app| app.effects.names()).unwrap(), ["ask"]);
        }
        log.borrow_mut().push(format!("close {t}"));
        Reply::Button(PromptButton::Cancel)
    });
    let outer = Prompt {
        title: "outer".to_owned(),
        ..close_prompt()
    };
    rig.app(|app| app.ask(outer, |_, _| {}));
    run_effects(rig.hwnd, rig.cell());
    assert_eq!(
        *order.borrow(),
        ["open outer", "close outer", "open inner", "close inner"]
    );
    assert!(rig.app(|app| app.effects.is_empty()));
}

#[test]
fn a_tripped_guard_drops_queued_effects_unrun() {
    let rig = Rig::new();
    let ran = Rc::new(RefCell::new(false));
    let then = ran.clone();
    rig.app(|app| {
        app.ask(close_prompt(), |_, _| {});
        app.effect(Effect::Run(Box::new(move |_| *then.borrow_mut() = true)));
        app.effect(Effect::Destroy);
    });
    let guard = crash::Guard::new();
    assert!(guard.run(|| panic!("a test trips its own guard")).is_none());
    assert!(guard.tripped());
    script::answer_with(|_, effect| panic!("nothing runs: {}", effect.name()));
    run_effects_in(&guard, rig.hwnd, rig.cell());
    assert!(script::ran().is_empty());
    assert!(!*ran.borrow());
    assert!(rig.alive());
    assert!(rig.app(|app| app.effects.is_empty()));
    assert!(!system::modal_open());
}

/// Whether `msg` waits in this thread's queue for `hwnd`, taken out.
fn queued(hwnd: HWND, msg: u32) -> bool {
    let mut m = MSG::default();
    // SAFETY: this thread's own queue.
    unsafe { PeekMessageW(&mut m, Some(hwnd), msg, msg, PM_REMOVE) }.as_bool()
}

#[test]
fn a_message_that_finds_the_app_borrowed_never_touches_it() {
    let rig = Rig::new();
    let hwnd = rig.hwnd;
    rig.app(|_| {
        // A paint is redone once the borrow ends; a close is posted again
        // then, never carried out by Windows' default answer, and not
        // while the borrow lasts, where a loop would only spin on it.
        send(hwnd, WM_PAINT, 0);
        send(hwnd, WM_CLOSE, 0);
        send(hwnd, WM_CLOSE, 0);
        assert!(!queued(hwnd, WM_CLOSE));
    });
    #[cfg(debug_assertions)]
    assert_eq!(rig.cell().nested(), 3);
    assert!(rig.alive());
    assert!(queued(hwnd, WM_CLOSE), "the close is posted again");
    assert!(!queued(hwnd, WM_CLOSE), "once");
}

#[test]
fn a_client_asking_for_the_tree_while_the_app_is_borrowed_gets_it() {
    let rig = Rig::new();
    let hwnd = rig.hwnd;
    // The cell has the root from the start: no client asks before there is
    // one to give.
    assert!(rig.cell().root().is_some());
    assert!(!rig.app(|app| app.uia.active()));
    rig.app(|_| {
        // SAFETY: a plain send to this thread's own window; WM_GETOBJECT's
        // arguments are numbers, not pointers.
        unsafe {
            SendMessageW(
                hwnd,
                WM_GETOBJECT,
                Some(WPARAM(0)),
                Some(LPARAM(uia::ROOT_OBJECT_ID as isize)),
            )
        };
    });
    #[cfg(debug_assertions)]
    assert_eq!(rig.cell().nested(), 1);
    // The next message sets the first client up, as one asking then would
    // have been.
    rig.send(WM_TIMER, 0);
    assert!(rig.app(|app| app.uia.active()));
    assert!(rig.cell().root().is_some());
}

/// Whether the window has part of itself waiting to be drawn.
#[cfg(debug_assertions)]
fn needs_paint(hwnd: HWND) -> bool {
    // SAFETY: asks only whether this thread's own window has an update
    // region.
    unsafe { windows::Win32::Graphics::Gdi::GetUpdateRect(hwnd, None, false) }.as_bool()
}

#[cfg(debug_assertions)]
#[test]
fn a_frame_lost_with_the_device_is_drawn_again_on_a_new_one() {
    let rig = Rig::new();
    let hwnd = rig.hwnd;
    // A hidden window keeps no update region, so this one is shown, cloaked
    // first: the compositor never puts it on the screen, and it is never
    // activated.
    cloak(hwnd, true);
    // SAFETY: a plain call on this thread's own window.
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNA);
    }
    // Help opening, as the row poured in: the frames are mid-animation.
    rig.app(|app| app.open_help());
    rig.send(WM_PAINT, 0);
    let first = rig.app(|app| {
        (
            app.renderer.presented(),
            app.renderer.devices(),
            app.renderer.frame_waitable(),
        )
    });
    // Shown, the window was drawn at once (WM_SIZE) and painted.
    let base = first.0;
    assert!(base >= 1, "the first frame is presented");
    assert_eq!(first.1, 1);
    assert!(first.2.is_some_and(|h| !h.is_invalid()));

    // The driver resets as the next frame is presented.
    rig.app(|app| app.renderer.lose_device_for_test());
    rig.send(WM_PAINT, 0);
    let lost = rig.app(|app| (app.renderer.presented(), app.renderer.frame_waitable()));
    assert_eq!(lost.0, base, "the lost frame never reached the screen");
    assert!(lost.1.is_none(), "everything on the device went with it");
    assert!(needs_paint(hwnd), "the window asks to be drawn again");

    // The paint that asks draws on a new device, and nothing more is asked.
    rig.send(WM_PAINT, 0);
    let again = rig.app(|app| (app.renderer.presented(), app.renderer.devices()));
    assert_eq!(
        again,
        (base + 1, 2),
        "the next paint draws, on a new device"
    );
    assert!(!needs_paint(hwnd));
    assert!(rig.app(|app| app.renderer.frame_waitable()).is_some());
}

/// Move the test window far off every screen and show it cloaked: it
/// keeps an update region and is composed, but nothing reaches the screen
/// and it is never activated.
#[cfg(debug_assertions)]
fn show_off_screen(hwnd: HWND) {
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
    };
    // SAFETY: plain calls on this thread's own window.
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            None,
            -32000,
            -32000,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
    cloak(hwnd, true);
    // SAFETY: as above.
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNA);
    }
}

#[cfg(debug_assertions)]
#[test]
fn the_first_frame_is_presented_while_cloaked_even_when_its_device_is_lost() {
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
    };
    let rig = Rig::new();
    let hwnd = rig.hwnd;
    // Off every screen, so uncloaking shows nothing.
    // SAFETY: a plain call on this thread's own window.
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            None,
            -32000,
            -32000,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
    // The first frame's present is lost with its device.
    rig.app(|app| app.renderer.lose_device_for_test());
    show_first_frame(hwnd, rig.cell(), SW_SHOWNA);
    let (presented, devices, cloaked_at) = rig.app(|app| {
        (
            app.renderer.presented(),
            app.renderer.devices(),
            app.renderer.cloaked_at_present().to_vec(),
        )
    });
    assert_eq!(devices, 2, "the lost first frame was drawn again");
    assert_eq!(presented, 1);
    assert_eq!(cloaked_at, vec![true], "presented while cloaked");
    assert!(
        !rig.app(|app| app.renderer.window_cloaked()),
        "uncloaked after"
    );
}

#[cfg(debug_assertions)]
#[test]
fn a_device_lost_again_and_again_falls_back_to_warp() {
    let rig = Rig::new();
    let hwnd = rig.hwnd;
    show_off_screen(hwnd);
    rig.send(WM_PAINT, 0);
    let base = rig.app(|app| app.renderer.presented());
    let hardware = rig.app(|app| !app.renderer.warp());
    for _ in 0..3 {
        rig.app(|app| app.renderer.lose_device_for_test());
        rig.send(WM_PAINT, 0);
    }
    rig.send(WM_PAINT, 0);
    let (presented, devices, warp) = rig.app(|app| {
        (
            app.renderer.presented(),
            app.renderer.devices(),
            app.renderer.warp(),
        )
    });
    assert_eq!(
        (presented, devices),
        (base + 1, 4),
        "drawn again on a new device"
    );
    assert!(warp, "the device after three losses in a row is WARP's");
    if !hardware {
        println!("note: this machine draws with WARP from the start");
    }
}

#[cfg(debug_assertions)]
#[test]
fn a_device_lost_again_and_again_stops_asking_for_repaints() {
    let rig = Rig::new();
    let hwnd = rig.hwnd;
    show_off_screen(hwnd);
    rig.send(WM_PAINT, 0);
    let base = rig.app(|app| app.renderer.presented());
    for i in 1..=10 {
        rig.app(|app| app.renderer.lose_device_for_test());
        rig.send(WM_PAINT, 0);
        assert_eq!(needs_paint(hwnd), i < 10, "loss {i}");
    }
    // Anything else that draws the window tries again.
    rig.send(WM_PAINT, 0);
    assert_eq!(rig.app(|app| app.renderer.presented()), base + 1);
}

#[test]
fn a_silent_video_dragged_onto_the_floating_wheel_loses_its_sound_formats() {
    // a silent AVI dragged from another app reaches the
    // floating wheel with no facts, so MP3 is on it; the window reads the
    // file, and once its facts are in the wheel holds no sound format.
    use windows::Win32::System::Ole::{OleInitialize, OleUninitialize};
    let rig = empty_rig();
    // SAFETY: OLE on this test's thread, for the wheel's drop target, let
    // go of at the end.
    unsafe { OleInitialize(None) }.unwrap();
    let avi = rig.dir.path().join("s.avi");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../kiln-media/tests/data/silent.avi"),
        &avi,
    )
    .unwrap();
    // The window's inspector, its results handed to this test instead of
    // posted to a window no one pumps for them.
    let (tx, rx) = std::sync::mpsc::channel();
    let inspect_kiln = Kiln::new();
    rig.app(|app| {
        app.inspector = Inspector::new(
            move |path, opts, cancel| kiln_core::inspect(&inspect_kiln, path, opts, cancel),
            move |path, facts| {
                if let Some(facts) = facts {
                    let _ = tx.send((path, facts));
                }
            },
        );
    });
    let fw = FloatWheel::open(
        |_| {},
        (400, 400),
        WheelKind::Convert,
        &Settings::default(),
        Rc::new(std::cell::Cell::new(false)),
    )
    .unwrap();
    let hwnd = fw.hwnd();
    let sound = |app: &mut App| {
        let w = app.float.as_ref().and_then(|fw| fw.wheel())?;
        Some(
            w.items
                .iter()
                .chain(w.flyout.iter().flat_map(|g| g.items.iter()))
                .filter(|i| i.kind == Some(kiln_core::Kind::Audio))
                .map(|i| i.label.clone())
                .collect::<Vec<_>>(),
        )
    };
    rig.app(|app| {
        app.float = Some(fw);
        app.float_news(FloatNote::from_wheel(
            hwnd,
            FloatNews::Files(vec![avi.clone()]),
        ));
    });
    assert!(
        pump_until(&rig, Duration::from_secs(5), |app| sound(app).is_some()),
        "the wheel blooms"
    );
    let before = rig.app(|app| sound(app)).unwrap();
    assert!(before.contains(&"MP3".to_owned()), "{before:?}");
    // The dragged file was queued for reading when it arrived.
    let (path, facts) = rx
        .recv_timeout(Duration::from_secs(30))
        .expect("the dragged file is read");
    assert_eq!(path, avi);
    assert!(facts.silent, "{facts:?}");
    rig.app(|app| {
        app.on_inspected(Inspected { path, facts });
        assert_eq!(sound(app), Some(vec![]));
        let w = app.float.as_ref().and_then(|fw| fw.wheel()).unwrap();
        assert!(w.items.iter().any(|i| i.label == "MP4"));
        app.close_float();
    });
    // SAFETY: paired with the OleInitialize above.
    unsafe { OleUninitialize() };
}

#[test]
fn a_floating_wheel_goes_on_fading_under_a_prompt() {
    use windows::Win32::System::Ole::{OleInitialize, OleUninitialize};
    // The window stays hidden, as in the tray: only the floating wheel moves.
    let rig = Rig::new();
    // SAFETY: OLE on this test's thread, for the wheel's drop target, let
    // go of at the end.
    unsafe { OleInitialize(None) }.unwrap();
    let photo = rig.dir.path().join("photo.png");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/photo.png"),
        &photo,
    )
    .unwrap();
    let mut fw = FloatWheel::open(
        |_| {},
        (400, 400),
        WheelKind::Convert,
        &Settings::default(),
        Rc::new(std::cell::Cell::new(false)),
    )
    .unwrap();
    fw.files_arrived(vec![photo], &Kiln::new(), &Availability::default());
    assert!(
        theme::fluent::motion::animations(),
        "Animation effects are on"
    );
    let fading = fw.fade_out().expect("a wheel with files fades out");
    let wheel = fading.hwnd();
    rig.app(|app| app.float_fading = Some(fading));
    // A prompt's own loop runs this thread's messages for a second, as a
    // task dialog's would; nothing else draws a frame meanwhile.
    script::answer_with(|_, effect| match effect {
        Effect::Ask { .. } => {
            let until = Instant::now() + Duration::from_secs(1);
            let mut msg = MSG::default();
            while Instant::now() < until {
                // SAFETY: this thread's own queue.
                unsafe {
                    while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                        DispatchMessageW(&msg);
                    }
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            Reply::Button(PromptButton::Cancel)
        }
        other => panic!("unexpected {}", other.name()),
    });
    rig.app(|app| app.ask(close_prompt(), |_, _| {}));
    run_effects(rig.hwnd, rig.cell());
    assert_eq!(script::ran(), ["ask"]);
    // It faded out under the prompt, and its window went.
    // SAFETY: asks only whether the handle is still a window.
    assert!(!unsafe { IsWindow(Some(wheel)) }.as_bool());
    rig.app(|app| app.float_fading = None);
    // SAFETY: matches the OleInitialize above.
    unsafe { OleUninitialize() };
}

/// Windows' pixel rectangle for `r`, in DIPs, at the test window's 100%.
#[cfg(debug_assertions)]
fn px(r: &layout::Rect) -> (i32, i32, i32, i32) {
    (
        r.x.floor() as i32,
        r.y.floor() as i32,
        r.right().ceil() as i32,
        r.bottom().ceil() as i32,
    )
}

#[cfg(debug_assertions)]
#[test]
fn a_running_rows_progress_is_presented_alone_and_anything_else_presents_the_window() {
    let rig = Rig::new();
    let hwnd = rig.hwnd;
    show_off_screen(hwnd);
    rig.app(|app| {
        app.files.entries_mut()[0].job = JobState::Running {
            action: Action::Convert("jpg".into()),
            fraction: Some(0.25),
            started: None,
        };
    });
    // Nothing else moves once the row has poured in: the progress clock's pace.
    let mut settled = false;
    for _ in 0..60 {
        rig.send(WM_PAINT, 0);
        if rig.app(|app| app.pace()) == anim::Pace::Progress {
            settled = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(settled, "the window comes to the progress clock's pace");
    let (row, panel) = rig.app(|app| {
        let l = app.layout();
        (l.rows[0], l.panel)
    });

    // The progress clock's frame, with nothing else asked for: just the row.
    rig.send(WM_PAINT, 0);
    let before = rig.app(|app| app.renderer.dirty_at_present().len());
    rig.send(WM_KILN_FRAME, 0);
    let shown = rig.app(|app| app.renderer.dirty_at_present().to_vec());
    assert_eq!(shown.len(), before + 1, "the frame was presented");
    let Some(Some(rects)) = shown.last() else {
        panic!(
            "the progress clock's frame presented the whole window: {:?}",
            shown.last()
        );
    };
    assert!(!rects.is_empty());
    let (left, top, right, bottom) = px(&row);
    let (pl, pt, pr, pb) = px(&panel);
    for r in rects {
        assert!(
            r.left >= left.max(pl)
                && r.top >= top.max(pt)
                && r.right <= right.min(pr)
                && r.bottom <= bottom.min(pb),
            "{r:?} lies outside the row {:?}",
            (left, top, right, bottom)
        );
    }
    // The row is all of it: nothing but that row was marked.
    assert_eq!(rects.len(), 1);

    // Anything else drawing the window, such as the pointer moving over a
    // row, presents all of it.
    rig.app(|app| app.invalidate());
    rig.send(WM_KILN_FRAME, 0);
    assert_eq!(
        rig.app(|app| app.renderer.dirty_at_present().last().cloned()),
        Some(None),
        "an invalidated window is drawn whole"
    );
    // And so does Windows asking for a paint.
    rig.send(WM_PAINT, 0);
    assert_eq!(
        rig.app(|app| app.renderer.dirty_at_present().last().cloned()),
        Some(None)
    );
    // The progress clock again, after a whole frame: the row alone once more.
    rig.send(WM_KILN_FRAME, 0);
    assert!(matches!(
        rig.app(|app| app.renderer.dirty_at_present().last().cloned()),
        Some(Some(_))
    ));

    // The frame after one drawn at the display's pace is whole: it is
    // where an animation ends, and its last state reaches the whole window.
    rig.app(|app| app.last_pace.set(anim::Pace::Display));
    rig.send(WM_KILN_FRAME, 0);
    assert_eq!(
        rig.app(|app| app.renderer.dirty_at_present().last().cloned()),
        Some(None),
        "the frame after a Display frame is whole"
    );
    rig.send(WM_KILN_FRAME, 0);
    assert!(matches!(
        rig.app(|app| app.renderer.dirty_at_present().last().cloned()),
        Some(Some(_))
    ));

    // A frame that never reached the screen leaves the next one whole.
    let shown = rig.app(|app| app.renderer.presented());
    rig.app(|app| app.renderer.lose_device_for_test());
    rig.send(WM_KILN_FRAME, 0);
    assert_eq!(rig.app(|app| app.renderer.presented()), shown);
    assert!(
        rig.app(|app| app.whole.get()),
        "the lost frame asks for a whole one"
    );
}

#[cfg(debug_assertions)]
#[test]
fn the_render_caches_start_again_with_a_new_device_scale_or_theme() {
    let rig = Rig::new();
    show_off_screen(rig.hwnd);
    rig.send(WM_PAINT, 0);
    // Frames until the window's own settling (its size, the first slides)
    // has stopped adding layouts: only then is "as it was" the same boxes.
    let mut seen = 0;
    for n in 0..40 {
        rig.app(|app| app.invalidate());
        rig.send(WM_PAINT, 0);
        let kept = rig.app(|app| app.renderer.caches().1);
        if n > 0 && kept == seen {
            break;
        }
        seen = kept;
    }
    let (clears, kept) = rig.app(|app| app.renderer.caches());
    assert!(kept > 0, "a frame lays out its text");
    // A frame again as it was uses what is kept and adds nothing.
    rig.app(|app| app.invalidate());
    rig.send(WM_PAINT, 0);
    let (again, kept_again) = rig.app(|app| app.renderer.caches());
    assert_eq!(again, clears, "nothing cleared them");
    assert_eq!(kept_again, kept, "the same words are the same layouts");

    // A new scale (and so a new Text size, which zooms the scale).
    rig.app(|app| app.renderer.set_dpi(144.0));
    let (c, k) = rig.app(|app| app.renderer.caches());
    assert_eq!((c, k), (clears + 1, 0), "a new scale");
    rig.send(WM_PAINT, 0);
    assert!(rig.app(|app| app.renderer.caches().1) > 0);

    // A new theme.
    rig.app(|app| {
        app.prefs.palette = if app.prefs.palette == theme::fluent::LIGHT {
            theme::fluent::DARK
        } else {
            theme::fluent::LIGHT
        };
        app.invalidate();
    });
    rig.send(WM_PAINT, 0);
    assert_eq!(rig.app(|app| app.renderer.caches().0), clears + 2);

    // A lost device.
    rig.app(|app| app.renderer.lose_device_for_test());
    rig.send(WM_PAINT, 0);
    assert_eq!(rig.app(|app| app.renderer.caches().0), clears + 3);
}

/// A drive letter no volume has on this PC, so nothing is made on it.
fn unused_drive() -> Option<char> {
    // SAFETY: takes no arguments.
    let mask = unsafe { windows::Win32::Storage::FileSystem::GetLogicalDrives() };
    (b'D'..=b'Z')
        .rev()
        .find(|&c| mask & (1 << (c - b'A')) == 0)
        .map(char::from)
}

#[test]
fn a_file_already_in_the_format_picked_is_left_alone_and_says_so() {
    // UX-06: a JPG and a PNG, converted to JPG: the PNG converts, and the
    // JPG's row says "Already JPG" and starts nothing.
    let rig = Rig::new();
    let (jpg, png) = (rig.dir.path().join("a.jpg"), rig.dir.path().join("b.png"));
    rig.app(|app| {
        for (i, p) in [&jpg, &png].into_iter().enumerate() {
            let e = model::entry_for(&app.kiln, &Availability::default(), p.clone(), 1, None);
            app.files.insert(1 + i, e);
            app.motion.rows_added(1 + i, 1, Instant::now());
        }
        app.start(
            Action::Convert("jpg".into()),
            vec![jpg.clone(), png.clone()],
            None,
        );
        assert_eq!(
            app.notes.get(&jpg),
            Some(&("Already JPG".to_owned(), model::Tone::Muted))
        );
        assert_eq!(app.files.entries()[1].job, JobState::Idle);
        assert!(matches!(
            app.files.entries()[2].job,
            JobState::Running { .. }
        ));
        assert!(app.notes.get(&png).is_none());
        // Only the PNG was asked of the runner.
        let asked: Vec<&PathBuf> = app.work.jobs.values().flat_map(|j| &j.paths).collect();
        assert_eq!(asked, [&png]);
        app.work.runner.cancel_all();
    });
}

/// List `paths` after the rig's own row, as a drop would, and return them.
fn list(app: &mut App, paths: &[&PathBuf]) {
    for (i, p) in paths.iter().enumerate() {
        let e = model::entry_for(&app.kiln, &Availability::default(), (*p).clone(), 1, None);
        app.files.insert(1 + i, e);
        app.motion.rows_added(1 + i, 1, Instant::now());
    }
}

/// Which files were asked of the runner.
fn asked(app: &App) -> Vec<PathBuf> {
    app.work
        .jobs
        .values()
        .flat_map(|j| j.paths.clone())
        .collect()
}

fn muted(line: &str) -> Option<(String, model::Tone)> {
    Some((line.to_owned(), model::Tone::Muted))
}

#[test]
fn a_file_that_cant_become_the_format_picked_is_left_out_and_says_so() {
    // A DOCX and a PDF are offered WEBP, which only the PDF can become:
    // only the PDF's job starts, and the DOCX's row says why it didn't.
    let rig = Rig::new();
    let (docx, pdf) = (rig.dir.path().join("a.docx"), rig.dir.path().join("b.pdf"));
    rig.app(|app| {
        list(app, &[&docx, &pdf]);
        app.start(
            Action::Convert("webp".into()),
            vec![docx.clone(), pdf.clone()],
            None,
        );
        assert_eq!(app.notes.get(&docx).cloned(), muted("Can't become WEBP"));
        assert_eq!(app.files.entries()[1].job, JobState::Idle);
        assert!(app.notes.get(&pdf).is_none());
        assert_eq!(asked(app), std::slice::from_ref(&pdf));
        app.work.runner.cancel_all();
    });
}

#[test]
fn a_video_with_no_subtitle_track_is_left_out_of_a_subtitle_conversion() {
    // An MKV that holds subtitles and an AVI that holds none, to SRT: the
    // MKV's job starts; the AVI is left out with its note.
    let rig = Rig::new();
    let (mkv, avi) = (rig.dir.path().join("a.mkv"), rig.dir.path().join("b.avi"));
    rig.app(|app| {
        list(app, &[&mkv, &avi]);
        app.files.entries_mut()[1].facts = Some(kiln_core::Facts {
            ext: "mkv".into(),
            kind: kiln_core::Kind::Video,
            bytes: 1,
            picture: None,
            media: Some(kiln_core::MediaInfo {
                subtitle_tracks: 1,
                ..kiln_core::MediaInfo::default()
            }),
            pdf: None,
            bars: None,
            estimates: Default::default(),
            compressed: None,
            audio_problem: None,
            silent: false,
        });
        app.start(
            Action::Convert("srt".into()),
            vec![mkv.clone(), avi.clone()],
            None,
        );
        assert_eq!(app.notes.get(&avi).cloned(), muted("Can't become SRT"));
        assert_eq!(app.files.entries()[2].job, JobState::Idle);
        assert_eq!(asked(app), std::slice::from_ref(&mkv));
        app.work.runner.cancel_all();
    });
}

#[test]
fn a_file_that_needs_a_missing_helper_for_the_format_is_left_out() {
    // Without FFmpeg, an MP4 still becomes MOV on Kiln's own engine and
    // an AVI can't: the AVI is left out, not started and failed. Its row
    // names the helper and gets it.
    let rig = Rig::new();
    let (mp4, avi) = (rig.dir.path().join("a.mp4"), rig.dir.path().join("b.avi"));
    rig.app(|app| {
        let look = app.engines.ask();
        app.engines
            .found(look, Availability::all().without(&["ffmpeg"]));
        list(app, &[&mp4, &avi]);
        app.start(
            Action::Convert("mov".into()),
            vec![mp4.clone(), avi.clone()],
            None,
        );
        assert_eq!(
            app.notes.get(&avi).cloned(),
            muted("Needs FFmpeg to become MOV")
        );
        assert_eq!(app.files.entries()[2].job, JobState::Idle);
        assert_eq!(app.files.entries()[2].link(false), Some("Get FFmpeg"));
        assert_eq!(asked(app), std::slice::from_ref(&mp4));
        // FFmpeg has no download page: Get it opens Settings at its card.
        app.follow_link(2, false);
        let settings = app.settings_ui.as_ref().expect("Settings opened");
        assert!(settings.is_open(settings_page::Helper::Ffmpeg));
        assert!(app.effects.is_empty(), "nothing opened outside Kiln");
        app.settings_ui = None;
        // Selected, the side pane has the same Get it.
        app.files.select_only(&[2]);
        app.refresh_panel();
        assert!(app.live_panel().is_some(), "the pane shows");
        let pane = app.live_panel().unwrap().model.clone();
        assert!(pane.controls().contains(&InspectorControl::GetHelper));
        assert_eq!(pane.label(InspectorControl::GetHelper), Some("Get FFmpeg"));
        app.panel_act(InspectorControl::GetHelper, false);
        let settings = app.settings_ui.as_ref().expect("Settings opened");
        assert!(settings.is_open(settings_page::Helper::Ffmpeg));
        app.settings_ui = None;
        app.files.select_only(&[]);
        // None of them can: nothing starts, and every row says why.
        app.work.runner.cancel_all();
        app.work.jobs.clear();
        let wav = rig.dir.path().join("c.wav");
        list(app, &[&wav]);
        app.start(Action::Convert("mov".into()), vec![wav.clone()], None);
        assert_eq!(app.notes.get(&wav).cloned(), muted("Can't become MOV"));
        assert!(app.work.jobs.is_empty());
        let i = app.files.index_of(&wav).unwrap();
        assert_ne!(app.files.entries()[i].link(false), Some("Get FFmpeg"));
    });
}

#[test]
fn a_row_waiting_on_a_helper_gets_it_and_clears_once_it_is_found() {
    // A Markdown file and a text file to DOCX without Pandoc: the text
    // file goes ahead, the Markdown file's row says it needs Pandoc, and
    // its Get it opens Pandoc's download page, only that.
    static OPENED: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(vec![]);
    let rig = Rig::new();
    script::answer_with(|_, effect| match effect {
        Effect::Open { target, .. } => {
            OPENED.lock().unwrap().push(target.clone());
            Reply::Ok(true)
        }
        other => panic!("unexpected {}", other.name()),
    });
    let (md, txt) = (rig.dir.path().join("a.md"), rig.dir.path().join("b.txt"));
    let no_pandoc = Availability::all().without(&["pandoc"]);
    rig.app(|app| {
        let look = app.engines.ask();
        app.engines.found(look, no_pandoc.clone());
        list(app, &[&md, &txt]);
        app.start(
            Action::Convert("docx".into()),
            vec![md.clone(), txt.clone()],
            None,
        );
        assert_eq!(
            app.notes.get(&md).cloned(),
            muted("Needs Pandoc to become DOCX")
        );
        assert_eq!(asked(app), std::slice::from_ref(&txt));
        let i = app.files.index_of(&md).unwrap();
        assert_eq!(app.files.entries()[i].link(false), Some("Get Pandoc"));
        app.follow_link(i, false);
    });
    run_effects(rig.hwnd, rig.cell());
    assert_eq!(steps(), ["open"]);
    assert_eq!(
        *OPENED.lock().unwrap(),
        [PathBuf::from(model::helper_page("pandoc").unwrap())]
    );
    // Pandoc is found: the row says its own line again and Get it goes.
    rig.app(|app| {
        let look = app.engines.ask();
        app.engines_found(workers::EnginesFound {
            id: look,
            available: Availability::all(),
            failed: false,
            status: None,
            office: None,
            save_to: None,
            winget: None,
        });
        let i = app.files.index_of(&md).unwrap();
        assert_eq!(app.notes.get(&md), None);
        assert_ne!(app.files.entries()[i].link(false), Some("Get Pandoc"));
    });
}

#[test]
fn a_new_job_on_a_row_holding_a_result_asks_nothing_and_keeps_it() {
    // The rig's row holds a result. A new job on it asks nothing: the old
    // result is never removed, and the JPG beside it is told at once that
    // it is in the format already.
    let rig = Rig::new();
    let held = rig.held_file();
    let jpg = rig.dir.path().join("b.jpg");
    let held_row = rig.app(|app| {
        list(app, &[&jpg]);
        app.files.entries()[0].path.clone()
    });
    script::answer_with(|_, effect| panic!("nothing is asked: {}", effect.name()));
    rig.app(|app| {
        app.start(
            Action::Convert("jpg".into()),
            vec![held_row.clone(), jpg.clone()],
            None,
        );
        assert!(!app.effects.names().contains(&"ask"));
        assert_eq!(
            app.notes.get(&jpg).cloned(),
            Some(("Already JPG".to_owned(), model::Tone::Muted))
        );
        app.work.runner.cancel_all();
    });
    assert_eq!(std::fs::read(&held).unwrap(), b"first");
}

#[test]
fn a_drag_wheel_drop_tells_of_the_files_it_left_out() {
    // A drop of a JPG and a PNG on JPG: the PNG converts, the JPG's row
    // keeps its note for when the window is shown, and the drop's notice
    // will name the JPG too, as the window may be hidden.
    let rig = Rig::new();
    let (jpg, png) = (rig.dir.path().join("a.jpg"), rig.dir.path().join("b.png"));
    rig.app(|app| {
        list(app, &[&jpg, &png]);
        app.launch_as(
            Action::Convert("jpg".into()),
            vec![jpg.clone(), png.clone()],
            None,
            Origin::Drag,
        );
        assert_eq!(app.notes.get(&jpg).cloned(), muted("Already JPG"));
        assert_eq!(asked(app), std::slice::from_ref(&png));
        let batch = app.work.batches.values().next().expect("the PNG's batch");
        assert_eq!(batch.files, 1);
        assert_eq!(batch.left, ["a.jpg is already JPG."]);
        app.work.runner.cancel_all();
    });
}

#[test]
fn a_job_from_a_drive_not_yet_held_on_starts_once_its_folder_is_open() {
    let Some(letter) = unused_drive() else {
        eprintln!("skipped: every drive letter is in use");
        return;
    };
    let rig = Rig::new();
    let source = PathBuf::from(format!(r"{letter}:\pictures\b.png"));
    rig.app(|app| {
        let e = model::entry_for(&app.kiln, &Availability::default(), source.clone(), 1, None);
        app.files.insert(1, e);
        app.motion.rows_added(1, 1, Instant::now());
        // Its drive's folder isn't open: a worker opens it, and the job
        // waits, with nothing started on the UI thread.
        assert!(app.held.as_ref().unwrap().folder_now(&source).is_none());
        let action = Action::Convert("jpg".into());
        app.launch_with(
            action,
            vec![source.clone()],
            None,
            Origin::Window,
            Via::Window,
        );
        assert_eq!(app.work.pending.len(), 1);
        assert!(app.work.jobs.is_empty());
        // Its row shows as queued meanwhile.
        assert!(matches!(
            app.files.entries()[1].job,
            JobState::Running { fraction: None, .. }
        ));
    });
    // The worker's answer comes through the window's channel.
    let until = Instant::now() + Duration::from_secs(10);
    let mut msg = MSG::default();
    while rig.app(|app| app.work.jobs.is_empty()) && Instant::now() < until {
        // SAFETY: this thread's own queue.
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                DispatchMessageW(&msg);
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    rig.app(|app| {
        assert!(app.work.pending.is_empty());
        let held = app.held.as_ref().unwrap();
        // A drive with no volume can't hold: its results wait locally.
        assert_eq!(held.folder_now(&source), Some(held.local()));
        let jobs: Vec<_> = app.work.jobs.values().collect();
        assert_eq!(jobs.len(), 1, "the job started once its folder was open");
        assert_eq!(jobs[0].paths, std::slice::from_ref(&source));
        assert_eq!(jobs[0].out_dir.as_deref(), Some(held.local()));
        app.work.runner.cancel_all();
    });
}

#[test]
fn a_cancel_while_a_job_waits_for_its_folder_starts_nothing() {
    let Some(letter) = unused_drive() else {
        eprintln!("skipped: every drive letter is in use");
        return;
    };
    let rig = Rig::new();
    let source = PathBuf::from(format!(r"{letter}:\pictures\c.png"));
    rig.app(|app| {
        let e = model::entry_for(&app.kiln, &Availability::default(), source.clone(), 1, None);
        app.files.insert(1, e);
        app.motion.rows_added(1, 1, Instant::now());
        let action = Action::Convert("jpg".into());
        app.launch_with(
            action,
            vec![source.clone()],
            None,
            Origin::Window,
            Via::Window,
        );
        assert_eq!(app.work.pending.len(), 1);
        app.cancel_row(1);
        assert!(app.work.pending.is_empty());
        assert_eq!(app.files.entries()[1].job, JobState::Cancelled);
    });
    // The folder's answer still comes, and starts nothing.
    let until = Instant::now() + Duration::from_secs(2);
    let mut msg = MSG::default();
    while Instant::now() < until {
        // SAFETY: this thread's own queue.
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                DispatchMessageW(&msg);
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    rig.app(|app| {
        assert!(app.work.jobs.is_empty());
        assert_eq!(app.files.entries()[1].job, JobState::Cancelled);
    });
}

/// Deliver this thread's messages until `done` says so, or `limit` runs
/// out. True if `done` did.
fn pump_until(rig: &Rig, limit: Duration, done: impl Fn(&mut App) -> bool) -> bool {
    let until = Instant::now() + limit;
    let mut msg = MSG::default();
    loop {
        if rig.app(&done) {
            return true;
        }
        if Instant::now() >= until {
            return false;
        }
        // SAFETY: this thread's own queue.
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                DispatchMessageW(&msg);
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// A folder `name` in `dir` of `n` small PNGs.
fn folder_of(dir: &std::path::Path, name: &str, n: usize) -> PathBuf {
    let folder = dir.join(name);
    std::fs::create_dir_all(&folder).unwrap();
    for i in 0..n {
        std::fs::write(folder.join(format!("{i:04}.png")), b"\x89PNG\r\n\x1a\nrest").unwrap();
    }
    folder
}

#[test]
fn a_big_folder_is_asked_about_as_soon_as_it_is_listed_and_the_walk_goes_on() {
    let rig = Rig::new();
    rig.app(|app| {
        // A walk with a thread of none: its batches come from the test.
        let id = app.incoming.next_walk;
        app.incoming.next_walk += 1;
        app.incoming.walks.push(Walk {
            id,
            cancel: CancelToken::new(),
            keys: false,
            ask: WalkAsk::Unknown,
            files: vec![],
            found: model::Gathered::default(),
            total: 0,
            done: false,
            soon: false,
            added: model::Added::default(),
            listed: model::ListedPaths::default(),
            fresh: vec![],
            then: vec![],
        });
        let batch = |found, done| model::WalkBatch {
            part: model::Gathered::default(),
            from_folders: found,
            found,
            listed: 1000,
            done,
        };
        // The listing's batch, before any file is read: the prompt is
        // queued at once, with the whole count.
        app.walked(Walked {
            id,
            batch: batch(0, false),
        });
        assert_eq!(app.effects.names(), ["ask"]);
        assert_eq!(app.incoming.walks[0].ask, WalkAsk::Open);
        assert!(!app.incoming.walks[0].done);
        // The walk goes on while it is open, and adds nothing meanwhile.
        app.walked(Walked {
            id,
            batch: batch(400, false),
        });
        assert_eq!(app.incoming.walks.len(), 1);
        assert_eq!(app.files.len(), 1);
        assert_eq!(app.effects.len(), 1, "asked once");
        app.effects.clear();
    });
}

#[test]
fn cancel_on_a_big_folder_stops_its_walk_and_adds_nothing() {
    let rig = Rig::new();
    let big = folder_of(rig.dir.path(), "Big", 1000);
    let asked = Rc::new(RefCell::new(vec![]));
    let seen = asked.clone();
    script::answer_with(move |hwnd, effect| match effect {
        Effect::Ask { .. } => {
            // Asked while the walk is under way, its prompt open.
            let open = AppCell::of(hwnd)
                .and_then(|c| c.with(|app| app.incoming.walks.first().map(|w| w.ask)))
                .flatten();
            seen.borrow_mut().push((title(effect).unwrap(), open));
            Reply::Button(PromptButton::Cancel)
        }
        other => panic!("unexpected {}", other.name()),
    });
    let before = rig.app(|app| app.files.len());
    rig.app(|app| app.add(vec![big], false));
    assert!(pump_until(&rig, Duration::from_secs(20), |app| app
        .incoming
        .walks
        .is_empty()));
    assert_eq!(
        *asked.borrow(),
        [(model::big_folder_prompt(1000).title, Some(WalkAsk::Open))]
    );
    // Batches still on their way when it was cancelled add nothing.
    pump_until(&rig, Duration::from_millis(300), |_| false);
    rig.app(|app| {
        assert_eq!(app.files.len(), before);
        assert_eq!(app.status.text, "Nothing added.");
    });
}

#[test]
fn add_all_on_a_big_folder_adds_every_file_once_with_one_line() {
    let rig = Rig::new();
    let big = folder_of(rig.dir.path(), "Big", 1000);
    script::answer_with(move |_, effect| match effect {
        Effect::Ask { .. } => Reply::Button(PromptButton::AddAll),
        other => panic!("unexpected {}", other.name()),
    });
    let before = rig.app(|app| app.files.len());
    let ran = Rc::new(Cell::new(false));
    let after = ran.clone();
    rig.app(|app| {
        app.add(vec![big.clone()], false);
        // What waits on the add runs once its rows are in.
        app.after_add(move |app| {
            assert_eq!(app.files.len(), before + 1000);
            after.set(true);
        });
    });
    assert!(pump_until(&rig, Duration::from_secs(30), |app| app
        .incoming
        .walks
        .is_empty()));
    assert!(ran.get());
    rig.app(|app| {
        assert_eq!(app.files.len(), before + 1000);
        assert_eq!(app.status.text, "Added 1000 files from Big.");
        // Every new row is selected (UX-05), the first focused.
        let all: Vec<usize> = (before..before + 1000).collect();
        assert_eq!(app.files.selection(), all);
        assert_eq!(app.files.focus(), Some(before));
    });
    // The same folder again: every file is already in the list.
    rig.app(|app| app.add(vec![big], false));
    assert!(pump_until(&rig, Duration::from_secs(30), |app| app
        .incoming
        .walks
        .is_empty()));
    rig.app(|app| {
        assert_eq!(app.files.len(), before + 1000);
        assert!(app
            .status
            .text
            .contains("1000 files were already in the list."));
    });
}

#[test]
fn a_settings_write_that_fails_says_so_in_the_footer() {
    let rig = Rig::new();
    rig.app(|app| {
        // Somewhere it can't be written: inside a file.
        let file = rig.dir.path().join("a file");
        std::fs::write(&file, b"x").unwrap();
        let path = file.join("settings.toml");
        app.prefs.store = settings::Store::new(Some(path));
        tell_writes(&mut app.prefs.store, &app.wake);
        app.prefs.settings.suggestions = !app.prefs.settings.suggestions;
        app.save_settings();
        // Saved off this thread: nothing is said yet.
        assert!(!app.status.text.starts_with("Settings couldn't be saved"));
    });
    assert!(pump_until(&rig, Duration::from_secs(10), |app| app
        .status
        .text
        .starts_with("Settings couldn't be saved")));
}

#[test]
fn the_end_of_a_session_writes_the_settings_before_it_is_answered() {
    let rig = Rig::new();
    let path = rig.dir.path().join("settings.toml");
    rig.app(|app| {
        app.prefs.settings.suggestions = !app.prefs.settings.suggestions;
        app.save_settings();
    });
    // Saved off this thread, a moment later: not yet.
    assert!(!path.exists());
    rig.send(WM_ENDSESSION, 1);
    assert!(path.exists(), "written before WM_ENDSESSION was answered");
}

#[test]
fn the_end_of_a_session_while_the_app_is_borrowed_writes_the_settings() {
    let rig = Rig::new();
    let path = rig.dir.path().join("settings.toml");
    let hwnd = rig.hwnd;
    rig.app(|app| {
        app.prefs.settings.suggestions = !app.prefs.settings.suggestions;
        app.save_settings();
        assert!(!path.exists());
        // Windows ends the session while `App` is in use, as from a
        // dialog's loop: the cell writes what was saved.
        send(hwnd, WM_ENDSESSION, 1);
        assert!(path.exists(), "written before WM_ENDSESSION was answered");
    });
}

/// Queue, behind a note that holds the notes worker until `gate` is let
/// go, where the held `a.jpg` came from. Its run folder's list of sources.
fn queue_a_source_note(app: &mut App, gate: std::sync::mpsc::Receiver<()>) -> PathBuf {
    let dir = app.held.as_ref().unwrap().local().to_path_buf();
    app.held_notes.send(Note::Hold(gate));
    app.held_notes.send(Note::Source {
        dir: dir.clone(),
        result: dir.join("a.jpg"),
        origin: PathBuf::from(r"C:\in"),
    });
    dir.join(kiln_core::SOURCES_NAME)
}

/// Let the notes worker go on after a moment, from another thread.
fn release_after(go: std::sync::mpsc::Sender<()>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        let _ = go.send(());
    })
}

#[test]
fn the_end_of_a_session_writes_the_held_results_notes_before_it_is_answered() {
    let rig = Rig::new();
    let (go, gate) = std::sync::mpsc::channel();
    let sources = rig.app(|app| queue_a_source_note(app, gate));
    // The worker is held: nothing is written yet.
    assert!(!sources.exists());
    let release = release_after(go);
    rig.send(WM_ENDSESSION, 1);
    let text =
        std::fs::read_to_string(&sources).expect("written before WM_ENDSESSION was answered");
    assert_eq!(text, "a.jpg\tC:\\in\n");
    release.join().unwrap();
}

#[test]
fn the_end_of_a_session_while_the_app_is_borrowed_writes_the_held_results_notes() {
    let rig = Rig::new();
    let hwnd = rig.hwnd;
    rig.app(|app| {
        let (go, gate) = std::sync::mpsc::channel();
        let sources = queue_a_source_note(app, gate);
        assert!(!sources.exists());
        let release = release_after(go);
        // Windows ends the session while `App` is in use, as from a
        // dialog's loop: the cell waits for the notes worker.
        send(hwnd, WM_ENDSESSION, 1);
        let text =
            std::fs::read_to_string(&sources).expect("written before WM_ENDSESSION was answered");
        assert_eq!(text, "a.jpg\tC:\\in\n");
        release.join().unwrap();
    });
}

/// A small PNG, `name`, in the rig's folder.
fn a_png(rig: &Rig, name: &str) -> PathBuf {
    let path = rig.dir.path().join(name);
    std::fs::write(&path, b"\x89PNG\r\n\x1a\nrest").unwrap();
    path
}

#[test]
fn files_that_arrive_while_last_times_results_are_looked_for_wait_for_the_answer() {
    let rig = Rig::new();
    let b = a_png(&rig, "b.png");
    rig.app(|app| {
        app.work.scanning = true;
        app.add_when_live(vec![b.clone()]);
        assert_eq!(app.incoming.queued, std::slice::from_ref(&b));
    });
    // Other messages meanwhile add nothing.
    assert!(!pump_until(&rig, Duration::from_millis(300), |app| app
        .incoming
        .queued
        .is_empty()));
    rig.app(|app| {
        assert_eq!(app.files.len(), 1);
        app.wake.send(Event::Swept);
    });
    assert!(pump_until(&rig, Duration::from_secs(30), |app| app
        .files
        .len()
        == 2));
    rig.app(|app| {
        assert!(!app.work.scanning);
        assert!(app.incoming.queued.is_empty());
    });
}

#[test]
fn files_wait_for_the_look_no_longer_than_its_timer() {
    let rig = Rig::new();
    let b = a_png(&rig, "b.png");
    rig.app(|app| {
        app.work.scanning = true;
        app.add_when_live(vec![b.clone()]);
    });
    rig.send(WM_TIMER, STALE_TIMER);
    assert!(pump_until(&rig, Duration::from_secs(30), |app| app
        .files
        .len()
        == 2));
    // The answer comes later, and changes nothing.
    rig.app(|app| {
        assert!(!app.work.scanning);
        app.swept();
        assert!(!app.work.scanning);
    });
}

#[test]
fn a_walks_rows_join_the_list_one_share_a_message() {
    let rig = Rig::new();
    rig.app(|app| {
        let id = app.incoming.next_walk;
        app.incoming.next_walk += 1;
        app.incoming.walks.push(Walk {
            id,
            cancel: CancelToken::new(),
            keys: false,
            ask: WalkAsk::Unknown,
            files: vec![],
            found: model::Gathered::default(),
            total: 0,
            done: false,
            soon: false,
            added: model::Added::default(),
            listed: model::ListedPaths::default(),
            fresh: vec![],
            then: vec![],
        });
        // Loose files, none of them on the disk: adding reads nothing.
        let n = WALK_ADD_AT_ONCE * 2 + 200;
        let files = (0..n)
            .map(|i| model::ReadFile {
                path: PathBuf::from(format!(r"C:\nowhere\{i:04}.png")),
                size: 1,
                modified: None,
                check: kiln_core::FileCheck::Ok,
            })
            .collect();
        let before = app.files.len();
        app.walked(Walked {
            id,
            batch: model::WalkBatch {
                part: model::Gathered {
                    files,
                    ..Default::default()
                },
                from_folders: 0,
                found: n,
                listed: 0,
                done: true,
            },
        });
        // Nothing yet: the first share comes in a message of its own.
        assert_eq!(app.files.len(), before);
        assert!(app.incoming.walks[0].soon);
        app.walk_on(id);
        assert_eq!(app.files.len(), before + WALK_ADD_AT_ONCE);
        app.walk_on(id);
        assert_eq!(app.files.len(), before + 2 * WALK_ADD_AT_ONCE);
        app.walk_on(id);
        assert_eq!(app.files.len(), before + n);
        assert!(app.incoming.walks.is_empty(), "over once all are in");
        assert_eq!(app.status.text, format!("Added {n} files."));
    });
}

/// A walk with no thread of its own, its batches fed by the test, and one
/// batch of `n` loose files, none on the disk, that ends it.
fn fed_walk(app: &mut App, n: usize) -> u64 {
    let id = app.incoming.next_walk;
    app.incoming.next_walk += 1;
    app.incoming.walks.push(Walk {
        id,
        cancel: CancelToken::new(),
        keys: false,
        ask: WalkAsk::Unknown,
        files: vec![],
        found: model::Gathered::default(),
        total: 0,
        done: false,
        soon: false,
        added: model::Added::default(),
        listed: model::ListedPaths::default(),
        fresh: vec![],
        then: vec![],
    });
    let files = (0..n)
        .map(|i| model::ReadFile {
            path: PathBuf::from(format!(r"C:\nowhere\{id}-{i:04}.png")),
            size: 1,
            modified: None,
            check: kiln_core::FileCheck::Ok,
        })
        .collect();
    app.walked(Walked {
        id,
        batch: model::WalkBatch {
            part: model::Gathered {
                files,
                ..Default::default()
            },
            from_folders: 0,
            found: n,
            listed: 0,
            done: true,
        },
    });
    id
}

#[test]
fn a_clear_stops_a_walk_and_runs_what_waited_on_it() {
    let rig = Rig::new();
    rig.app(|app| {
        let id = fed_walk(app, WALK_ADD_AT_ONCE + 100);
        let ran = Rc::new(Cell::new(false));
        let after = ran.clone();
        app.after_add(move |_| after.set(true));
        app.walk_on(id);
        assert_eq!(app.files.len(), 1 + WALK_ADD_AT_ONCE);
        app.clear_rows();
        assert!(app.incoming.walks.is_empty());
        assert!(ran.get(), "what waited on the walk ran");
        // A share still on its way adds nothing, and nothing says "Added".
        app.walk_on(id);
        assert_eq!(app.files.len(), 0);
        assert!(!app.status.text.starts_with("Added"));
    });
}

#[test]
fn a_walk_holds_its_rows_while_a_dialog_is_up_and_goes_on_after() {
    let rig = Rig::new();
    rig.app(|app| {
        let id = fed_walk(app, 10);
        let dialog = system::Modal::open();
        app.walk_on(id);
        assert_eq!(app.files.len(), 1, "nothing added under a dialog");
        drop(dialog);
        app.resume_walks();
        assert!(app.incoming.walks[0].soon);
        app.walk_on(id);
        assert_eq!(app.files.len(), 11);
    });
}

#[test]
fn a_walk_holds_its_rows_until_the_engines_are_known() {
    let rig = Rig::new();
    rig.app(|app| {
        app.engines = Engines::default();
        let id = fed_walk(app, 10);
        app.walk_on(id);
        assert_eq!(app.files.len(), 1, "no row before the engines are known");
        let look = app.engines.ask();
        app.engines_found(workers::EnginesFound {
            id: look,
            available: Availability::all(),
            failed: false,
            status: None,
            office: None,
            save_to: None,
            winget: None,
        });
        assert!(app.incoming.walks[0].soon);
        app.walk_on(id);
        assert_eq!(app.files.len(), 11);
        // Its rows were made with the engines known: formats offered.
        assert!(!app.files.entries()[1].targets.is_empty());
    });
}

#[test]
fn only_the_latest_wheel_or_menu_waits_for_the_engines() {
    let rig = Rig::new();
    rig.app(|app| {
        app.engines = Engines::default();
        let runs = Rc::new(Cell::new(0));
        for _ in 0..3 {
            let r = runs.clone();
            app.when_engines_known(move |_| r.set(r.get() + 1));
        }
        let look = app.engines.ask();
        app.engines_found(workers::EnginesFound {
            id: look,
            available: Availability::all(),
            failed: false,
            status: None,
            office: None,
            save_to: None,
            winget: None,
        });
        assert_eq!(runs.get(), 1);
    });
}

#[test]
fn rows_and_a_wheel_wait_for_the_engines_no_longer_than_their_timer() {
    let rig = Rig::new();
    rig.app(|app| {
        app.engines = Engines::default();
        let id = fed_walk(app, 10);
        let ran = Rc::new(Cell::new(false));
        let r = ran.clone();
        app.when_engines_known(move |_| r.set(true));
        app.walk_on(id);
        assert_eq!(app.files.len(), 1, "waiting for the engines");
        // No answer, and the timer fires: everything goes ahead.
        app.engines_waited_out();
        assert!(ran.get(), "the waiting wheel or menu ran");
        assert!(app.incoming.walks[0].soon);
        app.walk_on(id);
        assert_eq!(app.files.len(), 11);
        assert!(app.incoming.walks.is_empty());
        // With no real answer, no helper app is said to be missing.
        assert_eq!(app.status.text, "Added 10 files.");
        assert!(!app.helper_notice);
        assert_eq!(app.incoming.helper_pending.len(), 10);
        // The answer comes late: the rows are worked out again.
        let look = app.engines.ask();
        app.engines_found(workers::EnginesFound {
            id: look,
            available: Availability::all(),
            failed: false,
            status: None,
            office: None,
            save_to: None,
            winget: None,
        });
        assert!(app.incoming.helper_pending.is_empty());
        assert!(!app.files.entries()[1].targets.is_empty());
    });
}

#[test]
fn a_failed_look_never_replaces_a_known_answer() {
    let rig = Rig::new();
    rig.app(|app| {
        let look = app.engines.ask();
        app.engines_found(workers::EnginesFound {
            id: look,
            available: Availability::default(),
            failed: true,
            status: None,
            office: None,
            save_to: None,
            winget: None,
        });
        assert_eq!(app.available(), &Availability::all());
    });
}

#[test]
fn a_path_just_chosen_or_reset_is_said_without_the_disk() {
    use settings::EngineStatus::{Chosen, Missing, OnPath};
    let mut s = Settings::default();
    let on_path = OnPath(PathBuf::from(r"C:\x\pandoc.exe"));
    // Nothing looked for yet.
    assert_eq!(status_now(&s, Engine::Pandoc, None), None);
    assert_eq!(
        status_now(&s, Engine::Pandoc, Some(&on_path)),
        Some(on_path.clone())
    );
    // Chosen since the last look: said as chosen until the look answers.
    let p = PathBuf::from(r"Q:\tools\pandoc.exe");
    s.engines.set(Engine::Pandoc, Some(p.clone()));
    let chosen = Chosen {
        path: p.clone(),
        exists: true,
    };
    assert_eq!(status_now(&s, Engine::Pandoc, Some(&on_path)), Some(chosen));
    // The look for it answered that it isn't there.
    let gone = Chosen {
        path: p,
        exists: false,
    };
    assert_eq!(
        status_now(&s, Engine::Pandoc, Some(&gone)),
        Some(gone.clone())
    );
    // Reset since: nothing, until the next look says.
    s.engines.set(Engine::Pandoc, None);
    assert_eq!(status_now(&s, Engine::Pandoc, Some(&gone)), None);
    assert_eq!(
        status_now(&s, Engine::Pandoc, Some(&Missing)),
        Some(Missing)
    );
}

/// Give row 0 of `rig` the held result `far`, outside the local holding
/// folder, so whether it is there goes by the gone thread's looks.
fn hold_far(rig: &Rig) -> PathBuf {
    let dir = rig.dir.path().join("far");
    std::fs::create_dir_all(&dir).unwrap();
    let far = dir.join("b.jpg");
    std::fs::write(&far, b"first").unwrap();
    let held = far.clone();
    rig.app(|app| {
        let JobState::Done { outputs, .. } = &mut app.files.entries_mut()[0].job else {
            panic!("row 0 holds a result");
        };
        *outputs = vec![held];
    });
    far
}

/// Finish a job on row 0 that writes `result`, as a redo from Options
/// does.
fn finish_on_row_0(app: &mut App, result: &std::path::Path) {
    let id = JobId(9_001);
    let action = Action::Convert("jpg".into());
    let source = app.files.entries()[0].path.clone();
    app.files.entries_mut()[0].job = JobState::Running {
        action: action.clone(),
        fraction: None,
        started: None,
    };
    app.motion.rows[0].job = Some(id);
    app.work.jobs.insert(
        id,
        JobInfo {
            paths: vec![source],
            action,
            batch: 0,
            drops: vec![],
            note: None,
            out_dir: result.parent().map(std::path::Path::to_path_buf),
        },
    );
    app.on_job(JobEvent::Finished(id, Ok(vec![result.to_path_buf()])));
}

/// Close `rig`'s window, turning down whatever it asks: the titles of
/// what it asked.
fn close_asks(rig: &Rig) -> Vec<String> {
    let asked = Rc::new(RefCell::new(vec![]));
    let seen = asked.clone();
    script::answer_with(move |_, effect| match effect {
        Effect::Ask { .. } => {
            seen.borrow_mut().push(title(effect).unwrap());
            Reply::Button(PromptButton::Cancel)
        }
        _ => Reply::Ok(false),
    });
    rig.send(WM_CLOSE, 0);
    let titles = asked.borrow().clone();
    titles
}

#[test]
fn a_result_that_takes_the_name_of_one_found_gone_is_still_held() {
    let rig = Rig::new();
    let far = hold_far(&rig);
    rig.app(|app| {
        // Removed by hand, and a look says so.
        std::fs::remove_file(&far).unwrap();
        app.ask_gone();
        let ask = app.gone_seen.asked;
        app.gone_found(workers::GoneFound {
            ask,
            gone: [far.clone()].into(),
        });
        assert_eq!(held_present(app), 0);
        // A redo takes the free name for its new result.
        std::fs::write(&far, b"second").unwrap();
        finish_on_row_0(app, &far);
        assert_eq!(app.files.entries()[0].held(), Some(&[far.clone()][..]));
        assert_eq!(held_present(app), 1, "the new result is held");
    });
    assert!(close_asks(&rig).is_empty());
    assert!(far.exists());
}

#[test]
fn a_look_asked_before_a_result_was_written_never_says_it_is_gone() {
    let rig = Rig::new();
    let far = hold_far(&rig);
    rig.app(|app| {
        std::fs::remove_file(&far).unwrap();
        // Asked while the name was free; answered once a redo took it.
        app.ask_gone();
        let ask = app.gone_seen.asked;
        std::fs::write(&far, b"second").unwrap();
        finish_on_row_0(app, &far);
        app.gone_found(workers::GoneFound {
            ask,
            gone: [far.clone()].into(),
        });
        assert_eq!(held_present(app), 1);
        assert!(!app.files.entries()[0].is_gone(present_in(app.held.as_ref(), &app.gone_seen)));
    });
    assert!(close_asks(&rig).is_empty());
    assert!(far.exists());
}

#[test]
fn an_older_looks_answer_after_a_newer_one_changes_nothing() {
    let rig = Rig::new();
    let far = hold_far(&rig);
    rig.app(|app| {
        app.ask_gone();
        let older = app.gone_seen.asked;
        app.ask_gone();
        let newer = app.gone_seen.asked;
        assert!(newer > older);
        app.gone_found(workers::GoneFound {
            ask: newer,
            gone: HashSet::new(),
        });
        // The older look ran while it was away, and answers last.
        app.gone_found(workers::GoneFound {
            ask: older,
            gone: [far.clone()].into(),
        });
        assert_eq!(held_present(app), 1);
    });
    assert!(close_asks(&rig).is_empty());
    assert!(far.exists());
}

#[test]
fn coming_back_to_kiln_looks_for_helper_apps_at_most_every_five_seconds() {
    let rig = Rig::new();
    rig.app(|app| {
        let t0 = Instant::now();
        let before = app.engines.asked;
        app.look_again_on_activate(t0);
        assert_eq!(app.engines.asked, before + 1, "the first activation looks");
        app.look_again_on_activate(t0 + Duration::from_secs(1));
        app.look_again_on_activate(t0 + Duration::from_millis(4_900));
        assert_eq!(app.engines.asked, before + 1, "not again within 5 s");
        app.look_again_on_activate(t0 + Duration::from_secs(5));
        assert_eq!(app.engines.asked, before + 2, "again after 5 s");
    });
}

#[test]
fn a_changed_path_looks_for_helper_apps_again() {
    let rig = Rig::new();
    rig.app(|app| {
        let before = app.engines.asked;
        app.environment_changed(Some("ImmersiveColorSet"));
        app.environment_changed(None);
        assert_eq!(app.engines.asked, before, "other settings don't look");
        app.environment_changed(Some("Environment"));
        assert_eq!(app.engines.asked, before + 1);
    });
}

#[test]
fn selecting_a_row_that_needs_a_helper_looks_for_it_once() {
    let rig = Rig::new();
    let kiln = Kiln::new();
    rig.app(|app| {
        let mut e = model::entry_for(
            &kiln,
            &Availability::all(),
            PathBuf::from(r"C:\in\clip.avi"),
            1,
            None,
        );
        e.check = kiln_core::FileCheck::Ok;
        e.targets.clear();
        e.missing = Some(model::Missing::new("ffmpeg", 12));
        app.files.insert(1, e);
        // The held PNG needs nothing.
        app.files.entries_mut()[0].missing = None;
        let before = app.engines.asked;
        app.files.click(0);
        app.look_if_helper_selected();
        assert_eq!(app.engines.asked, before);
        app.files.click(1);
        app.look_if_helper_selected();
        assert_eq!(app.engines.asked, before + 1, "selected: looked for");
        app.look_if_helper_selected();
        assert_eq!(app.engines.asked, before + 1, "still selected: once");
        app.files.click(0);
        app.look_if_helper_selected();
        app.files.click(1);
        app.look_if_helper_selected();
        assert_eq!(app.engines.asked, before + 2, "selected again: again");
    });
}

// Kiln's prompts inside the window (`surfaces::Ask`): the same seam,
// `Effect::Ask`, let through to the window's own dialog instead of the
// script's answer.

/// Answer every prompt by opening it inside the window, and a folder
/// picker by cancelling it; nothing else is expected.
fn prompts_in_window() {
    script::answer_with(|_, effect| match effect {
        Effect::Ask { .. } => Reply::InWindow,
        Effect::PickFolder { .. } => Reply::Path(None),
        other => panic!("unexpected {}", other.name()),
    });
}

/// The steps run so far, without those that only show the window or move
/// the focus, which a hidden test window asks for as a prompt opens.
fn steps() -> Vec<&'static str> {
    script::ran()
        .into_iter()
        .filter(|s| !matches!(*s, "show" | "bring forward" | "focus" | "capture"))
        .collect()
}

/// Press `key` on the window, as its keyboard message.
fn press(rig: &Rig, key: VIRTUAL_KEY) {
    rig.send(WM_KEYDOWN, key.0 as usize);
}

/// What the open prompt asks, and the button the keyboard is on.
fn open_prompt(rig: &Rig) -> Option<(String, String)> {
    rig.app(|app| {
        let tree = app.a11y_tree(&[]);
        let dialog = tree.find(&a11y::Id::Ask)?;
        let focused = tree.focused()?.name.clone();
        Some((dialog.name.clone(), focused))
    })
}

#[test]
fn closing_while_converting_asks_inside_the_window_and_esc_keeps_everything() {
    let rig = Rig::new();
    let held = rig.held_file();
    prompts_in_window();
    converting(&rig);
    rig.send(WM_CLOSE, 0);
    assert_eq!(steps(), ["ask"]);
    // Open in the window, the keyboard on Keep converting, counted as a
    // modal.
    assert_eq!(
        open_prompt(&rig),
        Some((
            "Stop converting and close?".to_owned(),
            "Keep converting".to_owned()
        ))
    );
    assert!(system::modal_open());
    assert!(!system::modal_loop_open());
    assert!(rig.alive());
    // Esc is Cancel: everything stays.
    press(&rig, VK_ESCAPE);
    assert_eq!(open_prompt(&rig), None);
    assert!(!system::modal_open());
    assert!(rig.alive());
    assert!(held.exists());
    rig.app(|app| {
        assert!(!app.destroyed && !app.keep_holding);
        assert!(app.effects.is_empty());
    });
    assert!(steps().is_empty(), "Cancel asks nothing more");
}

#[test]
fn enter_is_keep_converting_and_the_window_stays_open() {
    let rig = Rig::new();
    let held = rig.held_file();
    prompts_in_window();
    converting(&rig);
    rig.send(WM_CLOSE, 0);
    press(&rig, VK_RETURN);
    assert_eq!(steps(), ["ask"]);
    assert_eq!(open_prompt(&rig), None);
    assert!(rig.alive(), "nothing stopped: the window stays");
    assert!(held.exists());
    rig.app(|app| assert!(!app.destroyed && !app.keep_holding));
    assert!(!system::modal_open());
}

#[test]
fn a_held_enter_repeating_never_answers_the_prompt() {
    let rig = Rig::new();
    let held = rig.held_file();
    prompts_in_window();
    converting(&rig);
    rig.send(WM_CLOSE, 0);
    // Enter held down: Windows repeats it, bit 30 of lParam set.
    // SAFETY: a plain send to this thread's own window, no pointers.
    unsafe {
        SendMessageW(
            rig.hwnd,
            WM_KEYDOWN,
            Some(WPARAM(VK_RETURN.0 as usize)),
            Some(LPARAM(1 << 30)),
        )
    };
    assert!(open_prompt(&rig).is_some(), "still asking");
    assert_eq!(steps(), ["ask"]);
    // A fresh press is Keep converting.
    press(&rig, VK_RETURN);
    assert!(steps().is_empty());
    assert_eq!(open_prompt(&rig), None);
    assert!(held.exists());
}

#[test]
fn tab_goes_round_the_prompts_buttons_and_close_closes() {
    let rig = Rig::new();
    let held = rig.held_file();
    prompts_in_window();
    converting(&rig);
    rig.send(WM_CLOSE, 0);
    let focus = |rig: &Rig| open_prompt(rig).map(|(_, f)| f);
    assert_eq!(focus(&rig).as_deref(), Some("Keep converting"));
    press(&rig, VK_TAB);
    assert_eq!(focus(&rig).as_deref(), Some("Close"));
    press(&rig, VK_TAB);
    assert_eq!(
        focus(&rig).as_deref(),
        Some("Keep converting"),
        "Tab stays inside"
    );
    press(&rig, VK_TAB);
    // Close: the job stops, and Kiln quits (the drag-wheel is off). The
    // held result is never removed.
    press(&rig, VK_RETURN);
    assert!(!rig.alive());
    rig.app(|app| assert!(app.destroyed));
    assert!(held.exists());
}

#[test]
fn the_session_ending_with_the_prompt_open_drops_it_unanswered() {
    let rig = Rig::new();
    let held = rig.held_file();
    prompts_in_window();
    converting(&rig);
    rig.send(WM_CLOSE, 0);
    assert!(open_prompt(&rig).is_some());
    rig.send(WM_ENDSESSION, 1);
    assert_eq!(steps(), ["ask", "destroy"]);
    assert!(!rig.alive());
    // Nothing was answered: the result is held for the next start.
    assert!(held.exists());
    rig.app(|app| {
        assert!(app.destroyed);
        assert!(app.keep_holding);
        assert!(app.prompts.ask_ui.is_none());
        assert!(app.effects.is_empty());
    });
    assert!(!system::modal_open());
}

#[test]
fn a_close_while_the_prompt_is_open_brings_the_window_forward() {
    let rig = Rig::new();
    prompts_in_window();
    converting(&rig);
    rig.send(WM_CLOSE, 0);
    script::ran();
    // The title bar's Close, or Alt+F4, again; and Quit from the tray.
    rig.send(WM_CLOSE, 0);
    assert_eq!(script::ran(), ["show", "bring forward"]);
    rig.send(WM_COMMAND, TrayCommand::Quit.id() as usize);
    assert_eq!(script::ran(), ["show", "bring forward"]);
    // Still the one prompt, nothing hidden or closed.
    assert!(rig.alive());
    assert_eq!(
        open_prompt(&rig).map(|(t, _)| t).as_deref(),
        Some("Stop converting and close?")
    );
    rig.app(|app| {
        assert!(app.prompts.asks_waiting.is_empty());
        assert!(!app.destroyed);
    });
}

#[test]
fn a_prompt_in_a_minimised_window_brings_it_up() {
    let rig = Rig::new();
    prompts_in_window();
    converting(&rig);
    // Shown and in view: the prompt just opens.
    rig.app(|app| app.out_of_sight_for_test = Some(false));
    rig.send(WM_CLOSE, 0);
    assert_eq!(script::ran(), ["ask"]);
    press(&rig, VK_ESCAPE);
    script::ran();
    // Minimised: it comes up with the prompt, which would otherwise wait
    // unseen.
    rig.app(|app| app.out_of_sight_for_test = Some(true));
    rig.send(WM_CLOSE, 0);
    let ran = script::ran();
    assert_eq!(ran[0], "ask");
    assert!(ran.contains(&"bring forward"), "{ran:?}");
    assert!(open_prompt(&rig).is_some());
}

#[test]
fn ui_automation_reaches_the_caption_buttons_under_a_prompt_as_the_mouse_does() {
    use windows::Win32::UI::WindowsAndMessaging::{SC_MINIMIZE, WM_SYSCOMMAND};
    let rig = Rig::new();
    prompts_in_window();
    converting(&rig);
    rig.send(WM_CLOSE, 0);
    let invoke = |id: a11y::Id| {
        rig.app(|app| {
            app.a11y_do(a11y::Request {
                id,
                verb: a11y::Verb::Invoke,
            })
        })
    };
    // What the caption button posts, taken before it is delivered.
    let posted = || {
        let mut msg = MSG::default();
        // SAFETY: takes this thread's own window's posted messages.
        let got = unsafe {
            PeekMessageW(
                &mut msg,
                Some(rig.hwnd),
                WM_SYSCOMMAND,
                WM_SYSCOMMAND,
                PM_REMOVE,
            )
        }
        .as_bool();
        got.then_some(msg.wParam.0 as u32)
    };
    invoke(a11y::Id::CaptionButton(a11y::Caption::Min));
    assert_eq!(posted(), Some(SC_MINIMIZE));
    // Back stays as it is under the prompt: nothing.
    invoke(a11y::Id::BackButton);
    assert_eq!(posted(), None);
    assert!(open_prompt(&rig).is_some());
}

#[test]
fn files_dropped_while_the_prompt_is_open_wait_for_it() {
    let rig = Rig::new();
    let dir = tempfile::tempdir().unwrap();
    let late = dir.path().join("late.png");
    std::fs::write(&late, b"x").unwrap();
    prompts_in_window();
    converting(&rig);
    rig.send(WM_CLOSE, 0);
    rig.app(|app| {
        assert!(app.adds_wait());
        app.add_when_live(vec![late.clone()]);
        assert_eq!(app.incoming.queued, std::slice::from_ref(&late));
        assert!(
            app.incoming.walks.is_empty(),
            "nothing is added under the prompt"
        );
    });
    press(&rig, VK_ESCAPE);
    // Answered, the files go in: their walk starts.
    rig.app(|app| {
        assert!(!app.adds_wait());
        assert!(app.incoming.queued.is_empty());
        assert_eq!(app.incoming.walks.len(), 1);
    });
}

#[test]
fn ui_automation_answers_the_prompt_and_reaches_nothing_beneath() {
    let rig = Rig::new();
    let held = rig.held_file();
    prompts_in_window();
    converting(&rig);
    rig.send(WM_CLOSE, 0);
    // Beneath the prompt, the list's buttons hear nothing.
    rig.app(|app| {
        app.a11y_do(a11y::Request {
            id: a11y::Id::Clear,
            verb: a11y::Verb::Invoke,
        })
    });
    assert!(open_prompt(&rig).is_some());
    assert_eq!(rig.app(|app| app.files.len()), 1);
    // Focus moves to Keep converting, and Invoke presses it.
    rig.app(|app| {
        app.a11y_do(a11y::Request {
            id: a11y::Id::AskButton(0),
            verb: a11y::Verb::Focus,
        })
    });
    assert_eq!(open_prompt(&rig).unwrap().1, "Keep converting");
    rig.app(|app| {
        app.a11y_do(a11y::Request {
            id: a11y::Id::AskButton(0),
            verb: a11y::Verb::Invoke,
        })
    });
    assert_eq!(open_prompt(&rig), None);
    assert!(rig.alive() && held.exists());
    // Closing the dialog, its Window pattern, is Cancel too.
    rig.send(WM_CLOSE, 0);
    rig.app(|app| {
        app.a11y_do(a11y::Request {
            id: a11y::Id::Ask,
            verb: a11y::Verb::Close,
        })
    });
    assert_eq!(open_prompt(&rig), None);
    assert!(rig.alive() && held.exists());
    rig.app(|app| assert!(!app.keep_holding));
}

#[test]
fn prompts_open_when_the_window_stops_drawing_are_asked_by_windows_instead() {
    let rig = Rig::new();
    let held = rig.held_file();
    // The leave prompt opens in the window, and a big folder's waits.
    prompts_in_window();
    converting(&rig);
    rig.send(WM_CLOSE, 0);
    rig.app(|app| {
        app.ask(model::big_folder_prompt(2000), |_, _| {});
    });
    run_effects(rig.hwnd, rig.cell());
    rig.app(|app| assert_eq!(app.prompts.asks_waiting.len(), 1));
    steps();
    // The device gives up: the window may never be drawn again, so both
    // go to Windows' own dialog, in order, and are answered there.
    let asked = Rc::new(RefCell::new(vec![]));
    let seen = asked.clone();
    script::answer_with(move |_, effect| match effect {
        Effect::Ask { prompt, .. } => {
            seen.borrow_mut().push(prompt.dialog_title().to_owned());
            Reply::Button(PromptButton::Cancel)
        }
        other => panic!("unexpected {}", other.name()),
    });
    rig.app(|app| app.draw_given_up_for_test = true);
    rig.send(windows::Win32::UI::WindowsAndMessaging::WM_NULL, 0);
    assert_eq!(
        *asked.borrow(),
        [
            "Stop converting and close?",
            "That's 2000 files. Add them all?"
        ]
    );
    rig.app(|app| {
        assert!(app.prompts.ask_ui.is_none() && app.prompts.asks_waiting.is_empty());
        assert!(app.effects.is_empty());
    });
    assert!(!system::modal_open());
    // Cancel there kept everything.
    assert!(rig.alive() && held.exists());
}

#[test]
fn a_prompt_asked_while_one_is_open_waits_its_turn() {
    let rig = Rig::new();
    prompts_in_window();
    let order = Rc::new(RefCell::new(vec![]));
    let (a, b) = (order.clone(), order.clone());
    rig.app(|app| {
        app.ask(close_prompt(), move |_, _| a.borrow_mut().push("first"));
        app.ask(model::big_folder_prompt(2000), move |_, _| {
            b.borrow_mut().push("second")
        });
    });
    run_effects(rig.hwnd, rig.cell());
    assert_eq!(
        open_prompt(&rig).map(|(t, _)| t).as_deref(),
        Some("Stop converting and close?")
    );
    rig.app(|app| assert_eq!(app.prompts.asks_waiting.len(), 1));
    press(&rig, VK_ESCAPE);
    assert_eq!(*order.borrow(), ["first"]);
    assert_eq!(
        open_prompt(&rig).map(|(t, _)| t).as_deref(),
        Some("That's 2000 files. Add them all?")
    );
    assert!(system::modal_open());
    press(&rig, VK_ESCAPE);
    assert_eq!(*order.borrow(), ["first", "second"]);
    assert!(!system::modal_open());
}

/// The settings file the rig's store writes, once every save is written.
fn saved_settings(rig: &Rig) -> String {
    rig.app(|app| app.prefs.store.flush()).unwrap();
    std::fs::read_to_string(rig.dir.path().join("settings.toml")).unwrap_or_default()
}

/// A rig with its row taken off the list: the empty window, with the
/// first-run card as a fresh profile has it.
fn empty_rig() -> Rig {
    let rig = Rig::new();
    rig.app(|app| {
        app.files.remove(0);
    });
    rig
}

/// The middle of the empty window's first-run Dismiss, while it shows.
fn dismiss_middle(app: &App) -> Option<(f32, f32)> {
    app.dismiss_button()
        .map(|r| (r.x + r.w / 2.0, r.y + r.h / 2.0))
}

#[test]
fn the_first_run_card_shows_until_a_click_on_dismiss_which_is_saved() {
    let rig = empty_rig();
    rig.app(|app| {
        assert!(app.files.is_empty());
        let card = app
            .empty_state()
            .and_then(|e| e.card)
            .expect("the card shows");
        assert!(app.first_run().is_some());
        let tree = app.a11y_tree(&[]);
        assert_eq!(tree.find(&a11y::Id::FirstRunCard).unwrap().rect, card.card);
        // A press on Dismiss that slides off does nothing.
        let (x, y) = dismiss_middle(app).unwrap();
        app.mouse_down(x, y, 0);
        app.mouse_up(card.card.x - 4.0, y);
        assert!(app.first_run().is_some());
        // A click on it dismisses the card.
        app.mouse_down(x, y, 0);
        app.mouse_up(x, y);
        assert!(app.prefs.settings.first_run_dismissed);
        assert!(app.first_run().is_none() && app.dismiss_button().is_none());
        let tree = app.a11y_tree(&[]);
        assert!(tree.find(&a11y::Id::FirstRunCard).is_none());
        assert!(tree.find(&a11y::Id::AddFiles).is_some());
    });
    assert!(saved_settings(&rig).contains("first_run_dismissed = true"));
}

#[test]
fn tab_reaches_dismiss_after_add_files_and_enter_dismisses() {
    let rig = empty_rig();
    let focused = |rig: &Rig| rig.app(|app| app.a11y_tree(&[]).focused().map(|n| n.id.clone()));
    for want in [
        a11y::Id::HelpButton,
        a11y::Id::SettingsButton,
        a11y::Id::AddFiles,
        a11y::Id::FirstRunDismiss,
    ] {
        press(&rig, VK_TAB);
        assert_eq!(focused(&rig), Some(want));
    }
    press(&rig, VK_RETURN);
    rig.app(|app| assert!(app.prefs.settings.first_run_dismissed && app.first_run().is_none()));
    // The keyboard is on Add files now, and Tab no longer stops where the
    // card was.
    assert_eq!(focused(&rig), Some(a11y::Id::AddFiles));
    press(&rig, VK_TAB);
    assert_eq!(focused(&rig), Some(a11y::Id::CreditLink));
    assert!(saved_settings(&rig).contains("first_run_dismissed = true"));
}

#[test]
fn ui_automation_dismisses_the_card_and_helps_link_brings_it_back() {
    let rig = empty_rig();
    rig.app(|app| {
        app.a11y_do(a11y::Request {
            id: a11y::Id::FirstRunDismiss,
            verb: a11y::Verb::Invoke,
        });
        assert!(app.first_run().is_none());
    });
    assert!(saved_settings(&rig).contains("first_run_dismissed = true"));
    // Help's Show first-run help closes Help and the card is back.
    rig.app(|app| {
        app.open_help();
        assert!(app.help.is_some());
        app.a11y_do(a11y::Request {
            id: a11y::Id::HelpGuide,
            verb: a11y::Verb::Invoke,
        });
        assert!(app.help.is_none());
        assert!(!app.prefs.settings.first_run_dismissed);
        assert!(app.first_run().is_some());
        assert!(app.a11y_tree(&[]).find(&a11y::Id::FirstRunCard).is_some());
    });
    assert!(!saved_settings(&rig).contains("first_run_dismissed"));
    // And Dismiss works again after it.
    rig.app(|app| {
        let (x, y) = dismiss_middle(app).unwrap();
        app.mouse_down(x, y, 0);
        app.mouse_up(x, y);
        assert!(app.first_run().is_none());
    });
}

#[test]
fn the_card_shows_only_in_the_empty_window_and_its_third_step_follows_where_results_go() {
    let rig = empty_rig();
    let png = a_png(&rig, "a.png");
    rig.app(|app| {
        // Results go next to their originals: the third step says so.
        app.prefs.settings.save_to = None;
        assert_eq!(
            app.first_run().unwrap().steps[2],
            "The new file is saved next to the original. Show in folder takes you to it."
        );
        // Or in the folder chosen in Settings.
        app.prefs.settings.save_to = Some(PathBuf::from(r"C:\Converted"));
        assert_eq!(
            app.first_run().unwrap().steps[2],
            "The new file is saved in the folder chosen in Settings. Show in folder takes you to it."
        );
        app.prefs.settings.save_to = None;
        // Files listed: no card, nothing to Tab to or click.
        let e = model::entry_for(&app.kiln, &Availability::default(), png.clone(), 1, None);
        app.files.insert(0, e);
        assert!(app.first_run().is_none() && app.dismiss_button().is_none());
        assert!(app.a11y_tree(&[]).find(&a11y::Id::FirstRunCard).is_none());
        assert!(
            !app.prefs.settings.first_run_dismissed,
            "listing files isn't a dismiss"
        );
    });
}

/// A plain drop of `paths` on the window at its client point `(x, y)`
/// DIPs, with no key held, as OLE delivers one.
fn plain_drop(app: &mut App, paths: &[PathBuf], (x, y): (i32, i32)) {
    let mut p = windows::Win32::Foundation::POINT { x, y };
    // SAFETY: a point of this thread's own window.
    unsafe { windows::Win32::Graphics::Gdi::ClientToScreen(app.hwnd, &mut p) }.unwrap();
    app.dropped(&super::drop::DragEvent {
        keys: windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS(0),
        x: p.x,
        y: p.y,
        paths: paths.to_vec(),
        copy: true,
    });
}

#[test]
fn a_plain_drop_of_one_kind_opens_the_wheel_where_it_landed() {
    let rig = empty_rig();
    let (a, b) = (a_png(&rig, "a.png"), a_png(&rig, "b.png"));
    rig.app(|app| {
        app.engines.waited_out = true;
        plain_drop(app, &[a.clone(), b.clone()], (400, 250));
    });
    assert!(pump_until(&rig, Duration::from_secs(5), |app| app
        .wheel
        .is_some()));
    rig.app(|app| {
        let w = app.wheel.as_ref().unwrap();
        assert!(w.is_open());
        assert_eq!(w.kind(), WheelKind::Convert);
        let mut got = w.paths().to_vec();
        got.sort();
        assert_eq!(got, [a.clone(), b.clone()]);
        // At the drop point, as far as the content layer lets it be (its
        // shadow's room inside it).
        let at = app.dips(400, 250);
        let c = w.center();
        assert!(
            (c.0 - at.0).abs() < 1.0 && (c.1 - at.1).abs() < 1.0,
            "{c:?} {at:?}"
        );
        assert!(app.top_info.is_none());
        // Esc closes it, and the files stay listed.
        app.dispatch(Input::Key {
            key: VK_ESCAPE,
            shift: false,
            ctrl: false,
            alt: false,
        });
        assert!(!app.wheel.as_ref().is_some_and(|w| w.is_open()));
        assert_eq!(app.files.len(), 2);
        assert!(app.work.jobs.is_empty());
    });
}

#[test]
fn three_arrivals_100_ms_apart_open_one_wheel_250_ms_after_the_last() {
    // as a second launch hands files over, one at a time.
    let rig = empty_rig();
    let files: Vec<PathBuf> = ["a.png", "b.png", "c.png"]
        .into_iter()
        .map(|n| a_png(&rig, n))
        .collect();
    rig.app(|app| app.engines.waited_out = true);
    let mut last = Instant::now();
    for (k, f) in files.iter().enumerate() {
        if k > 0 {
            // Messages go on meanwhile; no wheel opens between arrivals.
            assert!(!pump_until(&rig, Duration::from_millis(100), |app| app
                .wheel
                .is_some()));
        }
        last = Instant::now();
        rig.app(|app| {
            app.arrive(std::slice::from_ref(f));
            app.add_when_live(vec![f.clone()]);
        });
    }
    assert!(pump_until(&rig, Duration::from_secs(5), |app| app
        .wheel
        .is_some()));
    assert!(last.elapsed() >= ways_in::ARRIVALS_SETTLE);
    rig.app(|app| {
        let w = app.wheel.as_ref().unwrap();
        let mut got = w.paths().to_vec();
        got.sort();
        assert_eq!(got, files, "one wheel for all three");
        assert_eq!(app.files.selected_rows().len(), 3);
        // Centred over the list, as far as the window lets it be.
        let l = app.layout();
        let c = w.center();
        assert!(l.list.contains(c.0, c.1), "{c:?} in {:?}", l.list);
    });
}

#[test]
fn arrivals_waiting_on_the_engines_open_nothing_once_their_files_were_converted() {
    // The wheel for files that arrived waits for the engines; meanwhile
    // the file is converted another way. Once the engines are known, no
    // wheel comes up for it.
    let rig = empty_rig();
    let png = a_png(&rig, "a.png");
    rig.app(|app| {
        app.engines = Engines::default();
        list_at_top(app, &png);
        app.files.select_only(&[0]);
        let at = (300.0, 200.0);
        app.choice_for_arrivals(vec![png.clone()], at, Pour::OverList);
        assert!(app.wheel.is_none(), "waiting for the engines");
        // Converted already, and done: a small photo converts in moments.
        app.files.entries_mut()[0].job = JobState::Done {
            action: Action::Convert("jpg".into()),
            outputs: vec![rig.dir.path().join("a.jpg")],
            saved: None,
        };
        app.engines_waited_out();
        assert!(app.wheel.is_none(), "no wheel for a file already converted");
    });
}

#[test]
fn arrivals_ask_nothing_once_a_wheel_opened_or_a_job_started_meanwhile() {
    let rig = empty_rig();
    let png = a_png(&rig, "a.png");
    rig.app(|app| {
        app.engines.waited_out = true;
        list_at_top(app, &png);
        app.files.select_only(&[0]);
        // The file arrives, and before the wheel is due it is converted
        // from the footer's Convert, and done.
        app.arrive(std::slice::from_ref(&png));
        app.convert_selection(false);
        assert!(app.wheel.is_some(), "the footer's wheel");
        app.wheel = None;
        app.start(Action::Convert("jpg".into()), vec![png.clone()], None);
        app.work.runner.cancel_all();
        app.files.entries_mut()[0].job = JobState::Done {
            action: Action::Convert("jpg".into()),
            outputs: vec![rig.dir.path().join("a.jpg")],
            saved: None,
        };
        std::thread::sleep(ways_in::ARRIVALS_SETTLE + Duration::from_millis(20));
        app.arrivals_due();
        assert!(app.wheel.is_none(), "nothing more is asked");
    });
}

#[test]
fn a_plain_drop_of_kinds_that_share_a_format_lists_them_selected_and_says_nothing() {
    // A video and a sound share WAV, M4A and the rest: the side pane
    // offers those for both, so no InfoBar says they share nothing, and
    // the wheel opens only for one kind.
    let rig = empty_rig();
    let (mp4, mp3) = (rig.dir.path().join("a.mp4"), rig.dir.path().join("b.mp3"));
    std::fs::write(&mp4, b"not really").unwrap();
    std::fs::write(&mp3, b"not really").unwrap();
    rig.app(|app| {
        app.engines.waited_out = true;
        plain_drop(app, &[mp4.clone(), mp3.clone()], (300, 200));
    });
    assert!(pump_until(&rig, Duration::from_secs(5), |app| app
        .files
        .len()
        == 2));
    // What the add leaves to do is done.
    pump_until(&rig, Duration::from_millis(200), |_| false);
    rig.app(|app| {
        assert!(app.wheel.is_none());
        assert!(app.top_info.is_none(), "{:?}", app.top_info);
        assert_eq!(app.files.selected_rows().len(), 2, "both selected");
    });
}

#[test]
fn a_plain_drop_of_mixed_kinds_lists_them_and_says_so_at_the_top() {
    // A photo and a sound share nothing.
    let rig = empty_rig();
    let png = a_png(&rig, "a.png");
    let mp3 = rig.dir.path().join("b.mp3");
    std::fs::write(&mp3, b"not really").unwrap();
    rig.app(|app| {
        app.engines.waited_out = true;
        plain_drop(app, &[png.clone(), mp3.clone()], (300, 200));
    });
    assert!(pump_until(&rig, Duration::from_secs(5), |app| app
        .files
        .len()
        == 2
        && app.top_info.is_some()));
    rig.app(|app| {
        assert!(app.wheel.is_none());
        assert_eq!(app.files.selected_rows().len(), 2, "both selected");
        assert_eq!(
            app.top_info.clone().unwrap(),
            (
                "2 files added.".to_owned(),
                "They don't share a format, so pick one file or one kind at a time.".to_owned()
            )
        );
        // The rows move down under it, and screen readers find it first.
        let l = app.layout();
        let bar = l.top_info.unwrap();
        assert!(l.rows[0].y >= bar.bottom());
        let tree = app.a11y_tree(&[]);
        let info = tree.find(&a11y::Id::TopInfo).unwrap();
        assert_eq!(info.name, "2 files added.");
        // Its Dismiss takes it away, and the rows move back up.
        app.a11y_do(a11y::Request {
            id: a11y::Id::TopInfoDismiss,
            verb: a11y::Verb::Invoke,
        });
        assert!(app.top_info.is_none() && app.layout().top_info.is_none());
    });
}

#[test]
fn the_row_menus_make_smaller_opens_its_dialog_and_its_accent_starts_the_job() {
    let rig = empty_rig();
    let png = a_png(&rig, "a.png");
    rig.app(|app| {
        list_at_top(app, &png);
        // As a 3 MB photo's row says: the presets are under it.
        app.files.entries_mut()[0].size = 3_000_000;
        app.files.select_only(&[0]);
        app.engines.waited_out = true;
        app.context_menu(None);
        let m = app.row_menu.as_ref().expect("the menu is up");
        // As the mock: Convert to…, then Make smaller in Tools…' place.
        assert_eq!(
            m.items[..2],
            [MenuItem::Wheel(WheelKind::Convert), MenuItem::Smaller]
        );
        assert!(!m.items.contains(&MenuItem::Wheel(WheelKind::Tools)));
        app.a11y_do(a11y::Request {
            id: a11y::Id::RowMenuItem(1),
            verb: a11y::Verb::Invoke,
        });
        assert!(app.row_menu.is_none() && app.wheel.is_none() && app.sheet.is_none());
        // Nothing starts until the dialog's Make smaller is pressed.
        assert!(app.prompts.tool_ui.is_some(), "the Make smaller dialog");
        assert!(app.work.jobs.is_empty());
        assert!(system::modal_open());
        app.drive_tool(|t, cx| t.key(cx, VK_RETURN, Mods::default()));
        assert!(app.prompts.tool_ui.is_none());
        let action = app.work.jobs.values().next().expect("a job").action.clone();
        assert_eq!(action, Action::Tool(Tool::Smaller, kiln_core::Kind::Image));
        assert_eq!(asked(app), std::slice::from_ref(&png));
        app.work.runner.cancel_all();
    });
}

#[test]
fn cancel_in_the_make_smaller_dialog_starts_nothing() {
    let rig = empty_rig();
    let png = a_png(&rig, "a.png");
    rig.app(|app| {
        list_at_top(app, &png);
        app.files.entries_mut()[0].size = 3_000_000;
        app.files.select_only(&[0]);
        app.start(
            Action::Tool(Tool::Smaller, kiln_core::Kind::Image),
            vec![png.clone()],
            None,
        );
        assert!(app.prompts.tool_ui.is_some());
        // UI Automation's Window pattern closing it is Cancel.
        app.a11y_do(a11y::Request {
            id: a11y::Id::Tool,
            verb: a11y::Verb::Close,
        });
        assert!(app.prompts.tool_ui.is_none() && app.work.jobs.is_empty());
        assert!(!system::modal_open());
    });
}

#[test]
fn the_right_click_menu_opens_in_the_window_and_its_keys_choose_from_it() {
    let rig = empty_rig();
    let png = a_png(&rig, "a.png");
    rig.app(|app| {
        list_at_top(app, &png);
        app.files.select_only(&[0]);
        app.engines.waited_out = true;
        // The Menu key: lit on its first line, Convert to….
        app.context_menu(None);
        let m = app.row_menu.as_ref().expect("the menu is up");
        assert_eq!(m.items[0], MenuItem::Wheel(WheelKind::Convert));
        assert_eq!(m.lit(), Some(0));
        assert_eq!(*m.items.last().unwrap(), MenuItem::Remove);
        // The tree has it over the list, its lit line with the keyboard.
        let tree = app.a11y_tree(&[]);
        let menu = tree.find(&a11y::Id::RowMenu).unwrap();
        assert_eq!(menu.children[0].name, "Convert to…");
        assert_eq!(menu.children.last().unwrap().name, "Remove from list");
        assert_eq!(tree.focused().unwrap().id, a11y::Id::RowMenuItem(0));
        // Keys go to it and no further: Esc closes it.
        let key = |key| Input::Key {
            key,
            shift: false,
            ctrl: false,
            alt: false,
        };
        assert_eq!(app.dispatch(key(VK_DOWN)), surfaces::Reply::Block);
        assert_ne!(app.row_menu.as_ref().unwrap().lit(), Some(0));
        app.dispatch(key(VK_ESCAPE));
        assert!(app.row_menu.is_none());
        // A press off the menu closes it and reaches nothing beneath.
        app.context_menu(None);
        assert_eq!(
            app.dispatch(Input::Down {
                x: 1.0,
                y: 1.0,
                keys: 0
            }),
            surfaces::Reply::Block
        );
        assert!(app.row_menu.is_none());
        // Remove from list, chosen from the tree, takes the row away.
        app.context_menu(None);
        let k = app.row_menu.as_ref().unwrap().items.len() - 1;
        app.a11y_do(a11y::Request {
            id: a11y::Id::RowMenuItem(k),
            verb: a11y::Verb::Invoke,
        });
        assert!(app.row_menu.is_none());
        assert!(app.files.index_of(&png).is_none());
    });
}

#[test]
fn a_work_area_change_asks_for_the_frame_again_once_app_is_let_go() {
    let rig = Rig::new();
    script::ran();
    // A taskbar set to hide itself, or not: the frame is asked for again
    // by the runner, so the WM_SIZE it may send reaches the renderer.
    rig.send(
        WM_SETTINGCHANGE,
        windows::Win32::UI::WindowsAndMessaging::SPI_SETWORKAREA.0 as usize,
    );
    assert_eq!(script::ran(), ["frame changed"]);
    rig.app(|app| assert!(app.effects.is_empty()));
}

#[test]
fn a_prompt_opening_while_the_menu_is_up_closes_it_and_enter_answers_the_prompt() {
    let rig = Rig::new();
    let held = rig.held_file();
    prompts_in_window();
    rig.app(|app| {
        app.files.select_only(&[0]);
        // Shift+F10 on the held row: Convert to… is lit.
        app.context_menu(None);
        assert_eq!(app.row_menu.as_ref().unwrap().lit(), Some(0));
    });
    // Alt+F4 or the caption's Close, while a row converts: the prompt
    // opens over the list.
    converting(&rig);
    rig.send(WM_CLOSE, 0);
    rig.app(|app| {
        assert!(app.prompts.ask_ui.is_some());
        assert!(app.row_menu.is_none(), "the menu gives way to the prompt");
    });
    assert_eq!(
        open_prompt(&rig).map(|(t, _)| t).as_deref(),
        Some("Stop converting and close?")
    );
    // Enter is the prompt's Keep converting, not Convert to…: no wheel
    // opens.
    press(&rig, VK_RETURN);
    assert_eq!(steps(), ["ask"]);
    rig.app(|app| {
        assert!(app.wheel.is_none());
        assert!(app.prompts.ask_ui.is_none() && app.row_menu.is_none());
    });
    assert!(rig.alive() && held.exists());
}

#[test]
fn a_menu_waiting_for_the_engines_doesnt_open_over_a_prompt_or_an_inactive_window() {
    let rig = Rig::new();
    prompts_in_window();
    let unknown = |app: &mut App| {
        app.engines.latest = None;
        app.engines.waited_out = false;
    };
    // A prompt opened while it waited: no menu over it.
    rig.app(|app| {
        unknown(app);
        app.active = true;
        app.files.select_only(&[0]);
        app.context_menu(None);
        assert!(app.row_menu.is_none(), "it waits for the engines");
        app.ask(model::big_folder_prompt(2000), |_, _| {});
    });
    run_effects(rig.hwnd, rig.cell());
    rig.app(|app| {
        assert!(app.prompts.ask_ui.is_some());
        app.engines_waited_out();
        assert!(app.row_menu.is_none());
    });
    press(&rig, VK_ESCAPE);
    // The window lost the keyboard while it waited: no menu either.
    rig.app(|app| {
        assert!(app.prompts.ask_ui.is_none());
        unknown(app);
        app.active = true;
        app.context_menu(None);
        app.active = false;
        app.engines_waited_out();
        assert!(app.row_menu.is_none());
        // Still active with nothing over the list, it opens.
        unknown(app);
        app.active = true;
        app.context_menu(None);
        app.engines_waited_out();
        assert!(app.row_menu.is_some());
    });
}

/// The keyboard's ask for the right-click menu (Shift+F10, or the Menu
/// key's release), as Windows sends it.
fn context_key(rig: &Rig) {
    // SAFETY: a plain send to this thread's own window, no pointers.
    unsafe {
        SendMessageW(
            rig.hwnd,
            WM_CONTEXTMENU,
            Some(WPARAM(rig.hwnd.0 as usize)),
            Some(LPARAM(-1)),
        )
    };
}

#[test]
fn the_menu_key_and_shift_f10_close_the_menu_they_opened() {
    let rig = Rig::new();
    rig.app(|app| app.files.select_only(&[0]));
    let up = |rig: &Rig| rig.app(|app| app.row_menu.is_some());
    // Shift+F10 opens it, and again closes it.
    context_key(&rig);
    assert!(up(&rig));
    context_key(&rig);
    assert!(!up(&rig));
    // The Menu key's press closes it, and the ask its release sends
    // doesn't open it again.
    context_key(&rig);
    press(&rig, VK_APPS);
    assert!(!up(&rig));
    context_key(&rig);
    assert!(!up(&rig));
    // The next press of it opens it.
    press(&rig, VK_APPS);
    context_key(&rig);
    assert!(up(&rig));
    // Left and Right leave it up: it has no submenus.
    press(&rig, VK_LEFT);
    press(&rig, VK_RIGHT);
    assert!(up(&rig));
    // Closed by another key, the next Shift+F10 opens it.
    press(&rig, VK_APPS);
    press(&rig, VK_SHIFT);
    context_key(&rig);
    assert!(up(&rig));
}

#[test]
fn a_menu_opened_by_the_pointer_gives_its_first_item_the_trees_focus() {
    let rig = Rig::new();
    rig.app(|app| {
        app.files.select_only(&[0]);
        app.context_menu(None);
        // As the pointer leaves it: nothing lit.
        let m = app.row_menu.as_mut().unwrap();
        m.hover(-1.0, -1.0);
        assert_eq!(m.lit(), None);
        let tree = app.a11y_tree(&[]);
        let menu = tree.find(&a11y::Id::RowMenu).unwrap();
        // Unnamed, as WinUI leaves a context flyout: read as "menu" once.
        assert_eq!(menu.name, "");
        let focused = tree.focused().unwrap();
        assert_eq!(focused.id, a11y::Id::RowMenuItem(0));
        assert!(focused.states.focusable);
    });
}

#[test]
fn the_menu_closes_on_losing_the_keyboard_a_resize_a_scroll_or_a_new_text_size() {
    let rig = Rig::new();
    let open = |rig: &Rig| {
        rig.app(|app| {
            app.files.select_only(&[0]);
            app.context_menu(None);
            assert!(app.row_menu.is_some());
        })
    };
    open(&rig);
    rig.send(WM_ACTIVATE, WA_INACTIVE as usize);
    assert!(rig.app(|app| app.row_menu.is_none()), "inactive");
    open(&rig);
    rig.send(WM_SIZE, SIZE_RESTORED as usize);
    assert!(rig.app(|app| app.row_menu.is_none()), "sized");
    open(&rig);
    rig.app(|app| {
        assert_eq!(app.dispatch(Input::Scroll(1.0)), surfaces::Reply::Block);
        assert!(app.row_menu.is_none(), "scrolled");
    });
    // Text size: its labels were measured at the old size.
    open(&rig);
    rig.app(|app| {
        app.text_scale *= 1.5;
        app.rezoom();
        assert!(app.row_menu.is_none(), "text size");
    });
}

#[test]
fn the_menu_stays_below_the_title_bar() {
    let rig = Rig::new();
    rig.app(|app| {
        let bar = app.renderer.title_bar(false).rect.h;
        let (w, h) = app.renderer.size_dips();
        assert_eq!(app.menu_bounds(), layout::Rect::new(0.0, bar, w, h - bar));
        app.files.select_only(&[0]);
        app.context_menu(None);
        assert!(app.row_menu.as_ref().unwrap().layout.rect.y >= bar);
    });
}

#[test]
fn the_menu_acts_on_its_rows_by_file_when_the_list_changes_under_it() {
    let rig = Rig::new();
    let a = a_png(&rig, "a.png");
    let b = a_png(&rig, "b.png");
    rig.app(|app| {
        list_at_top(app, &a);
        let i = app.files.index_of(&a).unwrap();
        app.files.select_only(&[i]);
        app.context_menu(None);
        // A row joins above it while the menu is up.
        list_at_top(app, &b);
        let remove = app.row_menu.as_ref().unwrap().items.len() - 1;
        app.a11y_do(a11y::Request {
            id: a11y::Id::RowMenuItem(remove),
            verb: a11y::Verb::Invoke,
        });
        // Its own row went, not the one now in its place.
        assert!(app.files.index_of(&a).is_none());
        assert!(app.files.index_of(&b).is_some());
        // A menu whose rows have all gone does nothing.
        let i = app.files.index_of(&b).unwrap();
        app.files.select_only(&[i]);
        app.context_menu(None);
        app.files.remove(i);
        let n = app.files.len();
        app.a11y_do(a11y::Request {
            id: a11y::Id::RowMenuItem(remove),
            verb: a11y::Verb::Invoke,
        });
        assert!(app.row_menu.is_none());
        assert_eq!(app.files.len(), n);
        assert!(app.prompts.ask_ui.is_none() && app.effects.is_empty());
    });
}

#[test]
fn the_side_panes_open_and_remove_act_on_the_row_it_shows() {
    let rig = Rig::new();
    script::answer_with(|_, effect| match effect {
        Effect::Open { .. } => Reply::Ok(true),
        other => panic!("unexpected {}", other.name()),
    });
    // A saved result: Open opens it.
    rig.app(|app| {
        let out = app.files.entries()[0].held().unwrap()[0].clone();
        app.files.entries_mut()[0].job = JobState::Done {
            action: Action::Convert("jpg".into()),
            outputs: vec![out.clone()],
            saved: Some(model::Saved::Folder(out.parent().unwrap().to_path_buf())),
        };
        app.files.select_only(&[0]);
        app.refresh_panel();
        assert!(app.live_panel().is_some(), "the pane shows");
        app.panel_act(InspectorControl::Open, false);
    });
    run_effects(rig.hwnd, rig.cell());
    assert_eq!(steps(), ["open"]);
    // A failed file: the pane has no Remove from list (the row menu and
    // Delete have it), and its menu does.
    let rig = Rig::new();
    rig.app(|app| {
        let png = app.files.entries()[0].path.clone();
        app.files.entries_mut()[0].job = JobState::Failed {
            message: "This file looks damaged.".into(),
            details: String::new(),
            engine: None,
            store_product: None,
        };
        app.files.select_only(&[0]);
        app.refresh_panel();
        assert!(app.live_panel().is_some(), "the pane shows");
        assert!(app.files.index_of(&png).is_some());
        app.context_menu(None);
        let m = app.row_menu.as_ref().expect("the menu is up");
        assert!(m.items.contains(&MenuItem::Remove), "{:?}", m.items);
    });
}

#[test]
fn a_format_or_make_smaller_in_the_side_pane_converts_at_once() {
    // no options sheet, no wheel; the job starts at Settings' quality.
    // Make smaller asks for a size in its own dialog first (Task 20).
    for (control, want) in [
        (InspectorControl::Target(0), None),
        (
            InspectorControl::Smaller,
            Some(Action::Tool(Tool::Smaller, kiln_core::Kind::Image)),
        ),
    ] {
        let rig = Rig::new();
        let png = a_png(&rig, "b.png");
        rig.app(|app| {
            list(app, &[&png]);
            // What a PNG becomes, as the engines found at start-up say.
            app.files.entries_mut()[1].targets = vec!["JPG".into(), "WEBP".into()];
            app.files.select_only(&[1]);
            app.refresh_panel();
            let p = app.live_panel().expect("the pane shows");
            assert!(
                p.model.controls().contains(&control),
                "{control:?} is offered: {:?}",
                p.model.controls()
            );
            let ext = p
                .model
                .format_pick(InspectorControl::Target(0))
                .map(|(e, _)| e.to_owned());
            app.files.entries_mut()[1].size = 3_000_000;
            app.panel_act(control, false);
            assert!(app.sheet.is_none() && app.wheel.is_none(), "{control:?}");
            // Make smaller asks its size first; its accent starts it.
            if control == InspectorControl::Smaller {
                assert!(app.prompts.tool_ui.is_some() && app.work.jobs.is_empty());
                app.drive_tool(|t, cx| t.key(cx, VK_RETURN, Mods::default()));
            }
            assert_eq!(asked(app), std::slice::from_ref(&png));
            let action = app.work.jobs.values().next().unwrap().action.clone();
            let want = want
                .clone()
                .unwrap_or_else(|| Action::Convert(ext.unwrap()));
            assert_eq!(action, want);
            app.work.runner.cancel_all();
        });
    }
}

/// List `path` as the only row, as a drop would.
fn list_at_top(app: &mut App, path: &Path) {
    let e = model::entry_for(
        &app.kiln,
        &Availability::default(),
        path.to_path_buf(),
        1,
        None,
    );
    app.files.insert(0, e);
    app.motion.rows_added(0, 1, Instant::now());
}

/// Two PNGs, `a` and `b`, listed, with `a` selected, and the engines known.
fn two_listed_a_selected(rig: &Rig) -> (PathBuf, PathBuf) {
    let (a, b) = (a_png(rig, "a.png"), a_png(rig, "b.png"));
    rig.app(|app| {
        app.engines.waited_out = true;
        list_at_top(app, &b);
        list_at_top(app, &a);
        app.files.select_only(&[0]);
    });
    (a, b)
}

/// Let a plain drop's add finish and what it leaves to do run.
fn settle_drop(rig: &Rig) {
    assert!(pump_until(rig, Duration::from_secs(20), |app| app
        .incoming
        .walks
        .is_empty()));
    pump_until(rig, Duration::from_millis(200), |_| false);
}

#[test]
fn a_plain_drop_of_a_file_already_listed_opens_the_wheel_for_it_not_the_selection() {
    // `a` is selected; `b`, already in the list, is dropped again.
    let rig = empty_rig();
    let (_a, b) = two_listed_a_selected(&rig);
    rig.app(|app| plain_drop(app, std::slice::from_ref(&b), (300, 200)));
    settle_drop(&rig);
    rig.app(|app| {
        assert_eq!(app.files.len(), 2, "nothing new");
        let w = app.wheel.as_ref().expect("the drop's wheel");
        assert!(w.is_open());
        assert_eq!(w.paths(), std::slice::from_ref(&b));
        assert_eq!(
            app.files
                .selected_rows()
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            [1],
            "the dropped row"
        );
    });
}

#[test]
fn a_plain_drop_that_adds_nothing_opens_no_wheel() {
    // every dropped item is skipped (not found), and `a` is selected.
    let rig = empty_rig();
    let (_a, _b) = two_listed_a_selected(&rig);
    let gone = rig.dir.path().join("gone.png");
    rig.app(|app| plain_drop(app, std::slice::from_ref(&gone), (300, 200)));
    settle_drop(&rig);
    rig.app(|app| {
        assert!(app.wheel.is_none(), "no wheel for what was selected");
        assert!(app.top_info.is_none());
        assert_eq!(
            app.files
                .selected_rows()
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            [0],
            "the selection stays"
        );
    });
}

#[test]
fn a_plain_drop_whose_big_folder_is_cancelled_opens_no_wheel() {
    let rig = empty_rig();
    let (_a, _b) = two_listed_a_selected(&rig);
    let big = folder_of(rig.dir.path(), "Big", 1000);
    script::answer_with(|_, effect| match effect {
        Effect::Ask { .. } => Reply::Button(PromptButton::Cancel),
        other => panic!("unexpected {}", other.name()),
    });
    rig.app(|app| plain_drop(app, std::slice::from_ref(&big), (300, 200)));
    settle_drop(&rig);
    rig.app(|app| {
        assert_eq!(app.status.text, "Nothing added.");
        assert!(app.wheel.is_none(), "no wheel for what was selected");
        assert_eq!(app.files.len(), 2);
    });
}

#[test]
fn a_plain_drop_while_a_wheel_is_open_for_other_files_never_leaves_that_wheel() {
    // A wheel opened from the footer for `a`; then `b` is dropped.
    let rig = empty_rig();
    let (a, b) = two_listed_a_selected(&rig);
    rig.app(|app| {
        app.convert_selection(false);
        let w = app.wheel.as_ref().expect("the footer's wheel");
        assert_eq!(w.paths(), std::slice::from_ref(&a));
        plain_drop(app, std::slice::from_ref(&b), (300, 200));
    });
    settle_drop(&rig);
    rig.app(|app| {
        let open = app.wheel.as_ref().filter(|w| w.is_open());
        // Closed, or open for the drop's own file: never for `a`.
        if let Some(w) = open {
            assert_eq!(w.paths(), std::slice::from_ref(&b));
        }
        assert_eq!(
            app.files
                .selected_rows()
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            [1]
        );
    });
    // A drop that adds nothing closes a wheel open for other files.
    let gone = rig.dir.path().join("gone.png");
    rig.app(|app| {
        app.files.select_only(&[0]);
        app.close_wheel();
        app.wheel = None;
        app.convert_selection(false);
        assert!(app.wheel.as_ref().is_some_and(|w| w.is_open()));
        plain_drop(app, std::slice::from_ref(&gone), (300, 200));
    });
    settle_drop(&rig);
    rig.app(|app| assert!(!app.wheel.as_ref().is_some_and(|w| w.is_open())));
}

#[test]
fn a_held_enter_never_picks_on_the_wheel() {
    // Enter opens the wheel, lit on the suggestion; Windows then
    // repeats the held key (bit 30 of lParam).
    let rig = empty_rig();
    let png = a_png(&rig, "a.png");
    rig.app(|app| {
        app.engines.waited_out = true;
        list_at_top(app, &png);
        app.files.select_only(&[0]);
    });
    press(&rig, VK_RETURN);
    rig.app(|app| {
        let w = app.wheel.as_ref().filter(|w| w.is_open()).expect("open");
        // Something is lit, so a fresh Enter would pick it.
        if w.lit().is_none() {
            app.drive_wheel(|w, cx| w.key(cx, VK_RIGHT, Mods::default()));
        }
        assert!(app.wheel.as_ref().unwrap().lit().is_some());
    });
    for key in [VK_RETURN, VK_SPACE] {
        // SAFETY: a plain send to this thread's own window, no pointers.
        unsafe {
            SendMessageW(
                rig.hwnd,
                WM_KEYDOWN,
                Some(WPARAM(key.0 as usize)),
                Some(LPARAM(1 << 30)),
            )
        };
    }
    rig.app(|app| {
        assert!(app.work.jobs.is_empty(), "nothing started");
        assert!(app.wheel.as_ref().is_some_and(|w| w.is_open()));
    });
}

#[test]
fn the_top_infobar_is_announced_and_its_dismiss_is_in_tab_order_before_the_list() {
    // a photo and a sound share nothing.
    let rig = empty_rig();
    let png = a_png(&rig, "a.png");
    let mp3 = rig.dir.path().join("b.mp3");
    std::fs::write(&mp3, b"not really").unwrap();
    rig.app(|app| {
        app.engines.waited_out = true;
        plain_drop(app, &[png.clone(), mp3.clone()], (300, 200));
    });
    assert!(pump_until(&rig, Duration::from_secs(5), |app| app
        .top_info
        .is_some()));
    rig.app(|app| {
        let tree = app.a11y_tree(&[]);
        let info = tree.find(&a11y::Id::TopInfo).unwrap();
        assert!(info.live, "announced as it comes");
        assert_eq!(info.status.as_deref(), Some("Information"));
        // Its Dismiss takes UI Automation's focus, as a Tab stop does.
        let dismiss = tree.find(&a11y::Id::TopInfoDismiss).unwrap();
        assert!(dismiss.states.focusable);
        app.button_focus = None;
    });
    let focused = |rig: &Rig| rig.app(|app| app.a11y_tree(&[]).focused().map(|n| n.id.clone()));
    // Reading order: the header, the InfoBar, then the list.
    press_shift(&rig, VK_TAB);
    assert_eq!(focused(&rig), Some(a11y::Id::TopInfoDismiss));
    press_shift(&rig, VK_TAB);
    assert_eq!(focused(&rig), Some(a11y::Id::SettingsButton));
    press(&rig, VK_TAB);
    assert_eq!(focused(&rig), Some(a11y::Id::TopInfoDismiss));
    // Enter dismisses it, and the keyboard goes back to the list.
    press(&rig, VK_RETURN);
    rig.app(|app| {
        assert!(app.top_info.is_none());
        assert_eq!(app.button_focus, None);
        assert_eq!(app.files.len(), 2);
    });
}

/// Press `key` with Shift held.
fn press_shift(rig: &Rig, key: VIRTUAL_KEY) {
    rig.app(|app| {
        app.dispatch(Input::Key {
            key,
            shift: true,
            ctrl: false,
            alt: false,
        })
    });
}

#[test]
fn the_top_infobar_is_told_to_screen_readers_as_it_shows() {
    let rig = empty_rig();
    rig.app(|app| {
        app.status.sets.clear();
        app.set_top_info(Some(ways_in::mixed_drop_info(2)));
        let (title, message) = ways_in::mixed_drop_info(2);
        assert_eq!(app.status.sets, [format!("{title} {message}")]);
        // Taken away, it says nothing.
        app.status.sets.clear();
        app.set_top_info(None);
        assert!(app.status.sets.is_empty());
    });
}

#[test]
fn the_top_infobar_goes_with_a_clear_a_removal_or_any_add() {
    let rig = empty_rig();
    let (a, b) = (a_png(&rig, "a.png"), a_png(&rig, "b.png"));
    let info = ways_in::mixed_drop_info(2);
    rig.app(|app| {
        list_at_top(app, &a);
        list_at_top(app, &b);
        app.set_top_info(Some(info.clone()));
        app.clear_rows();
        assert!(app.top_info.is_none(), "Clear list");
        list_at_top(app, &a);
        app.set_top_info(Some(info.clone()));
        app.remove_rows(std::slice::from_ref(&a));
        assert!(app.top_info.is_none(), "Remove from list");
        app.set_top_info(Some(info.clone()));
        // Ctrl+V and Add files add, as a drop does.
        app.add(vec![b.clone()], true);
        assert!(app.top_info.is_none(), "an add");
    });
}

/// The Make smaller dialog's estimate lines and the options it asks for.
fn smaller_lines(app: &App) -> ((String, String), kiln_core::ToolOptions) {
    let t = app.prompts.tool_ui.as_ref().expect("the dialog");
    let m = t.model();
    (
        m.estimate_lines(t.saving()).unwrap(),
        m.tool_options().unwrap(),
    )
}

/// Open Make smaller on the listed `png`, a 3 MB photo.
fn open_smaller(app: &mut App, png: &Path) {
    app.files.entries_mut()[0].size = 3_000_000;
    app.files.select_only(&[0]);
    app.start(
        Action::Tool(Tool::Smaller, kiln_core::Kind::Image),
        vec![png.to_path_buf()],
        None,
    );
    assert!(app.prompts.tool_ui.is_some());
}

#[test]
fn a_late_estimate_from_a_closed_dialog_never_reaches_the_next() {
    let rig = empty_rig();
    let png = a_png(&rig, "a.png");
    rig.app(|app| {
        // A worker that never answers: the test answers for it.
        app.prompts.estimator = Some(workers::Estimator::start_with(|_| {}));
        list_at_top(app, &png);
        open_smaller(app, &png);
        let first = app.prompts.tool_dialog;
        let (_, opts) = smaller_lines(app);
        app.close_tool();
        // Opened again with the same choices.
        open_smaller(app, &png);
        assert_ne!(app.prompts.tool_dialog, first);
        app.on_event(Event::Estimated(
            first,
            opts.clone(),
            crate::smaller::Estimate::About(4_100_000),
        ));
        let ((bold, _), _) = smaller_lines(app);
        assert_eq!(bold, "Working out the size…", "the old dialog's answer");
        let now = app.prompts.tool_dialog;
        app.on_event(Event::Estimated(
            now,
            opts,
            crate::smaller::Estimate::About(900_000),
        ));
        let ((bold, _), _) = smaller_lines(app);
        assert!(bold.starts_with("About "), "{bold}");
    });
}

#[test]
fn with_no_thread_for_the_estimate_the_dialog_shows_the_size_asked_for() {
    let rig = empty_rig();
    let png = a_png(&rig, "a.png");
    rig.app(|app| {
        app.prompts.estimator = Some(workers::Estimator::none());
        list_at_top(app, &png);
        open_smaller(app, &png);
        let ((bold, _), _) = smaller_lines(app);
        // The second preset, 1 MB.
        assert_eq!(bold, "Up to 1 MB");
        assert!(app.prompts.tool_ui.as_ref().unwrap().model().can_run());
    });
}

#[test]
fn a_redo_or_a_drag_wheel_job_leaves_no_arrival_waiting_for_its_files() {
    // a file that arrived, then converted by a redo before its wheel
    // was due, asks nothing more once the job is done.
    let rig = empty_rig();
    let png = a_png(&rig, "a.png");
    let other = a_png(&rig, "b.png");
    rig.app(|app| {
        app.engines.waited_out = true;
        list_at_top(app, &other);
        list_at_top(app, &png);
        app.arrive(&[png.clone(), other.clone()]);
        app.launch_full(
            Action::Convert("jpg".into()),
            vec![png.clone()],
            None,
            Origin::Window,
            Via::Window,
            None,
        );
        app.work.runner.cancel_all();
        std::thread::sleep(ways_in::ARRIVALS_SETTLE + Duration::from_millis(20));
        app.arrivals_due();
        // The wheel for what is still waiting: `b` alone.
        let w = app.wheel.as_ref().expect("the wheel for b");
        assert_eq!(w.paths(), std::slice::from_ref(&other));
    });
}

#[test]
fn the_estimate_line_is_live_and_told_as_it_comes() {
    let rig = empty_rig();
    let png = a_png(&rig, "a.png");
    rig.app(|app| {
        app.prompts.estimator = Some(workers::Estimator::start_with(|_| {}));
        list_at_top(app, &png);
        open_smaller(app, &png);
        let tree = app.a11y_tree(&[]);
        let line = tree.find(&a11y::Id::ToolText(2)).expect("the estimate");
        assert!(line.live);
        let (_, opts) = smaller_lines(app);
        app.status.sets.clear();
        let now = app.prompts.tool_dialog;
        app.on_event(Event::Estimated(
            now,
            opts,
            crate::smaller::Estimate::About(900_000),
        ));
        let ((bold, _), _) = smaller_lines(app);
        assert_eq!(app.status.sets, [bold]);
    });
}

#[test]
fn a_pair_with_nothing_to_set_never_opens_the_options_dialog() {
    // UX-08: Markdown to HTML has no knobs.
    let rig = empty_rig();
    let md = rig.dir.path().join("notes.md");
    std::fs::write(&md, b"# Notes").unwrap();
    rig.app(|app| {
        list_at_top(app, &md);
        assert!(!app.open_sheet_for("html", vec![md.clone()], true));
        assert!(app.sheet.is_none());
    });
}

#[test]
fn the_options_dialog_is_one_of_kilns_dialogs_and_ui_automation_reaches_it() {
    let rig = empty_rig();
    let png = a_png(&rig, "a.png");
    let other = a_png(&rig, "b.png");
    rig.app(|app| {
        list_at_top(app, &png);
        app.files.select_only(&[0]);
        assert!(app.open_sheet_for("jpg", vec![png.clone()], true));
        // In-window modal: drops wait, as under a prompt.
        assert!(system::modal_open());
        plain_drop(app, std::slice::from_ref(&other), (300, 200));
        assert_eq!(app.incoming.queued, std::slice::from_ref(&other));
        // The Window pattern's Close is Cancel.
        app.a11y_do(a11y::Request {
            id: a11y::Id::Sheet,
            verb: a11y::Verb::Close,
        });
        assert!(!app.sheet.as_ref().is_some_and(|s| s.is_open()));
        assert!(app.work.jobs.is_empty());
        // Opened again, More options' line is a drop-down UI Automation
        // opens and picks from.
        app.sheet = None;
        assert!(app.open_sheet_for("jpg", vec![png.clone()], true));
        let line = a11y::Id::SheetControl(SheetControl::Step(1));
        let ask = |app: &mut App, id: a11y::Id, verb: a11y::Verb| {
            app.a11y_do(a11y::Request { id, verb });
        };
        ask(
            app,
            a11y::Id::SheetControl(SheetControl::Advanced),
            a11y::Verb::Expand(true),
        );
        ask(app, line.clone(), a11y::Verb::Expand(true));
        let node = app.a11y_tree(&[]).find(&line).cloned().expect("the line");
        assert_eq!(node.role, a11y::Role::ComboBox);
        assert_eq!(node.states.expanded, Some(true));
        let was = node.value.clone().unwrap();
        let other = node
            .children
            .iter()
            .position(|c| c.name != was)
            .expect("another value");
        let want = node.children[other].name.clone();
        ask(
            app,
            a11y::Id::Choice(Box::new(line.clone()), other),
            a11y::Verb::Select,
        );
        let node = app.a11y_tree(&[]).find(&line).cloned().unwrap();
        assert_eq!(node.value.as_deref(), Some(want.as_str()));
        assert_eq!(node.states.expanded, Some(false));
        // UI Automation's Invoke on Convert starts the job.
        app.a11y_do(a11y::Request {
            id: a11y::Id::SheetControl(SheetControl::Convert),
            verb: a11y::Verb::Invoke,
        });
        assert!(!app.sheet.as_ref().is_some_and(|s| s.is_open()));
    });
}

#[test]
fn settings_hears_a_chosen_folder_is_gone_from_the_engines_thread() {
    // The window's thread never looks at the folder itself: one on a
    // sleeping network drive would hold it for the drive's timeout.
    let rig = Rig::new();
    let gone = rig.dir.path().join("Gone away");
    rig.app(|app| {
        app.engines = Engines::default();
        app.prefs.settings.save_to = Some(gone.clone());
        app.open_settings();
        let missing = |app: &App| app.settings_ui.as_ref().unwrap().facts().save_to_missing;
        assert!(!missing(app), "nothing is said before the look answers");
        let found = |app: &mut App, save_to: Option<(PathBuf, bool)>| {
            let id = app.engines.asked;
            app.engines_found(workers::EnginesFound {
                id,
                available: Availability::all(),
                failed: false,
                status: Some(std::array::from_fn(|_| settings::EngineStatus::Missing)),
                office: None,
                save_to,
                winget: None,
            });
        };
        found(app, Some((gone.clone(), true)));
        assert!(missing(app), "the look found it gone");
        // A look made for another folder says nothing of this one.
        let other = rig.dir.path().join("Other");
        app.prefs.settings.save_to = Some(other.clone());
        app.settings_facts_changed();
        assert!(!missing(app));
        found(app, Some((other, false)));
        assert!(!missing(app));
    });
}

#[test]
fn settings_says_where_the_helpers_are_as_the_engines_thread_answers() {
    use settings::EngineStatus::{Missing, OnPath};
    let rig = Rig::new();
    rig.app(|app| {
        app.engines = Engines::default();
        let pandoc = Engine::ALL
            .iter()
            .position(|e| *e == Engine::Pandoc)
            .unwrap();
        // Opening Settings asks the engines thread, and searches nothing
        // itself: before any answer nothing is said of the helpers.
        app.open_settings();
        let asked = app.engines.asked;
        assert!(asked > 0, "opening Settings looks again");
        let facts = |app: &App| app.settings_ui.as_ref().unwrap().facts().clone();
        assert_eq!(facts(app).engines, None);
        assert_eq!(facts(app).office, None);
        // The answer comes: the open page follows it, Office included.
        let mut status: [settings::EngineStatus; 4] = std::array::from_fn(|_| Missing);
        status[pandoc] = OnPath(PathBuf::from(r"C:\x\pandoc.exe"));
        let office = settings::OfficeStatus {
            badge: settings_page::FOUND,
            apps: Some("Word".into()),
            folder: Some(PathBuf::from(r"C:\Office")),
        };
        app.engines_found(workers::EnginesFound {
            id: asked,
            available: Availability::all(),
            failed: false,
            status: Some(status),
            office: Some(office.clone()),
            save_to: None,
            winget: None,
        });
        let f = facts(app);
        assert_eq!(
            f.engines.as_ref().unwrap()[pandoc],
            OnPath(PathBuf::from(r"C:\x\pandoc.exe"))
        );
        assert_eq!(f.office, Some(office.clone()));
        // An older look's answer never replaces it.
        app.engines_found(workers::EnginesFound {
            id: asked - 1,
            available: Availability::all(),
            failed: false,
            status: Some(std::array::from_fn(|_| Missing)),
            office: None,
            save_to: None,
            winget: None,
        });
        let f = facts(app);
        assert_eq!(
            f.engines.as_ref().unwrap()[pandoc],
            OnPath(PathBuf::from(r"C:\x\pandoc.exe"))
        );
        assert_eq!(f.office, Some(office));
    });
}

#[test]
fn a_new_look_keeps_the_keyboard_on_the_same_control() {
    use settings::EngineStatus::{Missing, OnPath};
    use settings_page::Helper;
    let rig = Rig::new();
    rig.app(|app| {
        app.engines = Engines::default();
        app.open_settings();
        let answer = |app: &mut App, status: [settings::EngineStatus; 4]| {
            let id = app.engines.asked;
            app.engines_found(workers::EnginesFound {
                id,
                available: Availability::all(),
                failed: false,
                status: Some(status),
                office: None,
                save_to: None,
                winget: None,
            });
        };
        answer(
            app,
            std::array::from_fn(|_| OnPath(PathBuf::from(r"C:\x\e.exe"))),
        );
        app.activate(Control::Helper(Helper::Pandoc), settings_page::Set::Press);
        let choose = Control::Choose(Engine::Pandoc);
        let at = |app: &App| app.settings_layout().unwrap().find(choose).unwrap();
        let before = at(app);
        let mut ui = app.settings_ui.take().unwrap();
        ui.focus_on(&app.surface_cx(), choose);
        app.settings_ui = Some(ui);
        // A new look finds Pandoc missing: its Get it comes before Choose,
        // which moves along, and keeps the keyboard.
        app.look_for_engines();
        let mut status: [settings::EngineStatus; 4] =
            std::array::from_fn(|_| OnPath(PathBuf::from(r"C:\x\e.exe")));
        status[1] = Missing;
        answer(app, status);
        let after = at(app);
        assert_eq!(after, before + 1, "Get it came in before Choose");
        assert_eq!(app.settings_ui.as_ref().unwrap().focus(), Some(choose));
    });
}

#[test]
fn a_folder_i_choose_opens_the_folder_picker_and_cancelling_keeps_next_to_the_original() {
    use settings_page::Set;
    let rig = Rig::new();
    let picked = Rc::new(RefCell::new(None::<PathBuf>));
    let answer = picked.clone();
    script::answer_with(move |_, effect| match effect {
        Effect::PickFolder { .. } => Reply::Path(answer.borrow().clone()),
        other => panic!("unexpected {}", other.name()),
    });
    rig.app(|app| {
        assert_eq!(app.prefs.settings.save_to, None);
        app.open_settings();
        app.activate(Control::SaveWhere, Set::Pick(1));
    });
    run_effects(rig.hwnd, rig.cell());
    assert_eq!(script::ran(), ["pick folder"]);
    // Cancelled: still next to the original, and the page says so.
    rig.app(|app| {
        assert_eq!(app.prefs.settings.save_to, None);
        let cx = app.surface_cx();
        let page = app.settings_ui.as_ref().unwrap().page(&cx);
        assert!(matches!(
            page.card(settings_page::Card::SaveWhere).unwrap().body,
            settings_page::Body::Combo { value: 0, .. }
        ));
    });
    // A folder chosen: new files go there, and it is saved.
    let folder = rig.dir.path().join("Converted");
    *picked.borrow_mut() = Some(folder.clone());
    rig.app(|app| app.activate(Control::SaveWhere, Set::Pick(1)));
    run_effects(rig.hwnd, rig.cell());
    assert_eq!(script::ran(), ["pick folder"]);
    rig.app(|app| {
        assert_eq!(
            app.prefs.settings.save_to.as_deref(),
            Some(folder.as_path())
        );
        app.prefs.store.flush().unwrap();
    });
    assert!(saved_settings(&rig).contains("save_to = "));
    // Next to the original again: no picker.
    rig.app(|app| app.activate(Control::SaveWhere, Set::Pick(0)));
    run_effects(rig.hwnd, rig.cell());
    assert!(script::ran().is_empty());
    rig.app(|app| assert_eq!(app.prefs.settings.save_to, None));
}

#[test]
fn licences_open_opens_the_notices_as_text() {
    use settings_page::Set;
    let rig = Rig::new();
    let notices = rig.dir.path().join("THIRD_PARTY_NOTICES.md");
    std::fs::write(&notices, "notices").unwrap();
    super::notices_here(Some(notices.clone()));
    let opened = Rc::new(RefCell::new(None::<PathBuf>));
    let seen = opened.clone();
    script::answer_with(move |_, effect| match effect {
        Effect::OpenText { target, .. } => {
            *seen.borrow_mut() = Some(target.clone());
            Reply::Ok(true)
        }
        other => panic!("unexpected {}", other.name()),
    });
    rig.app(|app| {
        app.open_settings();
        app.activate(Control::Licences, Set::Press);
    });
    run_effects(rig.hwnd, rig.cell());
    super::notices_here(None);
    assert_eq!(script::ran(), ["open as text"]);
    assert_eq!(opened.borrow().as_deref(), Some(notices.as_path()));
}

#[test]
fn donate_opens_the_donate_page() {
    use settings_page::Set;
    let rig = Rig::new();
    let opened = Rc::new(RefCell::new(None::<PathBuf>));
    let seen = opened.clone();
    script::answer_with(move |_, effect| match effect {
        Effect::Open { target, .. } => {
            *seen.borrow_mut() = Some(target.clone());
            Reply::Ok(true)
        }
        other => panic!("unexpected {}", other.name()),
    });
    rig.app(|app| {
        app.open_settings();
        app.activate(Control::Donate, Set::Press);
    });
    run_effects(rig.hwnd, rig.cell());
    assert_eq!(script::ran(), ["open"]);
    assert_eq!(
        opened.borrow().as_deref(),
        Some(std::path::Path::new(settings_page::DONATE_PAGE))
    );
}

/// Effects answered as opened (or not, by `opens`), the pages they opened
/// kept in order.
fn opens_pages(opens: bool) -> Rc<RefCell<Vec<PathBuf>>> {
    let opened = Rc::new(RefCell::new(vec![]));
    let seen = opened.clone();
    script::answer_with(move |_, effect| match effect {
        Effect::Open { target, .. } => {
            seen.borrow_mut().push(target.clone());
            Reply::Ok(opens)
        }
        other => panic!("unexpected {}", other.name()),
    });
    opened
}

#[test]
fn the_credit_link_opens_the_authors_page_by_click_key_and_ui_automation() {
    let rig = empty_rig();
    let opened = opens_pages(true);
    let page = PathBuf::from(crate::credit::PAGE);
    let link = rig.app(|app| app.credit().expect("the empty window shows it").name);
    let (x, y) = (link.x + link.w / 2.0, link.y + link.h / 2.0);
    // A press that slides off does nothing; a click opens the page.
    rig.app(|app| {
        app.mouse_down(x, y, 0);
        app.mouse_up(link.x - 40.0, y);
    });
    run_effects(rig.hwnd, rig.cell());
    assert!(opened.borrow().is_empty());
    rig.app(|app| {
        app.mouse_move(x, y, 0);
        assert_eq!(app.list.over_button(), Some(WindowButton::Credit));
        app.mouse_down(x, y, 0);
        app.mouse_up(x, y);
    });
    run_effects(rig.hwnd, rig.cell());
    assert_eq!(*opened.borrow(), std::slice::from_ref(&page));
    // Tab reaches it last in the empty window; Enter opens it.
    let focused = |rig: &Rig| rig.app(|app| app.a11y_tree(&[]).focused().map(|n| n.id.clone()));
    for _ in 0..6 {
        press(&rig, VK_TAB);
        if focused(&rig) == Some(a11y::Id::CreditLink) {
            break;
        }
    }
    assert_eq!(focused(&rig), Some(a11y::Id::CreditLink));
    rig.app(|app| assert_eq!(app.focus_button_rect(FocusButton::Credit), Some(link)));
    press(&rig, VK_RETURN);
    run_effects(rig.hwnd, rig.cell());
    assert_eq!(opened.borrow().len(), 2);
    // UI Automation's Invoke opens it too.
    rig.app(|app| {
        app.a11y_do(a11y::Request {
            id: a11y::Id::CreditLink,
            verb: a11y::Verb::Invoke,
        })
    });
    run_effects(rig.hwnd, rig.cell());
    assert_eq!(*opened.borrow(), [page.clone(), page.clone(), page]);
}

#[test]
fn the_credit_sits_under_the_status_with_files_and_says_when_the_page_didnt_open() {
    let rig = Rig::new();
    let opened = opens_pages(false);
    rig.app(|app| {
        assert!(!app.files.is_empty());
        let row = app.footer_row().unwrap();
        let c = app.credit().expect("room under the status");
        assert_eq!(Some(c), row.credit);
        assert_eq!(c.line.y, row.status.bottom());
        assert!(c.line.right() < row.add.x);
        app.a11y_do(a11y::Request {
            id: a11y::Id::CreditLink,
            verb: a11y::Verb::Invoke,
        });
    });
    run_effects(rig.hwnd, rig.cell());
    assert_eq!(opened.borrow().len(), 1);
    rig.app(|app| {
        assert!(app.status.alerting());
        assert_eq!(app.status.text, crate::credit::DIDNT_OPEN);
    });
}

#[test]
fn copy_diagnostics_puts_the_environment_on_the_clipboard() {
    use settings_page::Set;
    let rig = Rig::new();
    let copied = Rc::new(RefCell::new(None::<String>));
    let seen = copied.clone();
    script::answer_with(move |_, effect| match effect {
        Effect::Clipboard { text, .. } => {
            *seen.borrow_mut() = Some(text.clone());
            Reply::Ok(true)
        }
        other => panic!("unexpected {}", other.name()),
    });
    rig.app(|app| {
        app.open_settings();
        app.activate(Control::CopyDiagnostics, Set::Press);
    });
    run_effects(rig.hwnd, rig.cell());
    assert_eq!(script::ran(), ["clipboard"]);
    let text = copied.borrow().clone().unwrap();
    assert!(text.starts_with(&format!("Kiln {}", crate::update::CURRENT)));
    for needle in [
        "\nWindows ",
        "\nRuns from ",
        "\nHelper apps\n",
        "  FFmpeg: ",
    ] {
        assert!(text.contains(needle), "{needle:?} is missing from {text}");
    }
}

/// Press Install with winget for Pandoc, answering the question with
/// `button` and a console start with `started`. The console effects seen,
/// as (program, id), and the effects that ran.
fn press_winget(
    button: PromptButton,
    started: bool,
) -> (Vec<(PathBuf, String)>, Vec<&'static str>, Rig) {
    use settings_page::Set;
    let rig = Rig::new();
    let consoles = Rc::new(RefCell::new(vec![]));
    let seen = consoles.clone();
    script::answer_with(move |_, effect| match effect {
        Effect::Ask { .. } => Reply::Button(button),
        Effect::RunInConsole { winget, id, .. } => {
            seen.borrow_mut().push((winget.clone(), id.clone()));
            Reply::Ok(started)
        }
        other => panic!("unexpected {}", other.name()),
    });
    rig.app(|app| {
        app.engines.winget = Some(PathBuf::from(r"C:\Apps\winget.exe"));
        app.open_settings();
        app.activate(Control::Winget(Engine::Pandoc), Set::Press);
    });
    run_effects(rig.hwnd, rig.cell());
    let consoles = consoles.borrow().clone();
    (consoles, script::ran(), rig)
}

fn winget_alert(rig: &Rig) -> Option<(settings_page::Card, String)> {
    rig.app(|app| app.settings_ui.as_ref().unwrap().facts().alert.clone())
}

#[test]
fn install_with_winget_asks_first_and_cancel_starts_nothing() {
    let (consoles, ran, rig) = press_winget(PromptButton::Cancel, true);
    assert_eq!(ran, ["ask"]);
    assert!(consoles.is_empty());
    assert_eq!(winget_alert(&rig), None);
}

#[test]
fn install_with_winget_runs_winget_for_the_helpers_package() {
    let (consoles, ran, rig) = press_winget(PromptButton::Install, true);
    assert_eq!(ran, ["ask", "run in console"]);
    assert_eq!(
        consoles,
        [(
            PathBuf::from(r"C:\Apps\winget.exe"),
            "JohnMacFarlane.Pandoc".to_owned()
        )]
    );
    assert_eq!(winget_alert(&rig), None);
}

#[test]
fn a_winget_console_that_didnt_start_says_so_on_the_helpers_card() {
    let (_, ran, rig) = press_winget(PromptButton::Install, false);
    assert_eq!(ran, ["ask", "run in console"]);
    assert_eq!(
        winget_alert(&rig),
        Some((
            settings_page::Card::Helper(settings_page::Helper::Pandoc),
            "Couldn't start winget for Pandoc.".to_owned()
        ))
    );
}

#[test]
fn the_drag_wheel_toggle_turns_kilns_start_up_entry_on_and_off() {
    use settings_page::Set;
    let rig = Rig::new();
    rig.run.delete().unwrap();
    rig.app(|app| {
        app.open_settings();
        app.activate(Control::DragWheel, Set::Turn(true));
        assert!(app.autostart.finish(Duration::from_secs(10)));
        assert!(app.prefs.settings.drag_wheel);
    });
    assert_eq!(
        rig.run.value(),
        Some(autostart::run_value(std::path::Path::new(RIG_EXE)))
    );
    rig.app(|app| {
        app.activate(Control::DragWheel, Set::Turn(false));
        assert!(app.autostart.finish(Duration::from_secs(10)));
        assert!(!app.prefs.settings.drag_wheel);
        // The page's switch reads the setting.
        let cx = app.surface_cx();
        let page = app.settings_ui.as_ref().unwrap().page(&cx);
        assert_eq!(
            page.card(settings_page::Card::DragWheel).unwrap().body,
            settings_page::Body::Toggle {
                control: Control::DragWheel,
                on: false
            }
        );
    });
    assert_eq!(rig.run.value(), None);
}

#[test]
fn the_quality_cards_change_the_settings() {
    use crate::quality::DefaultSetting as D;
    use settings_page::Set;
    let rig = Rig::new();
    rig.app(|app| {
        app.open_settings();
        app.activate(
            Control::Preset(D::Video, kiln_core::Preset::Best),
            Set::Pick(2),
        );
        assert_eq!(app.prefs.settings.quality.video, kiln_core::Preset::Best);
        // Largest video size: Original, 2160p, 1080p, 720p, 480p.
        app.activate(Control::Choice(D::Resolution), Set::Pick(3));
        assert_eq!(
            app.prefs.settings.quality.resolution,
            kiln_core::Limit::Max(720)
        );
        app.activate(Control::Choice(D::Audio), Set::Pick(0));
        assert_eq!(app.prefs.settings.quality.audio_kbps, None);
        app.activate(Control::Choice(D::PageDpi), Set::Pick(2));
        assert_eq!(app.prefs.settings.quality.page_dpi, 300);
        app.activate(Control::Theme, Set::Pick(1));
        assert_eq!(app.prefs.settings.theme, settings::ThemeChoice::Light);
        app.prefs.store.flush().unwrap();
    });
    let text = saved_settings(&rig);
    assert!(text.contains("page_dpi = 300"), "{text}");
    assert!(
        text.contains("theme = 'light'") || text.contains("theme = \"light\""),
        "{text}"
    );
}

#[test]
fn a_named_theme_is_drawn_on_its_solid_base_and_is_saved_by_name() {
    use settings_page::Set;
    let rig = Rig::new();
    rig.app(|app| {
        app.open_settings();
        let nord = settings_page::THEMES
            .iter()
            .position(|t| *t == settings::ThemeChoice::Nord)
            .unwrap();
        app.activate(Control::Theme, Set::Pick(nord));
        assert_eq!(app.prefs.settings.theme, settings::ThemeChoice::Nord);
        // No Mica behind a named theme, whatever Windows allows.
        assert_eq!(app.prefs.backdrop, titlebar::Backdrop::Solid);
        // A contrast theme still wins over the choice.
        app.prefs.contrast = Some(theme::SysColors {
            window: theme::Rgb(0, 0, 0),
            window_text: theme::Rgb(255, 255, 255),
            highlight: theme::Rgb(0x1A, 0xEB, 0xFF),
            highlight_text: theme::Rgb(0, 0, 0),
            gray_text: theme::Rgb(0x3F, 0xF2, 0x3F),
            hotlight: theme::Rgb(255, 255, 0),
            button_face: theme::Rgb(0, 0, 0),
            button_text: theme::Rgb(255, 255, 255),
        });
        app.apply_theme();
        assert!(app.prefs.palette.high_contrast);
        // Settings says so beside the Theme list.
        assert!(app.settings_facts().contrast);
        assert_eq!(app.prefs.settings.theme, settings::ThemeChoice::Nord);
        app.prefs.store.flush().unwrap();
    });
    let text = saved_settings(&rig);
    assert!(
        text.contains("theme = 'nord'") || text.contains("theme = \"nord\""),
        "{text}"
    );
}

#[test]
fn ctrl_comma_opens_settings_and_escape_and_back_close_it() {
    let rig = Rig::new();
    rig.app(|app| assert!(app.settings_ui.is_none()));
    // The window reads Ctrl from the keyboard's state, which a sent
    // message doesn't change: the key goes in as the list hears it.
    rig.app(|app| app.list_key(VK_OEM_COMMA, false, true, false));
    assert!(
        rig.app(|app| app.settings_ui.is_some()),
        "Ctrl+, opened nothing"
    );
    press(&rig, VK_ESCAPE);
    assert!(rig.app(|app| app.settings_ui.is_none()), "Esc left it open");
    rig.app(|app| app.open_settings());
    rig.app(|app| app.title_act(titlebar::Part::Back));
    assert!(
        rig.app(|app| app.settings_ui.is_none()),
        "Back left it open"
    );
}

/// Hand the window notice `id` as a second launch does, under `tag`.
fn send_notice(rig: &Rig, tag: usize, id: u64) -> isize {
    use windows::Win32::System::DataExchange::COPYDATASTRUCT;
    use windows::Win32::UI::WindowsAndMessaging::WM_COPYDATA;
    let data = id.to_le_bytes();
    let cds = COPYDATASTRUCT {
        dwData: tag,
        cbData: data.len() as u32,
        lpData: data.as_ptr() as *mut _,
    };
    // SAFETY: a send to this thread's own window; `cds` and `data` outlive
    // it.
    unsafe {
        SendMessageW(
            rig.hwnd,
            WM_COPYDATA,
            Some(WPARAM(0)),
            Some(LPARAM(&cds as *const _ as isize)),
        )
    }
    .0
}

/// A finished, saved notice about the rig's row, shown as the installed
/// Kiln shows it: its id.
fn saved_toast(rig: &Rig) -> u64 {
    toast::shown::take();
    rig.app(|app| {
        app.notify_by = toast::Channel::Toast;
        let words = toast::saved(&["a.jpg".into()], &["in".into()], None);
        let about = NoticeAbout {
            row: Some(PathBuf::from(r"C:\in\a.png")),
            saved: true,
            reveal: None,
        };
        app.notify_about("Kiln", "Saved a.jpg next to the original.", words, about);
    });
    let (toasts, balloons) = toast::shown::take();
    assert!(balloons.is_empty());
    let [xml] = &toasts[..] else {
        panic!("one toast: {toasts:?}")
    };
    // Its body and Show in folder carry the same link.
    let link = xml
        .split("launch=\"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .unwrap();
    assert_eq!(xml.matches(link).count(), 2, "{xml}");
    assert!(xml.contains("Show in folder") && xml.contains("Dismiss"));
    handoff::notice_id(link).expect("the link reads back")
}

#[test]
fn show_in_folder_on_several_saved_files_shows_every_one() {
    let rig = Rig::new();
    toast::shown::take();
    let folder = PathBuf::from(r"D:\Converted");
    let files = vec![folder.join("a.jpg"), folder.join("b.jpg")];
    let reveal = Some((folder.clone(), files.clone()));
    rig.app(|app| {
        app.notify_by = toast::Channel::Toast;
        let names = ["a.jpg".to_owned(), "b.jpg".to_owned()];
        let words = toast::saved(&names, &["Converted".into(), "Converted".into()], None);
        let about = NoticeAbout {
            row: Some(PathBuf::from(r"C:\in\a.png")),
            saved: true,
            reveal,
        };
        app.notify_about("Kiln", "Saved 2 files in Converted.", words, about);
    });
    let (toasts, _) = toast::shown::take();
    let link = toasts[0]
        .split("launch=\"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .unwrap()
        .to_owned();
    let seen = Rc::new(RefCell::new(None));
    let answer = seen.clone();
    script::answer_with(move |_, effect| match effect {
        Effect::ShowInFolder { folder, files, .. } => {
            *answer.borrow_mut() = Some((folder.clone(), files.clone()));
            Reply::Ok(true)
        }
        other => panic!("only Explorer opens: {}", other.name()),
    });
    let id = handoff::notice_id(&link).unwrap();
    assert_eq!(send_notice(&rig, handoff::SHOW_NOTICE, id), 1);
    assert_eq!(*seen.borrow(), Some((folder, files)));
}

#[test]
fn a_notice_link_with_a_current_id_shows_that_rows_file_in_its_folder() {
    let rig = Rig::new();
    let id = saved_toast(&rig);
    script::answer_with(|_, effect| match effect {
        Effect::ShowInFolder { folder, files, .. } => {
            assert_eq!(folder, &PathBuf::from(r"C:\in"));
            assert_eq!(files, &[PathBuf::from(r"C:\in\a.png")]);
            Reply::Ok(true)
        }
        other => panic!("only Explorer opens: {}", other.name()),
    });
    assert_eq!(send_notice(&rig, handoff::SHOW_NOTICE, id), 1);
    assert_eq!(script::ran(), ["show in folder"]);
    // Again, from the toast still in the notification centre: the same.
    assert_eq!(send_notice(&rig, handoff::SHOW_NOTICE, id), 1);
    assert_eq!(script::ran(), ["show in folder"]);
}

#[test]
fn a_notice_link_with_an_unknown_or_old_id_does_nothing() {
    let rig = Rig::new();
    let id = saved_toast(&rig);
    script::answer_with(|_, effect| panic!("nothing happens: {}", effect.name()));
    for other in [id.wrapping_add(1), id.wrapping_sub(1), 0, u64::MAX, !id] {
        assert_eq!(send_notice(&rig, handoff::SHOW_NOTICE, other), 0, "{other}");
    }
    // Under another tag, the same bytes are no notice either.
    send_notice(&rig, 0x1234, id);
    assert!(script::ran().is_empty());
    assert!(!rig.app(|app| app.window_shown()));
    // Once KEPT newer notices have come, the first is forgotten.
    rig.app(|app| {
        for _ in 0..toast::KEPT {
            app.notify("Kiln", "Something.");
        }
    });
    toast::shown::take();
    assert_eq!(send_notice(&rig, handoff::SHOW_NOTICE, id), 0);
    assert!(script::ran().is_empty());
}

#[test]
fn a_notice_about_a_held_result_opens_kiln_on_its_row() {
    let rig = Rig::new();
    toast::shown::take();
    rig.app(|app| {
        app.notify_by = toast::Channel::Toast;
        let about = NoticeAbout {
            row: Some(PathBuf::from(r"C:\in\a.png")),
            saved: false,
            reveal: None,
        };
        app.notify_about(
            "Kiln",
            "a.jpg is ready.",
            toast::plain("Kiln", "a.jpg is ready."),
            about,
        );
    });
    let (toasts, _) = toast::shown::take();
    let link = toasts[0]
        .split("launch=\"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .unwrap()
        .to_owned();
    // No Show in folder for a result that isn't saved.
    assert!(!toasts[0].contains("<actions>"), "{}", toasts[0]);
    script::answer_with(|_, effect| match effect {
        Effect::Show(_) | Effect::BringForward | Effect::Foreground => Reply::Ok(true),
        other => panic!("Kiln opens: {}", other.name()),
    });
    let id = handoff::notice_id(&link).unwrap();
    assert_eq!(send_notice(&rig, handoff::SHOW_NOTICE, id), 1);
    assert!(script::ran().contains(&"show"));
    rig.app(|app| assert_eq!(app.files.selection(), [0]));
}

#[test]
fn a_notice_with_settings_open_only_brings_kiln_forward() {
    let rig = Rig::new();
    toast::shown::take();
    rig.app(|app| {
        app.notify_by = toast::Channel::Toast;
        let about = NoticeAbout {
            row: Some(PathBuf::from(r"C:\in\a.png")),
            saved: false,
            reveal: None,
        };
        app.notify_about(
            "Kiln",
            "a.jpg is ready.",
            toast::plain("Kiln", "a.jpg is ready."),
            about,
        );
        app.open_settings();
        assert!(app.files.selection().is_empty());
    });
    let (toasts, _) = toast::shown::take();
    let link = toasts[0]
        .split("launch=\"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .unwrap()
        .to_owned();
    script::answer_with(|_, effect| match effect {
        Effect::Show(_) | Effect::BringForward | Effect::Foreground => Reply::Ok(true),
        other => panic!("Kiln only comes forward: {}", other.name()),
    });
    let id = handoff::notice_id(&link).unwrap();
    assert_eq!(send_notice(&rig, handoff::SHOW_NOTICE, id), 1);
    rig.app(|app| {
        // Nothing moved behind the page.
        assert!(app.files.selection().is_empty());
        assert!(app.settings_ui.is_some());
    });
}

#[test]
fn the_portable_kiln_shows_the_balloon_and_its_click_opens_the_notice() {
    let rig = Rig::new();
    toast::shown::take();
    rig.app(|app| {
        app.notify_by = toast::Channel::Balloon;
        let about = NoticeAbout {
            row: Some(PathBuf::from(r"C:\in\a.png")),
            saved: true,
            reveal: None,
        };
        app.notify_about(
            "Kiln",
            "Saved a.jpg next to the original.",
            toast::saved(&["a.jpg".into()], &["in".into()], None),
            about,
        );
    });
    let (toasts, balloons) = toast::shown::take();
    assert!(toasts.is_empty());
    assert_eq!(
        balloons,
        [(
            "Kiln".to_owned(),
            "Saved a.jpg next to the original.".to_owned()
        )]
    );
    script::answer_with(|_, effect| match effect {
        Effect::ShowInFolder { .. } => Reply::Ok(true),
        other => panic!("only Explorer opens: {}", other.name()),
    });
    // The balloon's click, as the notification area sends it.
    // SAFETY: a plain send to this thread's own window, no pointers.
    unsafe {
        SendMessageW(
            rig.hwnd,
            WM_KILN_NOTIFY,
            Some(WPARAM(0)),
            Some(LPARAM(NIN_BALLOONUSERCLICK as isize)),
        )
    };
    assert_eq!(script::ran(), ["show in folder"]);
}

/// Finish a conversion of `source`, a new row, whose result `name` is in
/// the rig's holding folder, as a job does. Returns the row.
fn finish_held(rig: &Rig, source: &std::path::Path, name: &str) -> usize {
    rig.app(|app| {
        let e = model::entry_for(
            &app.kiln,
            &Availability::default(),
            source.to_path_buf(),
            1,
            None,
        );
        let i = app.files.len();
        app.files.insert(i, e);
        app.motion.rows_added(i, 1, Instant::now());
        let local = app.held.as_ref().unwrap().local().to_path_buf();
        let result = local.join(name);
        std::fs::write(&result, b"result").unwrap();
        let id = JobId(9_100 + i as u64);
        let action = Action::Convert("jpg".into());
        app.files.entries_mut()[i].job = JobState::Running {
            action: action.clone(),
            fraction: None,
            started: None,
        };
        app.motion.rows[i].job = Some(id);
        app.work.jobs.insert(
            id,
            JobInfo {
                paths: vec![source.to_path_buf()],
                action,
                batch: 0,
                drops: vec![],
                note: None,
                out_dir: Some(local),
            },
        );
        app.notify_by = toast::Channel::Balloon;
        toast::shown::take();
        app.on_job(JobEvent::Finished(id, Ok(vec![result])));
        i
    })
}

/// The texts of the balloons shown since the last job finished.
fn notices() -> Vec<String> {
    toast::shown::take().1.into_iter().map(|(_, t)| t).collect()
}

/// Wait until no export is moving a result.
fn saves_settle(rig: &Rig) -> bool {
    pump_until(rig, Duration::from_secs(10), |app| app.exports.is_empty())
}

/// A folder of originals and the chosen folder, in the rig's scratch.
fn originals_and_chosen(rig: &Rig) -> (PathBuf, PathBuf) {
    let originals = rig.dir.path().join("originals");
    let chosen = rig.dir.path().join("Converted");
    std::fs::create_dir_all(&originals).unwrap();
    std::fs::create_dir_all(&chosen).unwrap();
    (originals, chosen)
}

#[test]
fn a_result_goes_to_the_folder_chosen_in_settings() {
    let rig = Rig::new();
    let (originals, chosen) = originals_and_chosen(&rig);
    rig.app(|app| app.prefs.settings.save_to = Some(chosen.clone()));
    let i = finish_held(&rig, &originals.join("b.png"), "b.jpg");
    assert!(saves_settle(&rig));
    assert_eq!(std::fs::read(chosen.join("b.jpg")).unwrap(), b"result");
    assert!(
        !originals.join("b.jpg").exists(),
        "not next to the original"
    );
    rig.app(|app| {
        let (folder, files) = app.files.entries()[i].saved().unwrap();
        assert_eq!(folder, chosen.as_path());
        assert_eq!(files, [chosen.join("b.jpg")]);
    });
    assert_eq!(notices(), ["Saved b.jpg in Converted."]);
}

#[test]
fn a_chosen_folder_that_is_gone_keeps_the_result_in_kiln_and_says_so() {
    let rig = Rig::new();
    let (originals, _) = originals_and_chosen(&rig);
    let gone = rig.dir.path().join("Gone away");
    rig.app(|app| app.prefs.settings.save_to = Some(gone.clone()));
    let source = originals.join("d.png");
    let i = finish_held(&rig, &source, "d.jpg");
    assert!(saves_settle(&rig));
    // Nothing went anywhere else, and the folder wasn't made.
    assert!(!gone.exists());
    assert!(!originals.join("d.jpg").exists());
    rig.app(|app| {
        let e = &app.files.entries()[i];
        assert!(e.saved().is_none());
        let held = e.held().expect("still in Kiln").to_vec();
        assert!(held[0].exists());
        assert_eq!(
            app.notes.get(&source).map(|n| n.0.as_str()),
            Some(model::KEPT_LINE)
        );
    });
    assert_eq!(
        notices(),
        ["Couldn't save d.jpg. Kiln tries again the next time it starts."]
    );
}

/// Everyone denied adding files to a folder, until dropped.
struct Unwritable(PathBuf);

impl Unwritable {
    fn deny(dir: &std::path::Path) -> Option<Unwritable> {
        let ok = std::process::Command::new("icacls")
            .arg(dir)
            .args(["/deny", "*S-1-1-0:(WD,AD)"])
            .output()
            .is_ok_and(|o| o.status.success());
        ok.then(|| Unwritable(dir.to_path_buf()))
    }
}

impl Drop for Unwritable {
    fn drop(&mut self) {
        let _ = std::process::Command::new("icacls")
            .arg(&self.0)
            .args(["/remove:d", "*S-1-1-0"])
            .output();
    }
}

#[test]
fn a_chosen_folder_kiln_cant_write_in_keeps_the_result_in_kiln_and_says_so() {
    let rig = Rig::new();
    let (originals, chosen) = originals_and_chosen(&rig);
    let Some(_deny) = Unwritable::deny(&chosen) else {
        eprintln!("skipped: icacls couldn't deny writing");
        return;
    };
    rig.app(|app| app.prefs.settings.save_to = Some(chosen.clone()));
    let source = originals.join("e.png");
    let i = finish_held(&rig, &source, "e.jpg");
    assert!(saves_settle(&rig));
    assert!(!chosen.join("e.jpg").exists());
    assert!(!originals.join("e.jpg").exists());
    rig.app(|app| {
        let e = &app.files.entries()[i];
        assert!(e.saved().is_none());
        assert!(e.held().is_some_and(|h| h[0].exists()));
        assert_eq!(
            app.notes.get(&source).map(|n| n.0.as_str()),
            Some(model::KEPT_LINE)
        );
    });
}

#[test]
fn a_name_taken_in_the_chosen_folder_is_never_written_over() {
    let rig = Rig::new();
    let (originals, chosen) = originals_and_chosen(&rig);
    std::fs::write(chosen.join("f.jpg"), b"mine").unwrap();
    rig.app(|app| app.prefs.settings.save_to = Some(chosen.clone()));
    let i = finish_held(&rig, &originals.join("f.png"), "f.jpg");
    assert!(saves_settle(&rig));
    assert_eq!(std::fs::read(chosen.join("f.jpg")).unwrap(), b"mine");
    rig.app(|app| {
        let (folder, files) = app.files.entries()[i].saved().unwrap();
        assert_eq!(folder, chosen.as_path());
        assert_eq!(files.len(), 1);
        assert_ne!(files[0], chosen.join("f.jpg"));
        assert_eq!(files[0].parent(), Some(chosen.as_path()));
        assert_eq!(std::fs::read(&files[0]).unwrap(), b"result");
    });
}

#[test]
fn next_to_the_original_again_saves_next_to_the_original() {
    use settings_page::Set;
    let rig = Rig::new();
    let (originals, chosen) = originals_and_chosen(&rig);
    rig.app(|app| {
        app.prefs.settings.save_to = Some(chosen.clone());
        app.open_settings();
        app.activate(Control::SaveWhere, Set::Pick(0));
        assert_eq!(app.prefs.settings.save_to, None);
    });
    finish_held(&rig, &originals.join("g.png"), "g.jpg");
    assert!(saves_settle(&rig));
    assert_eq!(std::fs::read(originals.join("g.jpg")).unwrap(), b"result");
    assert!(!chosen.join("g.jpg").exists());
    assert_eq!(notices(), ["Saved g.jpg next to the original."]);
}

#[test]
fn a_wheel_open_on_a_video_found_silent_drops_its_sound_formats() {
    // a silent AVI's wheel opens before its facts are in, with MP3 on
    // it; once the facts say it has no sound, no sound format is left on
    // the ring or in More.
    let rig = empty_rig();
    let avi = rig.dir.path().join("s.avi");
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../kiln-media/tests/data/silent.avi"),
        &avi,
    )
    .unwrap();
    let sound = |w: &WheelUi| {
        let ring = w.ring();
        ring.items
            .iter()
            .chain(ring.flyout.iter().flat_map(|g| g.items.iter()))
            .filter(|i| i.kind == Some(kiln_core::Kind::Audio))
            .map(|i| i.label.clone())
            .collect::<Vec<_>>()
    };
    rig.app(|app| {
        app.engines.waited_out = true;
        list_at_top(app, &avi);
        app.files.select_only(&[0]);
        app.convert_selection(false);
        let w = app.wheel.as_ref().expect("the footer's list");
        assert!(sound(w).contains(&"MP3".to_owned()), "{:?}", sound(w));
        let facts = kiln_core::Facts {
            ext: "avi".into(),
            kind: kiln_core::Kind::Video,
            bytes: 1,
            picture: None,
            media: Some(kiln_core::MediaInfo {
                width: Some(64),
                height: Some(64),
                ..kiln_core::MediaInfo::default()
            }),
            pdf: None,
            bars: None,
            estimates: Default::default(),
            compressed: None,
            audio_problem: Some("This video has no sound to convert.".into()),
            silent: true,
        };
        app.on_inspected(Inspected {
            path: avi.clone(),
            facts,
        });
        let w = app
            .wheel
            .as_ref()
            .filter(|w| w.is_open())
            .expect("still open");
        assert_eq!(sound(w), Vec::<String>::new());
        let ring = w.ring();
        assert!(ring
            .items
            .iter()
            .chain(ring.flyout.iter().flat_map(|g| g.items.iter()))
            .any(|i| i.label == "MP4"));
    });
}

#[test]
fn get_ffmpeg_in_the_side_pane_of_a_file_with_some_formats_opens_its_settings_card() {
    // a WEBM whose sound formats Kiln makes itself, with 14 more
    // waiting on FFmpeg: the pane's Get FFmpeg does what D22's Get it
    // does, and opens Settings at FFmpeg's card.
    let rig = empty_rig();
    let webm = rig.dir.path().join("sample.webm");
    rig.app(|app| {
        app.engines.waited_out = true;
        list_at_top(app, &webm);
        let e = &mut app.files.entries_mut()[0];
        e.targets = vec!["MP3".into(), "M4A".into()];
        e.missing = Some(model::Missing::new("ffmpeg", 14));
        app.files.select_only(&[0]);
        app.refresh_panel();
        let pane = app.live_panel().expect("the pane shows").model.clone();
        assert_eq!(pane.label(InspectorControl::GetHelper), Some("Get FFmpeg"));
        app.panel_act(InspectorControl::GetHelper, false);
        let settings = app.settings_ui.as_ref().expect("Settings opened");
        assert!(settings.is_open(settings_page::Helper::Ffmpeg));
        assert!(app.effects.is_empty(), "nothing opened outside Kiln");
    });
}

#[test]
fn the_convert_wheel_of_a_jpg_cut_short_offers_no_tools() {
    // D46 capture finding: as its pane and menu, the footer's
    // Convert wheel on a JPG cut short offers no tool, Make smaller
    // included, on the ring or in More; its formats stay.
    let rig = empty_rig();
    let jpg = rig.dir.path().join("cut.jpg");
    std::fs::write(&jpg, [0xFF, 0xD8, 0xFF, 0xE0, 0, 16]).unwrap();
    rig.app(|app| {
        list_at_top(app, &jpg);
        app.files.entries_mut()[0].check = kiln_core::FileCheck::CutShort;
        app.files.entries_mut()[0].size = 3_000_000;
        app.files.select_only(&[0]);
        app.engines.waited_out = true;
        app.convert_selection(false);
        let w = app
            .wheel
            .as_ref()
            .filter(|w| w.is_open())
            .expect("the wheel opens");
        let ring = w.ring();
        let all: Vec<&wheel::Item> = ring
            .items
            .iter()
            .chain(ring.flyout.iter().flat_map(|g| g.items.iter()))
            .collect();
        assert!(
            !all.iter()
                .any(|i| matches!(i.action, wheel::Action::Tool(..))),
            "{:?}",
            all.iter().map(|i| &i.label).collect::<Vec<_>>()
        );
        assert!(all.iter().any(|i| i.label == "PNG"));
    });
}

#[test]
fn ctrl_enter_on_a_jpg_cut_short_opens_no_tools_and_says_why() {
    // as its menu, the keyboard opens no Tools wheel on a
    // JPG cut short; it says why instead.
    let rig = empty_rig();
    let jpg = rig.dir.path().join("cut.jpg");
    std::fs::write(&jpg, [0xFF, 0xD8, 0xFF, 0xE0, 0, 16]).unwrap();
    rig.app(|app| {
        list_at_top(app, &jpg);
        app.files.entries_mut()[0].check = kiln_core::FileCheck::CutShort;
        app.files.entries_mut()[0].size = 3_000_000;
        app.files.select_only(&[0]);
        app.engines.waited_out = true;
        app.list_key(VK_RETURN, false, true, false);
        assert!(
            !app.wheel.as_ref().is_some_and(|w| w.is_open()),
            "no wheel opens"
        );
        assert_eq!(app.status.alert.as_deref(), Some(CUT_SHORT_NO_TOOLS));
    });
}

#[test]
fn the_row_menu_of_a_jpg_cut_short_offers_no_tools() {
    // a JPG cut short can't be made smaller or have a tool run
    // on it, so its menu offers neither.
    let rig = empty_rig();
    let jpg = rig.dir.path().join("cut.jpg");
    std::fs::write(&jpg, [0xFF, 0xD8, 0xFF, 0xE0, 0, 16]).unwrap();
    rig.app(|app| {
        list_at_top(app, &jpg);
        app.files.entries_mut()[0].check = kiln_core::FileCheck::CutShort;
        app.files.entries_mut()[0].size = 3_000_000;
        app.files.select_only(&[0]);
        app.engines.waited_out = true;
        app.context_menu(None);
        let m = app.row_menu.as_ref().expect("the menu is up");
        assert!(!m.items.contains(&MenuItem::Smaller), "{:?}", m.items);
        assert!(
            !m.items.contains(&MenuItem::Wheel(WheelKind::Tools)),
            "{:?}",
            m.items
        );
    });
}

/// Resize the rig's window so its client area is `w` x `h` physical
/// pixels (96 dpi, so DIPs), which sends it WM_SIZE.
#[cfg(debug_assertions)]
fn set_client_size(hwnd: HWND, w: i32, h: i32) {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{
        AdjustWindowRectEx, SetWindowPos, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER,
        WS_OVERLAPPEDWINDOW,
    };
    let mut rc = RECT {
        left: 0,
        top: 0,
        right: w,
        bottom: h,
    };
    // SAFETY: plain calls on this thread's own window and a local RECT.
    unsafe {
        let _ = AdjustWindowRectEx(
            &mut rc,
            WS_OVERLAPPEDWINDOW,
            false,
            WINDOW_EX_STYLE::default(),
        );
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            rc.right - rc.left,
            rc.bottom - rc.top,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// Animation effects off for this test's thread only, as they were after it
/// (the global switch is shared by tests running in parallel).
#[cfg(debug_assertions)]
fn animations_off() -> theme::fluent::motion::AnimationsFor {
    theme::fluent::motion::AnimationsFor::new(false)
}

#[cfg(debug_assertions)]
#[test]
fn the_pane_is_there_in_the_frame_drawn_as_the_window_is_widened() {
    // a row selected in a window too narrow for the pane, then the
    // window widened to 1100 DIPs: the frame drawn for that resize has the
    // pane. With no slide (Animation effects off) the fold is where it is
    // going at once, and nothing else asks for a frame afterwards.
    let _off = animations_off();
    let rig = Rig::new();
    let hwnd = rig.hwnd;
    show_off_screen(hwnd);
    set_client_size(hwnd, 700, 600);
    rig.app(|app| app.files.select_only(&[0]));
    rig.send(WM_PAINT, 0);
    rig.app(|app| {
        assert_eq!(app.renderer.size_dips().0, 700.0);
        assert!(app.layout().inspector.is_none(), "folded at 700");
        assert_eq!(app.drawn, (0.0, false), "drawn without it");
    });
    set_client_size(hwnd, 1100, 720);
    rig.app(|app| {
        assert_eq!(app.renderer.size_dips().0, 1100.0);
        assert!(app.layout().inspector.is_some(), "the layout has the pane");
        assert_eq!(
            app.drawn,
            (1.0, true),
            "the frame drawn for the resize has it"
        );
    });
}

#[cfg(debug_assertions)]
#[test]
fn with_animations_on_the_pane_slides_in_from_the_frame_drawn_as_the_window_is_widened() {
    // As in the real capture: Animation effects on. The resize starts
    // the slide (the frame drawn for it is where the slide begins: the
    // pane not yet in, or already in), the fold is going to 1, and the
    // frames after it show the pane arriving and then there.
    let _on = theme::fluent::motion::AnimationsFor::new(true);
    let rig = Rig::new();
    let hwnd = rig.hwnd;
    show_off_screen(hwnd);
    set_client_size(hwnd, 700, 600);
    rig.app(|app| app.files.select_only(&[0]));
    // The pane that was there folds away at 700 over a slide of its own.
    std::thread::sleep(Duration::from_millis(u64::from(anim::FOLD_MS) + 60));
    rig.send(WM_PAINT, 0);
    rig.app(|app| assert_eq!(app.drawn, (0.0, false), "drawn without it"));
    set_client_size(hwnd, 1100, 720);
    rig.app(|app| {
        assert_eq!(app.renderer.size_dips().0, 1100.0);
        assert_eq!(app.motion.fold_target(), 1.0);
        assert!(
            app.motion.folding(Instant::now()),
            "the slide is running, so frames keep being asked for"
        );
        // The first frame at the new size: where the slide begins.
        let (fold, pane) = app.drawn;
        assert!((0.0..1.0).contains(&fold), "{fold}");
        assert_eq!(pane, fold > 0.0, "the pane is drawn as soon as it shows");
    });
    // Half way through, a frame has the pane partly in.
    std::thread::sleep(Duration::from_millis(u64::from(anim::FOLD_MS) / 2));
    rig.send(WM_PAINT, 0);
    rig.app(|app| {
        let (fold, pane) = app.drawn;
        assert!(fold > 0.0 && pane, "arriving: {:?}", app.drawn);
    });
    // After it, the pane is there.
    std::thread::sleep(Duration::from_millis(u64::from(anim::FOLD_MS)));
    rig.send(WM_PAINT, 0);
    rig.app(|app| assert_eq!(app.drawn, (1.0, true), "the pane has slid in"));
}

/// Row 0 running a conversion to JPG, as a job leaves it.
fn run_row(app: &mut App, i: usize) {
    app.files.entries_mut()[i].job = JobState::Running {
        action: Action::Convert("jpg".into()),
        fraction: Some(0.25),
        started: None,
    };
}

#[test]
fn the_footer_button_stays_convert_and_is_disabled_while_the_selection_runs() {
    // the accent never means stop. The row has the Cancel.
    let rig = Rig::new();
    rig.app(|app| {
        run_row(app, 0);
        app.files.select_only(&[0]);
        assert_eq!(app.convert_label(), "Convert");
        let why = app.convert_block().expect("disabled while it runs");
        assert!(why.contains("still converting"), "{why}");
        // Its tooltip and its UI Automation help say why.
        assert_eq!(app.tip_text(TIP_CONVERT), why);
        // Pressing it stops nothing and opens nothing.
        app.convert_selection(false);
        assert!(matches!(
            app.files.entries()[0].job,
            JobState::Running { .. }
        ));
        assert!(app.wheel.is_none());
        // Not selected, the row's own Cancel is the way, and Convert reads
        // as for any selection.
        app.files.select_only(&[]);
        assert_eq!(
            app.convert_block().as_deref(),
            Some("Select a file to convert.")
        );
    });
}

#[test]
fn convert_is_disabled_in_its_own_words_while_a_result_is_copied_to_another_drive() {
    // not "still converting", which it isn't.
    let rig = Rig::new();
    rig.app(|app| {
        let out = rig.dir.path().join("held.jpg");
        app.files.entries_mut()[0].job = JobState::Done {
            action: Action::Convert("jpg".into()),
            outputs: vec![out.clone()],
            saved: None,
        };
        app.copying.insert(out, 0.5);
        app.files.select_only(&[0]);
        assert!(app.row_copying(0).is_some());
        let why = app.convert_block().expect("disabled while it copies");
        assert_eq!(why, model::COPYING_BLOCK);
        assert!(why.contains("copied to another drive") && !why.contains("converting"));
        assert_eq!(app.tip_text(TIP_CONVERT), why);
    });
}

#[test]
fn space_cancels_one_running_row_and_the_hint_says_so_only_then() {
    let rig = Rig::new();
    let (a, _b) = two_listed_a_selected(&rig);
    rig.app(|app| {
        assert!(!app.space_cancels(), "nothing running");
        let i = app.files.index_of(&a).unwrap();
        run_row(app, i);
        app.files.select_only(&[i]);
        assert!(app.space_cancels());
        // Two rows selected: Space does nothing, so nothing says it does.
        app.files.select_only(&[0, 1]);
        assert!(!app.space_cancels());
        app.files.select_only(&[]);
        assert!(!app.space_cancels());
    });
}

/// A file of `name` in the rig's folder, with `bytes` in it.
fn file_of(rig: &Rig, name: &str, bytes: &[u8]) -> PathBuf {
    let path = rig.dir.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn the_footer_button_is_disabled_with_the_reason_while_the_selection_shares_no_format() {
    // a photo and a song share nothing to convert to.
    let rig = empty_rig();
    let png = a_png(&rig, "a.png");
    let mp3 = file_of(&rig, "b.mp3", b"ID3\x03\x00rest");
    rig.app(|app| {
        app.engines.waited_out = true;
        list_at_top(app, &mp3);
        list_at_top(app, &png);
        app.files.select_only(&[0, 1]);
        let why = app.convert_block().expect("disabled");
        assert!(
            why.starts_with("These files have no format in common: "),
            "{why}"
        );
        assert_eq!(app.tip_text(TIP_CONVERT), why);
        // The label and the button are Convert's, not Cancel's.
        assert_eq!(app.convert_label(), "Convert 2 files");
        // Pressing it says why and opens no wheel.
        app.convert_selection(false);
        assert!(app.wheel.is_none());
        // One of them alone can be converted.
        app.files.select_only(&[1]);
        assert_eq!(app.convert_block(), None);
        // Two photos share formats.
        let b = a_png(&rig, "c.png");
        list_at_top(app, &b);
        app.files.select_only(&[0, 1]);
        assert_eq!(app.convert_block(), None);
    });
}

/// A mouse message for the rig's window at `(x, y)` physical pixels (DIPs
/// at the rig's 96 dpi), sent to Kiln's own window as a click sends it.
#[cfg(debug_assertions)]
fn mouse_at(rig: &Rig, msg: u32, x: f32, y: f32) {
    let lparam = ((y as i32 as isize) << 16) | (x as i32 as isize & 0xFFFF);
    // SAFETY: a plain send to this thread's own window.
    unsafe {
        SendMessageW(rig.hwnd, msg, Some(WPARAM(0)), Some(LPARAM(lparam)));
    }
}

#[cfg(debug_assertions)]
#[test]
fn a_click_selects_a_row_without_its_focus_ring_and_a_key_brings_it() {
    // Captures show a ring round the selected row. A selection
    // made by a click, sent as the window's own messages, must show only the
    // fill and the pill; the ring is the keyboard's (and a UI Automation
    // client's, which moves focus as a key does).
    use windows::Win32::UI::WindowsAndMessaging::{WM_LBUTTONDOWN, WM_LBUTTONUP};
    let rig = empty_rig();
    two_listed_a_selected(&rig);
    let hwnd = rig.hwnd;
    show_off_screen(hwnd);
    set_client_size(hwnd, 1100, 720);
    rig.send(WM_PAINT, 0);
    let (x, y) = rig.app(|app| {
        app.files.select_only(&[]);
        let r = app.layout().rows[1];
        (r.x + 300.0, r.y + r.h / 2.0)
    });
    mouse_at(&rig, WM_LBUTTONDOWN, x, y);
    mouse_at(&rig, WM_LBUTTONUP, x, y);
    rig.app(|app| {
        assert_eq!(app.files.selection(), [1], "the click selected the row");
        assert_eq!(app.files.visible_focus(), None, "no ring after a click");
    });
    // A key puts the keyboard on the row, and with it the ring.
    press(&rig, VK_UP);
    rig.app(|app| {
        assert!(app.files.visible_focus().is_some(), "a key shows the ring");
    });
    // And a click hides it again.
    mouse_at(&rig, WM_LBUTTONDOWN, x, y);
    mouse_at(&rig, WM_LBUTTONUP, x, y);
    rig.app(|app| assert_eq!(app.files.visible_focus(), None));
}

/// A PNG listed and selected as row 1 with JPG and WEBP offered, WEBP the
/// suggestion, and the side pane built for it.
fn png_with_a_suggestion(app: &mut App, png: &PathBuf) {
    list(app, &[png]);
    let e = &mut app.files.entries_mut()[1];
    e.targets = vec!["JPG".into(), "WEBP".into()];
    e.suggestion = Some(kiln_core::Suggestion {
        action: kiln_core::Suggested::Convert("webp".into()),
        reason: String::new(),
        estimate: None,
    });
    app.files.select_only(&[1]);
    app.refresh_panel();
}

#[test]
fn with_the_pane_showing_the_footers_convert_converts_to_the_suggested_format_and_says_so() {
    // no wheel; the accent button is "Convert to WEBP" and does it.
    let rig = Rig::new();
    let png = a_png(&rig, "b.png");
    rig.app(|app| {
        png_with_a_suggestion(app, &png);
        assert!(app.live_panel().is_some(), "the pane shows");
        assert_eq!(app.convert_label(), "Convert to WEBP");
        app.convert_selection(false);
        assert!(app.wheel.is_none(), "no wheel from the footer");
        assert_eq!(asked(app), std::slice::from_ref(&png));
        let action = app.work.jobs.values().next().unwrap().action.clone();
        assert_eq!(action, Action::Convert("webp".into()));
        app.work.runner.cancel_all();
    });
}

#[test]
fn with_nothing_suggested_the_footers_convert_takes_the_panes_first_format() {
    let rig = Rig::new();
    let png = a_png(&rig, "b.png");
    rig.app(|app| {
        png_with_a_suggestion(app, &png);
        app.files.entries_mut()[1].suggestion = None;
        app.panel_stale();
        app.refresh_panel();
        let first = app.live_panel().unwrap().model.targets()[0].ext.clone();
        assert_eq!(
            app.convert_label(),
            format!("Convert to {}", first.to_uppercase())
        );
        app.convert_selection(false);
        let action = app.work.jobs.values().next().unwrap().action.clone();
        assert_eq!(action, Action::Convert(first));
        app.work.runner.cancel_all();
    });
}

#[test]
fn enter_on_the_list_does_what_the_footers_convert_does() {
    let rig = Rig::new();
    let png = a_png(&rig, "b.png");
    rig.app(|app| {
        png_with_a_suggestion(app, &png);
        app.list_key(VK_RETURN, false, false, false);
        assert!(app.wheel.is_none());
        let action = app.work.jobs.values().next().unwrap().action.clone();
        assert_eq!(action, Action::Convert("webp".into()));
        app.work.runner.cancel_all();
    });
}

#[cfg(debug_assertions)]
#[test]
fn with_the_pane_folded_convert_opens_the_list_beside_the_button_and_a_pick_converts() {
    // below 760 DIPs there is no pane to take the choice.
    let _off = animations_off();
    let rig = Rig::new();
    let png = a_png(&rig, "b.png");
    let hwnd = rig.hwnd;
    show_off_screen(hwnd);
    set_client_size(hwnd, 700, 600);
    rig.app(|app| {
        app.engines.waited_out = true;
        png_with_a_suggestion(app, &png);
    });
    rig.send(WM_PAINT, 0);
    rig.app(|app| {
        assert!(app.layout().inspector.is_none(), "folded at 700");
        assert!(app.live_panel().is_none(), "no pane carries the actions");
        assert_eq!(app.convert_label(), "Convert");
        app.convert_selection(false);
        let w = app.wheel.as_ref().expect("the list");
        assert!(w.list_only(), "no ring");
        assert!(w.flyout_open());
        // It lies above the footer and ends at the button's right edge.
        let b = app.convert_rect().unwrap();
        let rows = w.ring().flyout_rows(None);
        let place = crate::wheel::flyout_place(w.ring(), &rows, app.layout().list, 0.0);
        assert!(
            place.rect.bottom() <= b.y,
            "above the footer: {place:?} {b:?}"
        );
        assert!(place.rect.right() <= b.right() + 0.5);
        // A pick on its first format converts the file to it.
        let first = rows
            .iter()
            .find_map(|r| match r {
                crate::wheel::FlyoutRow::Item(s, i) if matches!(i.action, Action::Convert(_)) => {
                    Some((*s, i.action.clone()))
                }
                _ => None,
            })
            .unwrap();
        drop(rows);
        let want = first.1;
        app.choose(first.0);
        app.tick();
        assert_eq!(asked(app), std::slice::from_ref(&png));
        assert_eq!(app.work.jobs.values().next().unwrap().action, want);
        app.work.runner.cancel_all();
    });
}

#[test]
fn a_result_in_the_footer_goes_when_the_selection_changes() {
    // "Converted 1 file to MOV." stays while a different file is
    // selected until now; it goes at the next selection change.
    let rig = Rig::new();
    let png = a_png(&rig, "b.png");
    rig.app(|app| {
        list(app, &[&png]);
        app.files.select_only(&[0]);
        app.say("Converted 1 file to MOV.");
        app.expire_status();
        assert_eq!(
            app.status.text, "Converted 1 file to MOV.",
            "stays while drawn"
        );
        app.files.select_only(&[1]);
        app.expire_status();
        assert_eq!(
            app.status.text,
            model::status_line(app.files.len(), 0, 0, 0)
        );
        assert!(!app.alerting());
    });
}

#[test]
fn an_alert_in_the_footer_goes_after_five_seconds_and_the_hint_returns() {
    // With the pane folded where the hint shows.
    let rig = Rig::new();
    rig.app(|app| {
        app.files.select_only(&[0]);
        app.alert("Select a file to convert.".to_owned());
        app.expire_status();
        assert!(app.alerting(), "just said");
        // Not yet: four seconds in.
        app.status.seen = Some((
            Instant::now() - Duration::from_secs(4),
            app.files.selection(),
        ));
        app.expire_status();
        assert!(app.alerting());
        // Five seconds in.
        app.status.seen = Some((
            Instant::now() - Duration::from_secs(5),
            app.files.selection(),
        ));
        app.expire_status();
        assert!(!app.alerting());
        assert_eq!(
            app.status.text,
            model::status_line(app.files.len(), 0, 0, 0)
        );
        // The footer is back to its count; folded, the hint follows.
        assert!(ways_in::idle_hint(
            app.files.len(),
            false,
            &app.status.text,
            &app.status.text,
            None,
            false
        )
        .is_some());
    });
}

#[test]
fn the_work_under_way_and_an_offer_to_undo_do_not_expire() {
    let rig = Rig::new();
    rig.app(|app| {
        app.say("Converting 3 files…");
        app.status.working = Some(app.status.text.clone());
        app.status.seen = Some((Instant::now() - Duration::from_secs(60), vec![]));
        app.expire_status();
        assert_eq!(app.status.text, "Converting 3 files…");
    });
}

#[cfg(debug_assertions)]
#[test]
fn the_panes_more_opens_the_list_to_the_left_of_the_pane_and_never_over_it() {
    // More formats and tools is the same list as the footer's, anchored
    // so it does not cover the pane.
    let _off = animations_off();
    let rig = Rig::new();
    let png = a_png(&rig, "b.png");
    show_off_screen(rig.hwnd);
    set_client_size(rig.hwnd, 1100, 720);
    rig.app(|app| {
        app.engines.waited_out = true;
        png_with_a_suggestion(app, &png);
        // More than the pane lists, so it has More formats and tools.
        app.files.entries_mut()[1].targets =
            ["JPG", "WEBP", "BMP", "GIF", "TIFF", "ICO", "TGA", "AVIF"]
                .map(String::from)
                .to_vec();
        app.panel_stale();
        app.refresh_panel();
        let pane = app.layout().inspector.expect("the pane shows");
        let more = app
            .live_panel()
            .and_then(|p| p.rect_of(InspectorControl::More))
            .expect("More formats and tools");
        app.panel_act(InspectorControl::More, false);
        let w = app.wheel.as_ref().expect("the list");
        assert!(w.list_only() && w.flyout_open());
        let rows = w.ring().flyout_rows(None);
        let place = crate::wheel::flyout_place(w.ring(), &rows, app.layout().list, 0.0);
        assert!(place.rect.right() <= pane.x, "{:?} {:?}", place.rect, pane);
        // Level with the link that asked for it, as far as the list lets it.
        assert!(place.rect.y <= more.y + more.h && place.rect.bottom() >= more.y);
        // No ring: a press on the pane, off the list, closes it.
        assert_eq!(w.ring().segments(), 0);
    });
}

#[test]
fn convert_to_in_the_row_menu_opens_the_list_at_the_row() {
    // the menu keeps Convert to... and Make smaller; the first opens
    // the list, not the wheel.
    let rig = Rig::new();
    let png = a_png(&rig, "b.png");
    rig.app(|app| {
        app.engines.waited_out = true;
        png_with_a_suggestion(app, &png);
        app.context_menu(None);
        let m = app.row_menu.as_ref().expect("the menu is up");
        let convert = m
            .items
            .iter()
            .position(|i| *i == MenuItem::Wheel(WheelKind::Convert))
            .expect("Convert to...");
        assert!(m.items.contains(&MenuItem::Smaller), "{:?}", m.items);
        app.menu_pick(convert);
        let w = app.wheel.as_ref().expect("the list opens");
        assert!(w.list_only() && w.flyout_open());
        assert_eq!(w.kind(), WheelKind::Convert);
    });
}

#[test]
fn a_press_off_the_list_alone_closes_it_and_escape_does_too() {
    let rig = Rig::new();
    let png = a_png(&rig, "b.png");
    rig.app(|app| {
        app.engines.waited_out = true;
        png_with_a_suggestion(app, &png);
        app.files.entries_mut()[1].targets =
            ["JPG", "WEBP", "BMP", "GIF", "TIFF", "ICO", "TGA", "AVIF"]
                .map(String::from)
                .to_vec();
        app.panel_stale();
        app.refresh_panel();
        app.panel_act(InspectorControl::More, true);
        assert!(app.wheel.as_ref().unwrap().is_open());
        // From the keyboard it starts on a line, and Esc closes it.
        assert!(app.wheel.as_ref().unwrap().lit().is_some());
        app.drive_wheel(|w, cx| w.key(cx, VK_ESCAPE, Mods::default()));
        assert!(!app.wheel.as_ref().unwrap().is_open());
    });
}

#[cfg(debug_assertions)]
#[test]
fn a_waiting_files_folder_is_a_link_that_opens_it_with_its_whole_path_as_the_tip() {
    let _off = animations_off();
    let rig = Rig::new();
    let png = a_png(&rig, "b.png");
    show_off_screen(rig.hwnd);
    set_client_size(rig.hwnd, 1100, 720);
    let wanted = png.parent().unwrap().to_path_buf();
    rig.app(|app| {
        app.engines.waited_out = true;
        png_with_a_suggestion(app, &png);
        app.panel_stale();
        app.refresh_panel();
        let link = app.folder_link().expect("the Folder link is placed");
        // The whole path is its tip, over the link and nowhere else.
        let tools = app.tip_tools();
        assert!(tools.contains(&(TIP_FOLDER, link)), "{tools:?}");
        assert_eq!(app.tip_text(TIP_FOLDER), wanted.display().to_string());
        // Under the pointer it is the link, and a click on it is its action.
        let at = (link.x + link.w / 2.0, link.y + link.h / 2.0);
        let panel = app.live_panel().expect("the pane shows");
        assert_eq!(panel.control_at(at.0, at.1), Some(InspectorControl::Folder));
    });
    let seen = Rc::new(RefCell::new(None));
    let answer = seen.clone();
    script::answer_with(move |_, effect| match effect {
        Effect::ShowInFolder { folder, files, .. } => {
            *answer.borrow_mut() = Some((folder.clone(), files.clone()));
            Reply::Ok(true)
        }
        other => panic!("only Explorer opens: {}", other.name()),
    });
    rig.app(|app| app.panel_act(InspectorControl::Folder, false));
    run_effects(rig.hwnd, rig.cell());
    assert_eq!(*seen.borrow(), Some((wanted, vec![png])));
}

#[cfg(debug_assertions)]
#[test]
fn a_pane_title_cut_to_two_lines_ends_in_an_ellipsis_and_has_the_whole_name_as_its_tip() {
    let _off = animations_off();
    let rig = Rig::new();
    let long = format!("{}.png", "a-long-file-name-".repeat(12));
    let short = a_png(&rig, "b.png");
    let long_png = a_png(&rig, &long);
    show_off_screen(rig.hwnd);
    set_client_size(rig.hwnd, 1100, 720);
    rig.app(|app| {
        app.engines.waited_out = true;
        // A name that fits has no tip of its own on the title.
        png_with_a_suggestion(app, &short);
        assert!(!app.tip_tools().iter().any(|(id, _)| *id == TIP_TITLE));
        // The long name is cut, and the cut ends in an ellipsis.
        list(app, &[&long_png]);
        app.files.select_only(&[1]);
        app.panel_stale();
        app.refresh_panel();
        let panel = app.live_panel().expect("the pane shows");
        let title = panel.layout.title;
        let shown = app.renderer.inspector_title(&long, title.w);
        assert!(shown.ends_with('\u{2026}'), "{shown}");
        // The whole name is the tip over the title, and still the UIA name.
        let tools = app.tip_tools();
        assert!(tools.contains(&(TIP_TITLE, title)), "{tools:?}");
        assert_eq!(app.tip_text(TIP_TITLE), long);
        let tree_name = panel_file(&panel.model, app.files.entries());
        assert_eq!(tree_name, long);
    });
}

// The options row of More's list (owner report: "Options for this
// conversion…" did nothing).

/// The ways More's list is reached.
#[derive(Clone, Copy, Debug)]
enum WayIn {
    /// The wheel's ring, More's segment and its list beside the ring.
    Ring,
    /// The side pane's "More formats and tools".
    PaneMore,
    /// The footer Convert's list.
    FooterConvert,
}

const WAYS_IN: [WayIn; 3] = [WayIn::Ring, WayIn::PaneMore, WayIn::FooterConvert];

/// Open More's list for `png` by `way`, from the keyboard or not.
fn open_more_by(app: &mut App, way: WayIn, png: &PathBuf, keyboard: bool) {
    app.engines.waited_out = true;
    png_with_a_suggestion(app, png);
    match way {
        WayIn::Ring => {
            app.open_wheel(
                WheelKind::Convert,
                vec![png.clone()],
                (500.0, 350.0),
                keyboard,
                wheel::Pour::OverList,
            );
            let more = Spot::Inner(app.wheel.as_ref().unwrap().ring().items.len());
            app.drive_wheel(|w, cx| w.choose(more, cx.now, keyboard));
        }
        WayIn::PaneMore => {
            app.files.entries_mut()[1].targets =
                ["JPG", "WEBP", "BMP", "GIF", "TIFF", "ICO", "TGA", "AVIF"]
                    .map(String::from)
                    .to_vec();
            app.panel_stale();
            app.refresh_panel();
            app.panel_act(InspectorControl::More, keyboard);
        }
        WayIn::FooterConvert => {
            let l = app.layout();
            app.open_choice(
                WheelKind::Convert,
                vec![png.clone()],
                (l.list.right(), l.list.bottom()),
                keyboard,
            );
        }
    }
    assert!(
        app.wheel.as_ref().is_some_and(|w| w.flyout_open()),
        "{way:?}: More's list is open"
    );
}

/// The formats More's list holds, as spots, with whether each has
/// something to set for `png`.
fn list_formats(app: &App, png: &PathBuf) -> Vec<(Spot, String, bool)> {
    let w = app.wheel.as_ref().expect("the wheel");
    w.ring()
        .more_items()
        .enumerate()
        .filter_map(|(j, item)| match &item.action {
            wheel::Action::Convert(ext) => {
                let settable = app
                    .sheet_formats(std::slice::from_ref(png), ext)
                    .is_some_and(|(_, _, knobs)| !knobs.is_empty());
                Some((Spot::Flyout(j), ext.clone(), settable))
            }
            _ => None,
        })
        .collect()
}

/// The middle of the tree's node for `spot` on the open wheel.
fn middle_of(app: &mut App, spot: Spot) -> Option<(f32, f32)> {
    let tree = app.a11y_tree(&[]);
    let r = tree.find(&a11y::Id::WheelItem(spot))?.rect;
    Some((r.x + r.w / 2.0, r.y + r.h / 2.0))
}

fn point_at(app: &mut App, (x, y): (f32, f32)) {
    app.dispatch(Input::Move { x, y, keys: 0 });
}

fn press_at(app: &mut App, (x, y): (f32, f32)) {
    app.dispatch(Input::Down { x, y, keys: 0 });
}

fn key_down(app: &mut App, key: VIRTUAL_KEY) {
    app.dispatch(Input::Key {
        key,
        shift: false,
        ctrl: false,
        alt: false,
    });
}

/// What was lit before the footer row was chosen, and how it was chosen.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Lit {
    /// Nothing lit, then a press on the footer.
    Nothing,
    /// A format with settings, pointed at; then a press on the footer.
    PointedWithSettings,
    /// A format with no settings, pointed at; then a press on the footer.
    PointedWithout,
    /// A format with settings, by the arrow keys; then Enter on the footer.
    KeyedWithSettings,
    /// A format with no settings, by the arrow keys; then Enter.
    KeyedWithout,
    /// Nothing lit, then Invoke on the footer through UI Automation.
    InvokedNothing,
    /// A format with settings, pointed at; then Invoke on the footer.
    InvokedWithSettings,
}

/// Try one case: `Ok(opened)`, or `Err` when the png offers no format of
/// the kind asked.
fn footer_case(
    rig: &Rig,
    way: WayIn,
    lit: Lit,
    png: &PathBuf,
) -> std::result::Result<bool, String> {
    let keyboard = matches!(lit, Lit::KeyedWithSettings | Lit::KeyedWithout);
    rig.app(|app| {
        open_more_by(app, way, png, keyboard);
        let formats = list_formats(app, png);
        let wants = matches!(
            lit,
            Lit::PointedWithSettings | Lit::KeyedWithSettings | Lit::InvokedWithSettings
        );
        let target = formats.iter().find(|(_, _, s)| *s == wants);
        match lit {
            Lit::Nothing | Lit::InvokedNothing => {}
            Lit::PointedWithSettings | Lit::PointedWithout | Lit::InvokedWithSettings => {
                let Some((spot, ..)) = target else {
                    return Err(format!("no format with settings={wants}: {formats:?}"));
                };
                let at = middle_of(app, *spot).ok_or("the line has no node")?;
                point_at(app, at);
            }
            Lit::KeyedWithSettings | Lit::KeyedWithout => {
                let Some((spot, ..)) = target else {
                    return Err(format!("no format with settings={wants}: {formats:?}"));
                };
                for _ in 0..60 {
                    if app.wheel.as_ref().unwrap().lit() == Some(*spot) {
                        break;
                    }
                    key_down(app, VK_DOWN);
                }
                assert_eq!(app.wheel.as_ref().unwrap().lit(), Some(*spot));
            }
        }
        match lit {
            Lit::InvokedNothing | Lit::InvokedWithSettings => {
                app.a11y_do(a11y::Request {
                    id: a11y::Id::WheelItem(Spot::Options),
                    verb: a11y::Verb::Invoke,
                });
            }
            Lit::KeyedWithSettings | Lit::KeyedWithout => {
                for _ in 0..60 {
                    if app.wheel.as_ref().unwrap().lit() == Some(Spot::Options) {
                        break;
                    }
                    key_down(app, VK_DOWN);
                }
                key_down(app, VK_RETURN);
            }
            _ => {
                let at = middle_of(app, Spot::Options).ok_or("the footer has no node")?;
                press_at(app, at);
            }
        }
        app.tick();
        Ok(app.sheet.is_some())
    })
}

#[test]
fn the_options_row_opens_the_dialog_every_way_in_and_every_way_of_choosing_it() {
    let mut table = vec![];
    let mut wrong = vec![];
    for way in WAYS_IN {
        for lit in [
            Lit::Nothing,
            Lit::PointedWithSettings,
            Lit::PointedWithout,
            Lit::KeyedWithSettings,
            Lit::KeyedWithout,
            Lit::InvokedNothing,
            Lit::InvokedWithSettings,
        ] {
            let rig = Rig::new();
            let png = a_png(&rig, "b.png");
            let got = footer_case(&rig, way, lit, &png);
            table.push(format!("{way:?} / {lit:?}: {got:?}"));
            if got == Ok(false) {
                wrong.push(format!("{way:?} / {lit:?}"));
            }
        }
    }
    eprintln!("{}", table.join("\n"));
    assert!(wrong.is_empty(), "opened nothing: {wrong:?}");
}

/// The footer row's name in the tree, if it is shown, and whether it is
/// usable.
fn footer_row(app: &mut App) -> Option<(String, bool)> {
    let tree = app.a11y_tree(&[]);
    let n = tree.find(&a11y::Id::WheelItem(Spot::Options))?;
    Some((n.name.clone(), !n.states.disabled))
}

#[test]
fn the_options_row_names_the_format_it_is_for_and_falls_back_to_the_suggested_one() {
    for way in WAYS_IN {
        let rig = Rig::new();
        let png = a_png(&rig, "b.png");
        rig.app(|app| {
            open_more_by(app, way, &png, false);
            // Nothing lit and nothing suggested: the first format in the
            // list that has settings.
            let first = app.wheel.as_ref().unwrap().ring().settable[0].to_ascii_uppercase();
            assert_eq!(
                footer_row(app),
                Some((format!("Options for {first}\u{2026}"), true)),
                "{way:?}"
            );
            // Once a suggestion arrives: the suggestion.
            app.drive_wheel(|w, _| {
                w.suggest(kiln_core::Suggestion {
                    action: kiln_core::Suggested::Convert("webp".into()),
                    reason: String::new(),
                    estimate: None,
                })
            });
            assert_eq!(
                footer_row(app),
                Some(("Options for WEBP\u{2026}".to_owned(), true)),
                "{way:?}"
            );
            // A lit format with settings: that one.
            let formats = list_formats(app, &png);
            let (spot, ext, _) = formats.iter().find(|(_, _, s)| *s).unwrap().clone();
            let at = middle_of(app, spot).unwrap();
            point_at(app, at);
            assert_eq!(
                footer_row(app),
                Some((
                    format!("Options for {}\u{2026}", ext.to_ascii_uppercase()),
                    true
                )),
                "{way:?}"
            );
        });
    }
}

#[test]
fn a_lit_format_with_nothing_to_set_leaves_the_options_row_on_the_one_it_falls_back_to() {
    for way in [WayIn::PaneMore, WayIn::FooterConvert] {
        let rig = Rig::new();
        let png = a_png(&rig, "b.png");
        rig.app(|app| {
            open_more_by(app, way, &png, false);
            let formats = list_formats(app, &png);
            let (spot, ..) = formats.iter().find(|(_, _, s)| !*s).expect("one without");
            let at = middle_of(app, *spot).unwrap();
            point_at(app, at);
            let first = app.wheel.as_ref().unwrap().ring().settable[0].to_ascii_uppercase();
            assert_eq!(
                footer_row(app),
                Some((format!("Options for {first}\u{2026}"), true)),
                "{way:?}"
            );
        });
    }
}

#[test]
fn with_no_format_to_set_in_the_list_there_is_no_options_row() {
    // Markdown to anything has nothing to set (UX-08).
    let rig = Rig::new();
    let md = rig.dir.path().join("notes.md");
    std::fs::write(&md, b"# Notes").unwrap();
    rig.app(|app| {
        app.engines.waited_out = true;
        list(app, &[&md]);
        app.files.select_only(&[1]);
        let l = app.layout();
        app.open_choice(
            WheelKind::Convert,
            vec![md.clone()],
            (l.list.right(), l.list.bottom()),
            false,
        );
        let w = app.wheel.as_ref().expect("the list opens");
        assert!(w.flyout_open());
        assert!(w.ring().more_len() > 0);
        assert!(w.ring().settable.is_empty());
        assert_eq!(footer_row(app), None);
    });
}
