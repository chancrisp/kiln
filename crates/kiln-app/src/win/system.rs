//! Small pieces of Windows the app leans on: the clipboard, the app-mode
//! setting, the display settings (contrast themes, Text size, Animation
//! effects), the file, folder, program and save pickers, the leave prompt,
//! the right-click menu, Explorer, the shutdown block reason, and what the
//! log and crash files need to know of Windows.

use std::cell::Cell;
use std::path::{Path, PathBuf};

use windows::core::{s, w, Result, BOOL, HRESULT, HSTRING, PCWSTR};
use windows::Win32::Foundation::{GlobalFree, HANDLE, HWND, LPARAM};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    SetClipboardData,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::{CF_HDROP, CF_UNICODETEXT};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};

use windows::Win32::UI::Controls::{
    TASKDIALOGCONFIG, TASKDIALOG_BUTTON, TDF_ALLOW_DIALOG_CANCELLATION,
    TDF_POSITION_RELATIVE_TO_WINDOW,
};
use windows::Win32::UI::Shell::Common::{COMDLG_FILTERSPEC, ITEMIDLIST};
use windows::Win32::UI::Shell::{
    FileOpenDialog, IFileDialog, IFileOpenDialog, ILCreateFromPathW, ILFree, IShellItem,
    SHCreateItemFromParsingName, SHOpenFolderAndSelectItems, ShellExecuteW, FOS_ALLOWMULTISELECT,
    FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, HDROP, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, IDCANCEL, IDOK, MB_ICONWARNING, MB_OKCANCEL, SW_SHOWNORMAL,
};

use crate::model::{Prompt, PromptButton};
use crate::settings::Engine;

thread_local! {
    /// How many of Kiln's modal dialogs and menus are open on this thread.
    static MODALS: Cell<u32> = const { Cell::new(0) };
    /// How many of Kiln's prompts are open, or waiting to open, inside its
    /// window (`surfaces::Ask`). They run no loop of their own.
    static IN_WINDOW: Cell<u32> = const { Cell::new(0) };
}

/// Whether a dialog, picker, prompt or menu of Kiln's is open, the
/// prompts inside the window too. Its modal loop still delivers the
/// window's messages, so anything that would start work of its own waits
/// until it closes.
pub fn modal_open() -> bool {
    MODALS.with(Cell::get) > 0 || IN_WINDOW.with(Cell::get) > 0
}

/// Whether a modal loop of Kiln's is running: a dialog, picker or menu of
/// Windows', not a prompt inside the window, which the window's own loop
/// draws and answers.
pub fn modal_loop_open() -> bool {
    MODALS.with(Cell::get) > 0
}

/// Counts a prompt inside the window as open for as long as it lives.
pub(super) struct InWindow;

impl InWindow {
    pub(super) fn open() -> Self {
        IN_WINDOW.with(|m| m.set(m.get() + 1));
        InWindow
    }
}

impl Drop for InWindow {
    fn drop(&mut self) {
        IN_WINDOW.with(|m| m.set(m.get().saturating_sub(1)));
    }
}

/// The window's own modal loops, moving or sizing it and its system menu,
/// count too: they deliver its messages the same way. `open` is the ENTER
/// message, `!open` the matching EXIT.
pub fn window_loop(open: bool) {
    MODALS.with(|m| {
        m.set(match open {
            true => m.get() + 1,
            false => m.get().saturating_sub(1),
        })
    });
}

/// Counts a modal as open for as long as it lives.
pub(super) struct Modal;

impl Modal {
    pub(super) fn open() -> Self {
        MODALS.with(|m| m.set(m.get() + 1));
        Modal
    }
}

impl Drop for Modal {
    fn drop(&mut self) {
        MODALS.with(|m| m.set(m.get().saturating_sub(1)));
    }
}

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

/// The text on the clipboard, if it holds any; empty otherwise. Another
/// program can have the clipboard open for a moment, so opening it is
/// tried twice.
pub fn clipboard_text(hwnd: HWND) -> String {
    // SAFETY: the clipboard is opened and closed here; the handle it
    // gives stays the clipboard's, read between GlobalLock and
    // GlobalUnlock up to its first NUL, never past its size.
    unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32).is_err() {
            return String::new();
        }
        if OpenClipboard(Some(hwnd)).is_err() {
            std::thread::sleep(std::time::Duration::from_millis(20));
            if OpenClipboard(Some(hwnd)).is_err() {
                return String::new();
            }
        }
        let text = match GetClipboardData(CF_UNICODETEXT.0 as u32) {
            Ok(h) if !h.is_invalid() => {
                let mem = windows::Win32::Foundation::HGLOBAL(h.0);
                let size = windows::Win32::System::Memory::GlobalSize(mem) / 2;
                let p = GlobalLock(mem) as *const u16;
                if p.is_null() {
                    String::new()
                } else {
                    let all = std::slice::from_raw_parts(p, size);
                    let end = all.iter().position(|&c| c == 0).unwrap_or(all.len());
                    let s = String::from_utf16_lossy(&all[..end]);
                    let _ = GlobalUnlock(mem);
                    s
                }
            }
            _ => String::new(),
        };
        let _ = CloseClipboard();
        text
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

/// The colours of Windows' default High Contrast Black theme, as
/// `GetSysColor` returns them in it. What `KILN_TEST_CONTRAST=1` stands in
/// for the real theme's: window and button face black, window and button
/// text white, highlight cyan with black text, gray text green, hot light
/// yellow.
const HIGH_CONTRAST_BLACK: crate::theme::SysColors = {
    use crate::theme::Rgb;
    crate::theme::SysColors {
        window: Rgb::hex(0x000000),
        window_text: Rgb::hex(0xFFFFFF),
        highlight: Rgb::hex(0x1AEBFF),
        highlight_text: Rgb::hex(0x000000),
        gray_text: Rgb::hex(0x3FF23F),
        hotlight: Rgb::hex(0xFFFF00),
        button_face: Rgb::hex(0x000000),
        button_text: Rgb::hex(0xFFFFFF),
    }
};

/// Whether `KILN_TEST_CONTRAST` has this value: only `1` turns it on.
fn test_contrast_on(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|v| v == "1")
}

/// Whether Kiln started with `KILN_TEST_CONTRAST=1`, a test hook that draws
/// the app in High Contrast Black without changing Windows' settings. Read
/// once, the first time the contrast theme is asked for, at start.
fn test_contrast() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    // Read only by a debug or test-hooks build; a shipped one reads it as unset.
    #[cfg(any(debug_assertions, feature = "test-hooks"))]
    {
        *ON.get_or_init(|| test_contrast_on(std::env::var_os("KILN_TEST_CONTRAST").as_deref()))
    }
    #[cfg(not(any(debug_assertions, feature = "test-hooks")))]
    {
        *ON.get_or_init(|| test_contrast_on(None))
    }
}

/// The contrast theme's colours, while one is on (`SPI_GETHIGHCONTRAST`
/// with `HCF_HIGHCONTRASTON`). Only reads; Kiln never changes it. With
/// `KILN_TEST_CONTRAST=1` it is always High Contrast Black's.
pub fn high_contrast() -> Option<crate::theme::SysColors> {
    use windows::Win32::Graphics::Gdi::{
        GetSysColor, COLOR_BTNFACE, COLOR_BTNTEXT, COLOR_GRAYTEXT, COLOR_HIGHLIGHT,
        COLOR_HIGHLIGHTTEXT, COLOR_HOTLIGHT, COLOR_WINDOW, COLOR_WINDOWTEXT, SYS_COLOR_INDEX,
    };
    use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
    use windows::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, SPI_GETHIGHCONTRAST, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    };
    if test_contrast() {
        return Some(HIGH_CONTRAST_BLACK);
    }
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
        button_face: sys(COLOR_BTNFACE),
        button_text: sys(COLOR_BTNTEXT),
    })
}

/// The factor `KILN_TEST_TEXT_SCALE` names (for example `1.5`): a number
/// from 1.0 to 2.25, the range Windows allows. Anything else is no factor.
#[cfg_attr(not(any(debug_assertions, feature = "test-hooks")), allow(dead_code))]
fn parse_text_scale(value: Option<&std::ffi::OsStr>) -> Option<f32> {
    let f: f32 = value?.to_str()?.trim().parse().ok()?;
    (f.is_finite() && (1.0..=crate::layout::MAX_TEXT_SCALE).contains(&f)).then_some(f)
}

/// A test hook that stands in for Windows' Text size without changing it,
/// read once. Off in a build without debug assertions or `test-hooks`.
#[cfg(any(debug_assertions, feature = "test-hooks"))]
fn test_text_scale() -> Option<f32> {
    static SCALE: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    *SCALE.get_or_init(|| parse_text_scale(std::env::var_os("KILN_TEST_TEXT_SCALE").as_deref()))
}

#[cfg(not(any(debug_assertions, feature = "test-hooks")))]
fn test_text_scale() -> Option<f32> {
    None
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

    /// The factor now: 1.0 to 2.25, or 1.0 if it can't be read. With
    /// `KILN_TEST_TEXT_SCALE` set in a debug or `test-hooks` build, that
    /// value stands in for Windows' own.
    pub fn factor(&self) -> f32 {
        if let Some(f) = test_text_scale() {
            return f;
        }
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
    let _modal = Modal::open();
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
    let _modal = Modal::open();
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

/// Ask a prompt in a Windows task dialog, which screen readers read: only
/// for a window that can no longer draw, as prompts open inside the
/// window (`surfaces::Ask`). Returns the button chosen; closing the
/// dialog is Cancel.
pub fn ask(hwnd: HWND, prompt: &Prompt) -> PromptButton {
    let _modal = Modal::open();
    let id = |k: usize, b: PromptButton| match b {
        PromptButton::Cancel => IDCANCEL.0,
        _ => PROMPT_FIRST_ID + k as i32,
    };
    let labels: Vec<HSTRING> = prompt
        .buttons
        .iter()
        .map(|&b| HSTRING::from(prompt.label(b)))
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
    // Save all is safe to land on, and so is the first way to save what
    // a crashed run left. A lone Stop is not, so Cancel is.
    let default = match prompt.accent() {
        Some(k) => id(k, prompt.buttons[k]),
        None => IDCANCEL.0,
    };
    let title = HSTRING::from(prompt.dialog_title());
    let text = prompt.dialog_text().map(HSTRING::from);
    let config = TASKDIALOGCONFIG {
        cbSize: std::mem::size_of::<TASKDIALOGCONFIG>() as u32,
        hwndParent: hwnd,
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION | TDF_POSITION_RELATIVE_TO_WINDOW,
        pszWindowTitle: w!("Kiln"),
        pszMainInstruction: PCWSTR(title.as_ptr()),
        pszContent: text.as_ref().map_or(PCWSTR::null(), |t| PCWSTR(t.as_ptr())),
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

/// The `T` a message's `lparam` points at, borrowed; `None` for a null one.
///
/// # Safety
/// For this message, `lparam` must be null or point at a valid `T` that
/// stays valid and unchanged for `'a`, the length of the call that
/// handles the message.
pub(super) unsafe fn lparam_ref<'a, T>(lparam: LPARAM) -> Option<&'a T> {
    // SAFETY: the caller's promise.
    unsafe { (lparam.0 as *const T).as_ref() }
}

/// The `T` a message's `lparam` points at, to fill in; `None` for a null
/// one.
///
/// # Safety
/// For this message, `lparam` must be null or point at a valid `T` that
/// nothing else uses until the message is answered.
pub(super) unsafe fn lparam_mut<'a, T>(lparam: LPARAM) -> Option<&'a mut T> {
    // SAFETY: the caller's promise.
    unsafe { (lparam.0 as *mut T).as_mut() }
}

/// The text a message's `lparam` points at, as a string; `None` for a null
/// pointer or text that isn't valid UTF-16.
///
/// # Safety
/// For this message, `lparam` must be null or point at a NUL-ended UTF-16
/// string that stays valid for the call.
pub(super) unsafe fn lparam_text(lparam: LPARAM) -> Option<String> {
    let name = lparam.0 as *const u16;
    if name.is_null() {
        return None;
    }
    // SAFETY: checked above not to be null, and the caller's promise that
    // it is a NUL-ended string.
    unsafe { PCWSTR(name).to_string() }.ok()
}

/// The function `name` that `module` exports, as `F`. `None` when it has
/// none by that name.
///
/// # Safety
/// `F` must be an `unsafe extern "system" fn` type with the export's own
/// signature: calling it through any other is undefined.
unsafe fn proc_address<F: Copy>(
    module: windows::Win32::Foundation::HMODULE,
    name: windows::core::PCSTR,
) -> Option<F> {
    // A function pointer is what `GetProcAddress` gives, and what `F` is.
    const {
        assert!(std::mem::size_of::<F>() == std::mem::size_of::<usize>());
    }
    // SAFETY: `module` is a loaded module and `name` a C string; the
    // caller vouches that `F` is the export's type, and both are function
    // pointers of one size.
    unsafe { GetProcAddress(module, name).map(|f| std::mem::transmute_copy(&f)) }
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
    // SAFETY: `TaskDialogIndirect` is the export's own signature, from
    // its documentation. `config` outlives the call, which writes only
    // `button`.
    unsafe {
        let module = LoadLibraryW(w!("comctl32.dll")).ok()?;
        let f: TaskDialogIndirect = proc_address(module, s!("TaskDialogIndirect"))?;
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
/// The person's Documents folder, the known folder wherever it has been
/// moved to, or `None` if Windows can't say.
pub fn documents() -> Option<PathBuf> {
    use windows::Win32::UI::Shell::{FOLDERID_Documents, SHGetKnownFolderPath, KF_FLAG_DEFAULT};
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_Documents, KF_FLAG_DEFAULT, None).ok()?;
        let path = p.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(p.0 as *const _));
        path.filter(|p| p.is_absolute())
    }
}

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

/// Open the text file `file` in Notepad, which every Windows has unless
/// it was taken off; then in the file's usual app.
///
/// Notepad is named by its full path in the system folder: ShellExecute
/// looks for a bare name in the current folder too, which may be a
/// Downloads folder Kiln was started from by Open with.
pub fn open_text(hwnd: HWND, file: &Path) -> bool {
    let notepad = system_dir().map(|dir| notepad_command(&dir, file));
    notepad.is_some_and(|(program, args)| {
        shell_open(
            hwnd,
            &HSTRING::from(program.as_os_str()),
            Some(&HSTRING::from(args)),
        )
    }) || shell_open(hwnd, &HSTRING::from(file.as_os_str()), None)
}

/// Windows' system folder (`C:\Windows\System32`), from
/// `GetSystemDirectoryW`. None if Windows didn't give a full path.
fn system_dir() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
    let mut buf = vec![0u16; 260];
    loop {
        let n = unsafe { GetSystemDirectoryW(Some(&mut buf)) } as usize;
        if n == 0 {
            return None;
        }
        if n < buf.len() {
            let dir = PathBuf::from(std::ffi::OsString::from_wide(&buf[..n]));
            return dir.is_absolute().then_some(dir);
        }
        // Too small: `n` is the size needed, its NUL included.
        buf.resize(n, 0);
    }
}

/// The program and arguments that open `file` in Notepad, with Notepad
/// named by its full path in `system_dir`.
fn notepad_command(system_dir: &Path, file: &Path) -> (PathBuf, String) {
    (
        system_dir.join("notepad.exe"),
        format!("\"{}\"", file.display()),
    )
}

/// `ShellExecuteW`'s open of `what`, with `args`: true when it started.
fn shell_open(hwnd: HWND, what: &HSTRING, args: Option<&HSTRING>) -> bool {
    let args = args.map_or(PCWSTR::null(), |a| PCWSTR(a.as_ptr()));
    let r = unsafe { ShellExecuteW(Some(hwnd), w!("open"), what, args, None, SW_SHOWNORMAL) };
    // Anything above 32 is success.
    r.0 as isize > 32
}

/// Open `engine`'s official download page in the browser. It takes the
/// engine, not an address, so Get it can only ever open one of the fixed
/// pages. False if Windows couldn't, or the engine has no page.
pub fn open_download_page(hwnd: HWND, engine: Engine) -> bool {
    let Some(page) = engine.download_page() else {
        return false;
    };
    let url = HSTRING::from(page);
    let r = unsafe { ShellExecuteW(Some(hwnd), w!("open"), &url, None, None, SW_SHOWNORMAL) };
    r.0 as isize > 32
}

/// Run `winget install -e --id <id>` in a console window of its own, which
/// stays open to show winget's output. Nothing waits for it. False if the
/// console couldn't be started.
///
/// `cmd.exe` is named by its full path in the system folder, as Notepad is.
pub fn run_in_console(winget: &Path, id: &str) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    let Some(dir) = system_dir() else {
        return false;
    };
    std::process::Command::new(dir.join("cmd.exe"))
        .raw_arg(crate::settings::winget_console_args(winget, id))
        .current_dir(&dir)
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .is_ok()
}

/// The taskbar button's progress, through `ITaskbarList3`.
pub struct Taskbar(Option<windows::Win32::UI::Shell::ITaskbarList3>);

impl Taskbar {
    pub fn new() -> Self {
        use windows::Win32::UI::Shell::{ITaskbarList3, TaskbarList};
        let list: Option<ITaskbarList3> =
            unsafe { CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER) }.ok();
        if let Some(l) = &list {
            unsafe {
                let _ = l.HrInit();
            }
        }
        Taskbar(list)
    }

    /// Show `progress` on the window's taskbar button.
    pub fn set(&self, hwnd: HWND, progress: crate::model::TaskbarProgress) {
        use crate::model::TaskbarProgress;
        use windows::Win32::UI::Shell::{TBPF_INDETERMINATE, TBPF_NOPROGRESS, TBPF_NORMAL};
        let Some(l) = &self.0 else {
            return;
        };
        unsafe {
            let _ = match progress {
                TaskbarProgress::None => l.SetProgressState(hwnd, TBPF_NOPROGRESS),
                TaskbarProgress::Indeterminate => l.SetProgressState(hwnd, TBPF_INDETERMINATE),
                TaskbarProgress::Fraction(f) => {
                    let _ = l.SetProgressState(hwnd, TBPF_NORMAL);
                    l.SetProgressValue(hwnd, (f.clamp(0.0, 1.0) * 1000.0) as u64, 1000)
                }
            };
        }
    }
}

/// Whether the window is the one in front, and not minimised.
pub fn in_front(hwnd: HWND) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, IsIconic};
    unsafe { GetForegroundWindow() == hwnd && !IsIconic(hwnd).as_bool() }
}

/// Flash the window's taskbar button until the window is brought forward.
pub fn flash_until_front(hwnd: HWND) {
    use windows::Win32::UI::WindowsAndMessaging::{
        FlashWindowEx, FLASHWINFO, FLASHW_TIMERNOFG, FLASHW_TRAY,
    };
    let info = FLASHWINFO {
        cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
        hwnd,
        dwFlags: FLASHW_TRAY | FLASHW_TIMERNOFG,
        uCount: 3,
        dwTimeout: 0,
    };
    unsafe {
        let _ = FlashWindowEx(&info);
    }
}

/// The id of Kiln's one notification icon.
const NOTIFY_ID: u32 = 1;

pub(super) fn copy_wide<const N: usize>(into: &mut [u16; N], text: &str) {
    for (slot, c) in into.iter_mut().zip(text.encode_utf16().take(N - 1)) {
        *slot = c;
    }
}

/// Show a notification with `title` and `text`. It lives in the
/// notification area only while it shows; `callback` is the message the
/// window gets when it is clicked or goes away. Tests show nothing
/// (`App::notify_about`).
#[cfg_attr(test, allow(dead_code))]
pub fn notify(hwnd: HWND, callback: u32, title: &str, text: &str) {
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Shell::{
        Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD,
        NIM_DELETE, NOTIFYICONDATAW,
    };
    use windows::Win32::UI::WindowsAndMessaging::LoadIconW;
    let instance = unsafe { GetModuleHandleW(None) }.unwrap_or_default();
    let icon = unsafe { LoadIconW(Some(instance.into()), PCWSTR(1 as _)) }.unwrap_or_default();
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: NOTIFY_ID,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_INFO,
        uCallbackMessage: callback,
        hIcon: icon,
        dwInfoFlags: NIIF_INFO,
        ..Default::default()
    };
    copy_wide(&mut data.szTip, "Kiln");
    copy_wide(&mut data.szInfoTitle, title);
    copy_wide(&mut data.szInfo, text);
    unsafe {
        // One at a time: a new one replaces the old.
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
        let _ = Shell_NotifyIconW(NIM_ADD, &data);
    }
}

/// Take the notification icon away.
pub fn notify_done(hwnd: HWND) {
    use windows::Win32::UI::Shell::{Shell_NotifyIconW, NIM_DELETE, NOTIFYICONDATAW};
    let data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: NOTIFY_ID,
        ..Default::default()
    };
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
    }
}

/// Bring the window forward from where a notification was clicked.
pub fn bring_forward(hwnd: HWND) {
    use windows::Win32::UI::WindowsAndMessaging::{
        IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE,
    };
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, super::placement::show_cmd(SW_RESTORE));
        }
        // The test placement switch never takes the foreground.
        if !super::placement::quiet_on() {
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

/// What the log and crash files ask of Windows.
pub const DIAG: crate::diag::Platform = crate::diag::Platform {
    now: local_time,
    debug: debug_output,
    windows: windows_build,
    module_base,
};

/// The local time to the second.
fn local_time() -> crate::diag::LocalTime {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let t = unsafe { GetLocalTime() };
    (
        t.wYear,
        t.wMonth as u8,
        t.wDay as u8,
        t.wHour as u8,
        t.wMinute as u8,
        t.wSecond as u8,
    )
}

/// `kiln-app: {msg}` for an attached debugger.
fn debug_output(msg: &str) {
    use windows::Win32::System::Diagnostics::Debug::OutputDebugStringW;
    let line = HSTRING::from(format!("kiln-app: {msg}\n"));
    unsafe { OutputDebugStringW(&line) };
}

/// The Windows version and build, as major.minor.build.UBR
/// ("10.0.26300.1"). `RtlGetVersion` gives the real version whatever the
/// manifest says. The update number, UBR, is only in the registry, and is
/// left off when it can't be read.
pub fn windows_build() -> String {
    use windows::Win32::System::Registry::HKEY_LOCAL_MACHINE;
    let Some((major, minor, build)) = os_version() else {
        return "unknown".to_owned();
    };
    let mut version = format!("{major}.{minor}.{build}");
    let mut ubr: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let err = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion"),
            w!("UBR"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut ubr as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    if err.is_ok() {
        version.push_str(&format!(".{ubr}"));
    }
    version
}

/// Windows' edition as the registry names it ("Windows 11 Pro"), `None`
/// if it can't be read.
pub fn windows_edition() -> Option<String> {
    use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    let mut buf = [0u16; 128];
    let mut size = std::mem::size_of_val(&buf) as u32;
    let err = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion"),
            w!("ProductName"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
    };
    if err.is_err() {
        return None;
    }
    // `size` counts bytes, with the ending NUL.
    let chars = (size as usize / 2).saturating_sub(1).min(buf.len());
    let name = String::from_utf16_lossy(&buf[..chars]);
    (!name.is_empty()).then(|| crate::diag::edition_name(&name, build_number()))
}

/// Windows' major and minor version and build, as `RtlGetVersion` gives
/// them whatever the manifest says; `None` if it can't be read.
fn os_version() -> Option<(u32, u32, u32)> {
    use windows::Win32::Foundation::NTSTATUS;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::SystemInformation::OSVERSIONINFOW;
    type RtlGetVersion = unsafe extern "system" fn(*mut OSVERSIONINFOW) -> NTSTATUS;
    let mut info = OSVERSIONINFOW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    // ntdll is in every process; asked for by name, as the task dialog
    // is, so the app needs no import library for it.
    // SAFETY: `RtlGetVersion` is the export's own signature, from its
    // documentation, and `info` is its size-stamped structure.
    let read = unsafe {
        GetModuleHandleW(w!("ntdll.dll"))
            .ok()
            .and_then(|ntdll| proc_address::<RtlGetVersion>(ntdll, s!("RtlGetVersion")))
            .is_some_and(|f| f(&mut info).is_ok())
    };
    read.then_some((info.dwMajorVersion, info.dwMinorVersion, info.dwBuildNumber))
}

/// Windows' build number (22621 for Windows 11 22H2), 0 if it can't be
/// read.
pub fn build_number() -> u32 {
    os_version().map_or(0, |(_, _, build)| build)
}

/// Whether Windows' Transparency effects are on (`EnableTransparency`
/// under the user's Personalize key). If it can't be read, they count as
/// on, as Windows' default. A debug build started with
/// `KILN_TEST_TRANSPARENCY=0` takes them as off, a test hook that shows
/// the solid base without changing Windows' settings.
pub fn transparency_on() -> bool {
    if cfg!(debug_assertions)
        && std::env::var_os("KILN_TEST_TRANSPARENCY").is_some_and(|v| v == "0")
    {
        return false;
    }
    let mut value: u32 = 1;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: reads one DWORD into a local of its size.
    let err = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("EnableTransparency"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    err.is_err() || value != 0
}

/// The address `kiln-app.exe` is loaded at. With it, a crash file's bare
/// backtrace addresses can be matched to that build's symbols.
pub fn module_base() -> usize {
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    unsafe { GetModuleHandleW(None) }.map_or(0, |m| m.0 as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::ThemeChoice;
    use crate::theme::{self, Rgb, SysColors};
    use std::ffi::OsStr;

    #[test]
    fn notepad_is_named_by_its_full_path_in_the_system_folder() {
        // A bare name lets ShellExecute pick a notepad.exe from the
        // current folder.
        let (program, args) = notepad_command(
            Path::new(r"C:\Windows\System32"),
            Path::new(r"C:\Program Files\Kiln\THIRD-PARTY-NOTICES.md"),
        );
        assert_eq!(program, PathBuf::from(r"C:\Windows\System32\notepad.exe"));
        assert!(program.is_absolute());
        assert_eq!(args, r#""C:\Program Files\Kiln\THIRD-PARTY-NOTICES.md""#);
    }

    #[test]
    fn the_system_folder_is_a_full_path() {
        let dir = system_dir().expect("Windows names its system folder");
        assert!(dir.is_absolute(), "{}", dir.display());
        assert!(dir.is_dir(), "{}", dir.display());
    }

    #[test]
    fn the_text_scale_test_hook_takes_a_factor_windows_allows() {
        assert_eq!(parse_text_scale(Some(OsStr::new("1.5"))), Some(1.5));
        assert_eq!(parse_text_scale(Some(OsStr::new(" 2.25 "))), Some(2.25));
        assert_eq!(parse_text_scale(Some(OsStr::new("1"))), Some(1.0));
        for other in ["", "0.5", "2.5", "big", "NaN", "inf", "-1"] {
            assert_eq!(parse_text_scale(Some(OsStr::new(other))), None, "{other:?}");
        }
        assert_eq!(parse_text_scale(None), None);
    }

    #[test]
    fn only_a_one_turns_the_test_contrast_on() {
        assert!(test_contrast_on(Some(OsStr::new("1"))));
        for other in ["", "0", "true", "yes", "11", " 1", "1 "] {
            assert!(!test_contrast_on(Some(OsStr::new(other))), "{other:?}");
        }
        assert!(!test_contrast_on(None));
    }

    #[test]
    fn the_test_contrast_is_high_contrast_black_in_every_field() {
        // Every field named, so a new one in `SysColors` stops this compiling.
        let SysColors {
            window,
            window_text,
            highlight,
            highlight_text,
            gray_text,
            hotlight,
            button_face,
            button_text,
        } = HIGH_CONTRAST_BLACK;
        assert_eq!(window, Rgb(0x00, 0x00, 0x00));
        assert_eq!(window_text, Rgb(0xFF, 0xFF, 0xFF));
        assert_eq!(highlight, Rgb(0x1A, 0xEB, 0xFF));
        assert_eq!(highlight_text, Rgb(0x00, 0x00, 0x00));
        assert_eq!(gray_text, Rgb(0x3F, 0xF2, 0x3F));
        assert_eq!(hotlight, Rgb(0xFF, 0xFF, 0x00));
        assert_eq!(button_face, Rgb(0x00, 0x00, 0x00));
        assert_eq!(button_text, Rgb(0xFF, 0xFF, 0xFF));
    }

    #[test]
    fn the_test_contrast_parses_into_the_palette_the_window_draws_with() {
        let p = super::super::palette_for(Some(HIGH_CONTRAST_BLACK), ThemeChoice::System, false);
        assert_eq!(p, theme::fluent::contrast_palette(HIGH_CONTRAST_BLACK));
        assert!(p.high_contrast);
        assert_eq!((p.base, p.layer.solid), (Rgb(0, 0, 0), Rgb(0, 0, 0)));
        assert_eq!(p.text, Rgb(0xFF, 0xFF, 0xFF));
        assert_eq!(p.text_disabled, Rgb(0x3F, 0xF2, 0x3F));
        assert_eq!(p.accent, Rgb(0x1A, 0xEB, 0xFF));
        assert_eq!(p.on_accent, Rgb(0, 0, 0));
        assert_eq!(p.accent_text, Rgb(0xFF, 0xFF, 0x00));
        // The light flag changes nothing under a contrast theme.
        assert_eq!(
            super::super::palette_for(Some(HIGH_CONTRAST_BLACK), ThemeChoice::System, true),
            p
        );
    }
}
