//! Graphics cards for video: a Direct3D 11 device with video support on a
//! card, and the DXGI device manager Media Foundation shares it through.
//!
//! The decoder (through the source reader), the video processor and a
//! hardware encoder all take the same [`D3d`], so frames stay on the card
//! from one to the next. [`d3d`] makes one on the first card that will have
//! it; [`adapters`] and [`d3d_for`] let a caller pick the card itself, as the
//! encoder must when only one card's hardware encoder works. The software
//! rasteriser (WARP) is never used: frames there are frames in memory, which
//! the no-device path already handles.

/// A graphics card DXGI lists, as Kiln names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adapter {
    /// The card's own name ("NVIDIA GeForce RTX 5070 Ti").
    pub name: String,
    /// The PCI vendor ID: 0x10DE NVIDIA, 0x1002 AMD, 0x8086 Intel.
    pub vendor: u32,
    /// The adapter's LUID, its high part above its low part. Media
    /// Foundation lists a card's hardware transforms by it.
    pub luid: u64,
}

#[cfg(windows)]
pub use win::{adapters, d3d, d3d_for, D3d};

#[cfg(windows)]
mod win {
    use super::Adapter;
    use crate::mf::com::Mf;
    use std::sync::Arc;
    use windows::core::Interface;
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::Graphics::Direct3D::{
        D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
    };
    use windows::Win32::Graphics::Direct3D11::{
        D3D11CreateDevice, ID3D11Device, ID3D11Multithread, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION,
    };
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, IDXGIAdapter, IDXGIAdapter1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE,
    };
    use windows::Win32::Media::MediaFoundation::{IMFDXGIDeviceManager, MFCreateDXGIDeviceManager};

    /// A D3D11 device made with video support and multithread protection on,
    /// and a DXGI device manager over it. Cloning shares the same device.
    #[derive(Clone)]
    pub struct D3d {
        device: ID3D11Device,
        manager: IMFDXGIDeviceManager,
        adapter: Adapter,
        // Last, so Media Foundation outlives the manager.
        _mf: Arc<Mf>,
    }

    // SAFETY: the device is made with multithread protection on, and the DXGI
    // device manager serialises access to it; both are free-threaded COM
    // objects, used from the multithreaded apartment.
    unsafe impl Send for D3d {}
    // SAFETY: as above; nothing in `D3d` is changed after it is made.
    unsafe impl Sync for D3d {}

    impl std::fmt::Debug for D3d {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("D3d")
                .field("adapter", &self.adapter)
                .finish()
        }
    }

    impl D3d {
        /// The D3D11 device.
        pub fn device(&self) -> &ID3D11Device {
            &self.device
        }

        /// The DXGI device manager to hand Media Foundation
        /// (`MF_SOURCE_READER_D3D_MANAGER`, `MFT_MESSAGE_SET_D3D_MANAGER`).
        pub fn manager(&self) -> &IMFDXGIDeviceManager {
            &self.manager
        }

        /// The card the device is on.
        pub fn adapter(&self) -> &Adapter {
            &self.adapter
        }
    }

    /// The hardware cards DXGI lists, in its order (the main display's card
    /// first). The software rasteriser is left out. No device is made.
    pub fn adapters() -> Vec<Adapter> {
        dxgi_adapters().into_iter().map(|(_, a)| a).collect()
    }

    /// A device on the first hardware card that will make one with video
    /// support. `None` when there is no such card.
    pub fn d3d() -> Option<D3d> {
        dxgi_adapters()
            .into_iter()
            .find_map(|(dxgi, a)| device_on(&dxgi, a))
    }

    /// A device on the card whose LUID is `luid`. `None` when DXGI doesn't
    /// list it or it won't make a device with video support.
    pub fn d3d_for(luid: u64) -> Option<D3d> {
        dxgi_adapters()
            .into_iter()
            .find(|(_, a)| a.luid == luid)
            .and_then(|(dxgi, a)| device_on(&dxgi, a))
    }

    fn dxgi_adapters() -> Vec<(IDXGIAdapter1, Adapter)> {
        crate::mf::com::com();
        let mut out = Vec::new();
        // SAFETY: plain DXGI enumeration calls.
        unsafe {
            let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else {
                return out;
            };
            let mut i = 0;
            while let Ok(a) = factory.EnumAdapters1(i) {
                i += 1;
                let Ok(desc) = a.GetDesc1() else { continue };
                if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                    continue;
                }
                let end = desc
                    .Description
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(desc.Description.len());
                let luid = ((desc.AdapterLuid.HighPart as u32 as u64) << 32)
                    | desc.AdapterLuid.LowPart as u64;
                out.push((
                    a,
                    Adapter {
                        name: String::from_utf16_lossy(&desc.Description[..end]),
                        vendor: desc.VendorId,
                        luid,
                    },
                ));
            }
        }
        out
    }

    /// A device with video support and multithread protection on `adapter`,
    /// and a DXGI device manager over it.
    fn device_on(dxgi: &IDXGIAdapter1, adapter: Adapter) -> Option<D3d> {
        let mf = Mf::start().ok()?;
        // SAFETY: plain device creation; the out pointers are locals.
        unsafe {
            let dxgi: IDXGIAdapter = dxgi.cast().ok()?;
            let mut device = None;
            D3D11CreateDevice(
                &dxgi,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_VIDEO_SUPPORT | D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )
            .ok()?;
            let device: ID3D11Device = device?;
            let mt: ID3D11Multithread = device.cast().ok()?;
            let _ = mt.SetMultithreadProtected(true);
            let mut token = 0u32;
            let mut manager = None;
            MFCreateDXGIDeviceManager(&mut token, &mut manager).ok()?;
            let manager: IMFDXGIDeviceManager = manager?;
            manager.ResetDevice(&device, token).ok()?;
            Some(D3d {
                device,
                manager,
                adapter,
                _mf: Arc::new(mf),
            })
        }
    }
}

/// No graphics card is used off Windows.
#[cfg(not(windows))]
pub use other::{adapters, d3d, d3d_for, D3d};

#[cfg(not(windows))]
mod other {
    use super::Adapter;

    /// A graphics card device; there is never one off Windows.
    #[derive(Debug, Clone)]
    pub enum D3d {}

    impl D3d {
        pub fn adapter(&self) -> &Adapter {
            match *self {}
        }
    }

    pub fn adapters() -> Vec<Adapter> {
        Vec::new()
    }

    pub fn d3d() -> Option<D3d> {
        None
    }

    pub fn d3d_for(_luid: u64) -> Option<D3d> {
        None
    }
}
