//! The Mica check (VIS-01): the window's own frame set-up
//! (`win::style_frame`) and the production composition let the system
//! backdrop show through what Kiln clears to transparent. It shows a
//! window and reads the screen, so it runs by hand, on Windows 11 22H2 or
//! later with Transparency effects on:
//! `cargo test -p kiln-app mica -- --ignored`. `KILN_MICA_PROOF` names the
//! PNG the screen capture is saved to.

use super::*;
use windows::core::{w, GUID};
use windows::Win32::Foundation::{GENERIC_WRITE, HINSTANCE, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

/// The test window's client area, in pixels, and the band at its top that
/// is cleared to transparent.
const W: u32 = 480;
const H: u32 = 320;
const BAND: u32 = 96;

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: the message's own arguments, handed on unchanged.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Answer this thread's messages for `ms` milliseconds.
fn pump(ms: u64) {
    let until = std::time::Instant::now() + std::time::Duration::from_millis(ms);
    let mut msg = MSG::default();
    while std::time::Instant::now() < until {
        // SAFETY: this thread's own queue.
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// The screen's pixels in `r`: B, G, R, A rows, top first.
fn screen(r: RECT) -> Vec<u8> {
    let (w, h) = (r.right - r.left, r.bottom - r.top);
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
    // SAFETY: the screen's DC, a memory DC and a top-down DIB of this
    // size; the bits are read while the section is alive, then everything
    // is let go.
    unsafe {
        let src = GetDC(None);
        let dc = CreateCompatibleDC(Some(src));
        let dib = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
        let old = SelectObject(dc, dib.into());
        BitBlt(dc, 0, 0, w, h, Some(src), r.left, r.top, SRCCOPY).unwrap();
        let _ = GdiFlush();
        let px = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize).to_vec();
        SelectObject(dc, old);
        let _ = DeleteObject(dib.into());
        let _ = DeleteDC(dc);
        ReleaseDC(None, src);
        px
    }
}

/// Save BGRA pixels, alpha ignored, as a PNG at `path`.
fn save_png(path: &std::path::Path, px: &[u8], w: u32, h: u32) {
    let mut opaque = px.to_vec();
    for p in opaque.chunks_mut(4) {
        p[3] = 255;
    }
    let wide: Vec<u16> = path
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain([0])
        .collect();
    // SAFETY: WIC objects made here and used on this thread; the pixels
    // and the path outlive the calls that read them.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let wic: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).unwrap();
        let stream = wic.CreateStream().unwrap();
        stream
            .InitializeFromFilename(windows::core::PCWSTR(wide.as_ptr()), GENERIC_WRITE.0)
            .unwrap();
        let encoder = wic
            .CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())
            .unwrap();
        encoder
            .Initialize(&stream, WICBitmapEncoderNoCache)
            .unwrap();
        let mut frame = None;
        encoder
            .CreateNewFrame(&mut frame, std::ptr::null_mut())
            .unwrap();
        let frame = frame.unwrap();
        frame.Initialize(None).unwrap();
        frame.SetSize(w, h).unwrap();
        let mut format: GUID = GUID_WICPixelFormat32bppBGRA;
        frame.SetPixelFormat(&mut format).unwrap();
        frame.WritePixels(h, w * 4, &opaque).unwrap();
        frame.Commit().unwrap();
        encoder.Commit().unwrap();
    }
}

#[test]
#[ignore = "shows a window and reads the screen: run by hand for the Mica proof"]
fn mica_shows_through_what_the_swap_chain_leaves_transparent() {
    // SAFETY: this test binary's module handle.
    let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }.unwrap().into();
    let class = w!("KilnMicaProof");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: instance,
        lpszClassName: class,
        ..Default::default()
    };
    // SAFETY: a plain registration.
    unsafe { RegisterClassW(&wc) };
    let mut frame = RECT {
        left: 0,
        top: 0,
        right: W as i32,
        bottom: H as i32,
    };
    // SAFETY: a local rectangle, adjusted in place.
    unsafe {
        let _ = AdjustWindowRectEx(
            &mut frame,
            WS_OVERLAPPEDWINDOW,
            false,
            WS_EX_NOREDIRECTIONBITMAP,
        );
    }
    // Shown as it is made, topmost so nothing covers the band, and never
    // activated.
    // SAFETY: a plain window of this thread.
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
            class,
            w!("Kiln Mica proof"),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            120,
            120,
            frame.right - frame.left,
            frame.bottom - frame.top,
            None,
            None,
            Some(instance),
            None,
        )
    }
    .unwrap();
    // The system backdrop behind the whole client area, as Kiln's window
    // asks for it where Windows allows Mica: the frame isn't extended.
    crate::win::style_frame(hwnd, true, crate::win::titlebar::Backdrop::Mica);

    // The production set-up: the device, swap chain and composition.
    // SAFETY: a plain factory for this thread.
    let factory: ID2D1Factory1 =
        unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }.unwrap();
    let mut gpu = Gpu::new(&factory, hwnd, (W, H), 96.0, false).unwrap();
    gpu.begin((W, H), 96.0).unwrap();
    // Opaque red below the band; the band cleared to transparent.
    // SAFETY: drawing on this thread's device context between BeginDraw
    // and EndDraw; the rectangle is a local.
    unsafe {
        let dc = &gpu.dc;
        dc.BeginDraw();
        dc.Clear(Some(&D2D1_COLOR_F {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        }));
        dc.PushAxisAlignedClip(
            &D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: W as f32,
                bottom: BAND as f32,
            },
            D2D1_ANTIALIAS_MODE_ALIASED,
        );
        dc.Clear(Some(&D2D1_COLOR_F::default()));
        dc.PopAxisAlignedClip();
        dc.EndDraw(None, None).unwrap();
    }
    gpu.present().ok().unwrap();
    pump(800);
    // SAFETY: a plain wait for the compositor's next frame.
    unsafe {
        let _ = DwmFlush();
    }

    let mut origin = POINT::default();
    // SAFETY: this thread's own window; the point is a local.
    unsafe {
        let _ = ClientToScreen(hwnd, &mut origin);
    }
    let shot = RECT {
        left: origin.x,
        top: origin.y,
        right: origin.x + W as i32,
        bottom: origin.y + H as i32,
    };
    let px = screen(shot);
    // SAFETY: a plain send to this thread's own window, which closes it.
    unsafe {
        SendMessageW(hwnd, WM_CLOSE, None, None);
    }
    let path = std::env::var_os("KILN_MICA_PROOF")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("kiln-mica-proof.png"));
    save_png(&path, &px, W, H);
    println!("capture saved to {}", path.display());

    let at = |x: u32, y: u32| {
        let i = ((y * W + x) * 4) as usize;
        (px[i + 2], px[i + 1], px[i])
    };
    let band = at(W / 2, BAND / 2);
    let below = at(W / 2, (BAND + H) / 2);
    println!("band centre {band:?}, below the band {below:?}");
    assert_eq!(below, (255, 0, 0), "the opaque part shows as drawn");
    let (r, g, b) = band;
    assert!(r > 6 || g > 6 || b > 6, "the band is black: no backdrop");
    assert_ne!(band, below, "the band shows what is drawn below it");
}

#[test]
fn the_buffers_keep_room_past_the_client_area_and_shrink_once_far_too_big() {
    let first = buffers_for((800, 600), (0, 0));
    assert_eq!(first, (800 + 200 + SPARE, 600 + 150 + SPARE));
    // Growing within them, or shrinking a little, keeps them.
    assert_eq!(buffers_for((900, 700), first), first);
    assert_eq!(buffers_for((600, 500), first), first);
    // Outgrown one way, they grow both ways.
    assert_eq!(
        buffers_for((1200, 600), first),
        (1200 + 300 + SPARE, 600 + 150 + SPARE)
    );
    // Under half of them both ways, they shrink.
    assert_eq!(
        buffers_for((400, 300), first),
        (400 + 100 + SPARE, 300 + 75 + SPARE)
    );
}

#[test]
fn a_window_that_never_asked_for_warp_never_shares_one_that_did() {
    // Another window's losses asked for WARP: its device is WARP's.
    let (forced, warp) = d3d_device(true).unwrap();
    assert!(warp);
    // A window whose own losses never did gets the device a new one would
    // be, not that WARP device.
    let (own, own_warp) = d3d_device(false).unwrap();
    assert!(own != forced, "the forced WARP device is not shared");
    let (_, hardware_or_fallback) = new_d3d_device(false).unwrap();
    assert_eq!(own_warp, hardware_or_fallback);
    // That one is shared with the next window that needs no WARP.
    let (again, _) = d3d_device(false).unwrap();
    assert!(again == own, "the device is shared");
    // A window that asks for WARP shares it only if it is WARP's.
    let (warp_again, warp) = d3d_device(true).unwrap();
    assert!(warp);
    assert_eq!(warp_again == own, own_warp);
    forget_shared_device();
}

/// The test window's side for the partial-present proof, in pixels.
const SIDE: i32 = 200;

/// The window's pixels as PrintWindow with `PW_RENDERFULLCONTENT` gives
/// them (B, G, R, A rows, top first), and their size: it captures what a
/// window shows through DirectComposition.
fn print_window(hwnd: HWND) -> (Vec<u8>, i32, i32) {
    let mut rc = RECT::default();
    // SAFETY: this thread's own window; the rectangle is a local.
    unsafe {
        let _ = GetClientRect(hwnd, &mut rc);
    }
    let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
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
        let printed = windows::Win32::Storage::Xps::PrintWindow(
            hwnd,
            dc,
            windows::Win32::Storage::Xps::PRINT_WINDOW_FLAGS(2),
        );
        assert!(printed.as_bool(), "PrintWindow failed");
        let _ = GdiFlush();
        let px = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize).to_vec();
        SelectObject(dc, old);
        let _ = DeleteObject(dib.into());
        let _ = DeleteDC(dc);
        (px, w, h)
    }
}

/// Draw one frame on `gpu`: `colour` over `part` (the whole buffer when
/// `None`), the drawing clipped to it, then present it as that part.
fn frame(gpu: &mut Gpu, colour: (f32, f32, f32), part: Option<RECT>) {
    gpu.begin((SIDE as u32, SIDE as u32), 96.0).unwrap();
    // SAFETY: drawing on this thread's device context between BeginDraw
    // and EndDraw; the rectangle is a local.
    unsafe {
        let dc = &gpu.dc;
        dc.BeginDraw();
        if let Some(r) = &part {
            dc.PushAxisAlignedClip(
                &D2D_RECT_F {
                    left: r.left as f32,
                    top: r.top as f32,
                    right: r.right as f32,
                    bottom: r.bottom as f32,
                },
                D2D1_ANTIALIAS_MODE_ALIASED,
            );
        }
        dc.Clear(Some(&D2D1_COLOR_F {
            r: colour.0,
            g: colour.1,
            b: colour.2,
            a: 1.0,
        }));
        if part.is_some() {
            dc.PopAxisAlignedClip();
        }
        dc.EndDraw(None, None).unwrap();
    }
    let hr = match part {
        Some(r) => gpu.present_part(&[r]),
        None => gpu.present(),
    };
    hr.ok().unwrap();
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

/// Flip-sequential buffers rotate: the buffer drawn on after a present is
/// not the frame just shown. A present with dirty rectangles shows only
/// those from the new frame and keeps the rest of the one before, however
/// stale the buffer drawn on is outside them, and every present, whole or
/// partial, spends exactly one signal of the waitable.
///
/// Four frames on the production surface (buffers bigger than the client
/// area): red whole, blue whole, then green inside one rectangle and
/// yellow inside another, each presented as its rectangle. What the
/// compositor shows is read back with PrintWindow: green and yellow stay
/// where they were drawn, blue stays elsewhere, and red, the stale buffer,
/// shows nowhere.
#[test]
fn a_partial_present_keeps_the_rest_of_the_frame_before_it() {
    use windows::Win32::System::StationsAndDesktops::{
        CloseDesktop, OpenInputDesktop, DESKTOP_CONTROL_FLAGS, DESKTOP_READOBJECTS,
    };
    // SAFETY: opens the input desktop for reading, and closes it again.
    match unsafe { OpenInputDesktop(DESKTOP_CONTROL_FLAGS::default(), false, DESKTOP_READOBJECTS) }
    {
        Ok(desk) => {
            // SAFETY: the handle just opened.
            let _ = unsafe { CloseDesktop(desk) };
        }
        Err(e) => {
            println!("skipped: no input desktop ({e})");
            return;
        }
    }
    // SAFETY: this test binary's module handle.
    let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }.unwrap().into();
    let class = w!("KilnPartialPresentProof");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: instance,
        lpszClassName: class,
        ..Default::default()
    };
    // SAFETY: a plain registration.
    unsafe { RegisterClassW(&wc) };
    // Inside the test area when `KILN_TEST_PLACE` is set, so the popup opens
    // on the monitor the run was given.
    let origin = crate::win::placement::area().map_or((40, 40), |a| (a.x + 40, a.y + 40));
    // Per-monitor aware for this thread for the whole test, so the physical
    // coordinates of `KILN_TEST_PLACE` are not virtualised on a scaled
    // primary monitor, and the window's size and capture agree.
    let _aware = PerMonitorAware::new();
    // A topmost tool popup that takes no activation, as the composition
    // capture test has.
    // SAFETY: a plain window of this thread.
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
            class,
            w!("Kiln partial present proof"),
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
    .unwrap();
    // SAFETY: a plain factory for this thread.
    let factory: ID2D1Factory1 =
        unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }.unwrap();
    let mut gpu = Gpu::new(&factory, hwnd, (SIDE as u32, SIDE as u32), 96.0, false).unwrap();
    assert!(
        gpu.size.0 > SIDE as u32 && gpu.size.1 > SIDE as u32,
        "the buffers are bigger than the client area: {:?}",
        gpu.size
    );
    assert!(
        !gpu.can_present_part(),
        "nothing is shown before the first present"
    );

    let rect = |left, top| RECT {
        left,
        top,
        right: left + 60,
        bottom: top + 60,
    };
    let (first, second) = (rect(20, 20), rect(120, 120));
    frame(&mut gpu, (1.0, 0.0, 0.0), None);
    assert!(gpu.can_present_part(), "shown after the first present");
    frame(&mut gpu, (0.0, 0.0, 1.0), None);
    frame(&mut gpu, (0.0, 1.0, 0.0), Some(first));
    frame(&mut gpu, (1.0, 1.0, 0.0), Some(second));
    assert_eq!(gpu.owed, 0, "every present found its signal waiting");
    assert!(!gpu.held, "and spent it");
    pump(500);
    // SAFETY: a plain wait for the compositor's next frame.
    unsafe {
        let _ = DwmFlush();
    }
    let (px, w, h) = print_window(hwnd);
    // SAFETY: a plain send to this thread's own window, which closes it.
    unsafe {
        SendMessageW(hwnd, WM_CLOSE, None, None);
    }
    // Positions as fractions of the window, in case it is scaled.
    let at = |fx: f32, fy: f32| {
        let (x, y) = ((fx * w as f32) as usize, (fy * h as f32) as usize);
        let i = (y * w as usize + x) * 4;
        (px[i + 2], px[i + 1], px[i])
    };
    let s = SIDE as f32;
    let near = |got: (u8, u8, u8), want: (u8, u8, u8)| {
        let d = |a: u8, b: u8| a.abs_diff(b) <= 6;
        d(got.0, want.0) && d(got.1, want.1) && d(got.2, want.2)
    };
    let samples = [
        ("first rectangle", (50.0, 50.0), (0, 255, 0)),
        ("second rectangle", (150.0, 150.0), (255, 255, 0)),
        ("top right", (150.0, 30.0), (0, 0, 255)),
        ("bottom left", (30.0, 150.0), (0, 0, 255)),
        ("between", (100.0, 100.0), (0, 0, 255)),
        ("far corner", (195.0, 5.0), (0, 0, 255)),
    ];
    for (what, (x, y), want) in samples {
        let got = at(x / s, y / s);
        println!("{what}: {got:?}, wanted {want:?}");
        assert!(
            near(got, want),
            "{what}: the compositor shows {got:?}, not {want:?}"
        );
    }
}
