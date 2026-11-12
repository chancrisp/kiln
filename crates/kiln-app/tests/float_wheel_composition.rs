//! The floating wheel on screen: what its window lets through, what
//! PrintWindow captures of it, and a frame drawn again after its graphics
//! device is lost.
//!
//! No test moves the pointer or sends input. A drag is the wheel's own
//! `IDropTarget` called directly, as in `float_wheel.rs`; where the window
//! is hit is asked of `WindowFromPoint`, as OLE asks it during a drag.

#![cfg(windows)]

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard, Once};
use std::time::{Duration, Instant};

use kiln_app::settings::Settings;
use kiln_app::theme::fluent;
use kiln_app::wheel::{self, WheelKind, R_INNER, R_OUTER};
use kiln_app::win::float_wheel::{FloatNote, FloatWheel, HALF};
use kiln_core::{Availability, Kiln};
use windows::core::{BOOL, HSTRING};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::System::Com::*;
use windows::Win32::System::Ole::*;
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, OpenInputDesktop, DESKTOP_CONTROL_FLAGS, DESKTOP_READOBJECTS,
};
use windows::Win32::System::SystemServices::MK_LBUTTON;
use windows::Win32::UI::HiDpi::{
    GetDpiForMonitor, GetDpiForWindow, SetProcessDpiAwarenessContext,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    BHID_DataObject, SHCreateShellItemArrayFromIDLists, SHParseDisplayName,
};
use windows::Win32::UI::WindowsAndMessaging::*;

/// The tests share the screen: one at a time.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

const ALLOWED: DROPEFFECT = DROPEFFECT(DROPEFFECT_COPY.0 | DROPEFFECT_MOVE.0 | DROPEFFECT_LINK.0);

/// The wheel's news is not what these tests look at.
fn ignore(_: FloatNote) {}

struct Rig {
    dir: tempfile::TempDir,
    _one: MutexGuard<'static, ()>,
}

impl Rig {
    fn new() -> Rig {
        let one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        static SETUP: Once = Once::new();
        // SAFETY: plain process and thread set-up calls.
        SETUP.call_once(|| unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            // A busy PC can keep this thread from reading its messages for
            // about 5 s; Windows would then put a "Ghost" window over the
            // wheel, and the hit tests would see the ghost.
            DisableProcessWindowsGhosting();
        });
        // SAFETY: OLE for this thread, undone when the rig goes.
        unsafe { OleInitialize(None).unwrap() };
        Rig {
            dir: tempfile::tempdir().unwrap(),
            _one: one,
        }
    }

    fn open(&self, kind: WheelKind) -> FloatWheel {
        FloatWheel::open(
            ignore,
            middle(),
            kind,
            &Settings::default(),
            Rc::new(Cell::new(false)),
        )
        .unwrap()
    }

    fn png(&self, name: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        let px: Vec<u8> = (0..16).flat_map(|i| [i * 16, 80, 160, 255]).collect();
        write_png(&path, 4, &px);
        path
    }

    fn wav(&self, name: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        std::fs::write(&path, wav_bytes()).unwrap();
        path
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        // SAFETY: the OleInitialize of `new`.
        unsafe { OleUninitialize() };
    }
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

/// The area `KILN_TEST_PLACE` names, as `(x, y, w, h)` in physical pixels
/// (`win/placement.rs`): with it set, the wheel is made in that area and
/// not on the primary monitor.
fn placed() -> Option<(i32, i32, i32, i32)> {
    let text = std::env::var("KILN_TEST_PLACE").ok()?;
    let n: Vec<i32> = text
        .split(',')
        .map(|p| p.trim().parse().ok())
        .collect::<Option<_>>()?;
    let [x, y, w, h] = n[..] else {
        return None;
    };
    (w > 0 && h > 0).then_some((x, y, w, h))
}

/// The middle of the primary monitor's work area, where the window fits
/// whole; of the `KILN_TEST_PLACE` area when that is set.
fn middle() -> (i32, i32) {
    if let Some((x, y, w, h)) = placed() {
        return (x + w / 2, y + h / 2);
    }
    // SAFETY: plain queries into locals.
    let monitor = unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: as above.
    assert!(unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool());
    let r = info.rcWork;
    ((r.left + r.right) / 2, (r.top + r.bottom) / 2)
}

fn wav_bytes() -> Vec<u8> {
    let data = vec![0u8; 16_000];
    let mut b = vec![];
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&8000u32.to_le_bytes());
    b.extend_from_slice(&16000u32.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data.len() as u32).to_le_bytes());
    b.extend_from_slice(&data);
    b
}

/// A PNG of `side` x `side` straight-alpha BGRA pixels, written with WIC.
fn write_png(path: &Path, side: u32, bgra: &[u8]) {
    // SAFETY: WIC calls on objects made here; the pixels outlive the call.
    unsafe {
        let wic: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).unwrap();
        let stream = wic.CreateStream().unwrap();
        stream
            .InitializeFromFilename(&HSTRING::from(path.as_os_str()), GENERIC_WRITE.0)
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
        frame.SetSize(side, side).unwrap();
        let mut format = GUID_WICPixelFormat32bppBGRA;
        frame.SetPixelFormat(&mut format).unwrap();
        frame.WritePixels(side, side * 4, bgra).unwrap();
        frame.Commit().unwrap();
        encoder.Commit().unwrap();
    }
}

/// A shell data object for `paths`, as Explorer hands one to a drag.
fn data_object(paths: &[PathBuf]) -> IDataObject {
    // SAFETY: shell calls on locals; each ID list is freed once used.
    unsafe {
        let pidls: Vec<*mut ITEMIDLIST> = paths
            .iter()
            .map(|p| {
                let mut pidl = std::ptr::null_mut();
                SHParseDisplayName(&HSTRING::from(p.as_os_str()), None, &mut pidl, 0, None)
                    .unwrap();
                pidl
            })
            .collect();
        let ids: Vec<*const ITEMIDLIST> = pidls.iter().map(|&p| p as *const _).collect();
        let items = SHCreateShellItemArrayFromIDLists(&ids).unwrap();
        let data: IDataObject = items.BindToHandler(None, &BHID_DataObject).unwrap();
        for p in pidls {
            CoTaskMemFree(Some(p as *const _));
        }
        data
    }
}

/// A drag with `paths` enters at the wheel's centre, and the wheel is
/// filled with what they can become, as Kiln fills it.
fn bring(fw: &mut FloatWheel, paths: &[PathBuf]) -> IDataObject {
    let data = data_object(paths);
    let mut effect = ALLOWED;
    let at = middle();
    // SAFETY: the wheel's own drop target, called as OLE calls it.
    unsafe {
        fw.drop_target()
            .DragEnter(&data, MK_LBUTTON, POINTL { x: at.0, y: at.1 }, &mut effect)
            .unwrap()
    };
    fw.files_arrived(paths.to_vec(), &Kiln::new(), &Availability::all());
    data
}

/// The screen point `(dx, dy)` DIPs from the wheel's centre.
fn at_dips(fw: &FloatWheel, dx: f32, dy: f32) -> (i32, i32) {
    // SAFETY: a plain query on the wheel's window.
    let k = unsafe { GetDpiForWindow(fw.hwnd()) } as f32 / 96.0;
    let (x, y) = middle();
    (x + (dx * k).round() as i32, y + (dy * k).round() as i32)
}

fn slice(fw: &FloatWheel, i: usize, n: usize) -> (i32, i32) {
    let (a0, a1) = wheel::segment(i, n);
    let (a, r) = ((a0 + a1) / 2.0, (R_INNER + R_OUTER) / 2.0);
    at_dips(fw, r * a.cos(), r * a.sin())
}

fn window_rect(hwnd: HWND) -> RECT {
    let mut r = RECT::default();
    // SAFETY: a plain query into a local.
    unsafe { GetWindowRect(hwnd, &mut r).unwrap() };
    r
}

/// A point the hit test got wrong: where in the window, the frame's alpha
/// there, and the class of the window that was hit.
type Wrong = (i32, i32, u8, String);

/// The class of the window at `at`, for a message.
fn class_at(at: POINT) -> String {
    // SAFETY: plain queries into a local buffer.
    unsafe {
        let h = WindowFromPoint(at);
        let mut buf = [0u16; 128];
        let n = GetClassNameW(h, &mut buf) as usize;
        String::from_utf16_lossy(&buf[..n])
    }
}

/// Where the hit test and the last frame disagree: points where a pixel
/// drawn with any alpha at all is not the wheel's window under the
/// pointer, or a clear pixel is. A layered window drawn with
/// `UpdateLayeredWindow` is hit exactly where its alpha is not zero, and
/// the wheel's window keeps that. Every pixel on the edge between drawn
/// and clear is asked, and one in 16 of the rest.
fn hit_mismatches(fw: &FloatWheel) -> (usize, usize, Vec<Wrong>) {
    let r = window_rect(fw.hwnd());
    let (side, px) = fw.pixels();
    assert_eq!(
        ((r.right - r.left) as u32, (r.bottom - r.top) as u32),
        (side, side)
    );
    let s = side as i32;
    let drawn_at = |x: i32, y: i32| {
        (0..s).contains(&x) && (0..s).contains(&y) && px[((y * s + x) * 4 + 3) as usize] > 0
    };
    let (mut drawn, mut clear, mut wrong) = (0, 0, vec![]);
    for y in 0..s {
        for x in 0..s {
            let d = drawn_at(x, y);
            let edge = [(1, 0), (-1, 0), (0, 1), (0, -1)]
                .iter()
                .any(|(dx, dy)| drawn_at(x + dx, y + dy) != d);
            if !edge && (x % 4 != 0 || y % 4 != 0) {
                continue;
            }
            let at = POINT {
                x: r.left + x,
                y: r.top + y,
            };
            // SAFETY: a plain query.
            let hit = unsafe { WindowFromPoint(at) } == fw.hwnd();
            if d {
                drawn += 1;
            } else {
                clear += 1;
            }
            if hit != d {
                let a = px[((y * s + x) * 4 + 3) as usize];
                wrong.push((x, y, a, class_at(at)));
            }
        }
    }
    (drawn, clear, wrong)
}

fn assert_hit_where_drawn(fw: &FloatWheel, what: &str) {
    let (drawn, clear, wrong) = hit_mismatches(fw);
    println!(
        "{what}: {drawn} drawn points hit, {clear} clear points pass through, {} wrong",
        wrong.len()
    );
    assert!(drawn > 0 && clear > 0, "{what}");
    assert!(
        wrong.is_empty(),
        "{what}: {} points where the hit test and the frame disagree, first {:?}",
        wrong.len(),
        &wrong[..wrong.len().min(8)]
    );
}

#[test]
fn outside_the_drawn_wheel_the_window_below_is_hit() {
    if let Some(why) = no_desktop() {
        println!("skipped: {why}");
        return;
    }
    let rig = Rig::new();
    let later = || Instant::now() + Duration::from_secs(5);

    // Before files: only the faint hub, there to be hit.
    let fw = rig.open(WheelKind::Convert);
    assert_hit_where_drawn(&fw, "before files");
    fw.close();

    let png = rig.png("photo.png");
    for palette in fluent::ALL_BUILT_IN {
        let mut fw = rig.open(WheelKind::Convert);
        fw.set_palette(palette);
        bring(&mut fw, std::slice::from_ref(&png));
        // Early in the bloom, while it grows.
        fw.frame(Instant::now() + Duration::from_millis(40))
            .unwrap();
        assert_hit_where_drawn(&fw, "blooming");
        // Bloomed, at rest, with a slice lit.
        let n = fw.wheel().unwrap().segments();
        fw.pointer(slice(&fw, 1, n));
        fw.frame(later()).unwrap();
        assert_hit_where_drawn(&fw, "at rest");
        // More's list open beside the ring: it is hit too.
        fw.pointer(slice(&fw, n - 1, n));
        fw.frame(later()).unwrap();
        assert_hit_where_drawn(&fw, "more open");
        // Fading out: what still shows, and the hit area it keeps.
        if let Some(fading) = fw.fade_out() {
            let mut fading = fading;
            fading
                .frame(Instant::now() + Duration::from_millis(60))
                .unwrap();
            assert_hit_where_drawn(&fading, "fading");
        }
    }

    // Nothing to offer: the caption's pill under the wheel is hit too.
    let mut fw = rig.open(WheelKind::Convert);
    bring(&mut fw, &[png.clone(), rig.wav("tone.wav")]);
    assert!(fw.caption().is_some());
    fw.frame(later()).unwrap();
    assert_hit_where_drawn(&fw, "caption");
    fw.close();
}

/// The window's pixels, as PrintWindow with `PW_RENDERFULLCONTENT` gives
/// them: B, G, R, A rows, top first.
fn print_window(hwnd: HWND, side: i32) -> Vec<u8> {
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: side,
            biHeight: -side,
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
        let px = std::slice::from_raw_parts(bits as *const u8, (side * side * 4) as usize).to_vec();
        SelectObject(dc, old);
        let _ = DeleteObject(dib.into());
        let _ = DeleteDC(dc);
        px
    }
}

/// The opaque pixels of the wheel's last frame, and how many of them `shot`
/// has as drawn.
fn opaque_kept(px: &[u8], shot: &[u8]) -> (usize, usize) {
    let (mut opaque, mut same) = (0, 0);
    for (p, s) in px.chunks_exact(4).zip(shot.chunks_exact(4)) {
        if p[3] == 255 {
            opaque += 1;
            same += usize::from(p[..3] == s[..3]);
        }
    }
    (opaque, same)
}

/// PrintWindow's capture of the wheel once its last frame is on screen:
/// a frame presented reaches the screen with one of the compositor's next
/// frames, so it is asked again after each, a second at most. Also how
/// many times it was asked.
fn capture(fw: &FloatWheel) -> (Vec<u8>, u32) {
    let (side, px) = fw.pixels();
    let mut tries = 0;
    loop {
        tries += 1;
        let shot = print_window(fw.hwnd(), side as i32);
        let (opaque, same) = opaque_kept(&px, &shot);
        if same == opaque || tries == 60 {
            return (shot, tries);
        }
        // SAFETY: a plain wait for the compositor's next frame.
        let _ = unsafe { windows::Win32::Graphics::Dwm::DwmFlush() };
    }
}

#[test]
fn print_window_captures_the_floating_wheel() {
    if let Some(why) = no_desktop() {
        println!("skipped: {why}");
        return;
    }
    let rig = Rig::new();
    let png = rig.png("photo.png");
    for palette in fluent::ALL_BUILT_IN {
        let mut fw = rig.open(WheelKind::Convert);
        fw.set_palette(palette);
        bring(&mut fw, std::slice::from_ref(&png));
        fw.frame(Instant::now() + Duration::from_secs(5)).unwrap();
        let (side, px) = fw.pixels();
        let (shot, tries) = capture(&fw);
        // Every opaque pixel of the frame is captured as drawn.
        let (opaque, same) = opaque_kept(&px, &shot);
        println!(
            "{}: {same} of {opaque} opaque pixels captured as drawn, at capture {tries}",
            palette.name
        );
        assert!(opaque > 10_000, "{}", palette.name);
        assert_eq!(same, opaque, "{}", palette.name);
        // An unlit segment is the overlay, by its inner edge on its middle.
        // SAFETY: a plain query on the wheel's window.
        let k = unsafe { GetDpiForWindow(fw.hwnd()) } as f32 / 96.0;
        let c = side as f32 / 2.0;
        let n = fw.wheel().unwrap().segments();
        let (a0, a1) = wheel::segment(1, n);
        let (a, r) = ((a0 + a1) / 2.0, R_INNER + 6.0);
        let (x, y) = ((c + r * a.cos() * k) as u32, (c + r * a.sin() * k) as u32);
        let i = ((y * side + x) * 4) as usize;
        let face = palette.overlay;
        assert_eq!(
            [shot[i + 2], shot[i + 1], shot[i]],
            [face.0, face.1, face.2]
        );
        fw.close();
    }
}

/// Debug builds only: the test hook that loses the device is not in a
/// release build.
#[cfg(debug_assertions)]
#[test]
fn a_frame_lost_with_the_device_is_drawn_again_on_a_new_one() {
    if let Some(why) = no_desktop() {
        println!("skipped: {why}");
        return;
    }
    let rig = Rig::new();
    let later = || Instant::now() + Duration::from_secs(5);
    let png = rig.png("photo.png");
    let mut fw = rig.open(WheelKind::Convert);
    bring(&mut fw, std::slice::from_ref(&png));
    fw.frame(later()).unwrap();
    let (presented, devices) = fw.presented_and_devices();
    assert_eq!(devices, 1);
    let (_, before) = fw.pixels();

    // The present reports the device removed: nothing is presented, and
    // the frame asks to be drawn again.
    fw.lose_device_for_test();
    assert!(fw.frame(later()).unwrap(), "a lost frame asks for another");
    assert_eq!(fw.presented_and_devices(), (presented, devices));
    // The next frame is drawn on a new device, the same as before.
    assert!(!fw.frame(later()).unwrap());
    assert_eq!(fw.presented_and_devices(), (presented + 1, 2));
    assert_eq!(fw.pixels().1, before);
    assert_hit_where_drawn(&fw, "on a new device");
    let (shot, _) = capture(&fw);
    let (opaque, same) = opaque_kept(&before, &shot);
    assert_eq!(same, opaque, "the new device's frame is on screen");

    // Lost again on a frame the frame clock draws: the clock draws
    // the next one on a new device, with no one asking.
    let (presented, _) = fw.presented_and_devices();
    fw.lose_device_for_test();
    let n = fw.wheel().unwrap().segments();
    fw.pointer(slice(&fw, 1, n));
    let end = Instant::now() + Duration::from_secs(2);
    while fw.presented_and_devices() < (presented + 1, 3) && Instant::now() < end {
        // SAFETY: this thread's own messages, pumped as a message loop does.
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        // The frame clock the message loop has: a frame for each wheel
        // that wants one and can take it.
        kiln_app::win::float_wheel::pump_frames();
        std::thread::sleep(Duration::from_millis(2));
    }
    let (now, devices) = fw.presented_and_devices();
    assert!(now > presented, "the frame clock drew the wheel again");
    assert_eq!(devices, 3);

    // Lost again and again: from the third in a row the device is WARP's,
    // and from the tenth a lost frame asks for no other.
    let mut asks = vec![];
    for _ in 0..10 {
        fw.lose_device_for_test();
        asks.push(fw.frame(later()).unwrap());
    }
    assert_eq!(
        asks,
        [true, true, true, true, true, true, true, true, true, false]
    );
    assert!(!fw.frame(later()).unwrap());
    assert!(
        fw.on_warp(),
        "the device after three losses in a row is WARP's"
    );
    fw.close();
}

/// Every monitor's work area and DPI.
fn monitors() -> Vec<(RECT, u32)> {
    extern "system" fn each(m: HMONITOR, _: HDC, _: *mut RECT, out: LPARAM) -> BOOL {
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let (mut x, mut y) = (0u32, 0u32);
        // SAFETY: plain queries into locals; `out` is the Vec passed to
        // EnumDisplayMonitors below, alive for its whole call.
        unsafe {
            if GetMonitorInfoW(m, &mut info).as_bool()
                && GetDpiForMonitor(m, MDT_EFFECTIVE_DPI, &mut x, &mut y).is_ok()
            {
                (*(out.0 as *mut Vec<(RECT, u32)>)).push((info.rcWork, x));
            }
        }
        true.into()
    }
    let mut found: Vec<(RECT, u32)> = vec![];
    // SAFETY: the callback only pushes onto `found`, which outlives the call.
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(each),
            LPARAM(&mut found as *mut Vec<(RECT, u32)> as isize),
        );
    }
    // With `KILN_TEST_PLACE` set, only the monitor that area is on: the
    // wheel opens nowhere else.
    if let Some((x, y, w, h)) = placed() {
        let (cx, cy) = (x + w / 2, y + h / 2);
        found.retain(|(r, _)| cx >= r.left && cx < r.right && cy >= r.top && cy < r.bottom);
    }
    found
}

#[test]
fn each_monitor_has_the_wheel_drawn_and_hit_at_its_own_dpi() {
    if let Some(why) = no_desktop() {
        println!("skipped: {why}");
        return;
    }
    let rig = Rig::new();
    let png = rig.png("photo.png");
    let found = monitors();
    assert!(!found.is_empty());
    for (work, dpi) in found {
        let at = ((work.left + work.right) / 2, (work.top + work.bottom) / 2);
        let mut fw = FloatWheel::open(
            ignore,
            at,
            WheelKind::Convert,
            &Settings::default(),
            Rc::new(Cell::new(false)),
        )
        .unwrap();
        bring(&mut fw, std::slice::from_ref(&png));
        fw.frame(Instant::now() + Duration::from_secs(5)).unwrap();
        // SAFETY: a plain query on the wheel's window.
        assert_eq!(unsafe { GetDpiForWindow(fw.hwnd()) }, dpi);
        let (side, px) = fw.pixels();
        assert_eq!(side, (2.0 * HALF * dpi as f32 / 96.0).ceil() as u32);
        // The hub, opaque, under the point it opened at.
        let r = window_rect(fw.hwnd());
        let (x, y) = ((at.0 - r.left) as u32, (at.1 - r.top) as u32);
        assert_eq!(px[((y * side + x) * 4 + 3) as usize], 255, "{dpi} dpi");
        assert_hit_where_drawn(&fw, &format!("{side} px at {dpi} dpi"));
        let (shot, _) = capture(&fw);
        let (opaque, same) = opaque_kept(&px, &shot);
        assert_eq!(same, opaque, "{dpi} dpi: captured as drawn");
        fw.close();
    }
}
