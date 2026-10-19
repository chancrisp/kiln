//! One window: a second launch, such as files dropped on the app icon while
//! Kiln is open, hands its files to the open window and exits.

use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HWND, LPARAM, WPARAM,
};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, FindWindowW, GetWindowThreadProcessId, IsIconic, SendMessageTimeoutW,
    SetForegroundWindow, ShowWindow, SMTO_ABORTIFHUNG, SW_RESTORE, WM_COPYDATA,
};

/// The window class every Kiln window registers.
pub const CLASS: PCWSTR = w!("KilnWindow");
/// Marks a WM_COPYDATA message as a list of files for Kiln: "KILN".
pub const OPEN_FILES: usize = 0x4B49_4C4E;
/// How long a second launch waits for the first one's window to appear.
const WAIT_FOR_WINDOW: Duration = Duration::from_secs(3);

/// Held by the first Kiln for as long as it runs.
pub struct FirstWindow(HANDLE);

impl Drop for FirstWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// `Some` when this is the first Kiln and should open its window. `None`
/// when another Kiln took `files` and this one should exit.
pub fn claim(files: &[PathBuf]) -> Option<FirstWindow> {
    let mutex = unsafe { CreateMutexW(None, false, w!("Local\\KilnWindow")) }.ok()?;
    let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if !existed {
        return Some(FirstWindow(mutex));
    }
    // The other Kiln may still be starting: give its window a moment.
    let start = Instant::now();
    let hwnd = loop {
        if let Ok(h) = unsafe { FindWindowW(CLASS, PCWSTR::null()) } {
            if !h.is_invalid() {
                break Some(h);
            }
        }
        if start.elapsed() > WAIT_FOR_WINDOW {
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    match hwnd {
        Some(h) if send(h, files) => {
            unsafe {
                let _ = CloseHandle(mutex);
            }
            None
        }
        // No window to hand to: open one here after all.
        _ => Some(FirstWindow(mutex)),
    }
}

/// Bring `hwnd` forward and give it `files`. False if it didn't answer.
fn send(hwnd: HWND, files: &[PathBuf]) -> bool {
    let data = encode(files);
    let cds = COPYDATASTRUCT {
        dwData: OPEN_FILES,
        cbData: (data.len() * 2) as u32,
        lpData: data.as_ptr() as *mut _,
    };
    let mut result = 0usize;
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let _ = AllowSetForegroundWindow(pid);
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let _ = SetForegroundWindow(hwnd);
        let sent = SendMessageTimeoutW(
            hwnd,
            WM_COPYDATA,
            WPARAM(0),
            LPARAM(&cds as *const _ as isize),
            SMTO_ABORTIFHUNG,
            5000,
            Some(&mut result),
        );
        sent.0 != 0 && result == 1
    }
}

/// Paths as UTF-16, each ended by a NUL. An empty list is an empty buffer.
pub fn encode(files: &[PathBuf]) -> Vec<u16> {
    files
        .iter()
        .flat_map(|p| p.as_os_str().encode_wide().chain(Some(0)))
        .collect()
}

/// The paths in a buffer from `encode`. Empty entries are skipped.
pub fn decode(data: &[u16]) -> Vec<PathBuf> {
    data.split(|&c| c == 0)
        .filter(|s| !s.is_empty())
        .map(|s| PathBuf::from(OsString::from_wide(s)))
        .collect()
}

/// Read a WM_COPYDATA message from another Kiln. `None` if it isn't one.
///
/// # Safety
/// `lparam` must be the LPARAM of a WM_COPYDATA message being handled.
pub unsafe fn received(lparam: LPARAM) -> Option<Vec<PathBuf>> {
    let cds = (lparam.0 as *const COPYDATASTRUCT).as_ref()?;
    if cds.dwData != OPEN_FILES || cds.lpData.is_null() || cds.cbData % 2 != 0 {
        return None;
    }
    let data = std::slice::from_raw_parts(cds.lpData as *const u16, cds.cbData as usize / 2);
    Some(decode(data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_survive_the_trip() {
        let files = [
            PathBuf::from(r"C:\photos\cone 6.png"),
            PathBuf::from(r"\\server\share\glaze ünïcode.mp4"),
        ];
        assert_eq!(decode(&encode(&files)), files);
        assert!(encode(&[]).is_empty());
    }

    #[test]
    fn unpaired_surrogates_are_kept() {
        let odd = PathBuf::from(OsString::from_wide(&[0x61, 0xD800, 0x62]));
        assert_eq!(decode(&encode(std::slice::from_ref(&odd))), [odd]);
    }

    #[test]
    fn stray_separators_are_ignored() {
        let data: Vec<u16> = "\0a\0\0b".encode_utf16().collect();
        assert_eq!(decode(&data), [PathBuf::from("a"), PathBuf::from("b")]);
    }
}
