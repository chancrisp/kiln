//! The tray icon, added by this test process for a window of its own, and
//! the menu Kiln draws for it. The icon sends no input and the menu is
//! driven only through UI Automation patterns: the menu opens with
//! `KILN_TEST_PLACE` set (a monitor's work area in physical pixels), is
//! shown without being activated, and nothing here moves the pointer or
//! sends a key.

#![cfg(windows)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, Once};
use std::time::{Duration, Instant};

use kiln_app::theme::fluent::{contrast_palette, ALL_BUILT_IN, DARK};
use kiln_app::theme::{Rgb, SysColors};
use kiln_app::win::tray::{self, Tray, CMD_DRAG_WHEEL, CMD_OPEN, CMD_QUIT, TRAY_ID};
use kiln_app::win::tray_menu::{self, TrayMenu};
use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Shell::{Shell_NotifyIconW, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW};
use windows::Win32::UI::WindowsAndMessaging::*;

/// The last command a window got from a menu, as `WM_COMMAND`s it was
/// posted: how many, and the latest.
static COMMANDS: AtomicUsize = AtomicUsize::new(0);
static LAST: AtomicUsize = AtomicUsize::new(0);

/// One test at a time: they share the screen and the class.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn one_at_a_time() -> MutexGuard<'static, ()> {
    ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner())
}

extern "system" fn owner_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_COMMAND && lparam.0 == 0 {
        LAST.store(wparam.0 & 0xFFFF, Ordering::SeqCst);
        COMMANDS.fetch_add(1, Ordering::SeqCst);
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// A hidden top-level window for the icon, and the menu's commands, to
/// belong to.
fn owner() -> HWND {
    static CLASS: Once = Once::new();
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None).unwrap().into();
        CLASS.call_once(|| {
            let wc = WNDCLASSW {
                lpfnWndProc: Some(owner_proc),
                hInstance: instance,
                lpszClassName: w!("KilnTrayTestOwner"),
                ..Default::default()
            };
            assert_ne!(RegisterClassW(&wc), 0);
        });
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("KilnTrayTestOwner"),
            w!("owner"),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        )
        .unwrap()
    }
}

/// Whether Explorer has an icon for `hwnd` with `id`: changing nothing on
/// it succeeds only if it is there.
fn present(hwnd: HWND, id: u32) -> bool {
    let data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: id,
        ..Default::default()
    };
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) }.as_bool()
}

fn remove(hwnd: HWND, id: u32) {
    let data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: id,
        ..Default::default()
    };
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
    }
}

fn have_taskbar() -> bool {
    unsafe { FindWindowW(w!("Shell_TrayWnd"), None) }.is_ok_and(|h| !h.is_invalid())
}

#[test]
fn the_icon_is_added_readded_and_removed() {
    let _one = one_at_a_time();
    if !have_taskbar() {
        eprintln!("skipped: no taskbar");
        return;
    }
    let hwnd = owner();
    assert!(!present(hwnd, TRAY_ID));

    let t = Tray::add(hwnd, tray::small_icon(), "Kiln").expect("Explorer takes the icon");
    assert!(present(hwnd, TRAY_ID));
    assert_eq!(t.restarted(), tray::taskbar_created());

    // As Explorer restarting takes it away, and TaskbarCreated brings it
    // back.
    remove(hwnd, TRAY_ID);
    assert!(!present(hwnd, TRAY_ID));
    // A notification has nowhere to show then, and says so. (None is
    // shown while the icon is there: a test puts nothing on the screen.)
    assert!(!t.inform("Kiln", "test"));
    assert!(t.readd());
    assert!(present(hwnd, TRAY_ID));
    // Again while it is there: still one icon, still there.
    assert!(t.readd());
    assert!(present(hwnd, TRAY_ID));

    drop(t);
    assert!(!present(hwnd, TRAY_ID));
    unsafe { DestroyWindow(hwnd).unwrap() };
}

// ---------------------------------------------------------------- the menu

/// The rectangle `KILN_TEST_PLACE` names, or `None` with the reason said:
/// without it a menu would take the screen's focus.
fn area() -> Option<RECT> {
    let Ok(text) = std::env::var("KILN_TEST_PLACE") else {
        eprintln!("skipped: KILN_TEST_PLACE is not set (the menu would take the focus)");
        return None;
    };
    let n: Vec<i32> = text
        .split(',')
        .map(|p| p.trim().parse().ok())
        .collect::<Option<_>>()?;
    let [x, y, w, h] = n[..] else {
        return None;
    };
    Some(RECT {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    })
}

/// Deliver this thread's messages until `done`, or `limit` has passed:
/// whether `done` held. A window's messages run here, where it was made.
fn pump_until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    loop {
        let mut msg = MSG::default();
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if done() {
            return true;
        }
        if start.elapsed() > limit {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Run `f` on a thread of its own while this one delivers the menu's
/// messages: a UI Automation client calling a window of this thread would
/// wait on it.
fn client<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
    let handle = std::thread::spawn(f);
    pump_until(Duration::from_secs(30), || handle.is_finished());
    handle.join().expect("the client thread finished")
}

/// A UI Automation client on the menu's window.
struct Ui {
    auto: IUIAutomation,
    root: IUIAutomationElement,
}

impl Ui {
    fn new(hwnd: isize) -> Ui {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let auto: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).unwrap();
            let root = auto
                .ElementFromHandle(HWND(hwnd as *mut core::ffi::c_void))
                .unwrap();
            Ui { auto, root }
        }
    }

    fn all(&self, control_type: UIA_CONTROLTYPE_ID) -> Vec<IUIAutomationElement> {
        unsafe {
            let cond = self
                .auto
                .CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(control_type.0))
                .unwrap();
            let found = self.root.FindAll(TreeScope_Descendants, &cond).unwrap();
            (0..found.Length().unwrap())
                .map(|i| found.GetElement(i).unwrap())
                .collect()
        }
    }

    fn name(e: &IUIAutomationElement) -> String {
        unsafe { e.CurrentName().unwrap().to_string() }
    }

    /// The menu items' names, in order.
    fn items(&self) -> Vec<String> {
        self.all(UIA_MenuItemControlTypeId)
            .iter()
            .map(Ui::name)
            .collect()
    }

    fn item(&self, name: &str) -> IUIAutomationElement {
        self.all(UIA_MenuItemControlTypeId)
            .into_iter()
            .find(|e| Ui::name(e) == name)
            .unwrap_or_else(|| panic!("no menu item {name}"))
    }

    fn rect(e: &IUIAutomationElement) -> RECT {
        unsafe { e.CurrentBoundingRectangle().unwrap() }
    }
}

fn open_menu(owner: HWND, at: POINT, drag_wheel_on: bool) -> TrayMenu {
    tray_menu::open(tray_menu::Open {
        owner,
        at,
        drag_wheel_on,
        palette: DARK,
        text_scale: 1.0,
    })
    .expect("the menu opens")
}

fn setup() -> Option<(MutexGuard<'static, ()>, RECT, HWND)> {
    let one = one_at_a_time();
    let area = area()?;
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    Some((one, area, owner()))
}

fn middle(area: RECT) -> POINT {
    POINT {
        x: (area.left + area.right) / 2,
        y: (area.top + area.bottom) / 2,
    }
}

#[test]
fn the_menu_is_a_menu_of_open_drag_wheel_and_quit_and_never_takes_the_foreground() {
    let Some((_one, area, owner)) = setup() else {
        return;
    };
    let before = unsafe { GetForegroundWindow() };
    let menu = open_menu(owner, middle(area), true);
    assert!(menu.is_open());
    assert_eq!(unsafe { GetForegroundWindow() }, before);

    let hwnd = menu.hwnd().0 as isize;
    let (items, types, toggled, drag_has_invoke, open_has_invoke) = client(move || {
        let ui = Ui::new(hwnd);
        let menus = ui.all(UIA_MenuControlTypeId).len();
        let drag = ui.item("Drag-wheel");
        let toggled = unsafe {
            drag.GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)
                .unwrap()
                .CurrentToggleState()
                .unwrap()
        };
        let drag_has_invoke = unsafe {
            drag.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
                .is_ok()
        };
        let open_has_invoke = unsafe {
            ui.item("Open Kiln")
                .GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
                .is_ok()
        };
        (ui.items(), menus, toggled, drag_has_invoke, open_has_invoke)
    });
    assert_eq!(items, ["Open Kiln", "Drag-wheel", "Quit Kiln"]);
    assert_eq!(types, 1, "one Menu holds the items");
    assert_eq!(toggled, ToggleState_On);
    // The drag-wheel is a switch: toggled, not invoked.
    assert!(!drag_has_invoke);
    assert!(open_has_invoke);
    assert_eq!(unsafe { GetForegroundWindow() }, before);
    drop(menu);
    unsafe { DestroyWindow(owner).unwrap() };
}

#[test]
fn off_the_drag_wheel_reads_unchecked() {
    let Some((_one, area, owner)) = setup() else {
        return;
    };
    let menu = open_menu(owner, middle(area), false);
    let hwnd = menu.hwnd().0 as isize;
    let toggled = client(move || {
        let ui = Ui::new(hwnd);
        unsafe {
            ui.item("Drag-wheel")
                .GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)
                .unwrap()
                .CurrentToggleState()
                .unwrap()
        }
    });
    assert_eq!(toggled, ToggleState_Off);
    drop(menu);
    unsafe { DestroyWindow(owner).unwrap() };
}

/// Choose `name` through UI Automation, and see which command the window
/// the menu is for was posted, and that the menu has gone.
fn choose(name: &'static str, toggle: bool, command: u16) {
    let Some((_one, area, owner)) = setup() else {
        return;
    };
    let menu = open_menu(owner, middle(area), true);
    let hwnd = menu.hwnd().0 as isize;
    let before = COMMANDS.load(Ordering::SeqCst);
    let asked = std::thread::spawn(move || {
        let ui = Ui::new(hwnd);
        let item = ui.item(name);
        unsafe {
            if toggle {
                item.GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)
                    .unwrap()
                    .Toggle()
                    .unwrap();
            } else {
                item.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
                    .unwrap()
                    .Invoke()
                    .unwrap();
            }
        }
    });
    assert!(
        pump_until(Duration::from_secs(10), || COMMANDS.load(Ordering::SeqCst)
            > before),
        "{name} posted no command"
    );
    asked.join().unwrap();
    assert_eq!(LAST.load(Ordering::SeqCst), usize::from(command));
    assert!(
        pump_until(Duration::from_secs(5), || !menu.is_open()),
        "the menu stayed open"
    );
    drop(menu);
    unsafe { DestroyWindow(owner).unwrap() };
}

#[test]
fn choosing_open_kiln_posts_its_command_and_closes_the_menu() {
    choose("Open Kiln", false, CMD_OPEN);
}

#[test]
fn choosing_the_drag_wheel_posts_its_command_and_closes_the_menu() {
    choose("Drag-wheel", true, CMD_DRAG_WHEEL);
}

#[test]
fn choosing_quit_kiln_posts_its_command_and_closes_the_menu() {
    choose("Quit Kiln", false, CMD_QUIT);
}

#[test]
fn the_menu_opens_at_the_click_and_stays_inside_the_work_area() {
    let Some((_one, area, owner)) = setup() else {
        return;
    };
    let at = middle(area);
    let menu = open_menu(owner, at, true);
    let hwnd = menu.hwnd().0 as isize;
    let rect = client(move || {
        let ui = Ui::new(hwnd);
        Ui::rect(&ui.all(UIA_MenuControlTypeId)[0])
    });
    // Down and right of the click, as a Windows menu opens.
    assert!((rect.left - at.x).abs() <= 1, "{rect:?} at {at:?}");
    assert!((rect.top - at.y).abs() <= 1, "{rect:?} at {at:?}");
    assert!(rect.right > rect.left && rect.bottom > rect.top);
    drop(menu);

    // A click in a corner opens it up and left, inside the area.
    let corner = POINT {
        x: area.right - 2,
        y: area.bottom - 2,
    };
    let menu = open_menu(owner, corner, true);
    let hwnd = menu.hwnd().0 as isize;
    let rect = client(move || {
        let ui = Ui::new(hwnd);
        Ui::rect(&ui.all(UIA_MenuControlTypeId)[0])
    });
    assert!(
        rect.left >= area.left
            && rect.top >= area.top
            && rect.right <= area.right
            && rect.bottom <= area.bottom,
        "{rect:?} not inside {area:?}"
    );
    drop(menu);
    unsafe { DestroyWindow(owner).unwrap() };
}

#[test]
fn the_menu_opens_in_every_theme_and_closes_when_asked() {
    let Some((_one, area, owner)) = setup() else {
        return;
    };
    // The eight palettes and a Windows contrast theme: nine looks.
    let mut palettes = ALL_BUILT_IN.to_vec();
    palettes.push(contrast_palette(SysColors {
        window: Rgb::hex(0x000000),
        window_text: Rgb::hex(0xFFFFFF),
        highlight: Rgb::hex(0x1AEBFF),
        highlight_text: Rgb::hex(0x000000),
        gray_text: Rgb::hex(0x3FF23F),
        hotlight: Rgb::hex(0xFFFF00),
        button_face: Rgb::hex(0x101010),
        button_text: Rgb::hex(0xF0F0F0),
    }));
    assert_eq!(palettes.len(), 9);
    for p in palettes {
        let menu = tray_menu::open(tray_menu::Open {
            owner,
            at: middle(area),
            drag_wheel_on: true,
            palette: p,
            text_scale: 1.0,
        })
        .unwrap_or_else(|e| panic!("{} opens: {e}", p.name));
        let hwnd = menu.hwnd().0 as isize;
        let items = client(move || Ui::new(hwnd).items());
        assert_eq!(items.len(), 3, "{}", p.name);
        menu.close();
        assert!(
            pump_until(Duration::from_secs(5), || !menu.is_open()),
            "{} didn't close",
            p.name
        );
    }
    unsafe { DestroyWindow(owner).unwrap() };
}

#[test]
fn the_menu_follows_the_text_size() {
    let Some((_one, area, owner)) = setup() else {
        return;
    };
    let height = |scale: f32| {
        let menu = tray_menu::open(tray_menu::Open {
            owner,
            at: middle(area),
            drag_wheel_on: true,
            palette: DARK,
            text_scale: scale,
        })
        .unwrap();
        let hwnd = menu.hwnd().0 as isize;
        let r = client(move || Ui::rect(&Ui::new(hwnd).all(UIA_MenuControlTypeId)[0]));
        drop(menu);
        r.bottom - r.top
    };
    let (one, big) = (height(1.0), height(1.5));
    assert!(big > one, "{one} px at 100%, {big} px at 150%");
    unsafe { DestroyWindow(owner).unwrap() };
}
