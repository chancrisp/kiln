//! PrintWindow with `PW_RENDERFULLCONTENT` captures what a window shows
//! through DirectComposition: the capture scripts rely on it once the main
//! window draws into a composition swap chain with no redirection bitmap.
//!
//! The test window is its own: a small popup that never takes activation,
//! cleared to red through a flip-model swap chain. Where there is no
//! desktop to compose on (a service session, a locked screen), and on CI
//! (`CI` set), the test says "skipped" and why, and returns.

#![cfg(windows)]

use windows::core::{w, Interface};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, OpenInputDesktop, DESKTOP_CONTROL_FLAGS, DESKTOP_READOBJECTS,
};
use windows::Win32::UI::WindowsAndMessaging::*;

/// The test window's side, in pixels.
const SIDE: i32 = 160;

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: the message's own arguments, handed on unchanged.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Why this is skipped on a CI runner (`CI` set), if it is: it times what
/// the display and the compositor do, which a shared runner, with no
/// steady compositor or real monitor, can't show. It runs in the local
/// gate.
fn on_ci() -> Option<&'static str> {
    std::env::var_os("CI")
        .is_some()
        .then_some("CI is set; display timing runs in the local gate only")
}

/// Why there is no desktop to compose on, if there is none, or why the
/// test is skipped on CI ([`on_ci`]).
fn no_desktop() -> Option<String> {
    if let Some(why) = on_ci() {
        return Some(why.to_owned());
    }
    // SAFETY: opens the input desktop for reading, and closes it again.
    match unsafe { OpenInputDesktop(DESKTOP_CONTROL_FLAGS::default(), false, DESKTOP_READOBJECTS) }
    {
        Ok(desk) => {
            // SAFETY: the handle just opened.
            let _ = unsafe { CloseDesktop(desk) };
            None
        }
        Err(e) => Some(format!("no input desktop ({e})")),
    }
}

/// A D3D11 device that can be drawn on with Direct2D: hardware, or WARP
/// where there is no graphics card.
fn device() -> windows::core::Result<ID3D11Device> {
    let mut last = None;
    for driver in [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP] {
        let mut device = None;
        // SAFETY: plain out-pointers to locals.
        let made = unsafe {
            D3D11CreateDevice(
                None,
                driver,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )
        };
        match (made, device) {
            (Ok(()), Some(d)) => return Ok(d),
            (Err(e), _) => last = Some(e),
            (Ok(()), None) => {}
        }
    }
    Err(last.unwrap_or_else(windows::core::Error::empty))
}

/// The window's pixels, as PrintWindow with `PW_RENDERFULLCONTENT` gives
/// them: B, G, R, A rows, top first.
fn print_window(hwnd: HWND, w: i32, h: i32) -> Vec<u8> {
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    // SAFETY: a memory DC and a top-down DIB of this size; the bits are
    // read while the section is alive, then everything is freed.
    unsafe {
        let dc = CreateCompatibleDC(None);
        let dib = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
            .expect("a DIB section");
        let old = SelectObject(dc, dib.into());
        let printed = PrintWindow(hwnd, dc, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT));
        assert!(printed.as_bool(), "PrintWindow failed");
        let _ = GdiFlush();
        let px = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize).to_vec();
        SelectObject(dc, old);
        let _ = DeleteObject(dib.into());
        let _ = DeleteDC(dc);
        px
    }
}

/// This thread per-monitor DPI aware until it drops, as it was after.
struct PerMonitorAware(windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT);

impl PerMonitorAware {
    fn new() -> Self {
        // SAFETY: a plain call on this thread.
        PerMonitorAware(unsafe {
            windows::Win32::UI::HiDpi::SetThreadDpiAwarenessContext(
                windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            )
        })
    }
}

impl Drop for PerMonitorAware {
    fn drop(&mut self) {
        // SAFETY: a plain call on this thread.
        unsafe { windows::Win32::UI::HiDpi::SetThreadDpiAwarenessContext(self.0) };
    }
}

#[test]
fn print_window_captures_a_composition_swap_chain() {
    if let Some(why) = no_desktop() {
        println!("skipped: {why}");
        return;
    }
    let device = match device() {
        Ok(d) => d,
        Err(e) => {
            println!("skipped: no Direct3D device ({e})");
            return;
        }
    };
    // SAFETY: this test binary's module handle.
    let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }.unwrap().into();
    let class = w!("KilnCompositionCaptureTest");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: instance,
        lpszClassName: class,
        ..Default::default()
    };
    // SAFETY: a plain registration.
    unsafe { RegisterClassW(&wc) };
    // Inside the test area when `KILN_TEST_PLACE` (`x,y,w,h`) is set, so the
    // popup opens on the monitor the run was given.
    let origin = std::env::var("KILN_TEST_PLACE")
        .ok()
        .and_then(|t| {
            let mut n = t.split(',').map(|v| v.trim().parse::<i32>().ok());
            Some((n.next()??, n.next()??))
        })
        .map_or((40, 40), |(x, y)| (x + 40, y + 40));
    // Per-monitor aware for this thread for the whole test, so the physical
    // coordinates of `KILN_TEST_PLACE` are not virtualised on a scaled
    // primary monitor, and the window's size and capture agree.
    let _aware = PerMonitorAware::new();
    // Shown as it is made, never activated: a topmost tool popup that
    // takes no activation, so nothing covers it and no window loses focus.
    // SAFETY: a plain window of this thread.
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
            class,
            w!("Kiln capture test"),
            WS_POPUP | WS_VISIBLE,
            origin.0,
            origin.1,
            SIDE,
            SIDE,
            None,
            None,
            Some(instance),
            None,
        )
    }
    .expect("the test window");

    // SAFETY: COM calls on objects made here, on this thread; every
    // descriptor is a local that outlives its call.
    let (_dcomp, _target, _visual, _swap) = unsafe {
        let dxgi: IDXGIDevice = device.cast().unwrap();
        let factory: IDXGIFactory2 = dxgi.GetAdapter().unwrap().GetParent().unwrap();
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: SIDE as u32,
            Height: SIDE as u32,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            ..Default::default()
        };
        let swap = factory
            .CreateSwapChainForComposition(&device, &desc, None)
            .unwrap();
        let back: ID3D11Texture2D = swap.GetBuffer(0).unwrap();
        let mut view = None;
        device
            .CreateRenderTargetView(&back, None, Some(&mut view))
            .unwrap();
        let context = device.GetImmediateContext().unwrap();
        // #FF0000, opaque.
        context.ClearRenderTargetView(view.as_ref().unwrap(), &[1.0, 0.0, 0.0, 1.0]);
        drop(view);
        drop(back);
        swap.Present(1, DXGI_PRESENT(0)).ok().unwrap();
        let dcomp: IDCompositionDevice = DCompositionCreateDevice(&dxgi).unwrap();
        let target = dcomp.CreateTargetForHwnd(hwnd, true).unwrap();
        let visual = dcomp.CreateVisual().unwrap();
        visual.SetContent(&swap).unwrap();
        target.SetRoot(&visual).unwrap();
        dcomp.Commit().unwrap();
        dcomp.WaitForCommitCompletion().unwrap();
        let _ = DwmFlush();
        (dcomp, target, visual, swap)
    };

    let px = print_window(hwnd, SIDE, SIDE);
    // SAFETY: this thread's own window.
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
    let i = ((SIDE / 2 * SIDE + SIDE / 2) * 4) as usize;
    let (b, g, r) = (px[i], px[i + 1], px[i + 2]);
    println!("centre pixel: {r},{g},{b}");
    assert!(
        r >= 250 && g <= 5 && b <= 5,
        "PrintWindow should capture the red the swap chain shows; got {r},{g},{b}"
    );
}
