//! The OLE drop target. It runs on the UI thread and hands each event to
//! the callback its window gave it, so all state stays with the window.
//! Nothing goes through a message: no pointer is ever passed as an
//! `lparam`.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use windows::core::{implement, Ref, Result};
use windows::Win32::Foundation::{LRESULT, POINTL};
use windows::Win32::System::Com::{IDataObject, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL};
use windows::Win32::System::Ole::{
    IDropTarget, IDropTarget_Impl, ReleaseStgMedium, CF_HDROP, DROPEFFECT, DROPEFFECT_COPY,
    DROPEFFECT_NONE,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

/// Which part of a drag an event is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragKind {
    Enter,
    Over,
    Leave,
    Drop,
}

/// Each drag event, as the window hears it.
pub struct DragEvent {
    /// Modifier and mouse-button state (MK_SHIFT, MK_CONTROL, MK_ALT).
    pub keys: MODIFIERKEYS_FLAGS,
    /// Screen coordinates, physical pixels.
    pub x: i32,
    pub y: i32,
    pub paths: Vec<PathBuf>,
    /// Whether the source allows a copy: a drop answered with anything
    /// else does nothing.
    pub copy: bool,
}

/// What the window does with each drag event, and its answer: non-zero
/// for a copy, zero for no drop. `None`: the window is busy and takes no
/// drop now, whatever else it would answer.
pub type OnDrag = Rc<dyn Fn(DragKind, &DragEvent) -> Option<LRESULT>>;

#[implement(IDropTarget)]
pub struct DropTarget {
    on_drag: OnDrag,
    has_files: Cell<bool>,
    /// Set while Kiln runs a drag of its own results: dropping them back
    /// onto Kiln does nothing.
    own: Rc<Cell<bool>>,
    /// The window's answer to each event decides the effect, whatever
    /// the keys say: copy where it answers non-zero, none elsewhere.
    answering: bool,
}

impl DropTarget {
    /// A target that tells `on_drag` each event and answers a drag of
    /// files with copy.
    pub fn new(own: Rc<Cell<bool>>, on_drag: OnDrag) -> Self {
        Self {
            on_drag,
            has_files: Cell::new(false),
            own,
            answering: false,
        }
    }

    /// A target whose `on_drag` answers each enter, over and drop:
    /// non-zero for a copy, zero for no drop. A move is never answered, so
    /// the source always stays where it is.
    pub fn answering(own: Rc<Cell<bool>>, on_drag: OnDrag) -> Self {
        Self {
            answering: true,
            ..Self::new(own, on_drag)
        }
    }

    fn send(&self, kind: DragKind, ev: &DragEvent) -> Option<LRESULT> {
        (self.on_drag)(kind, ev)
    }

    /// The effect for an event the window was told: copy for files, or
    /// with `answering`, what the window answered. A busy window takes
    /// no drop.
    fn effect(&self, allowed: DROPEFFECT, reply: Option<LRESULT>) -> DROPEFFECT {
        match reply {
            None => DROPEFFECT_NONE,
            Some(reply) if self.answering => answer(allowed, reply),
            Some(_) => DROPEFFECT_COPY,
        }
    }
}

/// Copy when the window said yes and the source allows a copy; otherwise
/// no drop. Never a move or a link, whatever keys are held.
fn answer(allowed: DROPEFFECT, reply: LRESULT) -> DROPEFFECT {
    if reply.0 != 0 && allowed.0 & DROPEFFECT_COPY.0 != 0 {
        DROPEFFECT_COPY
    } else {
        DROPEFFECT_NONE
    }
}

/// The effects the source allows, as OLE passes them in.
fn allowed(effect: *mut DROPEFFECT) -> DROPEFFECT {
    if effect.is_null() {
        DROPEFFECT_NONE
    } else {
        unsafe { *effect }
    }
}

/// Whether the source allows a copy, which is the only drop the floating
/// wheel answers.
fn allows_copy(effect: *mut DROPEFFECT) -> bool {
    allowed(effect).0 & DROPEFFECT_COPY.0 != 0
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

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(
        &self,
        data: Ref<IDataObject>,
        keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        // Kiln's own drag: refused, and the window hears nothing of it.
        let files = !self.own.get() && data.as_ref().is_some_and(offers_files);
        self.has_files.set(files);
        if !files {
            set_effect(effect, DROPEFFECT_NONE);
            return Ok(());
        }
        // Read the paths now, so the wheel can offer the right items
        // before anything is dropped.
        let ev = DragEvent {
            keys,
            x: pt.x,
            y: pt.y,
            paths: data.as_ref().map(dropped_paths).unwrap_or_default(),
            copy: allows_copy(effect),
        };
        let reply = self.send(DragKind::Enter, &ev);
        set_effect(effect, self.effect(allowed(effect), reply));
        Ok(())
    }

    fn DragOver(
        &self,
        keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        if !self.has_files.get() || self.own.get() {
            set_effect(effect, DROPEFFECT_NONE);
            return Ok(());
        }
        let ev = DragEvent {
            keys,
            x: pt.x,
            y: pt.y,
            paths: vec![],
            copy: allows_copy(effect),
        };
        let reply = self.send(DragKind::Over, &ev);
        set_effect(effect, self.effect(allowed(effect), reply));
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        if self.has_files.replace(false) {
            let ev = DragEvent {
                keys: MODIFIERKEYS_FLAGS(0),
                x: 0,
                y: 0,
                paths: vec![],
                copy: false,
            };
            self.send(DragKind::Leave, &ev);
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
        if self.own.get() {
            set_effect(effect, DROPEFFECT_NONE);
            return Ok(());
        }
        let paths = data.as_ref().map(dropped_paths).unwrap_or_default();
        let files = !paths.is_empty();
        let ev = DragEvent {
            keys,
            x: pt.x,
            y: pt.y,
            paths,
            copy: allows_copy(effect),
        };
        let reply = self.send(DragKind::Drop, &ev);
        let answered = if files {
            self.effect(allowed(effect), reply)
        } else {
            DROPEFFECT_NONE
        };
        set_effect(effect, answered);
        Ok(())
    }
}
