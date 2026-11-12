//! The floating wheel: Kiln's format wheel in a clear window of its own,
//! under the pointer over any app while a file is being dragged.
//!
//! - **The window** is a layered popup that is topmost and never active:
//!   it takes no focus and no activation, and has no taskbar button.
//! - **Drawing:** Direct2D draws into a premultiplied swap chain that
//!   DirectComposition shows in the window, which has no redirection
//!   bitmap (`render::gpu`, as Kiln's main window draws). Drawing happens
//!   on the thread that made the window, the UI thread, since the wheel's
//!   text formats are made once per thread.
//! - **What it lets through:** the window is hit only where its frame is
//!   drawn, with any alpha at all, as a window drawn with
//!   `UpdateLayeredWindow` is: each frame is read back, and the window's
//!   region is set to its drawn pixels whenever they change. Elsewhere
//!   clicks, drags and drops reach the window below, whichever app it is.
//! - **Before files arrive** only the hub is drawn, at alpha 1/255: nothing
//!   shows, but OLE's hit test finds the window under the pointer. If no
//!   drag with files reaches it within `NO_FILES_MS`, it closes itself.
//! - **Leaving:** once the drag leaves, the wheel closes itself when the
//!   pointer goes more than `REACH` past More's ring, or at once when the
//!   files can become nothing. Back within that margin, it stays open.
//! - **Closing:** every close the owner sees fades out over the in-window
//!   wheel's close (`anim::wheel_close`, 167 ms), or goes at once with animations off. From
//!   its first frame the fading wheel is no drop target: it revokes its
//!   registration and answers no drop to a drag still over it. Its owner
//!   hears `Closed` as the fade starts, not as it ends.
//! - **Drops:** it is its own OLE drop target. Over a slice it answers
//!   copy and anywhere else no drop, whatever keys are held, so the source
//!   is never moved.
//! - **The owner** hears what happens through the [`Tell`] it opened the
//!   wheel with, a `FloatNote` at a time, and fills the slices with
//!   `files_arrived`. Kiln's window queues each note on its channel
//!   (`channel::Wake`), so none is acted on inside the wheel's own call.
//! - **Frames:** while the wheel blooms, More pours or it fades, it wants
//!   frames, and the message loop draws one each time its swap chain's
//!   frame-latency waitable object is signalled (`frame_waits`,
//!   `draw_frame`), once every pending message is handled. It stops
//!   wanting them once nothing moves. The owner doesn't drive frames.

use std::cell::{Cell, OnceCell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use windows::core::{w, Error, Result, HRESULT, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::{IDropTarget, RegisterDragDrop, RevokeDragDrop};
use windows::Win32::System::Threading::WaitForSingleObject;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_numerics::{Matrix3x2, Vector2};

use kiln_core::{Availability, Kiln};

use crate::anim;
use crate::layout::Rect;
use crate::settings::Settings;
use crate::theme::{self, fluent::FluentPalette};
use crate::wheel::{
    self, FlyoutPlace, FlyoutRow, Item, Spot, Wheel, WheelKind, WheelMotion, FLYOUT_GAP,
    FLYOUT_W_DRAG, REACH, R_INNER, R_OUTER,
};

use super::crash;
use super::drop::{DragEvent, DragKind, DropTarget};
use super::render::gpu::{self, Gpu, Losses};
use super::render::{draw_wheel_on, WheelExtras, WheelResources, WheelView};

/// How long the wheel waits for a drag with files before it closes
/// unseen: dragged text, a window being moved or a selection never
/// brings files.
pub const NO_FILES_MS: u32 = 250;

/// The window's title, which is its name to screen readers.
pub const TITLE: &str = "Kiln format wheel";

const CLASS: PCWSTR = w!("KilnFloatWheel");
const NO_FILES_TIMER: usize = 1;
/// Watches the pointer after the drag left the window.
const LEAVE_TIMER: usize = 3;
const LEAVE_MS: u32 = 50;

/// What the hub says when the files can become nothing on that ring; the
/// reason is in the caption under the wheel.
pub const NO_FORMATS: &str = "No formats";
pub const NO_TOOLS: &str = "No tools";

/// How far the flyout shadow (`0 8 16`) reaches past what casts it, in
/// DIPs, with room to spare.
const SHADOW: f32 = 24.0;

/// How far the window reaches either side of the wheel's centre, in DIPs:
/// the ring, More's list beside it, and the list's shadow.
pub const HALF: f32 = R_OUTER + FLYOUT_GAP + FLYOUT_W_DRAG + SHADOW;

// The window holds all the wheel draws: the ring and the pointer's reach
// past it, and More's list on either side.
const _: () = assert!(R_OUTER + REACH + SHADOW <= HALF);

/// What the floating wheel tells its owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FloatNews {
    /// A drag brought these files. The owner fills the wheel with
    /// `files_arrived`.
    Files(Vec<PathBuf>),
    /// The drag was dropped on the wheel: on `spot`, a slice, or on no
    /// slice at all. Either way the owner closes the wheel.
    Dropped {
        picked: Option<Pick>,
        paths: Vec<PathBuf>,
    },
    /// No files came within `NO_FILES_MS`. The window has closed itself
    /// without showing anything.
    Closed,
}

/// The slice a drop landed on: the item itself and the wheel it was on,
/// so a flip after the drop can't change what was picked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    pub kind: WheelKind,
    pub spot: Spot,
    pub item: Item,
}

/// The wheel's news, and which floating wheel it came from. Plain data:
/// it crosses to the owner on a channel.
#[derive(Debug)]
pub struct FloatNote {
    /// The wheel's window, as a number: only compared, never used as one.
    hwnd: isize,
    pub news: FloatNews,
}

impl FloatNote {
    /// A note from the wheel whose window is `hwnd`, as a test sends it.
    #[cfg(test)]
    pub fn from_wheel(hwnd: HWND, news: FloatNews) -> FloatNote {
        FloatNote {
            hwnd: hwnd.0 as isize,
            news,
        }
    }

    /// Whether the note came from the wheel whose window is `hwnd`.
    pub fn is_from(&self, hwnd: HWND) -> bool {
        self.hwnd == hwnd.0 as isize
    }
}

/// How the wheel tells its owner each note. It is called on the UI
/// thread, from inside the wheel's own handlers, so it should only queue
/// the note.
pub type Tell = Rc<dyn Fn(FloatNote)>;

fn tell(owner: &Tell, hwnd: HWND, news: FloatNews) {
    owner(FloatNote {
        hwnd: hwnd.0 as isize,
        news,
    });
}

/// Where the window goes for a wheel centred on `at` (screen pixels) at
/// `dpi`: its top-left corner and its side, in pixels. It is centred on
/// `at`, then moved as far as it must to stay inside `screen`, the
/// pointer's monitor's whole rectangle, taskbar included, so a wheel
/// opened over the taskbar still opens under the pointer. The wheel's
/// centre stays at `at` all the same. A window larger than the monitor
/// stays centred and runs off it.
pub fn placement(at: (i32, i32), dpi: f32, screen: RECT) -> (POINT, i32) {
    let side = (2.0 * HALF * dpi / 96.0).ceil() as i32;
    let fit = |p: i32, lo: i32, hi: i32| {
        let start = p - side / 2;
        if hi - lo >= side {
            start.clamp(lo, hi - side)
        } else {
            start
        }
    };
    let corner = POINT {
        x: fit(at.0, screen.left, screen.right),
        y: fit(at.1, screen.top, screen.bottom),
    };
    (corner, side)
}

/// The floating wheel and its window. Dropping it closes the window.
pub struct FloatWheel {
    hwnd: HWND,
    shared: Rc<Shared>,
    target: IDropTarget,
}

/// What the window procedure and `FloatWheel` share. The window holds a
/// reference of its own until it is destroyed.
struct Shared {
    alive: Cell<bool>,
    ui: RefCell<Ui>,
}

thread_local! {
    /// The wheels made on this thread, for the message loop's frame clock.
    static WHEELS: RefCell<Vec<Weak<Shared>>> = const { RefCell::new(Vec::new()) };
}

/// How long to wait for a wheel's swap chain before drawing its frame
/// anyway, in case a frame that presented nothing left its waitable object
/// unsignalled.
pub const STALL_MS: u32 = 100;

/// The wheels on this thread that are open and want frames.
fn wanting() -> Vec<Rc<Shared>> {
    WHEELS.with(|w| {
        let mut w = w.borrow_mut();
        w.retain(|s| s.upgrade().is_some_and(|s| s.alive.get()));
        w.iter()
            .filter_map(Weak::upgrade)
            .filter(|s| s.ui.try_borrow().is_ok_and(|ui| ui.wants_frames.get()))
            .collect()
    })
}

/// What the message loop waits on for a wheel that wants frames.
#[derive(Clone, Copy)]
pub struct FrameWait {
    /// The swap chain's frame-latency waitable object, or `None` while the
    /// wheel has no device or holds its next frame's signal already (then
    /// it is drawn at once).
    pub wait: Option<HANDLE>,
    /// When the wheel's last frame was drawn: its own deadlines count from
    /// it (`STALL_MS` for a waitable that doesn't signal).
    pub framed: Instant,
}

/// What the message loop waits on for the wheels that want frames, one
/// entry each.
pub fn frame_waits() -> Vec<FrameWait> {
    wanting()
        .iter()
        .filter_map(|s| {
            let ui = s.ui.try_borrow().ok()?;
            let wait = ui
                .gpu
                .as_ref()
                .filter(|g| !g.holds_signal())
                .map(|g| g.waitable());
            Some(FrameWait {
                wait,
                framed: ui.framed,
            })
        })
        .collect()
}

/// Draw one frame of each wheel that wants one and waits on `waitable`
/// (`None`: has none to wait on). `signalled`: the wait on it returned, so
/// the frame's signal is taken. A wheel that has faded out goes. Called by
/// the message loop with every pending message handled.
pub fn draw_frame(waitable: Option<HANDLE>, signalled: bool) {
    for shared in wanting() {
        let (hwnd, faded) = {
            let Ok(mut ui) = shared.ui.try_borrow_mut() else {
                continue;
            };
            let wait = ui
                .gpu
                .as_ref()
                .filter(|g| !g.holds_signal())
                .map(|g| g.waitable());
            if wait != waitable {
                continue;
            }
            if signalled {
                if let Some(g) = ui.gpu.as_mut() {
                    g.took_signal();
                }
            }
            let now = Instant::now();
            // A lost device asks for another frame (`Ok(true)`); any
            // other failure would fail again, so frames stop.
            if let Err(e) = ui.frame(now) {
                super::log(&format!("floating wheel: a frame failed: {e}"));
            }
            (ui.hwnd, ui.faded(now))
        };
        // Faded out: the window goes. Its owner heard as it began.
        if faded {
            // SAFETY: plain call on this thread's own window.
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }
    }
}

/// Draw a frame of every wheel that wants one and is ready for it, for a
/// message pump that has no frame clock of its own: a modal loop's (the
/// main window's `MODAL_FRAME_TIMER`) and the tests'.
///
/// A wheel whose waitable hasn't signalled for `STALL_MS` is drawn anyway,
/// so a fade always ends.
pub fn pump_frames() {
    let mut drawn = vec![];
    for FrameWait { wait, framed } in frame_waits() {
        // One frame each: the wheels with no waitable are drawn together.
        if drawn.contains(&wait) {
            continue;
        }
        let (ready, signalled) = match wait {
            // SAFETY: a swap chain's waitable object, open while its wheel
            // is; a zero wait only asks.
            Some(h) if unsafe { WaitForSingleObject(h, 0) } == WAIT_OBJECT_0 => (true, true),
            Some(_) => (
                framed.elapsed() >= Duration::from_millis(u64::from(STALL_MS)),
                false,
            ),
            None => (true, false),
        };
        if ready {
            draw_frame(wait, signalled);
            drawn.push(wait);
        }
    }
}

/// One kind's slices for the dragged files.
struct Choices {
    items: Vec<Item>,
    flyout: Vec<wheel::Group>,
    /// Why there are no slices, the in-window wheel's sentence; `None`
    /// when there are some.
    sentence: Option<String>,
}

struct Files {
    paths: Vec<PathBuf>,
    /// Their kind, for the hub's icon.
    kind: Option<kiln_core::Kind>,
    convert: Choices,
    tools: Choices,
}

/// One kind's slices for `paths`, as the in-window wheel has them, the
/// rest in More. Files dragged from another app have no facts of their
/// own yet: no subtitle track is known.
fn choices_for(
    kind: WheelKind,
    kiln: &Kiln,
    available: &Availability,
    paths: &[PathBuf],
) -> Choices {
    let (items, flyout) = wheel::offering(kind, kiln, available, paths, &wheel::no_subtitles);
    let sentence = if items.is_empty() {
        wheel::offer_sentence(kind, &wheel::offer(kind, kiln, available, paths))
    } else {
        None
    };
    Choices {
        items,
        flyout,
        sentence,
    }
}

/// The window's region: the rectangles given, in window pixels, sorted
/// top to bottom and then left to right, in bands that don't overlap. No
/// rectangles at all make a window hit nowhere.
fn set_region(hwnd: HWND, rects: &[RECT]) -> Result<()> {
    let bound = rects.iter().fold(None::<RECT>, |b, r| {
        Some(match b {
            None => *r,
            Some(b) => RECT {
                left: b.left.min(r.left),
                top: b.top.min(r.top),
                right: b.right.max(r.right),
                bottom: b.bottom.max(r.bottom),
            },
        })
    });
    // RGNDATA: its header, then the rectangles. The header is two
    // rectangles long, and both are 4-byte aligned.
    const HEADER: usize = std::mem::size_of::<RGNDATAHEADER>();
    const _: () = assert!(HEADER == 2 * std::mem::size_of::<RECT>());
    let mut data = vec![RECT::default(); 2 + rects.len()];
    data[2..].copy_from_slice(rects);
    let header = RGNDATAHEADER {
        dwSize: HEADER as u32,
        iType: RDH_RECTANGLES,
        nCount: rects.len() as u32,
        nRgnSize: std::mem::size_of_val(rects) as u32,
        rcBound: bound.unwrap_or_default(),
    };
    let bytes = std::mem::size_of_val(data.as_slice()) as u32;
    // SAFETY: the header is written into the first two rectangles' room,
    // which `data` owns and which is aligned for it. ExtCreateRegion reads
    // `bytes` bytes of `data`, all of it, while `data` lives. SetWindowRgn
    // takes the region over when it succeeds; when it fails the region is
    // still ours, and is deleted.
    unsafe {
        std::ptr::write(data.as_mut_ptr() as *mut RGNDATAHEADER, header);
        let rgn = ExtCreateRegion(None, bytes, data.as_ptr() as *const RGNDATA);
        if rgn.is_invalid() {
            return Err(Error::from_thread());
        }
        if SetWindowRgn(hwnd, Some(rgn), false) == 0 {
            let e = Error::from_thread();
            let _ = DeleteObject(rgn.into());
            return Err(e);
        }
    }
    Ok(())
}

/// Where a frame of `side` x `side` premultiplied BGRA pixels is drawn,
/// with any alpha at all: each row's runs of such pixels, rows with the
/// same runs as one band, as `set_region` takes them.
fn drawn_area(px: &[u8], side: i32) -> Vec<RECT> {
    let mut rects = vec![];
    let mut band: (i32, Vec<(i32, i32)>) = (0, vec![]);
    let mut flush = |band: &(i32, Vec<(i32, i32)>), bottom: i32| {
        for &(left, right) in &band.1 {
            rects.push(RECT {
                left,
                top: band.0,
                right,
                bottom,
            });
        }
    };
    for y in 0..side {
        let row = &px[(y * side * 4) as usize..((y + 1) * side * 4) as usize];
        let drawn = |x: i32| row[(x * 4 + 3) as usize] > 0;
        let mut runs = vec![];
        let mut x = 0;
        while x < side {
            if drawn(x) {
                let start = x;
                while x < side && drawn(x) {
                    x += 1;
                }
                runs.push((start, x));
            } else {
                x += 1;
            }
        }
        if runs != band.1 {
            flush(&band, y);
            band = (y, runs);
        }
    }
    flush(&band, side);
    rects
}

struct Ui {
    hwnd: HWND,
    owner: Tell,
    kind: WheelKind,
    palette: FluentPalette,
    dpi: f32,
    /// The window's top-left corner, screen pixels.
    corner: POINT,
    /// The window's side, pixels.
    side: i32,
    /// The wheel's centre in the window, DIPs.
    center: (f32, f32),
    factory: ID2D1Factory1,
    /// What the wheel draws with on the device. Made again when the device
    /// is lost or the palette changes.
    res: Option<WheelResources>,
    /// The device, swap chain and composition, made at the first frame and
    /// again after the device is lost.
    gpu: Option<Gpu>,
    losses: Losses,
    /// The last frame presented: premultiplied BGRA, the top row first.
    pixels: Vec<u8>,
    /// The window's region as last set: where that frame is drawn.
    region: Vec<RECT>,
    /// Frames presented, and devices made, so far.
    presented: u64,
    devices: u64,
    /// Debug builds: the next present reports the device removed, as a
    /// driver reset would (`FloatWheel::lose_device_for_test`).
    #[cfg(debug_assertions)]
    fake_loss: bool,
    /// A drag with files has reached the window.
    entered: bool,
    files: Option<Files>,
    wheel: Option<Wheel>,
    /// What is lit, the lift and More's ring: the motion the wheel over
    /// Kiln's window shares.
    motion: WheelMotion,
    /// When the wheel began to fade out. It closes once faded.
    fading: Option<Instant>,
    /// It is closing: it takes no drop, and comes back for no drag.
    closing: bool,
    /// It wants frames: something moves, or a lost device is to be made
    /// again. The message loop draws them (`frame_waits`).
    wants_frames: Cell<bool>,
    /// When its last frame was drawn (`frame`), for its deadlines
    /// (`FrameWait::framed`).
    framed: Instant,
}

thread_local! {
    /// The DirectWrite factory the wheel's text formats are made with,
    /// one per thread, so `Text::shared` makes them once.
    static DWRITE: OnceCell<IDWriteFactory> = const { OnceCell::new() };
}

fn dwrite() -> Result<IDWriteFactory> {
    DWRITE.with(|cell| {
        if let Some(f) = cell.get() {
            return Ok(f.clone());
        }
        let f: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
        Ok(cell.get_or_init(|| f).clone())
    })
}

fn register_class() -> Result<()> {
    static REGISTERED: OnceLock<std::result::Result<(), HRESULT>> = OnceLock::new();
    let done = REGISTERED.get_or_init(|| unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None).map_err(|e| e.code())?.into();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            return Err(Error::from_thread().code());
        }
        Ok(())
    });
    done.map_err(Error::from)
}

/// The monitor's DPI for apps that follow it (`MDT_EFFECTIVE_DPI`).
pub(super) fn monitor_dpi(monitor: HMONITOR) -> f32 {
    let (mut x, mut y) = (0u32, 0u32);
    match unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y) } {
        Ok(()) if x > 0 => x as f32,
        _ => 96.0,
    }
}

/// The monitor's whole rectangle, the taskbar's part included.
fn monitor_rect(monitor: HMONITOR) -> RECT {
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        info.rcMonitor
    } else {
        RECT {
            left: i32::MIN / 4,
            top: i32::MIN / 4,
            right: i32::MAX / 4,
            bottom: i32::MAX / 4,
        }
    }
}

impl FloatWheel {
    /// Open the wheel's window centred on `at_screen` (screen pixels), on
    /// that point's monitor and at its DPI, in the theme `settings` and
    /// Windows choose, as Kiln's own window is. It shows nothing until a
    /// drag brings files. `owner` hears its news. The thread must
    /// have called `OleInitialize`.
    pub fn open(
        owner: impl Fn(FloatNote) + 'static,
        at_screen: (i32, i32),
        kind: WheelKind,
        settings: &Settings,
        own: Rc<Cell<bool>>,
    ) -> Result<FloatWheel> {
        let palette = theme::fluent::palette(
            super::system::high_contrast(),
            settings.theme,
            super::system::system_light(),
        );
        // Only convert and tools float; a flip goes between them.
        let kind = match kind {
            WheelKind::Tools => WheelKind::Tools,
            _ => WheelKind::Convert,
        };
        // The test placement switch brings the point into its area, and
        // keeps the window there.
        let area = super::placement::area();
        let at_screen = area.map_or(at_screen, |a| super::placement::point_in(a, at_screen));
        let at = POINT {
            x: at_screen.0,
            y: at_screen.1,
        };
        let monitor = unsafe { MonitorFromPoint(at, MONITOR_DEFAULTTONEAREST) };
        let dpi = monitor_dpi(monitor);
        let bounds = area.map_or_else(
            || monitor_rect(monitor),
            |a| RECT {
                left: a.x,
                top: a.y,
                right: a.right(),
                bottom: a.bottom(),
            },
        );
        let (corner, side) = placement(at_screen, dpi, bounds);
        let to_dips = 96.0 / dpi;
        let center = (
            (at.x - corner.x) as f32 * to_dips,
            (at.y - corner.y) as f32 * to_dips,
        );

        register_class()?;
        // SAFETY: a plain factory for this thread, with default options.
        let factory: ID2D1Factory1 =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)? };
        let instance: HINSTANCE = unsafe { GetModuleHandleW(None)? }.into();
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED
                    | WS_EX_NOREDIRECTIONBITMAP
                    | WS_EX_TOPMOST
                    | WS_EX_TOOLWINDOW
                    | WS_EX_NOACTIVATE,
                CLASS,
                w!("Kiln format wheel"),
                WS_POPUP,
                corner.x,
                corner.y,
                side,
                side,
                None,
                None,
                Some(instance),
                None,
            )?
        };
        let ui = Ui {
            hwnd,
            owner: Rc::new(owner),
            kind,
            palette,
            dpi,
            corner,
            side,
            center,
            factory,
            res: None,
            gpu: None,
            losses: Losses::new("floating wheel: "),
            pixels: vec![0; (side * side * 4) as usize],
            region: vec![],
            presented: 0,
            devices: 0,
            #[cfg(debug_assertions)]
            fake_loss: false,
            entered: false,
            files: None,
            wheel: None,
            motion: WheelMotion::new(Instant::now()),
            fading: None,
            closing: false,
            wants_frames: Cell::new(false),
            framed: Instant::now(),
        };
        let shared = Rc::new(Shared {
            alive: Cell::new(true),
            ui: RefCell::new(ui),
        });
        // The window's own reference, given back at WM_NCDESTROY.
        let theirs = Rc::into_raw(shared.clone());
        give_shared(hwnd, theirs);
        // Its effect is the window's answer: copy on a slice, else none.
        // Kiln's own drags of its results are refused, as over its window.
        let weak = Rc::downgrade(&shared);
        let on_drag =
            Rc::new(move |kind: DragKind, ev: &DragEvent| Some(answer_drag(&weak, hwnd, kind, ev)));
        let target: IDropTarget = DropTarget::answering(own, on_drag).into();
        // From here a failure drops the wheel, which destroys the window.
        let fw = FloatWheel {
            hwnd,
            shared,
            target,
        };
        // SAFETY: plain calls on the window just made, on this thread.
        unsafe {
            // A layered window shows nothing until it has its attributes:
            // whole, so what the composition shows is what is drawn.
            SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)?;
        }
        // Hit nowhere until the first frame says where it is drawn.
        set_region(hwnd, &[])?;
        unsafe { RegisterDragDrop(hwnd, &fw.target)? };
        WHEELS.with(|w| w.borrow_mut().push(Rc::downgrade(&fw.shared)));
        {
            let mut ui = fw.shared.ui.borrow_mut();
            // A device lost on the first frame is drawn again by the
            // message loop.
            if ui.frame(Instant::now())? {
                ui.kick();
            }
        }
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            SetTimer(Some(hwnd), NO_FILES_TIMER, NO_FILES_MS, None);
        }
        Ok(fw)
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// Whether its window is still there: it closes itself when no files
    /// come.
    pub fn is_open(&self) -> bool {
        self.shared.alive.get()
    }

    /// The drop target the window registered, as OLE calls it.
    pub fn drop_target(&self) -> IDropTarget {
        self.target.clone()
    }

    /// Fill the slices with what `paths` can become (`wheel::offering`,
    /// as the in-window wheel has them, the rest in More), and
    /// bloom. The same files again change nothing.
    pub fn files_arrived(&mut self, paths: Vec<PathBuf>, kiln: &Kiln, available: &Availability) {
        let mut ui = self.shared.ui.borrow_mut();
        if ui.files.as_ref().is_some_and(|f| f.paths == paths) {
            return;
        }
        let (convert, tools) = (
            choices_for(WheelKind::Convert, kiln, available, &paths),
            choices_for(WheelKind::Tools, kiln, available, &paths),
        );
        let kind = wheel::source_kind(kiln, &paths);
        ui.entered = true;
        ui.files = Some(Files {
            paths,
            kind,
            convert,
            tools,
        });
        ui.bloom();
    }

    /// Whether `path` is one of the dragged files.
    pub fn holds(&self, path: &std::path::Path) -> bool {
        let ui = self.shared.ui.borrow();
        ui.files
            .as_ref()
            .is_some_and(|f| f.paths.iter().any(|p| p == path))
    }

    /// What Kiln knows of the dragged files changed, as when a video turns
    /// out to have no sound: their slices are worked out again. A wheel
    /// already shown keeps its place and loses only what is gone; More
    /// shuts and nothing stays lit.
    pub fn restock(&mut self, kiln: &Kiln, available: &Availability) {
        let mut ui = self.shared.ui.borrow_mut();
        let Some(paths) = ui.files.as_ref().map(|f| f.paths.clone()) else {
            return;
        };
        let convert = choices_for(WheelKind::Convert, kiln, available, &paths);
        let tools = choices_for(WheelKind::Tools, kiln, available, &paths);
        if let Some(f) = ui.files.as_mut() {
            f.convert = convert;
            f.tools = tools;
        }
        let Some((items, flyout)) = ui.choices().map(|c| (c.items.clone(), c.flyout.clone()))
        else {
            return;
        };
        let changed = ui
            .wheel
            .as_ref()
            .is_some_and(|w| w.items != items || w.flyout != flyout);
        if changed {
            if let Some(w) = ui.wheel.as_mut() {
                w.items = items;
                w.flyout = flyout;
            }
            ui.motion.close_flyout();
            ui.motion.format = None;
            let wheel = ui.wheel.clone();
            if let Some(w) = wheel.as_ref() {
                ui.motion.set_hover(w, None);
            }
            ui.kick();
        }
    }

    /// The pointer is at `screen` (screen pixels): light the slice under
    /// it, pouring More's ring when it is on More. Returns that spot.
    pub fn pointer(&mut self, screen: (i32, i32)) -> Option<Spot> {
        self.shared.ui.borrow_mut().point_at(screen.0, screen.1)
    }

    /// Go from convert to tools, or back, and bloom again.
    pub fn flip(&mut self) {
        let mut ui = self.shared.ui.borrow_mut();
        ui.kind = match ui.kind {
            WheelKind::Convert => WheelKind::Tools,
            _ => WheelKind::Convert,
        };
        ui.bloom();
    }

    /// Draw the wheel as it is at `now` and put it on screen. True while it
    /// is still moving.
    pub fn frame(&mut self, now: Instant) -> Result<bool> {
        self.shared.ui.borrow_mut().frame(now)
    }

    /// Draw in `p` from the next frame on.
    pub fn set_palette(&mut self, p: FluentPalette) {
        let mut ui = self.shared.ui.borrow_mut();
        if ui.palette != p {
            ui.palette = p;
            ui.res = None;
            ui.kick();
        }
    }

    /// Which wheel it shows: convert or tools.
    pub fn kind(&self) -> WheelKind {
        self.shared.ui.borrow().kind
    }

    /// Whether the pointer is on a slice a drop can land on, so a release
    /// now may be a drop the wheel is about to hear of.
    pub fn lit(&self) -> bool {
        let ui = self.shared.ui.borrow();
        ui.on_slice(ui.motion.hover)
    }

    /// Why the files can become nothing on this ring, in the caption under
    /// the wheel; `None` while there are slices or before files arrive.
    pub fn caption(&self) -> Option<String> {
        self.shared.ui.borrow().caption()
    }

    /// The wheel as it stands, once files have arrived.
    pub fn wheel(&self) -> Option<Wheel> {
        self.shared.ui.borrow().wheel.clone()
    }

    /// What the hub's first line says: what is lit ("PNG → JPG"), or the
    /// files' code, or why nothing fits them. `None` until files arrive.
    pub fn hub_text(&self) -> Option<String> {
        self.shared.ui.borrow().hub().map(|h| h.pair)
    }

    /// The hub's second line, when it has one: "Pick a format", how many
    /// files, or what Make smaller keeps.
    pub fn hub_note(&self) -> Option<String> {
        self.shared
            .ui
            .borrow()
            .hub()
            .map(|h| h.hint)
            .filter(|h| !h.is_empty())
    }

    /// The last frame drawn: its side in pixels and its premultiplied BGRA
    /// pixels, the top row first.
    pub fn pixels(&self) -> (u32, Vec<u8>) {
        let ui = self.shared.ui.borrow();
        (ui.side as u32, ui.pixels.clone())
    }

    /// Frames presented so far, and graphics devices made: one, then one
    /// more after each loss. For tests.
    #[doc(hidden)]
    pub fn presented_and_devices(&self) -> (u64, u64) {
        let ui = self.shared.ui.borrow();
        (ui.presented, ui.devices)
    }

    /// Whether its graphics device is WARP's. For tests.
    #[doc(hidden)]
    pub fn on_warp(&self) -> bool {
        let ui = self.shared.ui.borrow();
        ui.gpu.as_ref().is_some_and(|g| g.warp)
    }

    /// Debug builds: make the next frame's present report the device
    /// removed, as a graphics driver reset does, so a test can see the
    /// wheel drawn again on a new device.
    #[cfg(debug_assertions)]
    #[doc(hidden)]
    pub fn lose_device_for_test(&mut self) {
        self.shared.ui.borrow_mut().fake_loss = true;
    }

    /// Close the window at once. Dropping the wheel does the same.
    pub fn close(self) {}

    /// Close the wheel as the in-window wheel closes: it stops being a
    /// drop target now, and fades out over 167 ms, then its window
    /// goes. With animations off, or with nothing drawn yet, it goes at
    /// once. The wheel still fading comes back, for its owner to keep
    /// until the fade is done or to drop to cut it short; `None` once the
    /// window is gone.
    pub fn fade_out(self) -> Option<FloatWheel> {
        if !self.is_open() {
            return None;
        }
        let at_once = match self.shared.ui.try_borrow_mut() {
            Ok(mut ui) => ui.begin_close(),
            Err(_) => true,
        };
        (!at_once).then_some(self)
    }

    /// Whether it is closing: fading out, and no drop target any more.
    pub fn closing(&self) -> bool {
        self.shared.ui.try_borrow().is_ok_and(|ui| ui.closing)
    }
}

impl Drop for FloatWheel {
    fn drop(&mut self) {
        if self.shared.alive.get() {
            unsafe {
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }
}

impl Ui {
    fn to_dips(&self, x: i32, y: i32) -> (f32, f32) {
        let k = 96.0 / self.dpi;
        (
            (x - self.corner.x) as f32 * k,
            (y - self.corner.y) as f32 * k,
        )
    }

    fn choices(&self) -> Option<&Choices> {
        let f = self.files.as_ref()?;
        Some(match self.kind {
            WheelKind::Tools => &f.tools,
            _ => &f.convert,
        })
    }

    /// Make the wheel for the files and the kind, and start its bloom.
    fn bloom(&mut self) {
        let Some(c) = self.choices() else {
            return;
        };
        let (items, flyout) = (c.items.clone(), c.flyout.clone());
        let source_code = self
            .files
            .as_ref()
            .map(|f| wheel::source_code(&f.paths))
            .unwrap_or_default();
        let source_kind = self.files.as_ref().and_then(|f| f.kind);
        let now = Instant::now();
        self.wheel = Some(Wheel {
            kind: self.kind,
            origin: wheel::Origin::Drag,
            items,
            flyout,
            center: self.center,
            scale: 1.0,
            source_code,
            source_kind,
            list_only: false,
            settable: vec![],
        });
        self.motion = WheelMotion::new(now);
        // Nothing can be clicked under a drag: pointing at More opens it.
        self.motion.hover_opens = true;
        self.kick();
    }

    /// The window's own bounds, in DIPs: where More's list may go.
    fn bounds(&self) -> Rect {
        let side = self.side as f32 * 96.0 / self.dpi;
        Rect::new(0.0, 0.0, side, side)
    }

    /// More's list while it is open: its lines and where they are.
    fn flyout(&self) -> Option<(Vec<FlyoutRow<'_>>, FlyoutPlace)> {
        let w = self.wheel.as_ref()?;
        if !self.motion.flyout_open() {
            return None;
        }
        let rows = w.flyout_rows(None);
        let place = wheel::flyout_place(w, &rows, self.bounds(), 0.0);
        Some((rows, place))
    }

    /// What is under `(x, y)`, screen pixels.
    fn spot_at(&self, x: i32, y: i32) -> Option<Spot> {
        let w = self.wheel.as_ref()?;
        let (dx, dy) = self.to_dips(x, y);
        if let Some((rows, place)) = self.flyout() {
            if place.rect.contains(dx, dy) {
                return wheel::flyout_hit(&place, &rows, dx, dy);
            }
        }
        wheel::hit(w, dx, dy)
    }

    /// The slice under `(x, y)`, screen pixels, as a drop there picks it.
    fn pick_at(&self, x: i32, y: i32) -> Option<Pick> {
        let spot = self.spot_at(x, y)?;
        let w = self.wheel.as_ref()?;
        Some(Pick {
            kind: w.kind,
            spot,
            item: w.item(spot)?.clone(),
        })
    }

    fn point_at(&mut self, x: i32, y: i32) -> Option<Spot> {
        let spot = self.spot_at(x, y);
        self.point(spot);
        spot
    }

    /// Light `spot`. More pours its ring; any other inner slice lets it
    /// go, as on the in-window wheel.
    fn point(&mut self, spot: Option<Spot>) {
        if self.motion.point(self.wheel.as_ref(), spot, Instant::now()) {
            self.kick();
        }
    }

    /// Whether `spot` is a slice a drop can land on: a format or a tool,
    /// not More.
    fn on_slice(&self, spot: Option<Spot>) -> bool {
        let w = self.wheel.as_ref();
        spot.and_then(|s| w?.item(s)).is_some()
    }

    /// The drag is at `(x, y)`: light what is under it, and answer
    /// whether a drop there would copy.
    fn answer(&mut self, x: i32, y: i32) -> LRESULT {
        // Closing, it takes nothing: until it has faded, a drag over it gets no drop.
        if self.closing {
            return LRESULT(0);
        }
        let spot = self.point_at(x, y);
        LRESULT(self.on_slice(spot) as isize)
    }

    /// What the hub says: as the wheel over Kiln's window says it, or R5's
    /// "No formats" or "No tools" when nothing fits the files (the reason
    /// is in the caption). `None` until files arrive. Files from another
    /// app have no facts yet, so no estimate.
    fn hub(&self) -> Option<wheel::HubLines> {
        let f = self.files.as_ref()?;
        let w = self.wheel.as_ref()?;
        let c = self.choices()?;
        if c.items.is_empty() {
            let none = match self.kind {
                WheelKind::Tools => NO_TOOLS,
                _ => NO_FORMATS,
            };
            return Some(wheel::HubLines {
                pair: none.to_owned(),
                hint: String::new(),
            });
        }
        Some(w.hub_lines(self.motion.hover, f.paths.len(), &|_| None))
    }

    fn caption(&self) -> Option<String> {
        self.wheel.as_ref()?;
        let c = self.choices()?;
        if !c.items.is_empty() {
            return None;
        }
        c.sentence.clone()
    }

    /// Whether the files can become nothing on the ring showing.
    fn nothing_offered(&self) -> bool {
        self.choices().is_some_and(|c| c.items.is_empty())
    }

    /// Whether the pointer at `(x, y)`, screen pixels, is more than `REACH`
    /// past the ring, and past More's open list, where the wheel closes.
    fn beyond_reach(&self, x: i32, y: i32) -> bool {
        let (dx, dy) = self.to_dips(x, y);
        let d = (dx - self.center.0).hypot(dy - self.center.1);
        let near_list = self.flyout().is_some_and(|(_, place)| {
            let r = place.rect;
            Rect::new(
                r.x - REACH,
                r.y - REACH,
                r.w + 2.0 * REACH,
                r.h + 2.0 * REACH,
            )
            .contains(dx, dy)
        });
        d > R_OUTER + REACH && !near_list
    }

    /// Start closing: no drop target from now on, and no timer but the
    /// fade's. True when the window should go at once: animations are off,
    /// or nothing shows yet to fade.
    fn begin_close(&mut self) -> bool {
        if !self.closing {
            self.closing = true;
            self.motion.hover = None;
            unsafe {
                let _ = RevokeDragDrop(self.hwnd);
                let _ = KillTimer(Some(self.hwnd), NO_FILES_TIMER);
                let _ = KillTimer(Some(self.hwnd), LEAVE_TIMER);
            }
        }
        if !theme::fluent::motion::animations() || self.wheel.is_none() {
            return true;
        }
        if self.fading.is_none() {
            self.fading = Some(Instant::now());
        }
        self.kick();
        false
    }

    /// Draw soon: the message loop draws frames until nothing moves.
    fn kick(&self) {
        self.wants_frames.set(true);
    }

    fn animating(&self, now: Instant) -> bool {
        let Some(w) = &self.wheel else {
            return false;
        };
        let _ = w;
        self.motion
            .animating(now, theme::fluent::motion::animations())
            || self.fading.is_some()
    }

    /// How much of the wheel still shows as it fades out: 1 until it
    /// starts, 0 once it is gone.
    fn fade(&self, now: Instant) -> f32 {
        match self.fading {
            Some(t) => anim::wheel_close(
                anim::elapsed_ms(t, now),
                theme::fluent::motion::animations(),
            ),
            None => 1.0,
        }
    }

    /// Whether it has faded out all the way.
    fn faded(&self, now: Instant) -> bool {
        self.fading.is_some() && self.fade(now) <= 0.0
    }

    /// The device and what the wheel draws with on it, each made if
    /// missing, and the back buffer the device context's target.
    fn prepare(&mut self) -> Result<()> {
        let side = self.side as u32;
        if self.gpu.is_none() {
            let g = Gpu::exact(
                &self.factory,
                self.hwnd,
                (side, side),
                self.dpi,
                self.losses.warp_only(),
            )?;
            self.losses.made(&g);
            self.gpu = Some(g);
            self.devices += 1;
            self.res = None;
        }
        let Some(g) = self.gpu.as_mut() else {
            return Err(Error::empty());
        };
        g.begin((side, side), self.dpi)?;
        if self.res.is_none() {
            let rt: &ID2D1RenderTarget = &g.dc;
            self.res = Some(WheelResources::new(rt, &dwrite()?)?);
        }
        Ok(())
    }

    /// The device is gone: drop everything made on it, to be made again at
    /// the next frame. True while that frame should come at once
    /// (`gpu::Losses`).
    fn lose(&mut self) -> bool {
        let reason = self.gpu.as_ref().map(|g| g.removed_reason());
        self.res = None;
        if let Some(g) = self.gpu.take() {
            g.forget();
        }
        self.losses.lost(reason)
    }

    /// Draw the wheel as it is at `now` and put it on screen. True while it
    /// is still moving, or when its device was lost and it must be drawn
    /// again.
    fn frame(&mut self, now: Instant) -> Result<bool> {
        self.framed = Instant::now();
        let again = match self.draw(now) {
            Ok(()) => Ok(self.animating(now)),
            Err(e) if gpu::is_loss(e.code()) => Ok(self.lose()),
            Err(e) => Err(e),
        };
        // Any other failure would fail again, so no more frames are asked
        // for.
        self.wants_frames.set(*again.as_ref().unwrap_or(&false));
        again
    }

    /// Draw the frame, set the window's region to where it is drawn, and
    /// present it.
    fn draw(&mut self, now: Instant) -> Result<()> {
        self.prepare()?;
        let hub = self.hub();
        let caption = self.caption();
        let fade = self.fade(now);
        let bounds = self.bounds();
        let animations = theme::fluent::motion::animations();
        let Ui {
            gpu,
            res,
            wheel,
            palette,
            dpi,
            center,
            motion,
            ..
        } = self;
        let (Some(g), Some(res)) = (gpu.as_mut(), res.as_mut()) else {
            return Err(Error::empty());
        };
        let rt: &ID2D1RenderTarget = &g.dc;
        // SAFETY: plain calls on this thread's device context, whose target
        // is the back buffer (`prepare`).
        unsafe {
            rt.BeginDraw();
            rt.SetTransform(&Matrix3x2::identity());
            rt.Clear(Some(&D2D1_COLOR_F::default()));
        }
        let drawn = match (wheel, hub) {
            // Before files: the hub alone, too faint to see, so OLE's hit
            // test finds the window under the pointer.
            (Some(w), Some(hub)) => {
                // The whole hit area receives the drop, the reach past the
                // segments too, so a release there never falls through to
                // the app beneath.
                faint_disc(rt, *center, R_OUTER + REACH)?;
                let flyout = motion.flyout_open().then(|| {
                    let rows = w.flyout_rows(None);
                    let place = wheel::flyout_place(w, &rows, bounds, 0.0);
                    super::render::FlyoutView {
                        rows,
                        place,
                        lit: motion.hover,
                        shown: motion.flyout_shown(now, animations),
                    }
                });
                let (open, grow) = motion.opening(now, animations);
                let wv = WheelView {
                    wheel: w,
                    hover: motion.hover,
                    hub,
                    opacity: open * fade,
                    grow,
                    smoke: false,
                    flyout,
                    thumb: None,
                    kind: w.source_kind,
                };
                let extras = WheelExtras {
                    caption: caption.as_deref(),
                };
                draw_wheel_on(rt, res, &wv, &extras, palette, *dpi, 1.0)
            }
            _ => faint_disc(rt, *center, R_INNER),
        };
        // SAFETY: ends the BeginDraw above.
        unsafe { rt.EndDraw(None, None)? };
        drawn?;
        // What a window drawn with UpdateLayeredWindow is hit by: every
        // pixel drawn with any alpha. The region goes on before the frame
        // it was read from: a growing frame is never cut off, and on a
        // shrinking one the region runs ahead of the picture until the
        // present, clipping only pixels of the last frame that are about
        // to go.
        g.read_back(&mut self.pixels)?;
        let area = drawn_area(&self.pixels, self.side);
        if area != self.region {
            set_region(self.hwnd, &area)?;
            self.region = area;
        }
        #[cfg(debug_assertions)]
        if std::mem::take(&mut self.fake_loss) {
            return Err(windows::Win32::Graphics::Dxgi::DXGI_ERROR_DEVICE_REMOVED.into());
        }
        g.present().ok()?;
        super::frame_log::float_presented();
        self.presented += 1;
        self.losses.presented();
        Ok(())
    }
}

/// A disc at alpha 1/255 round `center`, `radius` DIPs across: never
/// seen, but hit by OLE's test, which passes over clear pixels.
fn faint_disc(rt: &ID2D1RenderTarget, center: (f32, f32), radius: f32) -> Result<()> {
    let faint = D2D1_COLOR_F {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0 / 255.0,
    };
    unsafe {
        let brush = rt.CreateSolidColorBrush(&faint, None)?;
        rt.FillEllipse(
            &D2D1_ELLIPSE {
                point: Vector2 {
                    X: center.0,
                    Y: center.1,
                },
                radiusX: radius,
                radiusY: radius,
            },
            &brush,
        );
    }
    Ok(())
}

/// The wheel's procedure. Its work runs inside `crash::WINDOW`, the main
/// window's guard: after a panic in either, neither touches its state
/// again, and Kiln says it has to close and ends.
extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if crash::WINDOW.tripped() {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    // The message has no owner, so it turns off Kiln's window as well: the
    // way to that window is the wheel's state, which is no longer touched.
    crash::WINDOW
        .run(|| answer_message(hwnd, msg, wparam, lparam))
        .unwrap_or_else(|| crash::close_after_panic(HWND::default()))
}

/// The wheel's answer to one message.
fn answer_message(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // Clicked or not, it is never activated.
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_DESTROY => unsafe {
            let _ = KillTimer(Some(hwnd), NO_FILES_TIMER);
            let _ = KillTimer(Some(hwnd), LEAVE_TIMER);
            let _ = RevokeDragDrop(hwnd);
        },
        WM_NCDESTROY => {
            // SAFETY: the window's own reference, given by `give_shared`,
            // is taken once, here, at its last message, on its thread.
            if let Some(shared) = unsafe { take_shared(hwnd) } {
                shared.alive.set(false);
            }
        }
        _ => {}
    }
    // SAFETY: the borrow ends with this call; the reference it reads is
    // released only at WM_NCDESTROY, above, before which nothing else frees it.
    if let Some(shared) = unsafe { shared_of(hwnd) } {
        if let Some(answer) = handle(shared, hwnd, msg, wparam) {
            return answer;
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Give `hwnd` its reference to the wheel's `Shared` (from `Rc::into_raw`),
/// to be taken back by [`take_shared`] at WM_NCDESTROY.
fn give_shared(hwnd: HWND, raw: *const Shared) {
    // SAFETY: plain call on this thread's own window. The reference it
    // holds stays alive until `take_shared` gives it back.
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize) };
}

/// The window's `Shared`, while it has one. Only for the length of one call
/// of the window's procedure.
///
/// # Safety
/// `hwnd`'s user data must be null or what `give_shared` stored, and
/// `take_shared` must not run for `hwnd` while the borrow is in use.
unsafe fn shared_of<'a>(hwnd: HWND) -> Option<&'a Shared> {
    // SAFETY: reading a window's user data has no preconditions.
    let raw = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Shared;
    // SAFETY: when set, it is the pointer `give_shared` stored from an
    // `Rc::into_raw`. That reference is the window's own and is released
    // only by `take_shared` at WM_NCDESTROY, after which no call of the
    // procedure still holds this borrow; all on the window's thread.
    unsafe { raw.as_ref() }
}

/// Take the window's own reference to `Shared` back, clearing it from the
/// window; `None` when it has none.
///
/// # Safety
/// `hwnd`'s user data must be null or what `give_shared` stored, and no
/// borrow from `shared_of` may outlive this call.
unsafe fn take_shared(hwnd: HWND) -> Option<Rc<Shared>> {
    // SAFETY: plain call on this thread's own window.
    let raw = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) } as *const Shared;
    if raw.is_null() {
        return None;
    }
    // SAFETY: not null, so it is the pointer `give_shared` stored from
    // `Rc::into_raw`, and the user data is now cleared, so this is the only
    // time it is turned back into the `Rc`.
    Some(unsafe { Rc::from_raw(raw) })
}

fn handle(shared: &Shared, hwnd: HWND, msg: u32, wparam: WPARAM) -> Option<LRESULT> {
    match msg {
        WM_TIMER if wparam.0 == NO_FILES_TIMER => {
            unsafe {
                let _ = KillTimer(Some(hwnd), NO_FILES_TIMER);
            }
            let (entered, owner) = {
                let ui = shared.ui.try_borrow().ok()?;
                (ui.entered, ui.owner.clone())
            };
            if !entered {
                tell(&owner, hwnd, FloatNews::Closed);
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            }
            Some(LRESULT(0))
        }
        // The drag left: close once the pointer is past the margin. A drag
        // that ended elsewhere is closed by the owner, from the hooks.
        WM_TIMER if wparam.0 == LEAVE_TIMER => {
            let mut ui = shared.ui.try_borrow_mut().ok()?;
            let mut at = POINT::default();
            let read = unsafe { GetCursorPos(&mut at) }.is_ok();
            if read && !ui.closing && ui.beyond_reach(at.x, at.y) {
                let at_once = ui.begin_close();
                let owner = ui.owner.clone();
                drop(ui);
                closes(&owner, hwnd, at_once);
            }
            Some(LRESULT(0))
        }
        _ => None,
    }
}

/// The wheel's drop target's events, answered as the window's procedure
/// answers its messages: inside `crash::WINDOW`, and with no drop once the
/// window has gone or while its state is in use.
fn answer_drag(shared: &Weak<Shared>, hwnd: HWND, kind: DragKind, ev: &DragEvent) -> LRESULT {
    if crash::WINDOW.tripped() {
        return LRESULT(0);
    }
    // The message has no owner, as in `wndproc`.
    crash::WINDOW
        .run(|| {
            let shared = shared.upgrade().filter(|s| s.alive.get())?;
            drag(&shared, hwnd, kind, ev)
        })
        .unwrap_or_else(|| crash::close_after_panic(HWND::default()))
        .unwrap_or(LRESULT(0))
}

fn drag(shared: &Shared, hwnd: HWND, kind: DragKind, ev: &DragEvent) -> Option<LRESULT> {
    if kind == DragKind::Drop {
        let ui = shared.ui.try_borrow().ok()?;
        // Closing, it takes no drop, and says nothing of one.
        if ui.closing {
            return Some(LRESULT(0));
        }
        // Only a drop answered with copy picks anything: a source that
        // allows only a move is told no drop, and nothing converts.
        let picked = ui.pick_at(ev.x, ev.y).filter(|_| ev.copy);
        let on = picked.is_some() && !ev.paths.is_empty();
        tell(
            &ui.owner,
            hwnd,
            FloatNews::Dropped {
                picked,
                paths: ev.paths.clone(),
            },
        );
        return Some(LRESULT(on as isize));
    }
    let mut ui = shared.ui.try_borrow_mut().ok()?;
    match kind {
        // Closing, it is no drop target: a drag coming back in finds no
        // wheel, and the next tap opens a fresh one.
        _ if ui.closing => Some(LRESULT(0)),
        DragKind::Enter => {
            // SAFETY: plain timer calls on this thread's own window.
            unsafe {
                let _ = KillTimer(Some(hwnd), LEAVE_TIMER);
            }
            if !ev.paths.is_empty() && !ui.entered {
                ui.entered = true;
                // SAFETY: as above.
                unsafe {
                    let _ = KillTimer(Some(hwnd), NO_FILES_TIMER);
                }
            }
            if !ev.paths.is_empty() {
                tell(&ui.owner, hwnd, FloatNews::Files(ev.paths.clone()));
            }
            Some(ui.answer(ev.x, ev.y))
        }
        DragKind::Over => Some(ui.answer(ev.x, ev.y)),
        // It left (a drop was answered above).
        DragKind::Leave | DragKind::Drop => {
            ui.point(None);
            if ui.nothing_offered() {
                // Nothing to drop on: it fades out with the drag, as the
                // wheel over Kiln's window closes, or goes at once with
                // animations off.
                let at_once = ui.begin_close();
                let owner = ui.owner.clone();
                drop(ui);
                closes(&owner, hwnd, at_once);
            } else {
                // SAFETY: as above.
                unsafe {
                    SetTimer(Some(hwnd), LEAVE_TIMER, LEAVE_MS, None);
                }
            }
            Some(LRESULT(0))
        }
    }
}

/// The wheel has begun to close itself (`Ui::begin_close`): the owner
/// hears `Closed` now, and the window goes now when `at_once`, or once it
/// has faded.
fn closes(owner: &Tell, hwnd: HWND, at_once: bool) {
    tell(owner, hwnd, FloatNews::Closed);
    if at_once {
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: RECT = RECT {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1040,
    };

    #[test]
    fn a_dragged_video_found_silent_loses_its_sound_formats() {
        // a silent AVI dragged from Explorer has no facts when it
        // reaches the wheel, so MP3 is on it; once its facts say it has no
        // sound, the wheel's choices hold no sound format.
        let avi = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../kiln-media/tests/data/silent.avi");
        let kiln = Kiln::new();
        let all = Availability::all();
        let paths = vec![avi.clone()];
        let sound = |c: &Choices| {
            c.items
                .iter()
                .chain(c.flyout.iter().flat_map(|g| g.items.iter()))
                .filter(|i| i.kind == Some(kiln_core::Kind::Audio))
                .map(|i| i.label.clone())
                .collect::<Vec<_>>()
        };
        let before = choices_for(WheelKind::Convert, &kiln, &all, &paths);
        assert!(
            sound(&before).contains(&"MP3".to_owned()),
            "{:?}",
            sound(&before)
        );
        let facts = kiln_core::inspect(
            &Kiln::new(),
            &avi,
            &kiln_core::Options::default(),
            &kiln_core::CancelToken::new(),
        )
        .unwrap();
        assert!(facts.silent, "{facts:?}");
        kiln.learn(&avi, &facts);
        let after = choices_for(WheelKind::Convert, &kiln, &all, &paths);
        assert_eq!(sound(&after), Vec::<String>::new());
        assert!(after.items.iter().any(|i| i.label == "MP4"));
    }

    #[test]
    fn the_window_holds_the_ring_and_mores_list_on_either_side() {
        // The ring, the gap, More's list and its shadow, either side of
        // the centre.
        assert_eq!(HALF, 136.0 + 8.0 + 300.0 + 24.0);
        let (_, side) = placement((960, 520), 96.0, WORK);
        assert_eq!(side, 936);
        let (_, side) = placement((960, 520), 144.0, WORK);
        assert_eq!(side, 1404);
    }

    #[test]
    fn the_window_centres_on_the_pointer_where_it_fits() {
        let (corner, side) = placement((960, 520), 96.0, WORK);
        assert_eq!((corner.x + side / 2, corner.y + side / 2), (960, 520));
    }

    #[test]
    fn near_an_edge_the_window_moves_in_and_the_wheel_stays_put() {
        let (corner, side) = placement((40, 1020), 96.0, WORK);
        assert_eq!(corner.x, 0);
        assert_eq!(corner.y + side, 1040);
        // The wheel's centre is the pointer, wherever the window went.
        let (corner, side) = placement((1900, 10), 96.0, WORK);
        assert_eq!((corner.x + side, corner.y), (1920, 0));
    }

    #[test]
    fn a_monitor_left_of_and_above_the_main_one_places_the_same_way() {
        // A work area in negative coordinates, as on a monitor to the
        // left of and above the main one.
        let left = RECT {
            left: -1920,
            top: -1080,
            right: 0,
            bottom: -40,
        };
        let (corner, side) = placement((-960, -560), 96.0, left);
        assert_eq!((corner.x + side / 2, corner.y + side / 2), (-960, -560));
        let (corner, _) = placement((-1900, -1070), 96.0, left);
        assert_eq!((corner.x, corner.y), (-1920, -1080));
        let (corner, side) = placement((-5, -45), 96.0, left);
        assert_eq!((corner.x + side, corner.y + side), (0, -40));
    }

    #[test]
    fn over_the_taskbar_the_wheel_still_opens_under_the_pointer() {
        // The whole monitor, the taskbar's 40 pixels at the bottom
        // included: a wheel opened over the taskbar stays centred on the
        // pointer where it fits, and keeps to the monitor, not to the work
        // area above the taskbar.
        let monitor = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let (corner, side) = placement((960, 1060), 96.0, monitor);
        assert_eq!(corner.y + side, 1080);
        assert!(corner.y + side > WORK.bottom);
        let (corner, side) = placement((960, 540), 96.0, monitor);
        assert_eq!((corner.x + side / 2, corner.y + side / 2), (960, 540));
        // Low on the screen, it runs over the taskbar's rows rather than
        // stopping above them.
        let (corner, side) = placement((960, 900), 96.0, monitor);
        assert_eq!(corner.y + side, 1080);
        let (above, _) = placement((960, 900), 96.0, WORK);
        assert!(corner.y > above.y);
    }

    #[test]
    fn a_window_larger_than_the_screen_stays_centred() {
        let small = RECT {
            left: 0,
            top: 0,
            right: 400,
            bottom: 300,
        };
        let (corner, side) = placement((200, 150), 96.0, small);
        assert_eq!((corner.x + side / 2, corner.y + side / 2), (200, 150));
    }
}
