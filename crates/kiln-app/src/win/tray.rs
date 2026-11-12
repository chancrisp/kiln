//! Kiln's icon in the notification area, there only while Kiln's window
//! is hidden (with the drag-wheel on, closing the window hides it). It is
//! added as the window hides and removed as it shows. A click opens Kiln.
//! Its menu holds Open Kiln, Drag-wheel and Quit Kiln. It comes back when
//! Explorer restarts (`TaskbarCreated`).
//!
//! Notifications use this icon while it is there (`Tray::inform`), so Kiln
//! never shows two icons at once. While the window is shown,
//! `system::notify` adds a short-lived icon of its own, with another id, as
//! it always has.

use std::sync::OnceLock;

use windows::core::{w, Result, HSTRING, PCWSTR};
use windows::Win32::Foundation::{E_FAIL, HWND, LPARAM, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_INFO, NIM_ADD,
    NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, ChangeWindowMessageFilterEx, CreatePopupMenu, DestroyMenu, GetSystemMetrics,
    LoadImageW, PostMessageW, RegisterWindowMessageW, SetForegroundWindow, SetMenuDefaultItem,
    TrackPopupMenuEx, HICON, IMAGE_ICON, LR_SHARED, MF_CHECKED, MF_SEPARATOR, MF_STRING,
    MF_UNCHECKED, MSGFLT_ALLOW, SM_CXSMICON, SM_CYSMICON, SM_MENUDROPALIGNMENT, TPM_BOTTOMALIGN,
    TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTALIGN, TPM_RIGHTBUTTON, WM_APP, WM_CONTEXTMENU, WM_NULL,
};

use super::system::{copy_wide, Modal};

/// The message the tray icon sends the window. `WM_APP + 15` is the
/// hooks'. Its `wparam` and `lparam` are read as numbers only (`read`): no
/// `lparam` is dereferenced.
pub const WM_KILN_TRAY: u32 = WM_APP + 16;

/// The resident icon's id. `system::notify`'s short-lived icon is 1.
pub const TRAY_ID: u32 = 2;

/// The menu's command ids. The window also takes them as a `WM_COMMAND`
/// with no control (`lParam` 0): Kiln posts Quit to itself once a quit
/// that waited for an export can go ahead.
pub const CMD_OPEN: u16 = 0x4B01;
pub const CMD_DRAG_WHEEL: u16 = 0x4B02;
pub const CMD_QUIT: u16 = 0x4B03;

/// Said once, the first time closing the window hides it.
pub const FIRST_HIDE: &str =
    "Kiln is still running in the notification area for the drag-wheel. Quit it from there.";

/// A left click, or Enter or Space on the icon. NOTIFYICON_VERSION_4.
const NIN_SELECT: u32 = 0x400;
const NIN_KEYSELECT: u32 = 0x401;
const NIN_BALLOONHIDE: u32 = 0x403;
const NIN_BALLOONTIMEOUT: u32 = 0x404;
const NIN_BALLOONUSERCLICK: u32 = 0x405;

/// What the tray menu asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    Open,
    ToggleDragWheel,
    Quit,
}

impl TrayCommand {
    pub fn id(self) -> u16 {
        match self {
            TrayCommand::Open => CMD_OPEN,
            TrayCommand::ToggleDragWheel => CMD_DRAG_WHEEL,
            TrayCommand::Quit => CMD_QUIT,
        }
    }

    /// The command with menu id `id`. `None` for anything else, such as 0
    /// for a menu closed without a choice.
    pub fn from_id(id: usize) -> Option<TrayCommand> {
        [
            TrayCommand::Open,
            TrayCommand::ToggleDragWheel,
            TrayCommand::Quit,
        ]
        .into_iter()
        .find(|c| usize::from(c.id()) == id)
    }
}

/// What a `WM_KILN_TRAY` message says happened.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TrayEvent {
    /// A left click, or Enter on the icon: open Kiln.
    Open,
    /// A right click, or the menu key on the icon: the menu, at this
    /// point on the screen.
    Menu(POINT),
    /// A notification shown from the icon was clicked.
    NoticeClicked,
    /// A notification shown from the icon went away by itself or was
    /// dismissed.
    NoticeGone,
    Other,
}

/// Read a `WM_KILN_TRAY` message, as NOTIFYICON_VERSION_4 lays it out: the
/// event in the low word of `lparam`, the icon's id in its high word, and
/// the point on the screen in `wparam`.
pub fn read(wparam: WPARAM, lparam: LPARAM) -> TrayEvent {
    let event = (lparam.0 as u32) & 0xFFFF;
    let at = POINT {
        x: (wparam.0 & 0xFFFF) as u16 as i16 as i32,
        y: ((wparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32,
    };
    match event {
        NIN_SELECT | NIN_KEYSELECT => TrayEvent::Open,
        WM_CONTEXTMENU => TrayEvent::Menu(at),
        NIN_BALLOONUSERCLICK => TrayEvent::NoticeClicked,
        NIN_BALLOONTIMEOUT | NIN_BALLOONHIDE => TrayEvent::NoticeGone,
        _ => TrayEvent::Other,
    }
}

/// What closing the window does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Closing {
    /// Hide the window and stay in the notification area, once held
    /// results are asked about. `tell` says whether to say so: only the
    /// first time ever.
    Hide { tell: bool },
    /// Quit, as closing always did: held results ask first.
    Quit,
}

/// Closing hides the window while the drag-wheel is on, as long as the
/// icon is there to bring it back. Without the icon, Kiln could not be
/// found again, so it quits.
pub fn closing(drag_wheel: bool, in_tray: bool, told: bool) -> Closing {
    if drag_wheel && in_tray {
        Closing::Hide { tell: !told }
    } else {
        Closing::Quit
    }
}

/// Whether closing the window leaves Kiln running in the notification
/// area: only while the drag-wheel is on and its watch is running. With
/// the hooks refused the drag-wheel can't work, so nothing stays behind
/// and closing quits, as with the setting off.
pub fn resident(drag_wheel: bool, hooks_refused: bool) -> bool {
    drag_wheel && !hooks_refused
}

/// Whether the icon belongs in the notification area: only while the
/// window is hidden. That holds even after the drag-wheel is turned off
/// from the menu, so the hidden Kiln can still be opened or quit.
pub fn wanted(window_shown: bool) -> bool {
    !window_shown
}

/// The message Explorer broadcasts when the taskbar is made again, after
/// it restarted.
pub fn taskbar_created() -> u32 {
    static ID: OnceLock<u32> = OnceLock::new();
    *ID.get_or_init(|| unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) })
}

/// Kiln's icon at the small icon size. Shared: Windows keeps it, and it is
/// never destroyed. Null when the module has no icon, as in a test binary.
pub fn small_icon() -> HICON {
    unsafe {
        let Ok(instance) = GetModuleHandleW(None) else {
            return HICON::default();
        };
        LoadImageW(
            Some(instance.into()),
            PCWSTR(1 as _),
            IMAGE_ICON,
            GetSystemMetrics(SM_CXSMICON),
            GetSystemMetrics(SM_CYSMICON),
            LR_SHARED,
        )
        .map(|h| HICON(h.0))
        .unwrap_or_default()
    }
}

/// The icon in the notification area. It goes when this is dropped.
pub struct Tray {
    hwnd: HWND,
    id: u32,
    /// `TaskbarCreated`: on it, the window calls `readd`.
    restarted: u32,
    icon: HICON,
    tip: String,
}

impl Tray {
    /// Put the icon in the notification area. It sends `WM_KILN_TRAY` to
    /// `hwnd`. Fails when there's no taskbar to put it in.
    pub fn add(hwnd: HWND, icon: HICON, tip: &str) -> Result<Tray> {
        let tray = Tray {
            hwnd,
            id: TRAY_ID,
            restarted: taskbar_created(),
            icon,
            tip: tip.to_owned(),
        };
        tray.put()?;
        unsafe {
            // A Kiln run as administrator still hears Explorer restart.
            let _ = ChangeWindowMessageFilterEx(hwnd, tray.restarted, MSGFLT_ALLOW, None);
        }
        Ok(tray)
    }

    /// The `TaskbarCreated` message id.
    pub fn restarted(&self) -> u32 {
        self.restarted
    }

    /// Put the icon back after Explorer restarted, which took it away.
    /// False if Explorer didn't take it, as when it is still starting.
    pub fn readd(&self) -> bool {
        self.put().is_ok()
    }

    fn data(&self) -> NOTIFYICONDATAW {
        let mut data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: self.id,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP,
            uCallbackMessage: WM_KILN_TRAY,
            hIcon: self.icon,
            ..Default::default()
        };
        copy_wide(&mut data.szTip, &self.tip);
        data
    }

    fn put(&self) -> Result<()> {
        let mut data = self.data();
        unsafe {
            // One already there, as when Explorer kept it after all, is
            // replaced rather than refused.
            let _ = Shell_NotifyIconW(NIM_DELETE, &data);
            if !Shell_NotifyIconW(NIM_ADD, &data).as_bool() {
                return Err(E_FAIL.into());
            }
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            let _ = Shell_NotifyIconW(NIM_SETVERSION, &data);
        }
        Ok(())
    }

    /// Show a notification from this icon. False if Windows didn't take
    /// it, as when Explorer has gone.
    pub fn inform(&self, title: &str, text: &str) -> bool {
        let mut data = self.data();
        data.uFlags |= NIF_INFO;
        data.dwInfoFlags = NIIF_INFO;
        copy_wide(&mut data.szInfoTitle, title);
        copy_wide(&mut data.szInfo, text);
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) }.as_bool()
    }
}

/// The tray menu's items in order: each command, its words with their
/// access key, and whether it is checked; `None` is a separator.
pub fn menu_items(drag_wheel_on: bool) -> [Option<(TrayCommand, &'static str, bool)>; 4] {
    [
        Some((TrayCommand::Open, "&Open Kiln", false)),
        Some((TrayCommand::ToggleDragWheel, "&Drag-wheel", drag_wheel_on)),
        None,
        Some((TrayCommand::Quit, "&Quit Kiln", false)),
    ]
}

/// The tray menu for the window `hwnd`, at `at` on the screen. It takes
/// the window, not a `Tray`: the menu's own loop delivers the window's
/// messages, and one of them, such as a second launch showing the
/// window, may take the icon away while the menu is open.
pub fn show_menu(hwnd: HWND, at: POINT, drag_wheel_on: bool) -> Option<TrayCommand> {
    let _modal = Modal::open();
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        for item in menu_items(drag_wheel_on) {
            match item {
                Some((command, words, checked)) => {
                    let check = if checked { MF_CHECKED } else { MF_UNCHECKED };
                    let _ = AppendMenuW(
                        menu,
                        MF_STRING | check,
                        usize::from(command.id()),
                        &HSTRING::from(words),
                    );
                }
                None => {
                    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
                }
            }
        }
        // Open Kiln is what a click on the icon does: shown in bold.
        let _ = SetMenuDefaultItem(menu, u32::from(CMD_OPEN), 0);
        // Without the window in front, the menu wouldn't close when the
        // person clicks somewhere else.
        // (The test placement switch never takes the foreground.)
        if !super::placement::quiet_on() {
            let _ = SetForegroundWindow(hwnd);
        }
        let side = if GetSystemMetrics(SM_MENUDROPALIGNMENT) != 0 {
            TPM_RIGHTALIGN
        } else {
            TPM_LEFTALIGN
        };
        let chosen = TrackPopupMenuEx(
            menu,
            (TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN | side).0,
            at.x,
            at.y,
            hwnd,
            None,
        );
        let _ = DestroyMenu(menu);
        // And without a message after it, the next right click on the
        // icon can open the menu only to close it at once.
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        TrayCommand::from_id(chosen.0 as usize)
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        let data = self.data();
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &data);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_hides_while_the_drag_wheel_is_on_and_tells_once() {
        assert_eq!(closing(true, true, false), Closing::Hide { tell: true });
        assert_eq!(closing(true, true, true), Closing::Hide { tell: false });
        // Off, closing quits as it always did.
        assert_eq!(closing(false, true, false), Closing::Quit);
        assert_eq!(closing(false, false, true), Closing::Quit);
        // With no icon to bring the window back, it quits too.
        assert_eq!(closing(true, false, false), Closing::Quit);
    }

    #[test]
    fn with_the_hooks_refused_closing_quits_as_with_the_setting_off() {
        assert!(resident(true, false));
        // Refused: the drag-wheel can't work, so nothing stays resident.
        assert!(!resident(true, true));
        assert!(!resident(false, false));
        assert!(!resident(false, true));
        for told in [false, true] {
            assert_eq!(closing(resident(true, true), true, told), Closing::Quit);
            assert_eq!(closing(resident(false, false), true, told), Closing::Quit);
        }
        assert_eq!(
            closing(resident(true, false), true, false),
            Closing::Hide { tell: true }
        );
    }

    #[test]
    fn the_icon_is_there_only_while_the_window_is_hidden() {
        assert!(wanted(false));
        assert!(!wanted(true));
    }

    #[test]
    fn the_menu_reads_open_drag_wheel_then_quit() {
        let words = |on| {
            menu_items(on)
                .iter()
                .map(|i| i.map(|(c, w, checked)| (c, w, checked)))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            words(true),
            [
                Some((TrayCommand::Open, "&Open Kiln", false)),
                Some((TrayCommand::ToggleDragWheel, "&Drag-wheel", true)),
                None,
                Some((TrayCommand::Quit, "&Quit Kiln", false)),
            ]
        );
        // Off, the drag-wheel's item is there, unchecked, to turn it on.
        assert_eq!(
            words(false)[1],
            Some((TrayCommand::ToggleDragWheel, "&Drag-wheel", false))
        );
    }

    #[test]
    fn menu_ids_name_their_commands() {
        for c in [
            TrayCommand::Open,
            TrayCommand::ToggleDragWheel,
            TrayCommand::Quit,
        ] {
            assert_eq!(TrayCommand::from_id(usize::from(c.id())), Some(c));
        }
        // A menu closed without a choice returns 0.
        assert_eq!(TrayCommand::from_id(0), None);
        assert_eq!(TrayCommand::from_id(1), None);
    }

    #[test]
    fn version_4_messages_are_read() {
        let msg = |event: u32| LPARAM(((TRAY_ID << 16) | event) as isize);
        assert_eq!(read(WPARAM(0), msg(NIN_SELECT)), TrayEvent::Open);
        assert_eq!(read(WPARAM(0), msg(NIN_KEYSELECT)), TrayEvent::Open);
        // The point is in wparam, and can be left of or above the primary
        // monitor.
        let at = WPARAM(((300u32 << 16) | (-20i16 as u16 as u32)) as usize);
        assert_eq!(
            read(at, msg(WM_CONTEXTMENU)),
            TrayEvent::Menu(POINT { x: -20, y: 300 })
        );
        assert_eq!(
            read(WPARAM(0), msg(NIN_BALLOONUSERCLICK)),
            TrayEvent::NoticeClicked
        );
        assert_eq!(
            read(WPARAM(0), msg(NIN_BALLOONTIMEOUT)),
            TrayEvent::NoticeGone
        );
        // Mouse moves and the rest are nothing to act on.
        assert_eq!(read(WPARAM(0), msg(0x200)), TrayEvent::Other);
    }

    #[test]
    fn taskbar_created_is_a_registered_message() {
        let id = taskbar_created();
        assert!((0xC000..=0xFFFF).contains(&id), "{id:#x}");
        assert_eq!(taskbar_created(), id);
    }
}
