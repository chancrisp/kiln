//! The drag-wheel's input hooks, installed and removed on their own thread.
//!
//! No test here sends input or moves the pointer. Starting the hooks proves
//! Windows accepted both; stopping them proves the thread ends and takes
//! them down.

#![cfg(windows)]

use std::time::{Duration, Instant};

use kiln_app::win::hooks::{Hooks, WM_KILN_HOOK};
use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

extern "system" fn owner_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// A hidden message-only window to post verdicts to.
fn message_window() -> HWND {
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None).unwrap().into();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(owner_proc),
            hInstance: instance,
            lpszClassName: w!("KilnHooksTestOwner"),
            ..Default::default()
        };
        assert_ne!(RegisterClassW(&wc), 0);
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("KilnHooksTestOwner"),
            w!("owner"),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance),
            None,
        )
        .unwrap()
    }
}

#[test]
fn hooks_install_and_remove() {
    let owner = message_window();

    let hooks = Hooks::start(owner, WM_KILN_HOOK).expect("Windows accepts both low-level hooks");
    // The window telling the watch it closed the wheel is harmless while
    // nothing is open.
    hooks.wheel_closed();

    let began = Instant::now();
    hooks.stop();
    assert!(
        began.elapsed() < Duration::from_secs(1),
        "stopping took {:?}",
        began.elapsed()
    );

    // They can go on again after stopping, as the setting is toggled.
    let again = Hooks::start(owner, WM_KILN_HOOK).expect("the hooks start again after a stop");
    let began = Instant::now();
    drop(again);
    assert!(began.elapsed() < Duration::from_secs(1));

    unsafe { DestroyWindow(owner).unwrap() };
}
