//! Dragging held results out of Kiln, into Explorer, onto the desktop or
//! into another app. The shell builds the data object and runs the drag,
//! so the drag image, the cursor and every format a drop target expects
//! come from Windows.

use std::mem::ManuallyDrop;
use std::path::PathBuf;

use windows::core::{Error, Interface, Result, HSTRING};
use windows::Win32::Foundation::{GlobalFree, HWND};
use windows::Win32::System::Com::{
    CoTaskMemFree, IBindCtx, IDataObject, DVASPECT_CONTENT, FORMATETC, STGMEDIUM, STGMEDIUM_0,
    TYMED_HGLOBAL,
};
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::{IDropSource, DROPEFFECT_COPY, DROPEFFECT_MOVE};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    BHID_DataObject, IDataObjectAsyncCapability, IShellItemArray,
    SHCreateShellItemArrayFromIDLists, SHDoDragDrop, SHParseDisplayName, CFSTR_PREFERREDDROPEFFECT,
};

/// Absolute PIDLs from `SHParseDisplayName`, each freed when this goes,
/// whichever way the drag went.
struct Pidls(Vec<*mut ITEMIDLIST>);

impl Drop for Pidls {
    fn drop(&mut self) {
        for &p in &self.0 {
            unsafe { CoTaskMemFree(Some(p as *const _)) };
        }
    }
}

/// Drag `files` out of the window, Move preferred and Copy allowed, and
/// return once the drag is over. The drop target finishes its work first:
/// asynchronous mode is off. Whether a file was moved is for the caller to
/// see, by whether it's still there: Explorer's own move reports no effect.
/// A drop and a cancelled drag are both `Ok`.
pub fn drag_files(hwnd: HWND, files: &[PathBuf]) -> Result<()> {
    let mut pidls = Pidls(Vec::with_capacity(files.len()));
    for f in files {
        let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        let parsed = unsafe {
            SHParseDisplayName(
                &HSTRING::from(f.as_os_str()),
                None::<&IBindCtx>,
                &mut pidl,
                0,
                None,
            )
        };
        if !pidl.is_null() {
            pidls.0.push(pidl);
        }
        parsed?;
    }
    let refs: Vec<*const ITEMIDLIST> = pidls.0.iter().map(|&p| p as *const _).collect();
    let items: IShellItemArray = unsafe { SHCreateShellItemArrayFromIDLists(&refs)? };
    let data: IDataObject = unsafe { items.BindToHandler(None::<&IBindCtx>, &BHID_DataObject)? };
    prefer_move(&data)?;
    // Without the interface the data object is synchronous already.
    if let Ok(caps) = data.cast::<IDataObjectAsyncCapability>() {
        unsafe { caps.SetAsyncMode(false)? };
    }
    unsafe {
        SHDoDragDrop(
            Some(hwnd),
            &data,
            None::<&IDropSource>,
            DROPEFFECT_MOVE | DROPEFFECT_COPY,
        )?
    };
    Ok(())
}

/// Tell drop targets a move is preferred (`CFSTR_PREFERREDDROPEFFECT`).
fn prefer_move(data: &IDataObject) -> Result<()> {
    let format = unsafe { RegisterClipboardFormatW(CFSTR_PREFERREDDROPEFFECT) };
    if format == 0 {
        return Err(Error::from_thread());
    }
    let etc = FORMATETC {
        cfFormat: format as u16,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE, std::mem::size_of::<u32>())?;
        let dst = GlobalLock(mem) as *mut u32;
        if dst.is_null() {
            let _ = GlobalFree(Some(mem));
            return Err(Error::from_thread());
        }
        dst.write_unaligned(DROPEFFECT_MOVE.0);
        let _ = GlobalUnlock(mem);
        let medium = STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: STGMEDIUM_0 { hGlobal: mem },
            pUnkForRelease: ManuallyDrop::new(None),
        };
        // With fRelease, the data object owns the memory once it's taken
        // it; until then it's still Kiln's to free.
        if let Err(e) = data.SetData(&etc, &medium, true) {
            let _ = GlobalFree(Some(mem));
            return Err(e);
        }
    }
    Ok(())
}
