//! The OLE drop target. It runs on the UI thread and forwards each event
//! to the window procedure with SendMessage, so all state stays in `App`.

use std::path::PathBuf;

use windows::core::{implement, Ref, Result};
use windows::Win32::Foundation::{HWND, LPARAM, POINTL, WPARAM};
use windows::Win32::System::Com::{IDataObject, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL};
use windows::Win32::System::Ole::{
    IDropTarget, IDropTarget_Impl, ReleaseStgMedium, CF_HDROP, DROPEFFECT, DROPEFFECT_COPY,
    DROPEFFECT_NONE,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_APP};

pub const WM_KILN_DRAG: u32 = WM_APP + 1;
pub const WM_KILN_DROP: u32 = WM_APP + 2;

/// `WM_KILN_DRAG` wparam values.
pub const DRAG_ENTER: usize = 1;
pub const DRAG_OVER: usize = 2;
pub const DRAG_LEAVE: usize = 3;

/// Sent with `WM_KILN_DRAG` and `WM_KILN_DROP`; lparam points at it for
/// the duration of the SendMessage call.
pub struct DragEvent {
    /// Modifier and mouse-button state (MK_SHIFT, MK_CONTROL, MK_ALT).
    pub keys: MODIFIERKEYS_FLAGS,
    /// Screen coordinates, physical pixels.
    pub x: i32,
    pub y: i32,
    pub paths: Vec<PathBuf>,
}

#[implement(IDropTarget)]
pub struct DropTarget {
    hwnd: HWND,
    has_files: std::cell::Cell<bool>,
}

impl DropTarget {
    pub fn new(hwnd: HWND) -> Self {
        Self {
            hwnd,
            has_files: std::cell::Cell::new(false),
        }
    }

    fn send(&self, msg: u32, kind: usize, ev: &DragEvent) {
        unsafe {
            SendMessageW(
                self.hwnd,
                msg,
                Some(WPARAM(kind)),
                Some(LPARAM(ev as *const DragEvent as isize)),
            );
        }
    }
}

fn hdrop_format() -> FORMATETC {
    FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

fn offers_files(data: &IDataObject) -> bool {
    unsafe { data.QueryGetData(&hdrop_format()).is_ok() }
}

/// The file paths in a drag, read from its CF_HDROP payload.
fn dropped_paths(data: &IDataObject) -> Vec<PathBuf> {
    unsafe {
        let Ok(mut medium) = data.GetData(&hdrop_format()) else {
            return vec![];
        };
        let out = hdrop_paths(HDROP(medium.u.hGlobal.0));
        ReleaseStgMedium(&mut medium);
        out
    }
}

/// The paths in a CF_HDROP payload, from a drag or the clipboard.
///
/// # Safety
/// `hdrop` must be a valid HDROP for the duration of the call.
pub unsafe fn hdrop_paths(hdrop: HDROP) -> Vec<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    let mut out = Vec::new();
    let count = unsafe { DragQueryFileW(hdrop, u32::MAX, None) };
    for i in 0..count {
        let len = unsafe { DragQueryFileW(hdrop, i, None) } as usize;
        let mut buf = vec![0u16; len + 1];
        let got = unsafe { DragQueryFileW(hdrop, i, Some(&mut buf)) } as usize;
        out.push(PathBuf::from(std::ffi::OsString::from_wide(&buf[..got])));
    }
    out
}

fn set_effect(effect: *mut DROPEFFECT, value: DROPEFFECT) {
    if !effect.is_null() {
        unsafe { *effect = value };
    }
}

fn effect_for(files: bool) -> DROPEFFECT {
    if files {
        DROPEFFECT_COPY
    } else {
        DROPEFFECT_NONE
    }
}

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(
        &self,
        data: Ref<IDataObject>,
        keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        let files = data.as_ref().is_some_and(offers_files);
        self.has_files.set(files);
        set_effect(effect, effect_for(files));
        if files {
            // Read the paths now, so the wheel can offer the right items
            // before anything is dropped.
            let ev = DragEvent {
                keys,
                x: pt.x,
                y: pt.y,
                paths: data.as_ref().map(dropped_paths).unwrap_or_default(),
            };
            self.send(WM_KILN_DRAG, DRAG_ENTER, &ev);
        }
        Ok(())
    }

    fn DragOver(
        &self,
        keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        let files = self.has_files.get();
        set_effect(effect, effect_for(files));
        if files {
            let ev = DragEvent {
                keys,
                x: pt.x,
                y: pt.y,
                paths: vec![],
            };
            self.send(WM_KILN_DRAG, DRAG_OVER, &ev);
        }
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        if self.has_files.replace(false) {
            let ev = DragEvent {
                keys: MODIFIERKEYS_FLAGS(0),
                x: 0,
                y: 0,
                paths: vec![],
            };
            self.send(WM_KILN_DRAG, DRAG_LEAVE, &ev);
        }
        Ok(())
    }

    fn Drop(
        &self,
        data: Ref<IDataObject>,
        keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        self.has_files.set(false);
        let paths = data.as_ref().map(dropped_paths).unwrap_or_default();
        set_effect(effect, effect_for(!paths.is_empty()));
        let ev = DragEvent {
            keys,
            x: pt.x,
            y: pt.y,
            paths,
        };
        self.send(WM_KILN_DROP, 0, &ev);
        Ok(())
    }
}
