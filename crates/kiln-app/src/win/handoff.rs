//! One window: a second launch, such as files dropped on the app icon while
//! Kiln is open, hands its files to the open window and exits.
//!
//! A notification's `kiln:show?n=<id>` link comes the same way:
//! Windows starts a second Kiln with the link, which hands the id alone to
//! the running Kiln ([`send_notice`]). The link is anyone's to make, so it
//! is read strictly ([`notice_id`]) and the running Kiln acts only on the
//! ids of its own notices.

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
    AllowSetForegroundWindow, FindWindowW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    SendMessageTimeoutW, SetForegroundWindow, ShowWindow, SMTO_ABORTIFHUNG, SW_RESTORE, SW_SHOW,
    WM_COPYDATA,
};

/// The window class every Kiln window registers.
pub const CLASS: PCWSTR = w!("KilnWindow");
/// Marks a WM_COPYDATA message as a list of files for Kiln: "KILN".
pub const OPEN_FILES: usize = 0x4B49_4C4E;
/// Marks a WM_COPYDATA message as a notice's id for Kiln: "KILS".
pub const SHOW_NOTICE: usize = 0x4B49_4C53;
/// The link's scheme, and what comes after it for a notice.
pub const SCHEME: &str = "kiln:";
const SHOW: &str = "show?n=";
/// The longest link read: the scheme, `show?n=` and a 20-digit id.
const LONGEST_LINK: usize = 32;
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
/// when another Kiln took `files` and this one should exit. A start in the
/// background (`hidden`) with another Kiln running just exits: that Kiln
/// is already there for the drag-wheel, and its window stays as it is.
pub fn claim(files: &[PathBuf], hidden: bool) -> Option<FirstWindow> {
    let mutex = unsafe { CreateMutexW(None, false, w!("Local\\KilnWindow")) }.ok()?;
    let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if !existed {
        return Some(FirstWindow(mutex));
    }
    if hidden {
        // SAFETY: the handle this call opened, closed once.
        unsafe {
            let _ = CloseHandle(mutex);
        }
        return None;
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
        // The test placement switch never asks for the foreground.
        let quiet = super::placement::quiet_on();
        if !quiet {
            let _ = AllowSetForegroundWindow(pid);
        }
        // A Kiln hidden in the notification area shows its window again.
        // It also does so itself on the files, which may come first.
        if !IsWindowVisible(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, super::placement::show_cmd(SW_SHOW));
        }
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, super::placement::show_cmd(SW_RESTORE));
        }
        if !quiet {
            let _ = SetForegroundWindow(hwnd);
        }
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

/// Notice `id`'s link: `kiln:show?n=<id>`.
pub fn notice_link(id: u64) -> String {
    format!("{SCHEME}{SHOW}{id}")
}

/// The id in a notice's link, read strictly: `kiln:show?n=` (the scheme
/// in any case) and the id in decimal digits, as [`notice_link`] writes
/// it, with nothing before or after. Anything else is `None`.
pub fn notice_id(link: &str) -> Option<u64> {
    if link.len() > LONGEST_LINK {
        return None;
    }
    let scheme = link.get(..SCHEME.len())?;
    if !scheme.eq_ignore_ascii_case(SCHEME) {
        return None;
    }
    let digits = link[SCHEME.len()..].strip_prefix(SHOW)?;
    // ASCII digits only, without a leading zero: the one way
    // `notice_link` writes each id.
    let canonical = !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && (digits == "0" || !digits.starts_with('0'));
    if !canonical {
        return None;
    }
    digits.parse().ok()
}

/// A WM_COPYDATA's notice id: `data` is its 8 bytes, little-endian, under
/// [`SHOW_NOTICE`]. `None` for any other message.
pub fn notice_from(tag: usize, data: Option<&[u8]>) -> Option<u64> {
    if tag != SHOW_NOTICE {
        return None;
    }
    let bytes: [u8; 8] = data?.try_into().ok()?;
    Some(u64::from_le_bytes(bytes))
}

/// Hand notice `id` to the running Kiln, from a second launch started by
/// a notification's link, and let it come forward. With no Kiln running
/// nothing happens: the notice was an earlier run's.
pub fn send_notice(id: u64) {
    // SAFETY: plain lookups and a message to another process's window,
    // its data a local that outlives the call.
    unsafe {
        let Ok(hwnd) = FindWindowW(CLASS, PCWSTR::null()) else {
            return;
        };
        if hwnd.is_invalid() {
            return;
        }
        let data = id.to_le_bytes();
        let cds = COPYDATASTRUCT {
            dwData: SHOW_NOTICE,
            cbData: data.len() as u32,
            lpData: data.as_ptr() as *mut _,
        };
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        // It may open Explorer or its window: this launch came from a
        // press on the notification, so it may pass that on.
        if !super::placement::quiet_on() {
            let _ = AllowSetForegroundWindow(pid);
        }
        let mut result = 0usize;
        let _ = SendMessageTimeoutW(
            hwnd,
            WM_COPYDATA,
            WPARAM(0),
            LPARAM(&cds as *const _ as isize),
            SMTO_ABORTIFHUNG,
            5000,
            Some(&mut result),
        );
    }
}

/// Read a WM_COPYDATA message carrying a notice's id from another Kiln.
/// `None` if it isn't one.
///
/// # Safety
/// `lparam` must be the LPARAM of a WM_COPYDATA message being handled.
pub unsafe fn received_notice(lparam: LPARAM) -> Option<u64> {
    // SAFETY: the caller's promise: `lparam` is a WM_COPYDATA's, so it is
    // null or a COPYDATASTRUCT that lasts for the message.
    let cds = unsafe { super::system::lparam_ref::<COPYDATASTRUCT>(lparam) }?;
    if cds.dwData != SHOW_NOTICE || cds.lpData.is_null() || cds.cbData != 8 {
        return None;
    }
    // SAFETY: not null, and the sender promises `cbData` (8) bytes at
    // `lpData` for the length of the message; read as bytes, which any
    // address allows.
    let data = unsafe { std::slice::from_raw_parts(cds.lpData as *const u8, 8) };
    notice_from(cds.dwData, Some(data))
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
    // SAFETY: the caller's promise: `lparam` is a WM_COPYDATA's, so it is
    // null or a COPYDATASTRUCT that lasts for the message.
    let cds = super::system::lparam_ref::<COPYDATASTRUCT>(lparam)?;
    if cds.dwData != OPEN_FILES || cds.lpData.is_null() || cds.cbData % 2 != 0 {
        return None;
    }
    // SAFETY: not null, and the sender promises `cbData` bytes at `lpData`
    // for the length of the message.
    let data = units_at(cds.lpData as *const u8, cds.cbData as usize);
    Some(decode(&data))
}

/// The UTF-16 units in `len` bytes at `ptr`, copied. WM_COPYDATA doesn't
/// promise `ptr` is 2-byte aligned, so it is read as bytes, which any
/// address allows.
///
/// # Safety
/// `ptr` must point at `len` readable bytes, and `len` must be even.
unsafe fn units_at(ptr: *const u8, len: usize) -> Vec<u16> {
    // SAFETY: the caller's promise; a `u8` slice has no alignment need.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notice_link_carries_its_id_and_reads_back() {
        for id in [0, 1, 42, 9_007_199_254_740_993, u64::MAX] {
            let link = notice_link(id);
            assert_eq!(notice_id(&link), Some(id), "{link}");
        }
        assert_eq!(notice_link(42), "kiln:show?n=42");
        // The scheme in any case, as Windows may hand it over.
        assert_eq!(notice_id("KILN:show?n=42"), Some(42));
        assert_eq!(notice_id("Kiln:show?n=42"), Some(42));
    }

    #[test]
    fn anything_but_a_plain_notice_link_is_nothing() {
        for link in [
            "",
            "kiln:",
            "kiln:show",
            "kiln:show?n=",
            "kiln:show?n=-1",
            "kiln:show?n=+1",
            "kiln:show?n=1.0",
            "kiln:show?n=0x10",
            "kiln:show?n=01",
            "kiln:show?n=1 ",
            " kiln:show?n=1",
            "kiln:show?n=1&n=2",
            "kiln:show?n=1#x",
            "kiln:show?n=1/",
            "kiln:show?N=1",
            "kiln:SHOW?n=1",
            "kiln:open?n=1",
            "kiln:delete?n=1",
            r"kiln:show?path=C:\Windows",
            "kiln://show?n=1",
            "kiln:/show?n=1",
            "kiln:show?n=%31",
            "kiln:show%3Fn=1",
            "kiln:show?n=\u{FF11}",
            "kiln:show?n=\u{0661}",
            "kiln:show?n=1\u{0}",
            "kiln:show?n=18446744073709551616",
            "kiln:show?n=99999999999999999999",
            "kilnx:show?n=1",
            "https://example.com/?n=1",
            r"C:\photos\a.jpg",
        ] {
            assert_eq!(notice_id(link), None, "{link:?}");
        }
        // Very long input is refused before it is read.
        let long = format!("kiln:show?n=1{}", "0".repeat(100_000));
        assert_eq!(notice_id(&long), None);
    }

    #[test]
    fn a_notice_id_crosses_as_eight_bytes_under_its_own_tag() {
        let bytes = 42u64.to_le_bytes();
        assert_eq!(notice_from(SHOW_NOTICE, Some(&bytes)), Some(42));
        assert_eq!(
            notice_from(SHOW_NOTICE, Some(&u64::MAX.to_le_bytes())),
            Some(u64::MAX)
        );
        // Another tag, no data, or another length is nothing.
        assert_eq!(notice_from(OPEN_FILES, Some(&bytes)), None);
        assert_eq!(notice_from(0, Some(&bytes)), None);
        assert_eq!(notice_from(SHOW_NOTICE, None), None);
        assert_eq!(notice_from(SHOW_NOTICE, Some(&bytes[..7])), None);
        assert_eq!(notice_from(SHOW_NOTICE, Some(&[0; 9])), None);
        assert_eq!(notice_from(SHOW_NOTICE, Some(&[])), None);
    }

    #[test]
    fn paths_survive_the_trip() {
        let files = [
            PathBuf::from(r"C:\photos\cone 6.png"),
            PathBuf::from(r"\\server\share\clip ünïcode.mp4"),
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
    fn a_misaligned_buffer_is_read() {
        let units = encode(&[PathBuf::from(r"C: b.png"), PathBuf::from("d")]);
        let bytes: Vec<u8> = units.iter().flat_map(|u| u.to_le_bytes()).collect();
        // One byte in, so the start is odd whatever the allocation was.
        let mut shifted = vec![0u8];
        shifted.extend_from_slice(&bytes);
        // SAFETY: the slice holds `bytes.len()` bytes after the first.
        let got = unsafe { units_at(shifted[1..].as_ptr(), bytes.len()) };
        assert_eq!(got, units);
        assert_eq!(
            decode(&got),
            [PathBuf::from(r"C: b.png"), PathBuf::from("d")]
        );
    }

    #[test]
    fn stray_separators_are_ignored() {
        let data: Vec<u16> = "\0a\0\0b".encode_utf16().collect();
        assert_eq!(decode(&data), [PathBuf::from("a"), PathBuf::from("b")]);
    }
}
