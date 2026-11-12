//! A window's drawing surface: a Direct3D 11 device, Direct2D on it, and a
//! flip-model swap chain that DirectComposition shows in the window. The
//! window has no redirection bitmap (`WS_EX_NOREDIRECTIONBITMAP`), so what
//! the swap chain holds is all it shows. Kiln's main window and the
//! floating wheel each draw through one.
//!
//! Everything here is bound to the device: when the device is lost it is
//! all dropped together and made again (`Renderer::lose`, and the floating
//! wheel's own), counted by `Losses`.

use std::cell::RefCell;

use windows::core::{Interface, Result, HRESULT};
use windows::Win32::Foundation::{
    CloseHandle, D2DERR_RECREATE_TARGET, HANDLE, HMODULE, HWND, RECT, WAIT_OBJECT_0,
};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::Threading::WaitForSingleObjectEx;

/// Devices lost in a row before the next one is WARP's.
const WARP_AFTER: u32 = 3;

/// Devices lost in a row before a lost frame stops asking to be drawn
/// again: the next frame asked for by anything else tries once more.
const GIVE_UP_AFTER: u32 = 10;

/// The swap chain's flags, the same at creation and at every resize.
const SWAP_FLAGS: DXGI_SWAP_CHAIN_FLAG = DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT;

/// The longest a frame drawn outside the frame clock (a paint, or a frame
/// in a modal loop) waits for the swap chain to take it, in milliseconds.
/// A window the compositor isn't showing (hidden, or never shown, as in
/// the in-process tests) may never signal; past this the frame is drawn
/// anyway, and counted as owed (`Gpu::owed`).
const FRAME_WAIT_MS: u32 = 20;

/// Room to spare in the swap chain's buffers past the client area, in
/// pixels, on top of a quarter of its size.
const SPARE: u32 = 128;

/// The buffers' size for a client area of `client` pixels, when they are
/// `now`. They are bigger than the client area, and Direct2D's `Clear`
/// fills all of them with the ground: while the window grows under a drag,
/// the compositor shows its new edge before the next frame is presented,
/// and it shows that ground there, not what is behind the window. They
/// grow, with room to spare, once the client area outgrows them, and
/// shrink once it is under half of them both ways.
pub(super) fn buffers_for(client: (u32, u32), now: (u32, u32)) -> (u32, u32) {
    let fits = client.0 <= now.0 && client.1 <= now.1;
    let tight = client.0 * 2 > now.0 || client.1 * 2 > now.1;
    if fits && tight {
        return now;
    }
    let roomy = |c: u32| c + c / 4 + SPARE;
    (roomy(client.0), roomy(client.1))
}

/// Whether `code` means the device is gone and everything on it must be
/// made again.
pub(in crate::win) fn is_loss(code: HRESULT) -> bool {
    code == D2DERR_RECREATE_TARGET
        || code == DXGI_ERROR_DEVICE_REMOVED
        || code == DXGI_ERROR_DEVICE_RESET
}

/// The swap chain's frame-latency waitable object, closed with it.
struct Waitable(HANDLE);

impl Drop for Waitable {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: the handle GetFrameLatencyWaitableObject gave this
            // swap chain's owner to close, closed once.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

thread_local! {
    /// The device last made on this thread, whether it is WARP's, and
    /// whether that was asked for (`warp_only`) rather than a fallback for
    /// want of a graphics card device. The next surface made on the thread
    /// shares it while it is sound and of the kind it needs, since
    /// making one takes a tenth of a second or more: the floating wheel
    /// draws on the main window's device. A surface whose device was lost
    /// takes it out (`Gpu::forget`), and the UI thread lets go of it before
    /// Kiln exits (`forget_shared_device`).
    static SHARED: RefCell<Option<Shared>> = const { RefCell::new(None) };
}

/// Let go of the device this thread shares, if any, before the thread
/// ends. The UI thread does so before Kiln exits: a device still held when
/// the process ends would be released as it ends, and that fails its exit.
pub(in crate::win) fn forget_shared_device() {
    SHARED.with(|c| c.borrow_mut().take());
}

/// The device a thread shares (`SHARED`).
#[derive(Clone)]
struct Shared {
    device: ID3D11Device,
    warp: bool,
    /// WARP's because a window's losses asked for it (`warp_only`), not
    /// because there was no graphics card device.
    forced: bool,
}

/// A D3D11 device Direct2D can draw on, and whether it is WARP's: the one
/// this thread last made while it is sound and of the kind asked for, or
/// else a new one (`new_d3d_device`).
///
/// With `warp_only` that kind is WARP's. Without, it is the one a new
/// device would be: WARP made for another window's `warp_only` is not
/// shared, since this window's own losses never asked for it, while WARP
/// made for want of a graphics card device is.
fn d3d_device(warp_only: bool) -> Result<(ID3D11Device, bool)> {
    let shared = SHARED.with(|c| c.borrow().clone());
    if let Some(sh) = shared {
        // SAFETY: a plain query on this thread's device.
        let sound = unsafe { sh.device.GetDeviceRemovedReason() }.is_ok();
        let fits = if warp_only { sh.warp } else { !sh.forced };
        if sound && fits {
            return Ok((sh.device, sh.warp));
        }
    }
    let (device, warp) = new_d3d_device(warp_only)?;
    SHARED.with(|c| {
        *c.borrow_mut() = Some(Shared {
            device: device.clone(),
            warp,
            forced: warp_only,
        })
    });
    Ok((device, warp))
}

/// A new D3D11 device Direct2D can draw on: the graphics card's, or WARP
/// where it has none (or its driver won't make one), or WARP's alone when
/// `warp_only`.
fn new_d3d_device(warp_only: bool) -> Result<(ID3D11Device, bool)> {
    let mut last = None;
    let drivers = [
        (D3D_DRIVER_TYPE_HARDWARE, false),
        (D3D_DRIVER_TYPE_WARP, true),
    ];
    for (driver, warp) in drivers.into_iter().skip(usize::from(warp_only)) {
        let mut device = None;
        // SAFETY: the out-pointer is a local that outlives the call; no
        // feature levels are passed, so the default list is used.
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
            (Ok(()), Some(d)) => return Ok((d, warp)),
            (Err(e), _) => last = Some(e),
            (Ok(()), None) => {}
        }
    }
    Err(last.unwrap_or_else(windows::core::Error::empty))
}

/// Lost devices, counted for one window: a device lost again and again is
/// bounded. After `WARP_AFTER` in a row the next device is WARP's, and
/// after `GIVE_UP_AFTER` in a row a lost frame asks for no other, until
/// something else draws the window. A frame presented starts the count
/// again.
pub(in crate::win) struct Losses {
    /// Devices lost in a row, with no frame presented between.
    in_a_row: u32,
    /// The graphics card lost its device `WARP_AFTER` times in a row:
    /// devices are WARP's from then on.
    warp_only: bool,
    /// Which window's device, at the head of each log line: empty for
    /// Kiln's main window.
    what: &'static str,
}

impl Losses {
    pub(in crate::win) const fn new(what: &'static str) -> Losses {
        Losses {
            in_a_row: 0,
            warp_only: false,
            what,
        }
    }

    /// Whether the next device is to be WARP's.
    pub(in crate::win) fn warp_only(&self) -> bool {
        self.warp_only
    }

    /// A device was made: say so if it is WARP's for want of a graphics
    /// card device.
    pub(in crate::win) fn made(&self, g: &Gpu) {
        if g.warp && !self.warp_only {
            crate::diag::log(&format!(
                "{}drawing with WARP: no graphics card device",
                self.what
            ));
        }
    }

    /// Whether so many devices were lost in a row that the window has
    /// stopped asking to be drawn: a prompt inside it might never show.
    pub(in crate::win) fn given_up(&self) -> bool {
        self.in_a_row >= GIVE_UP_AFTER
    }

    /// A frame reached the screen.
    pub(in crate::win) fn presented(&mut self) {
        self.in_a_row = 0;
    }

    /// The device was lost, for `reason` (`Gpu::removed_reason`): count it
    /// and log it. True while the window should be drawn again at once.
    pub(in crate::win) fn lost(&mut self, reason: Option<HRESULT>) -> bool {
        let what = self.what;
        self.in_a_row += 1;
        if self.in_a_row == 1 {
            crate::diag::log(&format!(
                "{what}the graphics device was lost (removed reason {reason:?}); making it again"
            ));
        }
        if self.in_a_row == WARP_AFTER && !self.warp_only {
            self.warp_only = true;
            crate::diag::log(&format!(
                "{what}the graphics device was lost {WARP_AFTER} times in a row \
                 (last removed reason {reason:?}); drawing with WARP from now on"
            ));
        }
        if self.in_a_row == GIVE_UP_AFTER {
            crate::diag::log(&format!(
                "{what}the graphics device was lost {GIVE_UP_AFTER} times in a row \
                 (last removed reason {reason:?}); not asking for a repaint"
            ));
        }
        self.in_a_row < GIVE_UP_AFTER
    }
}

/// The device, the swap chain, the composition that shows it, and the
/// device context drawing into its back buffer.
pub(in crate::win) struct Gpu {
    /// Direct2D's device context. Drawing goes through it as an
    /// `ID2D1RenderTarget`; its target is the back buffer's bitmap.
    pub(in crate::win) dc: ID2D1DeviceContext,
    swap: IDXGISwapChain2,
    composition: IDCompositionDevice,
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,
    waitable: Waitable,
    /// The back buffer as Direct2D's target: released before a resize,
    /// made again at the next frame.
    bitmap: Option<ID2D1Bitmap1>,
    /// The swap chain's buffers' size in pixels: the client area's and,
    /// when `spare`, some to spare (`buffers_for`).
    size: (u32, u32),
    /// Whether the buffers keep room past the client area, for a window
    /// that is resized; a window that keeps its size has them exact.
    spare: bool,
    /// A copy of the back buffer the CPU can read (`read_back`), made at
    /// the first read and again when the buffers change size.
    staging: Option<ID3D11Texture2D>,
    /// Whether the composition has been committed since the first present,
    /// so the window shows this swap chain.
    shown: bool,
    /// Whether the next present must show the whole frame: what the
    /// screen holds isn't the previous frame of these buffers, as before
    /// the first present and after the buffers or the scale change
    /// (`can_present_part`).
    whole_next: bool,
    /// Whether the waitable's signal for the next frame has been taken
    /// (`take_turn`, or the message loop's wait, `took_signal`) and not
    /// yet spent by a present.
    ///
    /// Every present spends one signal: the swap chain gives one back for
    /// each frame it has taken. A present without one leaves a signal
    /// over once its frame is taken, and from then on every wait finds the
    /// waitable set at once, so `Present` itself does the waiting, and the
    /// graphics driver may spin the CPU while it does.
    held: bool,
    /// Frames presented without a signal, after a wait that timed out:
    /// signals the swap chain gives back past its due, taken (and dropped)
    /// as they come (`settle`).
    owed: u32,
    /// Whether the device is WARP's, not the graphics card's.
    pub(in crate::win) warp: bool,
    _d3d: ID3D11Device,
}

impl Gpu {
    /// Make the device and a swap chain for a client area of `size` pixels
    /// in `hwnd` (`buffers_for`), with
    /// the composition set up but not yet committed: the window shows it
    /// once its first frame is presented (`present`). `warp_only` skips
    /// the graphics card, after it has lost its device again and again.
    pub(super) fn new(
        factory: &ID2D1Factory1,
        hwnd: HWND,
        size: (u32, u32),
        dpi: f32,
        warp_only: bool,
    ) -> Result<Gpu> {
        Gpu::make(factory, hwnd, size, dpi, warp_only, true)
    }

    /// As `new`, for a window that keeps its size: the buffers are `size`
    /// exactly, with no room to spare.
    pub(in crate::win) fn exact(
        factory: &ID2D1Factory1,
        hwnd: HWND,
        size: (u32, u32),
        dpi: f32,
        warp_only: bool,
    ) -> Result<Gpu> {
        Gpu::make(factory, hwnd, size, dpi, warp_only, false)
    }

    /// The buffers' size for a client area of `client` pixels.
    fn buffers(&self, client: (u32, u32)) -> (u32, u32) {
        if self.spare {
            buffers_for(client, self.size)
        } else {
            client
        }
    }

    fn make(
        factory: &ID2D1Factory1,
        hwnd: HWND,
        size: (u32, u32),
        dpi: f32,
        warp_only: bool,
        spare: bool,
    ) -> Result<Gpu> {
        let (d3d, warp) = d3d_device(warp_only)?;
        let size = if spare {
            buffers_for(size, (0, 0))
        } else {
            size
        };
        // SAFETY: COM calls on objects made here, on this thread; each
        // descriptor is a local that outlives its call.
        unsafe {
            let dxgi: IDXGIDevice = d3d.cast()?;
            let d2d = factory.CreateDevice(&dxgi)?;
            let dc = d2d.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            dc.SetDpi(dpi, dpi);
            dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);

            let dxgi_factory: IDXGIFactory2 = dxgi.GetAdapter()?.GetParent()?;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: size.0,
                Height: size.1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                Stereo: false.into(),
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                Flags: SWAP_FLAGS.0 as u32,
            };
            let swap: IDXGISwapChain2 = dxgi_factory
                .CreateSwapChainForComposition(&d3d, &desc, None)?
                .cast()?;
            swap.SetMaximumFrameLatency(1)?;
            let waitable = Waitable(swap.GetFrameLatencyWaitableObject());

            let composition: IDCompositionDevice = DCompositionCreateDevice(&dxgi)?;
            let target = composition.CreateTargetForHwnd(hwnd, true)?;
            let visual = composition.CreateVisual()?;
            visual.SetContent(&swap)?;
            target.SetRoot(&visual)?;
            Ok(Gpu {
                dc,
                swap,
                composition,
                _target: target,
                _visual: visual,
                waitable,
                bitmap: None,
                size,
                spare,
                staging: None,
                shown: false,
                whole_next: true,
                // A new swap chain's waitable starts set: the first frame
                // takes its signal at once.
                held: false,
                owed: 0,
                warp,
                _d3d: d3d,
            })
        }
    }

    /// The device is lost: this surface goes, and its device is not shared
    /// again, so the next surface is made on a new one.
    pub(in crate::win) fn forget(self) {
        SHARED.with(|c| {
            let mut c = c.borrow_mut();
            if c.as_ref().is_some_and(|sh| sh.device == self._d3d) {
                *c = None;
            }
        });
    }

    /// Why the device was removed, as Direct3D says: `S_OK` while it
    /// hasn't been.
    pub(in crate::win) fn removed_reason(&self) -> HRESULT {
        // SAFETY: a plain query on this thread's device.
        match unsafe { self._d3d.GetDeviceRemovedReason() } {
            Ok(()) => HRESULT(0),
            Err(e) => e.code(),
        }
    }

    /// The swap chain's frame-latency waitable object: signalled when the
    /// swap chain can take another frame. It goes with this `Gpu`.
    pub(in crate::win) fn waitable(&self) -> HANDLE {
        self.waitable.0
    }

    /// Whether the signal for the next frame is taken already, so the
    /// message loop draws it without waiting.
    pub(in crate::win) fn holds_signal(&self) -> bool {
        self.held
    }

    /// The message loop's wait on `waitable` returned for this swap chain:
    /// its signal is taken for the next frame.
    pub(in crate::win) fn took_signal(&mut self) {
        self.held = true;
        self.settle();
    }

    /// Take the signal for the next frame if it is there now: whether it
    /// is taken.
    pub(in crate::win) fn take_signal_now(&mut self) -> bool {
        if !self.held && self.wait(0) {
            self.took_signal();
        }
        self.held
    }

    /// Wait, `FRAME_WAIT_MS` at most, for the signal for the next frame,
    /// unless it is taken already. Past that the frame is drawn anyway, and
    /// presenting it owes the signal (`present`).
    fn take_turn(&mut self) {
        if !self.held && self.wait(FRAME_WAIT_MS) {
            self.took_signal();
        }
    }

    /// Take the signals owed (`owed`) that are there now, and drop them.
    fn settle(&mut self) {
        while self.owed > 0 && self.wait(0) {
            self.owed -= 1;
        }
    }

    /// Wait up to `ms` milliseconds for the waitable: whether it was set.
    fn wait(&self, ms: u32) -> bool {
        if self.waitable.0.is_invalid() {
            return true;
        }
        // SAFETY: the swap chain's own waitable object, open while `self`
        // is.
        unsafe { WaitForSingleObjectEx(self.waitable.0, ms, false) == WAIT_OBJECT_0 }
    }

    /// Set the device context's DPI. The back buffer's bitmap carries the
    /// DPI too, so it is made again at the next frame.
    pub(super) fn set_dpi(&mut self, dpi: f32) {
        self.release_target();
        self.whole_next = true;
        // SAFETY: a plain setter on this thread's device context.
        unsafe { self.dc.SetDpi(dpi, dpi) };
    }

    /// Let go of the back buffer: Direct2D's target, then its bitmap.
    fn release_target(&mut self) {
        if self.bitmap.take().is_some() {
            // SAFETY: a plain setter on this thread's device context,
            // outside BeginDraw.
            unsafe { self.dc.SetTarget(None) };
        }
    }

    /// Size the swap chain's buffers for a client area of `size` pixels,
    /// if they don't fit it (`buffers_for`): the target bitmap is released
    /// first, as `ResizeBuffers` needs.
    pub(super) fn resize(&mut self, size: (u32, u32)) -> Result<()> {
        let size = self.buffers(size);
        if size == self.size {
            return Ok(());
        }
        self.release_target();
        self.staging = None;
        self.whole_next = true;
        // SAFETY: no reference to a back buffer is held (just released);
        // the count and format are kept and the flags are the creation's.
        unsafe {
            self.swap
                .ResizeBuffers(0, size.0, size.1, DXGI_FORMAT_UNKNOWN, SWAP_FLAGS)?;
        }
        self.size = size;
        Ok(())
    }

    /// Get the device context ready to draw a frame of `size` pixels at
    /// `dpi`: the swap chain ready to take it (`take_turn`), the buffers
    /// sized, and the back buffer its target.
    pub(in crate::win) fn begin(&mut self, size: (u32, u32), dpi: f32) -> Result<()> {
        self.take_turn();
        self.resize(size)?;
        if self.bitmap.is_some() {
            return Ok(());
        }
        // SAFETY: COM calls on this thread's objects; the properties are a
        // local that outlives the call.
        unsafe {
            let surface: IDXGISurface = self.swap.GetBuffer(0)?;
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: dpi,
                dpiY: dpi,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                colorContext: std::mem::ManuallyDrop::new(None),
            };
            let bitmap = self
                .dc
                .CreateBitmapFromDxgiSurface(&surface, Some(&props))?;
            self.dc.SetTarget(&bitmap);
            self.dc.SetDpi(dpi, dpi);
            self.bitmap = Some(bitmap);
        }
        Ok(())
    }

    /// Whether a frame that draws only part of the window can be presented
    /// as that part (`present_part`): the screen already holds a whole
    /// frame of these buffers, at this size and scale.
    pub(in crate::win) fn can_present_part(&self) -> bool {
        self.shown && !self.whole_next
    }

    /// Present the frame just drawn, synchronised to the display. The
    /// first present commits the composition and waits until the
    /// compositor has it, so the window never shows an empty swap chain.
    pub(in crate::win) fn present(&mut self) -> HRESULT {
        self.present_part(&[])
    }

    /// As `present`, for a frame that drew only inside `dirty`, in pixels
    /// of the client area: the compositor takes those rectangles from the
    /// new frame and keeps the rest of the frame it showed. An empty list,
    /// or a swap chain that can't show part of a frame
    /// (`can_present_part`), presents the whole.
    ///
    /// Either way the frame spends one signal of the waitable, as a
    /// whole present does.
    pub(in crate::win) fn present_part(&mut self, dirty: &[RECT]) -> HRESULT {
        // The frame spends the signal taken for it (`held`), or owes one.
        if !std::mem::take(&mut self.held) {
            self.owed = self.owed.saturating_add(1);
        }
        let part = !dirty.is_empty() && self.can_present_part();
        let hr = if part {
            let params = DXGI_PRESENT_PARAMETERS {
                DirtyRectsCount: dirty.len() as u32,
                pDirtyRects: dirty.as_ptr().cast_mut(),
                pScrollRect: std::ptr::null_mut(),
                pScrollOffset: std::ptr::null_mut(),
            };
            // SAFETY: a plain present of this thread's swap chain, after
            // EndDraw; `params` and the rectangles it points to outlive
            // the call, which only reads them.
            unsafe { self.swap.Present1(1, DXGI_PRESENT(0), &params) }
        } else {
            // SAFETY: a plain present of this thread's swap chain, after
            // EndDraw.
            unsafe { self.swap.Present(1, DXGI_PRESENT(0)) }
        };
        // A present that failed leaves the screen as it was: the next one
        // is whole.
        self.whole_next = hr.is_err();
        if hr.is_ok() && !self.shown {
            // SAFETY: plain calls on this thread's composition device.
            let committed = unsafe {
                self.composition
                    .Commit()
                    .and_then(|()| self.composition.WaitForCommitCompletion())
            };
            if let Err(e) = committed {
                return e.code();
            }
            self.shown = true;
        }
        hr
    }

    /// Copy the frame just drawn, before it is presented, into `out`: the
    /// back buffer's premultiplied BGRA pixels, the top row first, rows
    /// packed. It waits for the device to finish drawing it.
    pub(in crate::win) fn read_back(&mut self, out: &mut Vec<u8>) -> Result<()> {
        let (w, h) = self.size;
        // SAFETY: COM calls on this thread's device and swap chain, outside
        // BeginDraw; the descriptor is a local that outlives its call.
        // While mapped, the staging texture's memory holds `h` rows
        // `RowPitch` bytes apart, each at least `w * 4` bytes long, and it
        // is read only before `Unmap`.
        unsafe {
            let ctx = self._d3d.GetImmediateContext()?;
            if self.staging.is_none() {
                let desc = D3D11_TEXTURE2D_DESC {
                    Width: w,
                    Height: h,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Usage: D3D11_USAGE_STAGING,
                    BindFlags: 0,
                    CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                    MiscFlags: 0,
                };
                let mut tex = None;
                self._d3d.CreateTexture2D(&desc, None, Some(&mut tex))?;
                self.staging = tex;
            }
            let Some(staging) = self.staging.as_ref() else {
                return Err(windows::core::Error::empty());
            };
            let back: ID3D11Texture2D = self.swap.GetBuffer(0)?;
            ctx.CopyResource(staging, &back);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            ctx.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            out.clear();
            out.reserve((w * h * 4) as usize);
            let base = mapped.pData as *const u8;
            for y in 0..h as usize {
                let row = std::slice::from_raw_parts(
                    base.add(y * mapped.RowPitch as usize),
                    (w * 4) as usize,
                );
                out.extend_from_slice(row);
            }
            ctx.Unmap(staging, 0);
        }
        Ok(())
    }
}

/// Whether the compositor keeps `hwnd` cloaked.
#[cfg(all(test, debug_assertions))]
pub(crate) fn cloaked(hwnd: HWND) -> bool {
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
    let mut value = 0u32;
    // SAFETY: the attribute's value is a u32, a local passed with its size.
    let read = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut value as *mut u32 as *mut core::ffi::c_void,
            std::mem::size_of::<u32>() as u32,
        )
    };
    read.is_ok() && value != 0
}

#[cfg(test)]
mod tests;
