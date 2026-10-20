//! Native tooltips over Kiln's drawn controls: one tooltip control owned
//! by the main window, with a tool for each control's rectangle. The tools
//! are brought in line with the layout after every frame, and their text
//! is asked for only when a tip is about to show.

use windows::core::PWSTR;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::UI::Controls::{
    InitCommonControlsEx, ICC_WIN95_CLASSES, INITCOMMONCONTROLSEX, NMHDR, NMTTDISPINFOW,
    TOOLTIPS_CLASSW, TTF_SUBCLASS, TTM_ADDTOOLW, TTM_DELTOOLW, TTM_NEWTOOLRECTW,
    TTM_SETMAXTIPWIDTH, TTN_GETDISPINFOW, TTS_ALWAYSTIP, TTS_NOPREFIX, TTTOOLINFOW,
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
        };
        tips.set_dpi(dpi);
        Some(tips)
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

    /// The tool a WM_NOTIFY asks text for, if it's this control's
    /// TTN_GETDISPINFOW.
    pub fn asking(&self, lparam: LPARAM) -> Option<usize> {
        let hdr = unsafe { (lparam.0 as *const NMHDR).as_ref()? };
        (hdr.hwndFrom == self.hwnd && hdr.code == TTN_GETDISPINFOW).then_some(hdr.idFrom)
    }

    /// Answer the TTN_GETDISPINFOW in `lparam` with `text`.
    pub fn answer(&mut self, lparam: LPARAM, text: &str) {
        self.text = text.encode_utf16().chain(Some(0)).collect();
        if let Some(info) = unsafe { (lparam.0 as *mut NMTTDISPINFOW).as_mut() } {
            info.lpszText = PWSTR(self.text.as_mut_ptr());
        }
    }
}
