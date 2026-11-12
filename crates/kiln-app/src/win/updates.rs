//! The update check's one HTTPS request, through WinHTTP so it follows the
//! system's proxy settings and certificate store.

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Networking::WinHttp::*;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::update::{Reply, HOST, PATH, RELEASES_PAGE};

/// Largest answer read; a release description is far smaller.
const MAX_BODY: usize = 1 << 20;
/// Milliseconds for each stage of the request.
const TIMEOUT_MS: i32 = 10_000;

/// Closes a WinHTTP handle when dropped.
struct Handle(*mut core::ffi::c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

fn open(h: *mut core::ffi::c_void) -> Option<Handle> {
    (!h.is_null()).then_some(Handle(h))
}

/// Ask for the latest release. Blocks; run it off the UI thread.
pub fn fetch() -> Reply {
    fetch_inner().unwrap_or(Reply::Unreachable)
}

fn fetch_inner() -> Option<Reply> {
    let agent = HSTRING::from(format!("Kiln/{}", crate::update::CURRENT));
    unsafe {
        let session = open(WinHttpOpen(
            &agent,
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ))?;
        WinHttpSetTimeouts(session.0, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS).ok()?;
        let connect = open(WinHttpConnect(
            session.0,
            &HSTRING::from(HOST),
            INTERNET_DEFAULT_HTTPS_PORT,
            0,
        ))?;
        let request = open(WinHttpOpenRequest(
            connect.0,
            w!("GET"),
            &HSTRING::from(PATH),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ))?;
        let headers: Vec<u16> = "Accept: application/vnd.github+json\r\n"
            .encode_utf16()
            .collect();
        WinHttpSendRequest(request.0, Some(&headers), None, 0, 0, 0).ok()?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut()).ok()?;

        let mut status = 0u32;
        let mut len = std::mem::size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut _),
            &mut len,
            std::ptr::null_mut(),
        )
        .ok()?;
        if status != 200 {
            return Some(Reply::Status(status));
        }

        let mut body = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            let mut read = 0u32;
            WinHttpReadData(
                request.0,
                chunk.as_mut_ptr() as *mut _,
                chunk.len() as u32,
                &mut read,
            )
            .ok()?;
            if read == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..read as usize]);
            if body.len() > MAX_BODY {
                return None;
            }
        }
        Some(Reply::Body(String::from_utf8_lossy(&body).into_owned()))
    }
}

/// Open the releases page in the default browser.
pub fn open_releases_page() {
    unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &HSTRING::from(RELEASES_PAGE),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Makes a real request, so it only runs when asked for:
    /// `cargo test -p kiln-app -- --ignored live_update_check`.
    #[test]
    #[ignore = "uses the network"]
    fn live_update_check() {
        let reply = fetch();
        println!("{reply:?}");
        assert_ne!(reply, Reply::Unreachable);
    }
}
