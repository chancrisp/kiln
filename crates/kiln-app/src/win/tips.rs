//! Native tooltips over Kiln's drawn controls: one tooltip control owned
//! by the main window, with a tool for each control's rectangle. The tools
//! are brought in line with the layout after every frame, and their text
//! is asked for only when a tip is about to show.

use windows::core::{w, PWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::UI::Controls::{
    InitCommonControlsEx, SetWindowTheme, ICC_WIN95_CLASSES, INITCOMMONCONTROLSEX, NMHDR,
    NMTTDISPINFOW, TOOLTIPS_CLASSW, TTF_SUBCLASS, TTM_ADDTOOLW, TTM_DELTOOLW, TTM_NEWTOOLRECTW,
    TTM_SETMAXTIPWIDTH, TTN_GETDISPINFOW, TTS_ALWAYSTIP, TTS_NOPREFIX, TTTOOLINFOW,
};
// Only the held test tooltip tracks a position.
#[cfg(any(debug_assertions, feature = "test-hooks"))]
use windows::Win32::UI::Controls::{
    TTF_ABSOLUTE, TTF_TRACK, TTM_SETTOOLINFOW, TTM_TRACKACTIVATE, TTM_TRACKPOSITION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, SendMessageW, CW_USEDEFAULT, WINDOW_STYLE, WS_EX_TOPMOST, WS_POPUP,
};

use crate::layout::Rect;

/// How wide a tip may grow before it wraps, in DIPs. A row's tip is its
/// whole sentence and then the folder's path on a line of its own.
const MAX_TIP_WIDTH: f32 = 480.0;

pub struct Tips {
    hwnd: HWND,
    owner: HWND,
    /// The tools as last synced: each id and its rectangle in pixels.
    tools: Vec<(usize, RECT)>,
    /// The text handed out for the tip showing now. The tooltip reads it
    /// after the notification returns, so it lives here.
    text: Vec<u16>,
    /// The words of the tip a capture holds open (`pop`).
    #[cfg(any(debug_assertions, feature = "test-hooks"))]
    held: Vec<u16>,
    /// Whether that tip's tool is in the control.
    #[cfg(any(debug_assertions, feature = "test-hooks"))]
    held_added: bool,
}

/// Which control's tip `KILN_TEST_TIP` asks to have held open for a capture.
#[cfg_attr(not(any(debug_assertions, feature = "test-hooks")), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestTip {
    Settings,
    Help,
    Convert,
    Add,
    Folder,
}

/// The control `KILN_TEST_TIP` names, by its UI Automation id ("SettingsButton",
/// "HelpButton", "ConvertButton", "AddButton", "Folder") or the short word
/// ("settings"), in any case. Anything else is none.
#[cfg_attr(not(any(debug_assertions, feature = "test-hooks")), allow(dead_code))]
pub fn parse_test_tip(value: Option<&std::ffi::OsStr>) -> Option<TestTip> {
    let v = value?.to_str()?.trim().to_ascii_lowercase();
    match v.trim_end_matches("button") {
        "settings" => Some(TestTip::Settings),
        "help" => Some(TestTip::Help),
        "convert" => Some(TestTip::Convert),
        "add" => Some(TestTip::Add),
        "folder" => Some(TestTip::Folder),
        _ => None,
    }
}

/// The tip `KILN_TEST_TIP` asks for, read once. Off in a build without
/// debug assertions or `test-hooks`.
#[cfg(any(debug_assertions, feature = "test-hooks"))]
pub fn test_tip() -> Option<TestTip> {
    static TIP: std::sync::OnceLock<Option<TestTip>> = std::sync::OnceLock::new();
    *TIP.get_or_init(|| parse_test_tip(std::env::var_os("KILN_TEST_TIP").as_deref()))
}

#[cfg(not(any(debug_assertions, feature = "test-hooks")))]
pub fn test_tip() -> Option<TestTip> {
    None
}

impl Tips {
    /// The tooltip control for `owner`. `None` if Windows won't make one;
    /// Kiln then goes without tips.
    pub fn new(owner: HWND, instance: HINSTANCE, dpi: f32) -> Option<Self> {
        let icc = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_WIN95_CLASSES,
        };
        let hwnd = unsafe {
            let _ = InitCommonControlsEx(&icc);
            CreateWindowExW(
                WS_EX_TOPMOST,
                TOOLTIPS_CLASSW,
                None,
                WS_POPUP | WINDOW_STYLE(TTS_ALWAYSTIP | TTS_NOPREFIX),
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                Some(owner),
                None,
                Some(instance),
                None,
            )
            .ok()?
        };
        let tips = Self {
            hwnd,
            owner,
            tools: vec![],
            text: vec![0],
            #[cfg(any(debug_assertions, feature = "test-hooks"))]
            held: vec![0],
            #[cfg(any(debug_assertions, feature = "test-hooks"))]
            held_added: false,
        };
        tips.set_dpi(dpi);
        Some(tips)
    }

    /// Draw tips dark or light, as Kiln's theme is: Windows draws them, so
    /// they follow the theme's base, not the Windows app mode.
    pub fn set_dark(&self, dark: bool) {
        let class = if dark {
            w!("DarkMode_Explorer")
        } else {
            w!("Explorer")
        };
        // SAFETY: a plain call on the tooltip control this owns.
        unsafe {
            let _ = SetWindowTheme(self.hwnd, class, None);
        }
    }

    /// Follow a new monitor scale.
    pub fn set_dpi(&self, dpi: f32) {
        let px = (MAX_TIP_WIDTH * dpi / 96.0).round() as isize;
        unsafe {
            SendMessageW(
                self.hwnd,
                TTM_SETMAXTIPWIDTH,
                Some(WPARAM(0)),
                Some(LPARAM(px)),
            );
        }
    }

    fn info(&self, id: usize, rect: RECT) -> TTTOOLINFOW {
        TTTOOLINFOW {
            cbSize: std::mem::size_of::<TTTOOLINFOW>() as u32,
            uFlags: TTF_SUBCLASS,
            hwnd: self.owner,
            uId: id,
            rect,
            // LPSTR_TEXTCALLBACKW: the text is asked for when it shows.
            lpszText: PWSTR(-1isize as *mut u16),
            ..Default::default()
        }
    }

    fn send(&self, msg: u32, info: &TTTOOLINFOW) {
        unsafe {
            SendMessageW(
                self.hwnd,
                msg,
                Some(WPARAM(0)),
                Some(LPARAM(info as *const TTTOOLINFOW as isize)),
            );
        }
    }

    /// Make the tools match `want`, each id with its rectangle in DIPs,
    /// scaled by `dpi`. Tools that are gone are removed, moved ones move
    /// and new ones are added; the rest are left alone.
    pub fn sync(&mut self, want: &[(usize, Rect)], dpi: f32) {
        let k = dpi / 96.0;
        let px = |r: &Rect| RECT {
            left: (r.x * k).round() as i32,
            top: (r.y * k).round() as i32,
            right: (r.right() * k).round() as i32,
            bottom: (r.bottom() * k).round() as i32,
        };
        let want: Vec<(usize, RECT)> = want
            .iter()
            .filter(|(_, r)| r.w > 0.0 && r.h > 0.0)
            .map(|(id, r)| (*id, px(r)))
            .collect();
        if want == self.tools {
            return;
        }
        for &(id, rect) in &self.tools {
            if !want.iter().any(|(w, _)| *w == id) {
                self.send(TTM_DELTOOLW, &self.info(id, rect));
            }
        }
        for &(id, rect) in &want {
            match self.tools.iter().find(|(t, _)| *t == id) {
                Some((_, old)) if *old == rect => {}
                Some(_) => self.send(TTM_NEWTOOLRECTW, &self.info(id, rect)),
                None => self.send(TTM_ADDTOOLW, &self.info(id, rect)),
            }
        }
        self.tools = want;
    }

    /// Show a tip for a capture, held open at the screen point (`x`, `y`)
    /// until the window goes: a tip that follows no pointer, drawn by the
    /// same control, so it has the theme's look. Only a test build has it.
    #[cfg(any(debug_assertions, feature = "test-hooks"))]
    pub fn pop(&mut self, text: &str, x: i32, y: i32) {
        const HELD_ID: usize = usize::MAX - 1;
        self.held = text.encode_utf16().chain(Some(0)).collect();
        let mut info = TTTOOLINFOW {
            cbSize: std::mem::size_of::<TTTOOLINFOW>() as u32,
            uFlags: TTF_TRACK | TTF_ABSOLUTE,
            hwnd: self.owner,
            uId: HELD_ID,
            lpszText: PWSTR(self.held.as_mut_ptr()),
            ..Default::default()
        };
        info.rect = RECT::default();
        let activate = |on: usize| {
            // SAFETY: a plain message to the tooltip control this owns;
            // `info` outlives it and its text is `self.held`.
            unsafe {
                SendMessageW(
                    self.hwnd,
                    TTM_TRACKACTIVATE,
                    Some(WPARAM(on)),
                    Some(LPARAM(&info as *const TTTOOLINFOW as isize)),
                );
            }
        };
        if self.held_added {
            // The tip shown before gives way to the new words and place.
            activate(0);
            self.send(TTM_SETTOOLINFOW, &info);
        } else {
            self.send(TTM_ADDTOOLW, &info);
            self.held_added = true;
        }
        // SAFETY: a plain message to the tooltip control this owns.
        unsafe {
            SendMessageW(
                self.hwnd,
                TTM_TRACKPOSITION,
                Some(WPARAM(0)),
                Some(LPARAM(
                    ((y as u32 as isize) << 16) | (x as u32 as isize & 0xFFFF),
                )),
            );
        }
        activate(1);
    }

    /// The tool a WM_NOTIFY asks text for, if it's this control's
    /// TTN_GETDISPINFOW.
    pub fn asking(&self, lparam: LPARAM) -> Option<usize> {
        // SAFETY: for WM_NOTIFY, lparam is a valid NMHDR for the message.
        let hdr = unsafe { super::system::lparam_ref::<NMHDR>(lparam)? };
        (hdr.hwndFrom == self.hwnd && hdr.code == TTN_GETDISPINFOW).then_some(hdr.idFrom)
    }

    /// Answer the TTN_GETDISPINFOW in `lparam` with `text`.
    pub fn answer(&mut self, lparam: LPARAM, text: &str) {
        self.text = text.encode_utf16().chain(Some(0)).collect();
        // SAFETY: `asking` found this WM_NOTIFY to be a TTN_GETDISPINFOW,
        // so lparam is its NMTTDISPINFOW, ours to fill in until we answer.
        if let Some(info) = unsafe { super::system::lparam_mut::<NMTTDISPINFOW>(lparam) } {
            info.lpszText = PWSTR(self.text.as_mut_ptr());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn the_test_tip_names_a_control_by_its_id_or_a_short_word() {
        let p = |s: &str| parse_test_tip(Some(OsStr::new(s)));
        assert_eq!(p("SettingsButton"), Some(TestTip::Settings));
        assert_eq!(p(" help "), Some(TestTip::Help));
        assert_eq!(p("ConvertButton"), Some(TestTip::Convert));
        assert_eq!(p("ADD"), Some(TestTip::Add));
        assert_eq!(p("folder"), Some(TestTip::Folder));
        assert_eq!(p("nothing"), None);
        assert_eq!(p(""), None);
        assert_eq!(parse_test_tip(None), None);
    }
}
