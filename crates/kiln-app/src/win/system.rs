//! Small pieces of Windows the app leans on: the clipboard, the app-mode
//! setting, and the folder and file pickers.

use std::path::PathBuf;

use windows::core::{w, Result, HSTRING, PCWSTR};
use windows::Win32::Foundation::{GlobalFree, HANDLE, HWND};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FileOpenDialog, IFileOpenDialog, FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS,
    SIGDN_FILESYSPATH,
};

/// Put `text` on the clipboard.
pub fn set_clipboard(hwnd: HWND, text: &str) -> Result<()> {
    let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    unsafe {
        OpenClipboard(Some(hwnd))?;
        let result = (|| {
            EmptyClipboard()?;
            let mem = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2)?;
            let dst = GlobalLock(mem) as *mut u16;
            if dst.is_null() {
                let _ = GlobalFree(Some(mem));
                return Err(windows::core::Error::from_thread());
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), dst, wide.len());
            let _ = GlobalUnlock(mem);
            // On success the clipboard owns the memory.
            if let Err(e) = SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(mem.0))) {
                let _ = GlobalFree(Some(mem));
                return Err(e);
            }
            Ok(())
        })();
        let _ = CloseClipboard();
        result
    }
}

/// Whether Windows is set to light mode for apps. Older systems without
/// the setting count as dark, Kiln's default.
pub fn system_light() -> bool {
    let mut value: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let err = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    err.is_ok() && value == 1
}

/// Ask for a folder. `None` if the person cancels.
pub fn pick_folder(hwnd: HWND, title: &str) -> Option<PathBuf> {
    pick(hwnd, title, true)
}

/// Ask for a program (.exe). `None` if the person cancels.
pub fn pick_program(hwnd: HWND, title: &str) -> Option<PathBuf> {
    pick(hwnd, title, false)
}

fn pick(hwnd: HWND, title: &str, folder: bool) -> Option<PathBuf> {
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let mut opts = dialog.GetOptions().ok()? | FOS_FORCEFILESYSTEM;
        if folder {
            opts |= FOS_PICKFOLDERS;
        } else {
            opts |= FOS_FILEMUSTEXIST;
            let filters = [COMDLG_FILTERSPEC {
                pszName: w!("Programs"),
                pszSpec: w!("*.exe"),
            }];
            dialog.SetFileTypes(&filters).ok()?;
        }
        dialog.SetOptions(opts).ok()?;
        let title = HSTRING::from(title);
        dialog.SetTitle(PCWSTR(title.as_ptr())).ok()?;
        // Cancelling comes back as an error; either way there's no path.
        dialog.Show(Some(hwnd)).ok()?;
        let item = dialog.GetResult().ok()?;
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(name.0 as *const _));
        path
    }
}
