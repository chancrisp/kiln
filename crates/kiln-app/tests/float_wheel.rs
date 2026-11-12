//! The floating wheel, made in-process, with the test as its owner.
//!
//! No test moves the pointer or sends input. A drag is the wheel's own
//! `IDropTarget` called directly, with a shell data object for scratch
//! files, as OLE would call it.

#![cfg(windows)]

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard, Once};
use std::time::{Duration, Instant};

use kiln_app::layout::Rect;
use kiln_app::settings::Settings;
use kiln_app::theme::fluent::{self, FluentPalette};
use kiln_app::theme::{self, Rgb};
use kiln_app::wheel::{self, Spot, WheelKind, R_INNER, R_OUTER};
use kiln_app::win::float_wheel::{
    self as float_wheel, FloatNews, FloatNote, FloatWheel, Pick, NO_FILES_MS, NO_FORMATS, TITLE,
};
use kiln_core::{Availability, Kiln, Kind};
use windows::core::HSTRING;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO};
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Ole::*;
use windows::Win32::System::SystemServices::{
    MK_CONTROL, MK_LBUTTON, MK_SHIFT, MODIFIERKEYS_FLAGS,
};
use windows::Win32::UI::HiDpi::{
    GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    BHID_DataObject, SHCreateShellItemArrayFromIDLists, SHParseDisplayName,
};
use windows::Win32::UI::WindowsAndMessaging::*;

/// The tests share the screen and the process's DPI awareness: one at a
/// time.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

thread_local! {
    /// What the owner has heard, oldest first.
    static NOTES: RefCell<Vec<FloatNote>> = const { RefCell::new(vec![]) };
}

/// Every effect a source like Explorer allows.
const ALLOWED: DROPEFFECT = DROPEFFECT(DROPEFFECT_COPY.0 | DROPEFFECT_MOVE.0 | DROPEFFECT_LINK.0);

/// The owner's ear: each note the wheel tells it.
fn hear(note: FloatNote) {
    NOTES.with(|n| n.borrow_mut().push(note));
}

/// OLE on this thread, and a scratch folder.
struct Rig {
    dir: tempfile::TempDir,
    _one: MutexGuard<'static, ()>,
}

impl Rig {
    fn new() -> Rig {
        let one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        static SETUP: Once = Once::new();
        SETUP.call_once(|| unsafe {
            // As Kiln's manifest asks: screen pixels, each monitor's DPI.
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        });
        unsafe { OleInitialize(None).unwrap() };
        NOTES.with(|n| n.borrow_mut().clear());
        Rig {
            dir: tempfile::tempdir().unwrap(),
            _one: one,
        }
    }

    fn open(&self, kind: WheelKind) -> FloatWheel {
        self.open_owned(kind, Rc::new(Cell::new(false)))
    }

    /// The wheel, with `own` set while Kiln drags its own results.
    fn open_owned(&self, kind: WheelKind, own: Rc<Cell<bool>>) -> FloatWheel {
        FloatWheel::open(hear, middle(), kind, &Settings::default(), own).unwrap()
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
        unsafe { OleUninitialize() };
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
    let monitor = unsafe {
        MonitorFromPoint(
            POINT { x: 0, y: 0 },
            windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTOPRIMARY,
        )
    };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    assert!(unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool());
    let r = info.rcWork;
    ((r.left + r.right) / 2, (r.top + r.bottom) / 2)
}

/// One second of silence: 8 kHz, 16-bit mono.
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
        assert_eq!(format, GUID_WICPixelFormat32bppBGRA);
        frame.WritePixels(side, side * 4, bgra).unwrap();
        frame.Commit().unwrap();
        encoder.Commit().unwrap();
    }
}

/// A shell data object for `paths`, as Explorer hands one to a drag.
fn data_object(paths: &[PathBuf]) -> IDataObject {
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

/// Run the message loop for `ms`, or until `done` holds.
fn pump_until(ms: u64, mut done: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_millis(ms);
    loop {
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
        if done() || Instant::now() >= end {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn pump(ms: u64) {
    pump_until(ms, || false);
}

fn notes() -> Vec<FloatNote> {
    NOTES.with(|n| std::mem::take(&mut *n.borrow_mut()))
}

/// Wait for the owner to hear news from `fw`, and take it.
fn next_news(fw: &FloatWheel) -> FloatNews {
    let mut got = None;
    pump_until(2000, || {
        got = notes().into_iter().find(|n| n.is_from(fw.hwnd()));
        got.is_some()
    });
    got.expect("the owner heard nothing").news
}

fn names(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

fn keys(k: &[MODIFIERKEYS_FLAGS]) -> MODIFIERKEYS_FLAGS {
    MODIFIERKEYS_FLAGS(k.iter().fold(0, |a, f| a | f.0))
}

fn enter(fw: &FloatWheel, data: &IDataObject, at: (i32, i32), k: MODIFIERKEYS_FLAGS) -> DROPEFFECT {
    let mut effect = ALLOWED;
    unsafe {
        fw.drop_target()
            .DragEnter(data, k, POINTL { x: at.0, y: at.1 }, &mut effect)
            .unwrap()
    };
    effect
}

fn over(fw: &FloatWheel, at: (i32, i32), k: MODIFIERKEYS_FLAGS, allowed: DROPEFFECT) -> DROPEFFECT {
    let mut effect = allowed;
    unsafe {
        fw.drop_target()
            .DragOver(k, POINTL { x: at.0, y: at.1 }, &mut effect)
            .unwrap()
    };
    effect
}

fn drop_on(
    fw: &FloatWheel,
    data: &IDataObject,
    at: (i32, i32),
    k: MODIFIERKEYS_FLAGS,
    allowed: DROPEFFECT,
) -> DROPEFFECT {
    let mut effect = allowed;
    unsafe {
        fw.drop_target()
            .Drop(data, k, POINTL { x: at.0, y: at.1 }, &mut effect)
            .unwrap()
    };
    effect
}

/// A drag with `paths` enters at the wheel's centre; the owner hears of
/// the files and fills the wheel, as Kiln does.
fn bring(fw: &mut FloatWheel, paths: &[PathBuf]) -> IDataObject {
    let data = data_object(paths);
    // Over the hub: nothing to drop on yet, and the hub is never a slice.
    assert_eq!(enter(fw, &data, middle(), MK_LBUTTON), DROPEFFECT_NONE);
    let FloatNews::Files(got) = next_news(fw) else {
        panic!("expected files");
    };
    assert_eq!(names(&got), names(paths));
    fw.files_arrived(got, &Kiln::new(), &Availability::all());
    data
}

/// The screen point `(dx, dy)` DIPs from the wheel's centre.
fn at_dips(fw: &FloatWheel, dx: f32, dy: f32) -> (i32, i32) {
    let k = unsafe { GetDpiForWindow(fw.hwnd()) } as f32 / 96.0;
    let (x, y) = middle();
    (x + (dx * k).round() as i32, y + (dy * k).round() as i32)
}

/// The middle of inner slice `i` of `n`, on screen.
fn slice(fw: &FloatWheel, i: usize, n: usize) -> (i32, i32) {
    let (a0, a1) = wheel::segment(i, n);
    let (a, r) = ((a0 + a1) / 2.0, (R_INNER + R_OUTER) / 2.0);
    at_dips(fw, r * a.cos(), r * a.sin())
}

/// The pixel at screen point `at` in the last frame: B, G, R, A.
fn pixel(fw: &FloatWheel, at: (i32, i32)) -> [u8; 4] {
    let mut r = RECT::default();
    unsafe { GetWindowRect(fw.hwnd(), &mut r).unwrap() };
    let (side, px) = fw.pixels();
    let (x, y) = ((at.0 - r.left) as usize, (at.1 - r.top) as usize);
    let i = (y * side as usize + x) * 4;
    [px[i], px[i + 1], px[i + 2], px[i + 3]]
}

fn max_alpha(fw: &FloatWheel) -> u8 {
    let (_, px) = fw.pixels();
    px.chunks_exact(4).map(|p| p[3]).max().unwrap()
}

#[test]
fn files_fill_the_slices_with_what_they_can_become() {
    let rig = Rig::new();
    let (kiln, all) = (Kiln::new(), Availability::all());
    let mut fw = rig.open(WheelKind::Convert);
    assert!(fw.wheel().is_none());
    assert_eq!(fw.hub_text(), None);

    let png = rig.png("photo.png");
    bring(&mut fw, std::slice::from_ref(&png));
    let w = fw.wheel().expect("the files fill the wheel");
    let (items, flyout) = wheel::offering(
        WheelKind::Convert,
        &kiln,
        &all,
        std::slice::from_ref(&png),
        &wheel::no_subtitles,
    );
    assert!(!items.is_empty());
    assert_eq!((w.items, w.flyout), (items, flyout));
    // One file: its code in the hub, what to do under it, and its kind.
    assert_eq!(fw.hub_text().as_deref(), Some("PNG"));
    assert_eq!(fw.hub_note().as_deref(), Some("Pick a format"));
    assert_eq!(fw.wheel().unwrap().source_kind, Some(Kind::Image));

    // A flip shows the tools for the same files, and back.
    fw.flip();
    assert_eq!(fw.kind(), WheelKind::Tools);
    let tools = wheel::offering(
        WheelKind::Tools,
        &kiln,
        &all,
        std::slice::from_ref(&png),
        &wheel::no_subtitles,
    );
    assert!(!tools.0.is_empty());
    let w = fw.wheel().unwrap();
    assert_eq!((w.items, w.flyout), tools);
    fw.flip();
    assert_eq!(fw.kind(), WheelKind::Convert);
    fw.close();

    // Sound becomes sound.
    let mut fw = rig.open(WheelKind::Convert);
    let wav = rig.wav("tone.wav");
    bring(&mut fw, std::slice::from_ref(&wav));
    let w = fw.wheel().unwrap();
    let (items, _) = wheel::offering(
        WheelKind::Convert,
        &kiln,
        &all,
        std::slice::from_ref(&wav),
        &wheel::no_subtitles,
    );
    assert_eq!(w.items, items);
    assert!(w
        .items
        .iter()
        .all(|i| i.kind == Some(kiln_core::Kind::Audio)));
    fw.close();

    // Several files: what they can all become, and the count in the hub.
    let mut fw = rig.open(WheelKind::Convert);
    let three = vec![rig.png("a.png"), rig.png("b.png"), rig.png("c.png")];
    bring(&mut fw, &three);
    let (items, _) = wheel::offering(
        WheelKind::Convert,
        &kiln,
        &all,
        &three,
        &wheel::no_subtitles,
    );
    assert_eq!(fw.wheel().unwrap().items, items);
    assert_eq!(fw.hub_text().as_deref(), Some("PNG \u{b7} 3"));
    fw.close();

    // Files with nothing in common: no slices, "No formats" in the hub,
    // and the in-window wheel's sentence in the caption under it.
    let mut fw = rig.open(WheelKind::Convert);
    let mixed = vec![png.clone(), wav.clone()];
    bring(&mut fw, &mixed);
    assert_eq!(fw.wheel().unwrap().segments(), 0);
    assert_eq!(fw.hub_text().as_deref(), Some(NO_FORMATS));
    let offer = wheel::offer(WheelKind::Convert, &kiln, &all, &mixed);
    assert_eq!(
        fw.caption(),
        wheel::offer_sentence(WheelKind::Convert, &offer)
    );
    assert!(fw.caption().is_some());
    fw.close();
}

#[test]
fn with_nothing_to_offer_the_reason_sits_under_the_wheel_and_it_goes_with_the_drag() {
    let rig = Rig::new();
    let mut fw = rig.open(WheelKind::Convert);
    let mixed = vec![rig.png("photo.png"), rig.wav("tone.wav")];
    let data = bring(&mut fw, &mixed);
    assert_eq!(fw.hub_text().as_deref(), Some(NO_FORMATS));
    let caption = fw.caption().expect("the reason, in the caption");
    assert!(caption.len() > NO_FORMATS.len());
    // Bloomed: the caption's pill is drawn under the dish, opaque, in the
    // surface colour at its edge.
    for (name, palette) in fluent::ALL_BUILT_IN.map(|p| (p.name, p)) {
        fw.set_palette(palette);
        fw.frame(Instant::now() + Duration::from_secs(5)).unwrap();
        let under = pixel(&fw, at_dips(&fw, 0.0, R_OUTER + 16.0 + 3.0));
        assert_eq!(under[3], 255, "the pill under the wheel: {under:?}");
        let (side, px) = fw.pixels();
        write_png(
            &shots_dir().join(format!("{name}-convert-no-formats.png")),
            side,
            &straight(&px),
        );
    }
    // Nothing to drop on, anywhere.
    assert_eq!(
        over(&fw, slice(&fw, 0, 1), MK_LBUTTON, ALLOWED),
        DROPEFFECT_NONE
    );
    assert!(!fw.lit());
    // The drag leaves, and the wheel fades out with it, then closes; with
    // animations off it goes at once. The owner hears as the fade starts,
    // and from then on it is no drop target.
    unsafe { fw.drop_target().DragLeave().unwrap() };
    if theme::fluent::motion::animations() {
        assert!(fw.is_open(), "it fades before it goes");
        assert!(fw.closing());
    }
    let left = Instant::now();
    assert_eq!(next_news(&fw), FloatNews::Closed);
    assert!(left.elapsed() < Duration::from_millis(100), "told at once");
    // Back in during the fade: it doesn't come back.
    if fw.is_open() {
        assert_eq!(enter(&fw, &data, middle(), MK_LBUTTON), DROPEFFECT_NONE);
        assert!(fw.closing());
    }
    pump_until(1000, || !fw.is_open());
    assert!(!fw.is_open());
    assert!(left.elapsed() < Duration::from_secs(1));
    assert!(notes().iter().all(|n| !n.is_from(fw.hwnd())), "told once");
    drop(data);
}

#[test]
fn a_closing_wheel_fades_and_takes_no_drop() {
    let rig = Rig::new();
    let mut fw = rig.open(WheelKind::Convert);
    let png = rig.png("photo.png");
    let data = bring(&mut fw, std::slice::from_ref(&png));
    let n = fw.wheel().unwrap().segments();
    let on = slice(&fw, 1, n);
    assert_eq!(over(&fw, on, MK_LBUTTON, ALLOWED), DROPEFFECT_COPY);
    let hwnd = fw.hwnd();
    let target = fw.drop_target();
    let closed = Instant::now();
    match fw.fade_out() {
        // Fading, as the in-window wheel closes: no drop target any more.
        Some(fading) => {
            assert!(theme::fluent::motion::animations());
            assert!(fading.closing());
            assert!(fading.is_open());
            let mut effect = ALLOWED;
            unsafe {
                target
                    .DragOver(MK_LBUTTON, POINTL { x: on.0, y: on.1 }, &mut effect)
                    .unwrap()
            };
            assert_eq!(effect, DROPEFFECT_NONE);
            let mut effect = ALLOWED;
            unsafe {
                target
                    .Drop(&data, MK_LBUTTON, POINTL { x: on.0, y: on.1 }, &mut effect)
                    .unwrap()
            };
            assert_eq!(effect, DROPEFFECT_NONE);
            // It goes once faded, and never says a word about it.
            pump_until(1000, || !fading.is_open());
            assert!(!fading.is_open());
            assert!(closed.elapsed() >= Duration::from_millis(100));
            assert!(closed.elapsed() < Duration::from_secs(1));
            assert!(notes().iter().all(|n| !n.is_from(hwnd)));
        }
        // With animations off it went at once.
        None => assert!(!theme::fluent::motion::animations()),
    }
    assert!(!unsafe { IsWindow(Some(hwnd)) }.as_bool());
    drop(data);

    // Dropped while fading, it goes at once: a new wheel opens alone.
    let mut fw = rig.open(WheelKind::Convert);
    let data = bring(&mut fw, std::slice::from_ref(&png));
    let hwnd = fw.hwnd();
    drop(fw.fade_out());
    assert!(!unsafe { IsWindow(Some(hwnd)) }.as_bool());
    drop(data);

    // Before files arrive there is nothing to fade: it goes at once.
    let fw = rig.open(WheelKind::Convert);
    let hwnd = fw.hwnd();
    assert!(fw.fade_out().is_none());
    assert!(!unsafe { IsWindow(Some(hwnd)) }.as_bool());
}

#[test]
fn kilns_own_drag_is_refused() {
    let rig = Rig::new();
    let own = Rc::new(Cell::new(true));
    let fw = rig.open_owned(WheelKind::Convert, own.clone());
    let png = rig.png("held.png");
    let data = data_object(std::slice::from_ref(&png));
    // Kiln dragging out its own result: refused, and the wheel hears
    // nothing of it.
    assert_eq!(enter(&fw, &data, middle(), MK_LBUTTON), DROPEFFECT_NONE);
    assert_eq!(
        drop_on(&fw, &data, middle(), MK_LBUTTON, ALLOWED),
        DROPEFFECT_NONE
    );
    pump(100);
    assert!(notes().iter().all(|n| !n.is_from(fw.hwnd())));
    assert!(fw.wheel().is_none());
    fw.close();
}

#[test]
fn a_move_only_drop_picks_nothing() {
    let rig = Rig::new();
    let mut fw = rig.open(WheelKind::Convert);
    let png = rig.png("photo.png");
    let data = bring(&mut fw, std::slice::from_ref(&png));
    let n = fw.wheel().unwrap().segments();
    let on = slice(&fw, 0, n);
    assert_eq!(over(&fw, on, MK_LBUTTON, ALLOWED), DROPEFFECT_COPY);
    assert!(fw.lit(), "a release now may be a drop");
    // The source would only move it: no drop, and no pick for the owner
    // to convert.
    assert_eq!(
        drop_on(&fw, &data, on, MK_LBUTTON, DROPEFFECT_MOVE),
        DROPEFFECT_NONE
    );
    assert!(matches!(
        next_news(&fw),
        FloatNews::Dropped { picked: None, .. }
    ));
    fw.close();
}

#[test]
fn a_drop_on_a_slice_reports_that_spot_and_answers_copy() {
    let rig = Rig::new();
    let mut fw = rig.open(WheelKind::Convert);
    let png = rig.png("photo.png");
    let data = bring(&mut fw, std::slice::from_ref(&png));
    let w = fw.wheel().unwrap();
    let n = w.segments();
    let on = slice(&fw, 1, n);

    assert_eq!(over(&fw, on, MK_LBUTTON, ALLOWED), DROPEFFECT_COPY);
    // The hub names where the slice under the pointer goes.
    assert_eq!(
        fw.hub_text(),
        Some(format!("PNG \u{2192} {}", w.items[1].label))
    );

    assert_eq!(
        drop_on(&fw, &data, on, MK_LBUTTON, ALLOWED),
        DROPEFFECT_COPY
    );
    match next_news(&fw) {
        FloatNews::Dropped { picked, paths } => {
            // The item itself, so a flip after the drop changes nothing.
            let want = Pick {
                kind: WheelKind::Convert,
                spot: Spot::Inner(1),
                item: w.items[1].clone(),
            };
            assert_eq!(picked, Some(want));
            assert_eq!(names(&paths), names(std::slice::from_ref(&png)));
        }
        other => panic!("expected a drop, got {other:?}"),
    }
    assert!(fw.is_open(), "the owner closes it, not the drop");
    fw.close();
}

#[test]
fn shift_held_at_drop_still_answers_copy() {
    let rig = Rig::new();
    let mut fw = rig.open(WheelKind::Convert);
    let png = rig.png("photo.png");
    let data = bring(&mut fw, std::slice::from_ref(&png));
    let n = fw.wheel().unwrap().segments();
    let on = slice(&fw, 0, n);
    let move_or_copy = DROPEFFECT(DROPEFFECT_MOVE.0 | DROPEFFECT_COPY.0);
    // Explorer's Shift asks for a move; Ctrl+Shift for a link.
    for k in [
        keys(&[MK_LBUTTON, MK_SHIFT]),
        keys(&[MK_LBUTTON, MK_SHIFT, MK_CONTROL]),
        keys(&[MK_SHIFT]),
    ] {
        for allowed in [ALLOWED, move_or_copy] {
            assert_eq!(over(&fw, on, k, allowed), DROPEFFECT_COPY);
        }
    }
    let k = keys(&[MK_LBUTTON, MK_SHIFT]);
    assert_eq!(drop_on(&fw, &data, on, k, move_or_copy), DROPEFFECT_COPY);
    let FloatNews::Dropped { picked, .. } = next_news(&fw) else {
        panic!("expected a drop");
    };
    assert_eq!(picked.map(|p| p.spot), Some(Spot::Inner(0)));

    // The drag leaves and comes back straight onto a slice with Shift
    // held: copy from the first moment.
    unsafe { fw.drop_target().DragLeave().unwrap() };
    assert_eq!(enter(&fw, &data, on, k), DROPEFFECT_COPY);

    // A source that can only be moved is never moved, over a slice or
    // dropped on one.
    assert_eq!(over(&fw, on, k, DROPEFFECT_MOVE), DROPEFFECT_NONE);
    assert_eq!(drop_on(&fw, &data, on, k, DROPEFFECT_MOVE), DROPEFFECT_NONE);
    fw.close();
}

#[test]
fn outside_the_slices_answers_none() {
    let rig = Rig::new();
    let mut fw = rig.open(WheelKind::Convert);
    let png = rig.png("photo.png");
    let data = data_object(std::slice::from_ref(&png));

    // Before the slices are filled, where one will be is nothing yet.
    assert_eq!(enter(&fw, &data, middle(), MK_LBUTTON), DROPEFFECT_NONE);
    assert_eq!(
        over(&fw, at_dips(&fw, 0.0, -96.0), MK_LBUTTON, ALLOWED),
        DROPEFFECT_NONE
    );
    let FloatNews::Files(got) = next_news(&fw) else {
        panic!("expected files");
    };
    fw.files_arrived(got, &Kiln::new(), &Availability::all());

    let shift = keys(&[MK_LBUTTON, MK_SHIFT]);
    let hub = middle();
    let past = at_dips(&fw, 0.0, -(R_OUTER + wheel::REACH + 20.0));
    let corner = at_dips(&fw, -250.0, -250.0);
    for at in [hub, past, corner] {
        for k in [MK_LBUTTON, shift] {
            assert_eq!(over(&fw, at, k, ALLOWED), DROPEFFECT_NONE, "at {at:?}");
        }
    }
    // A drop on the hub drops nothing, and says so.
    assert_eq!(drop_on(&fw, &data, hub, shift, ALLOWED), DROPEFFECT_NONE);
    assert!(matches!(
        next_news(&fw),
        FloatNews::Dropped { picked: None, .. }
    ));
    fw.close();
}

#[test]
fn the_reach_past_the_slices_receives_the_drop() {
    let rig = Rig::new();
    let mut fw = rig.open(WheelKind::Convert);
    let png = rig.png("photo.png");
    bring(&mut fw, std::slice::from_ref(&png));
    let w = fw.wheel().unwrap();
    assert!(w.has_more(), "a PNG's formats fill More");
    let later = || Instant::now() + Duration::from_secs(5);
    let alpha = |fw: &FloatWheel, r: f32| pixel(fw, at_dips(fw, 0.0, -r))[3];

    // Past the slices but within reach: a slice lights, and the window is
    // there to take the drop, not the app beneath.
    let edge = R_OUTER + wheel::REACH;
    fw.frame(later()).unwrap();
    assert!(alpha(&fw, edge - 3.0) > 0);
    assert_eq!(alpha(&fw, edge + 3.0), 0);
    let inside = at_dips(&fw, 0.0, -(edge - 3.0));
    assert_eq!(fw.pointer(inside), Some(Spot::Inner(0)));
    assert_eq!(over(&fw, inside, MK_LBUTTON, ALLOWED), DROPEFFECT_COPY);
    let beyond = at_dips(&fw, 0.0, -(edge + 3.0));
    assert_eq!(fw.pointer(beyond), None);

    // Pointing at More opens its list beside the ring: its lines light
    // and take the drop.
    let more = slice(&fw, w.items.len(), w.segments());
    assert_eq!(fw.pointer(more), Some(Spot::Inner(w.items.len())));
    fw.frame(later()).unwrap();
    let (rows, place) = list_of(&fw);
    let line = rows
        .iter()
        .position(|r| matches!(r, wheel::FlyoutRow::Item(..)))
        .unwrap();
    let r = place.rows[line];
    let on = list_point(&fw, r.x + r.w / 2.0, r.y + r.h / 2.0);
    assert_eq!(pixel(&fw, on)[3], 255);
    assert_eq!(fw.pointer(on), Some(Spot::Flyout(0)));
    assert_eq!(over(&fw, on, MK_LBUTTON, ALLOWED), DROPEFFECT_COPY);
    fw.close();
}

/// More's list as the floating wheel lays it out: its lines and where
/// they are, in the window's DIPs.
fn list_of(fw: &FloatWheel) -> (Vec<wheel::FlyoutRow<'static>>, wheel::FlyoutPlace) {
    let w: &'static wheel::Wheel = Box::leak(Box::new(fw.wheel().unwrap()));
    let side = fw.pixels().0 as f32 / dpi_k(fw);
    let rows = w.flyout_rows(None);
    let place = wheel::flyout_place(w, &rows, Rect::new(0.0, 0.0, side, side), 0.0);
    (rows, place)
}

/// The window's pixels a DIP.
fn dpi_k(fw: &FloatWheel) -> f32 {
    (unsafe { GetDpiForWindow(fw.hwnd()) }) as f32 / 96.0
}

/// The screen point at `(x, y)` DIPs in the floating window.
fn list_point(fw: &FloatWheel, x: f32, y: f32) -> (i32, i32) {
    let c = fw.wheel().unwrap().center;
    at_dips(fw, x - c.0, y - c.1)
}

#[test]
fn no_files_within_250_ms_closes_unseen() {
    let rig = Rig::new();
    let opened = Instant::now();
    let fw = rig.open(WheelKind::Convert);
    let hwnd = fw.hwnd();
    // Only the hub, at alpha 1: there to be hit, never seen.
    assert_eq!(max_alpha(&fw), 1);
    assert_eq!(pixel(&fw, middle())[3], 1);
    assert_eq!(pixel(&fw, at_dips(&fw, 0.0, -96.0))[3], 0);

    assert_eq!(next_news(&fw), FloatNews::Closed);
    assert!(opened.elapsed() >= Duration::from_millis(NO_FILES_MS as u64 - 10));
    assert!(!fw.is_open());
    assert!(!unsafe { IsWindow(Some(hwnd)) }.as_bool());
    assert_eq!(max_alpha(&fw), 1, "it never drew more than the hub");
    fw.close();

    // Files within the time keep it open.
    let mut fw = rig.open(WheelKind::Convert);
    let png = rig.png("photo.png");
    bring(&mut fw, std::slice::from_ref(&png));
    pump(NO_FILES_MS as u64 + 150);
    assert!(fw.is_open());
    assert!(notes().is_empty());
    let kept = fw.hwnd();
    fw.close();
    assert!(!unsafe { IsWindow(Some(kept)) }.as_bool());
}

#[test]
fn the_window_never_takes_focus() {
    let rig = Rig::new();
    let before = unsafe { GetForegroundWindow() };
    let mut fw = rig.open(WheelKind::Convert);
    let hwnd = fw.hwnd();
    pump(40);
    assert_eq!(unsafe { GetForegroundWindow() }, before);

    let ex = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
    for flag in [
        WS_EX_LAYERED,
        WS_EX_TOPMOST,
        WS_EX_TOOLWINDOW,
        WS_EX_NOACTIVATE,
    ] {
        assert_ne!(ex & flag.0, 0, "{flag:?}");
    }
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32;
    assert_ne!(style & WS_POPUP.0, 0);
    assert!(unsafe { IsWindowVisible(hwnd) }.as_bool());
    // Its name to a screen reader.
    let mut buf = [0u16; 64];
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) } as usize;
    assert_eq!(String::from_utf16_lossy(&buf[..n]), TITLE);
    assert_eq!(TITLE, "Kiln format wheel");
    // A click would not activate it.
    let r = unsafe { SendMessageW(hwnd, WM_MOUSEACTIVATE, None, None) };
    assert_eq!(r.0 as u32, MA_NOACTIVATE);

    let png = rig.png("photo.png");
    bring(&mut fw, std::slice::from_ref(&png));
    pump(300);
    assert_eq!(unsafe { GetForegroundWindow() }, before);
    assert_ne!(unsafe { GetActiveWindow() }, hwnd);
    fw.close();
}

fn contrast() -> FluentPalette {
    fluent::contrast_palette(theme::SysColors {
        window: Rgb(0, 0, 0),
        window_text: Rgb(255, 255, 255),
        highlight: Rgb(26, 235, 255),
        highlight_text: Rgb(0, 0, 0),
        gray_text: Rgb(63, 242, 63),
        hotlight: Rgb(255, 255, 0),
        button_face: Rgb(0, 0, 0),
        button_text: Rgb(255, 255, 255),
    })
}

/// `C:\kiln\target\drag-wheel-shots`, or wherever the build's target is:
/// three folders up from this test's executable.
fn shots_dir() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let target = exe.ancestors().nth(3).unwrap().to_path_buf();
    let dir = target.join("drag-wheel-shots");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Premultiplied BGRA made straight, for a PNG.
fn straight(px: &[u8]) -> Vec<u8> {
    px.chunks_exact(4)
        .flat_map(|p| {
            let a = p[3] as u32;
            // A clear pixel has no colour to keep.
            let un = |c: u8| {
                (c as u32 * 255 + a / 2)
                    .checked_div(a)
                    .map_or(0, |v| v.min(255) as u8)
            };
            [un(p[0]), un(p[1]), un(p[2]), p[3]]
        })
        .collect()
}

#[test]
fn mores_list_and_its_shadow_stay_inside_the_window_with_its_lit_line() {
    let rig = Rig::new();
    let png = rig.png("photo.png");
    for palette in fluent::ALL_BUILT_IN {
        let mut fw = rig.open(WheelKind::Convert);
        fw.set_palette(palette);
        bring(&mut fw, std::slice::from_ref(&png));
        let w = fw.wheel().unwrap();
        assert!(w.has_more(), "a PNG has more formats than fit");
        let n = w.segments();
        // Onto More, which opens its list, then onto each of its lines in
        // turn: each lights, and nothing reaches the window's edge.
        assert_eq!(fw.pointer(slice(&fw, n - 1, n)), Some(Spot::Inner(n - 1)));
        let (rows, place) = list_of(&fw);
        for (row, r) in rows.iter().zip(&place.rows) {
            let wheel::FlyoutRow::Item(spot, _) = row else {
                continue;
            };
            let at = list_point(&fw, r.right() - 6.0, r.y + 3.0);
            assert_eq!(fw.pointer(at), Some(*spot));
            assert!(!fw.frame(Instant::now() + Duration::from_secs(5)).unwrap());
            let (side, px) = fw.pixels();
            let a = |x: u32, y: u32| px[((y * side + x) * 4 + 3) as usize];
            for i in 0..side {
                for (x, y) in [(i, 0), (i, side - 1), (0, i), (side - 1, i)] {
                    assert_eq!(a(x, y), 0, "{} {spot:?} at ({x}, {y})", palette.name);
                }
            }
            // The lit line has the hover's fill over the overlay.
            let fill = palette.control_hover.over(palette.overlay);
            let p = pixel(&fw, at);
            let near = [p[2], p[1], p[0]]
                .iter()
                .zip([fill.0, fill.1, fill.2])
                .all(|(a, b)| a.abs_diff(b) <= 2);
            assert!(near, "{} {spot:?}: {p:?}", palette.name);
        }
        fw.close();
    }
}

#[test]
fn light_and_dark_frames_render() {
    let rig = Rig::new();
    let one = vec![rig.png("photo.png")];
    let three = vec![rig.png("a.png"), rig.png("b.png"), rig.png("c.png")];
    let dir = shots_dir();
    let themes: Vec<_> = fluent::ALL_BUILT_IN
        .into_iter()
        .map(|p| (p.name, p))
        .chain([("contrast", contrast())])
        .collect();
    let themes_n = themes.len();
    let mut written = 0;
    for (theme_name, palette) in themes {
        for kind in [WheelKind::Convert, WheelKind::Tools] {
            for (files_name, paths) in [("1-file", &one), ("3-files", &three)] {
                for hovered in [true, false] {
                    let mut fw = rig.open(kind);
                    fw.set_palette(palette);
                    bring(&mut fw, paths);
                    let n = fw.wheel().unwrap().segments();
                    assert!(n > 1, "{theme_name} {kind:?} {files_name}");
                    let lit = slice(&fw, 1, n);
                    if hovered {
                        assert_eq!(fw.pointer(lit), Some(Spot::Inner(1)));
                    }
                    // Long after it opened: fully bloomed, at rest.
                    let later = Instant::now() + Duration::from_secs(5);
                    assert!(!fw.frame(later).unwrap());

                    let (side, px) = fw.pixels();
                    let a = |x: u32, y: u32| px[((y * side + x) * 4 + 3) as usize];
                    // Clear outside the wheel and its shadow.
                    for (x, y) in [(0, 0), (side - 1, 0), (0, side - 1), (side - 1, side - 1)] {
                        assert_eq!(a(x, y), 0, "{theme_name} corner");
                    }
                    // The wheel itself is opaque: the hub, and the slices.
                    assert_eq!(pixel(&fw, middle())[3], 255);
                    assert_eq!(pixel(&fw, lit)[3], 255);
                    assert_eq!(pixel(&fw, slice(&fw, 0, n))[3], 255);
                    // An unlit segment is the overlay exactly, by its
                    // inner edge on its middle, clear of its label.
                    let unlit = if hovered { 0 } else { 1 };
                    let probe = |i: usize| {
                        let (a0, a1) = wheel::segment(i, n);
                        let (a, r) = ((a0 + a1) / 2.0, R_INNER + 6.0);
                        pixel(&fw, at_dips(&fw, r * a.cos(), r * a.sin()))
                    };
                    let face = palette.overlay;
                    let p = probe(unlit);
                    assert_eq!(
                        [p[2], p[1], p[0], p[3]],
                        [face.0, face.1, face.2, 255],
                        "{theme_name} {kind:?} {files_name}: segment {unlit}"
                    );
                    if hovered {
                        // The lit segment is solid accent, where nothing
                        // is drawn on it.
                        let p = probe(1);
                        let lit = palette.accent;
                        assert_eq!(
                            [p[2], p[1], p[0]],
                            [lit.0, lit.1, lit.2],
                            "{theme_name}: lit"
                        );
                    }
                    let kind_name = match kind {
                        WheelKind::Tools => "tools",
                        _ => "convert",
                    };
                    let rest = if hovered { "" } else { "-rest" };
                    let name = format!("{theme_name}-{kind_name}-{files_name}{rest}.png");
                    write_png(&dir.join(name), side, &straight(&px));
                    written += 1;
                    fw.close();
                }
            }
        }
    }
    // Each theme: two wheels, one file or three, hovered or at rest.
    assert_eq!(written, themes_n * 8);
}

#[test]
fn each_wheel_keeps_its_own_frame_deadline() {
    let rig = Rig::new();
    let png = rig.png("a.png");
    // A fading wheel and a new one, both blooming at once.
    let mut a = rig.open(WheelKind::Convert);
    let _a = bring(&mut a, std::slice::from_ref(&png));
    let mut b = rig.open(WheelKind::Convert);
    let _b = bring(&mut b, std::slice::from_ref(&png));
    // Only the first draws, again and again, as a wheel whose waitable
    // signals does.
    for _ in 0..5 {
        std::thread::sleep(Duration::from_millis(20));
        a.frame(Instant::now()).unwrap();
    }
    let waits = float_wheel::frame_waits();
    assert_eq!(waits.len(), 2, "both want frames");
    // The second's deadlines still count from its own last frame, so it
    // comes due however often the first draws.
    assert!(
        waits[0].framed >= waits[1].framed + Duration::from_millis(80),
        "the first's last frame is later than the second's"
    );
    assert!(waits[1].framed.elapsed() >= Duration::from_millis(80));
}
