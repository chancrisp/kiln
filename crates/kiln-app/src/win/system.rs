//! Small pieces of Windows the app leans on: the clipboard, the app-mode
//! setting, the display settings (contrast themes, Text size, Animation
//! effects), the file, folder, program and save pickers, the leave prompt,
//! the right-click menu, Explorer, and the shutdown block reason.

use std::path::{Path, PathBuf};

use windows::core::{s, w, Result, BOOL, HRESULT, HSTRING, PCWSTR};
use windows::Win32::Foundation::{GlobalFree, HANDLE, HWND};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    SetClipboardData,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::{CF_HDROP, CF_UNICODETEXT};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::System::Shutdown::{ShutdownBlockReasonCreate, ShutdownBlockReasonDestroy};
use windows::Win32::UI::Controls::{
    TASKDIALOGCONFIG, TASKDIALOG_BUTTON, TDF_ALLOW_DIALOG_CANCELLATION,
    TDF_POSITION_RELATIVE_TO_WINDOW,
};
use windows::Win32::UI::Shell::Common::{COMDLG_FILTERSPEC, ITEMIDLIST};
use windows::Win32::UI::Shell::{
    FileOpenDialog, FileSaveDialog, IFileDialog, IFileOpenDialog, IFileSaveDialog,
    ILCreateFromPathW, ILFree, IShellItem, SHCreateItemFromParsingName, SHOpenFolderAndSelectItems,
    ShellExecuteW, FOS_ALLOWMULTISELECT, FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM,
    FOS_NOREADONLYRETURN, FOS_OVERWRITEPROMPT, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, HDROP,
    SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, MessageBoxW, TrackPopupMenuEx, IDCANCEL, IDOK,
    MB_ICONWARNING, MB_OKCANCEL, MF_SEPARATOR, MF_STRING, SW_SHOWNORMAL, TPM_LEFTALIGN,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, TPM_TOPALIGN,
};

use crate::model::{Prompt, PromptButton};
use crate::settings::Engine;

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

/// Whether the clipboard holds files, as File Explorer's Copy leaves them.
pub fn clipboard_has_files() -> bool {
    unsafe { IsClipboardFormatAvailable(CF_HDROP.0 as u32).is_ok() }
}

/// The files on the clipboard, if it holds any. Another program can have
/// the clipboard open for a moment, so opening it is tried twice.
pub fn clipboard_files(hwnd: HWND) -> Vec<PathBuf> {
    if !clipboard_has_files() {
        return vec![];
    }
    unsafe {
        if OpenClipboard(Some(hwnd)).is_err() {
            std::thread::sleep(std::time::Duration::from_millis(20));
            if OpenClipboard(Some(hwnd)).is_err() {
                return vec![];
            }
        }
        // The clipboard owns the data; it's only read here.
        let files = match GetClipboardData(CF_HDROP.0 as u32) {
            Ok(h) if !h.is_invalid() => super::drop::hdrop_paths(HDROP(h.0)),
            _ => vec![],
        };
        let _ = CloseClipboard();
        files
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

/// Whether Windows' Animation effects are on
/// (`SPI_GETCLIENTAREAANIMATION`). If it can't be read, they count as on,
/// as Windows' default.
pub fn animations_on() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    };
    let mut on = BOOL(1);
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut on as *mut BOOL as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    read.is_err() || on.as_bool()
}

/// The contrast theme's colours, while one is on (`SPI_GETHIGHCONTRAST`
/// with `HCF_HIGHCONTRASTON`). Only reads; Kiln never changes it.
pub fn high_contrast() -> Option<crate::theme::SysColors> {
    use windows::Win32::Graphics::Gdi::{
        GetSysColor, COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT, COLOR_HOTLIGHT,
        COLOR_WINDOW, COLOR_WINDOWTEXT, SYS_COLOR_INDEX,
    };
    use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
    use windows::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, SPI_GETHIGHCONTRAST, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    };
    let mut hc = HIGHCONTRASTW {
        cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
    unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            hc.cbSize,
            Some(&mut hc as *mut HIGHCONTRASTW as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .ok()?;
    }
    if !hc.dwFlags.contains(HCF_HIGHCONTRASTON) {
        return None;
    }
    // COLORREF is 0x00BBGGRR.
    let sys = |i: SYS_COLOR_INDEX| {
        let c = unsafe { GetSysColor(i) };
        crate::theme::Rgb(c as u8, (c >> 8) as u8, (c >> 16) as u8)
    };
    Some(crate::theme::SysColors {
        window: sys(COLOR_WINDOW),
        window_text: sys(COLOR_WINDOWTEXT),
        highlight: sys(COLOR_HIGHLIGHT),
        highlight_text: sys(COLOR_HIGHLIGHTTEXT),
        gray_text: sys(COLOR_GRAYTEXT),
        hotlight: sys(COLOR_HOTLIGHT),
    })
}

/// Windows' Text size (Settings > Accessibility > Text size), read through
/// `UISettings.TextScaleFactor`, and told when it changes.
pub struct TextScale {
    settings: Option<windows::UI::ViewManagement::UISettings>,
    token: Option<i64>,
}

impl TextScale {
    /// Start listening. `changed` runs on a thread pool thread whenever
    /// the factor changes, so it should only post a message to the window.
    /// If WinRT isn't there, the factor stays 1.0.
    pub fn new(changed: impl Fn() + Send + 'static) -> Self {
        use windows::Foundation::TypedEventHandler;
        use windows::UI::ViewManagement::UISettings;
        let Ok(settings) = UISettings::new() else {
            return Self {
                settings: None,
                token: None,
            };
        };
        let handler =
            TypedEventHandler::<UISettings, windows::core::IInspectable>::new(move |_, _| {
                changed();
                Ok(())
            });
        let token = settings.TextScaleFactorChanged(&handler).ok();
        Self {
            settings: Some(settings),
            token,
        }
    }

    /// The factor now: 1.0 to 2.25, or 1.0 if it can't be read.
    pub fn factor(&self) -> f32 {
        self.settings
            .as_ref()
            .and_then(|s| s.TextScaleFactor().ok())
            .map_or(1.0, |f| f as f32)
    }
}

impl Drop for TextScale {
    fn drop(&mut self) {
        if let (Some(s), Some(t)) = (&self.settings, self.token) {
            let _ = s.RemoveTextScaleFactorChanged(t);
        }
    }
}

/// Ask for a folder. `None` if the person cancels.
pub fn pick_folder(hwnd: HWND, title: &str) -> Option<PathBuf> {
    pick(hwnd, title, true, None)
}

/// Ask for a folder, starting in `start`. `None` if the person cancels.
pub fn pick_folder_in(hwnd: HWND, title: &str, start: &Path) -> Option<PathBuf> {
    pick(hwnd, title, true, Some(start))
}

/// Ask for a program (.exe). `None` if the person cancels.
pub fn pick_program(hwnd: HWND, title: &str) -> Option<PathBuf> {
    pick(hwnd, title, false, None)
}

fn pick(hwnd: HWND, title: &str, folder: bool, start: Option<&Path>) -> Option<PathBuf> {
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
        if let Some(start) = start {
            start_in(&dialog, start);
        }
        // Cancelling comes back as an error; either way there's no path.
        dialog.Show(Some(hwnd)).ok()?;
        let item = dialog.GetResult().ok()?;
        item_path(&item)
    }
}

/// Ask for files to add, any number at once, starting in `start` when
/// given; otherwise Windows picks the folder. Empty if the person cancels.
pub fn pick_files(hwnd: HWND, title: &str, start: Option<&Path>) -> Vec<PathBuf> {
    let picked = || -> Option<Vec<PathBuf>> {
        unsafe {
            let dialog: IFileOpenDialog =
                CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
            let opts = dialog.GetOptions().ok()?
                | FOS_ALLOWMULTISELECT
                | FOS_FILEMUSTEXIST
                | FOS_FORCEFILESYSTEM;
            dialog.SetOptions(opts).ok()?;
            let title = HSTRING::from(title);
            dialog.SetTitle(PCWSTR(title.as_ptr())).ok()?;
            if let Some(start) = start {
                start_in(&dialog, start);
            }
            // Cancelling comes back as an error; either way there's nothing.
            dialog.Show(Some(hwnd)).ok()?;
            let items = dialog.GetResults().ok()?;
            let count = items.GetCount().ok()?;
            Some(
                (0..count)
                    .filter_map(|i| item_path(&items.GetItemAt(i).ok()?))
                    .collect(),
            )
        }
    };
    picked().unwrap_or_default()
}

/// Show a right-click menu at `(x, y)` on the screen. `None` in `items`
/// is a separator. Returns the index of the item chosen, or `None` when
/// the menu was dismissed.
pub fn popup_menu(hwnd: HWND, items: &[Option<String>], x: i32, y: i32) -> Option<usize> {
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        // Kept alive until the menu has closed.
        let texts: Vec<Option<HSTRING>> = items
            .iter()
            .map(|t| t.as_deref().map(HSTRING::from))
            .collect();
        for (k, text) in texts.iter().enumerate() {
            let _ = match text {
                // Ids start at 1: 0 means nothing was chosen.
                Some(t) => AppendMenuW(menu, MF_STRING, k + 1, PCWSTR(t.as_ptr())),
                None => AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()),
            };
        }
        let chosen = TrackPopupMenuEx(
            menu,
            (TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_LEFTALIGN | TPM_TOPALIGN).0,
            x,
            y,
            hwnd,
            None,
        );
        let _ = DestroyMenu(menu);
        (chosen.0 as usize).checked_sub(1)
    }
}

/// Ask where to save `name`, a file of type `ext`, starting in `start`.
/// Windows asks before replacing a file that's already there. `None` if
/// the person cancels.
pub fn save_as(hwnd: HWND, title: &str, start: &Path, name: &str, ext: &str) -> Option<PathBuf> {
    unsafe {
        let dialog: IFileSaveDialog =
            CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let opts = dialog.GetOptions().ok()?
            | FOS_FORCEFILESYSTEM
            | FOS_OVERWRITEPROMPT
            | FOS_PATHMUSTEXIST
            | FOS_NOREADONLYRETURN;
        dialog.SetOptions(opts).ok()?;
        let title = HSTRING::from(title);
        dialog.SetTitle(PCWSTR(title.as_ptr())).ok()?;
        // Kept alive until the dialog has closed.
        let kind = HSTRING::from(ext.to_uppercase());
        let spec = HSTRING::from(format!("*.{ext}"));
        let ext_w = HSTRING::from(ext);
        if !ext.is_empty() {
            let filters = [COMDLG_FILTERSPEC {
                pszName: PCWSTR(kind.as_ptr()),
                pszSpec: PCWSTR(spec.as_ptr()),
            }];
            dialog.SetFileTypes(&filters).ok()?;
            dialog.SetDefaultExtension(PCWSTR(ext_w.as_ptr())).ok()?;
        }
        let name = HSTRING::from(name);
        dialog.SetFileName(PCWSTR(name.as_ptr())).ok()?;
        start_in(&dialog, start);
        dialog.Show(Some(hwnd)).ok()?;
        let item = dialog.GetResult().ok()?;
        item_path(&item)
    }
}

/// Open the dialog in `folder`, if it's there.
fn start_in(dialog: &IFileDialog, folder: &Path) {
    if folder.as_os_str().is_empty() {
        return;
    }
    let path = HSTRING::from(folder.as_os_str());
    unsafe {
        if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&path, None) {
            let _ = dialog.SetFolder(&item);
        }
    }
}

fn item_path(item: &IShellItem) -> Option<PathBuf> {
    unsafe {
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(name.0 as *const _));
        path
    }
}

/// The first button id of the leave prompt. Cancel uses IDCANCEL, so
/// Escape and the dialog's close button mean Cancel too.
const PROMPT_FIRST_ID: i32 = 100;

/// Ask the leave prompt in a Windows task dialog, which screen readers
/// read. Returns the button chosen; closing the dialog is Cancel.
pub fn ask(hwnd: HWND, prompt: &Prompt) -> PromptButton {
    let id = |k: usize, b: PromptButton| match b {
        PromptButton::Cancel => IDCANCEL.0,
        _ => PROMPT_FIRST_ID + k as i32,
    };
    let labels: Vec<HSTRING> = prompt
        .buttons
        .iter()
        .map(|b| HSTRING::from(b.label()))
        .collect();
    let buttons: Vec<TASKDIALOG_BUTTON> = prompt
        .buttons
        .iter()
        .enumerate()
        .map(|(k, &b)| TASKDIALOG_BUTTON {
            nButtonID: id(k, b),
            pszButtonText: PCWSTR(labels[k].as_ptr()),
        })
        .collect();
    // Export all is safe to land on. A lone Stop is not, so Cancel is.
    let default = match prompt.buttons.first() {
        Some(&b @ PromptButton::ExportAll) => id(0, b),
        _ => IDCANCEL.0,
    };
    let title = HSTRING::from(prompt.title.as_str());
    let config = TASKDIALOGCONFIG {
        cbSize: std::mem::size_of::<TASKDIALOGCONFIG>() as u32,
        hwndParent: hwnd,
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION | TDF_POSITION_RELATIVE_TO_WINDOW,
        pszWindowTitle: w!("Kiln"),
        pszMainInstruction: PCWSTR(title.as_ptr()),
        cButtons: buttons.len() as u32,
        pButtons: buttons.as_ptr(),
        nDefaultButton: default,
        ..Default::default()
    };
    let chosen = task_dialog(&config).unwrap_or_else(|| {
        // Without the task dialog (a build missing Kiln's manifest), a
        // plain OK or Cancel, where OK takes the first choice.
        let r =
            unsafe { MessageBoxW(Some(hwnd), &title, w!("Kiln"), MB_OKCANCEL | MB_ICONWARNING) };
        match prompt.buttons.first() {
            Some(&b) if r == IDOK => id(0, b),
            _ => IDCANCEL.0,
        }
    });
    prompt
        .buttons
        .iter()
        .enumerate()
        .find(|&(k, &b)| id(k, b) == chosen)
        .map_or(PromptButton::Cancel, |(_, &b)| b)
}

/// Show `config` and return the button id. Looked up at run time: the
/// task dialog lives only in Common Controls 6, which Kiln's manifest asks
/// for, and a test binary without the manifest must still load.
fn task_dialog(config: &TASKDIALOGCONFIG) -> Option<i32> {
    type TaskDialogIndirect = unsafe extern "system" fn(
        *const TASKDIALOGCONFIG,
        *mut i32,
        *mut i32,
        *mut BOOL,
    ) -> HRESULT;
    unsafe {
        let module = LoadLibraryW(w!("comctl32.dll")).ok()?;
        let f = GetProcAddress(module, s!("TaskDialogIndirect"))?;
        let f: TaskDialogIndirect = std::mem::transmute(f);
        let mut button = 0;
        f(
            config,
            &mut button,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
        .ok()
        .ok()?;
        Some(button)
    }
}

/// Open Explorer at `folder` with `files` selected.
pub fn show_in_folder(folder: &Path, files: &[PathBuf]) -> Result<()> {
    unsafe {
        let dir = ILCreateFromPathW(&HSTRING::from(folder.as_os_str()));
        if dir.is_null() {
            return Err(windows::core::Error::from_thread());
        }
        let items: Vec<*mut ITEMIDLIST> = files
            .iter()
            .map(|f| ILCreateFromPathW(&HSTRING::from(f.as_os_str())))
            .filter(|p| !p.is_null())
            .collect();
        let refs: Vec<*const ITEMIDLIST> = items.iter().map(|&p| p as *const _).collect();
        let result = SHOpenFolderAndSelectItems(dir, Some(&refs), 0);
        for p in items {
            ILFree(Some(p));
        }
        ILFree(Some(dir));
        result
    }
}

/// Open `file` in its usual app. False if Windows couldn't.
pub fn open_file(hwnd: HWND, file: &Path) -> bool {
    let path = HSTRING::from(file.as_os_str());
    let r = unsafe { ShellExecuteW(Some(hwnd), w!("open"), &path, None, None, SW_SHOWNORMAL) };
    // Anything above 32 is success.
    r.0 as isize > 32
}

/// Open `engine`'s official download page in the browser. It takes the
/// engine, not an address, so Get it can only ever open one of the four
/// fixed pages. False if Windows couldn't.
pub fn open_download_page(hwnd: HWND, engine: Engine) -> bool {
    let url = HSTRING::from(engine.download_page());
    let r = unsafe { ShellExecuteW(Some(hwnd), w!("open"), &url, None, None, SW_SHOWNORMAL) };
    r.0 as isize > 32
}

/// Tell Windows why it shouldn't shut down yet, or (`None`) that nothing
/// is at stake any more.
pub fn shutdown_reason(hwnd: HWND, reason: Option<&str>) {
    unsafe {
        let _ = match reason {
            Some(text) => ShutdownBlockReasonCreate(hwnd, &HSTRING::from(text)),
            None => ShutdownBlockReasonDestroy(hwnd),
        };
    }
}
