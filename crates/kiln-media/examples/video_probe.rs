//! A probe of what Windows' Media Foundation can read, decode, process and
//! encode on this PC. A diagnostic, not a test:
//!
//! ```text
//! cargo run -p kiln-media --example video_probe -- crates/kiln-media/tests/data/video
//! ```
//!
//! For each video file in the folder it reads every stream compressed, then
//! decodes the video on each graphics card and without one. It lists the
//! video decoders and every H.264 and HEVC encoder, asks each encoder about
//! its rate-control modes and encodes a few frames in each, from memory and
//! from the graphics card. Last, it runs the video processor over a PQ ramp
//! (HDR to standard range), a marked frame (rotation), and real decoded
//! frames from rot90.mp4 and hevc10_hdr.mp4 when the folder has them.
//!
//! Each file, encoder and processor pass runs in a child process (this same
//! program with --file, --encoder or --processor), as some drivers bring
//! their process down; the report then says how it died.

#[cfg(not(windows))]
fn main() {
    eprintln!("The video probe needs Windows' Media Foundation.");
}

#[cfg(windows)]
fn main() {
    win::main();
}

#[cfg(windows)]
mod win {
    use kiln_media::probing::{byte_stream, enum_transforms, Mf};
    use std::fs::File;
    use std::io::Read;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};
    use windows::core::{Interface, GUID, PCWSTR, PWSTR};
    use windows::Win32::Foundation::{E_FAIL, HMODULE};
    use windows::Win32::Graphics::Direct3D::{
        D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
    };
    use windows::Win32::Graphics::Direct3D11::{
        D3D11CreateDevice, ID3D11Device, ID3D11Multithread, ID3D11Texture2D, D3D11_BIND_DECODER,
        D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION, D3D11_SUBRESOURCE_DATA,
        D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    };
    use windows::Win32::Graphics::Dxgi::Common::{
        DXGI_FORMAT, DXGI_FORMAT_NV12, DXGI_FORMAT_P010, DXGI_SAMPLE_DESC,
    };
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, IDXGIAdapter, IDXGIAdapter1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE,
    };
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};
    use windows::Win32::System::Variant::VARIANT;

    type WinResult<T> = windows::core::Result<T>;

    const VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    const ALL: u32 = MF_SOURCE_READER_ALL_STREAMS.0 as u32;
    const ANY: u32 = MF_SOURCE_READER_ANY_STREAM.0 as u32;
    const HNS: f64 = 10_000_000.0;

    /// Runs the whole probe, each file, encoder and processor test in a
    /// process of its own (this same program, with `--file`, `--encoder` or
    /// `--processor`), so a driver that brings its process down costs only
    /// its own lines, which then say how it died.
    pub fn main() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let _mf = match Mf::start() {
            Ok(m) => m,
            Err(e) => {
                println!("Media Foundation won't start: {e}");
                return;
            }
        };
        if args.first().map(String::as_str) == Some("--amd") {
            amd_activation(args.get(1).map(String::as_str).unwrap_or(""));
            return;
        }
        let adapters = adapters();
        let devices: Vec<&D3d> = adapters
            .iter()
            .filter_map(|a| a.d3d.as_ref().ok())
            .collect();
        let arg = |i: usize| args.get(i).map(String::as_str).unwrap_or("");
        match arg(0) {
            "" => {
                eprintln!("usage: video_probe <folder of video files>");
                std::process::exit(2);
            }
            "--file" => file_report(Path::new(arg(1)), &devices),
            "--encoder" => {
                let codec = if arg(1) == "hevc" {
                    MFVideoFormat_HEVC
                } else {
                    MFVideoFormat_H264
                };
                let listing = match arg(2) {
                    "hw" => Listing::Hardware,
                    "sw" => Listing::Software,
                    l => match u64::from_str_radix(l, 16) {
                        Ok(luid) if l.len() == 16 => Listing::Card(luid),
                        _ => {
                            eprintln!("bad listing {l:?}: hw, sw or a 16-digit hex LUID");
                            std::process::exit(2);
                        }
                    },
                };
                let index: usize = arg(3).parse().unwrap_or(0);
                let src = Path::new(arg(4)).join("h264.mp4");
                let src = src.exists().then_some(src);
                match encoder_list(codec, listing).into_iter().nth(index) {
                    Some(act) => {
                        encoder_report(codec, listing, index, &act, &devices, src.as_deref())
                    }
                    None => println!("  [{index}] no longer listed"),
                }
            }
            "--processor" => {
                let dir = Path::new(arg(2));
                let rot90 = dir.join("rot90.mp4");
                let hdr = dir.join("hevc10_hdr.mp4");
                let d3d = match arg(1) {
                    "none" => None,
                    card => match devices
                        .iter()
                        .copied()
                        .find(|d| format!("{:016X}", d.luid) == card)
                    {
                        Some(d) => Some(d),
                        None => {
                            eprintln!(
                                "bad card {card:?}: none or the LUID of a card with a device"
                            );
                            std::process::exit(2);
                        }
                    },
                };
                processor_report(
                    d3d,
                    rot90.exists().then_some(rot90.as_path()),
                    hdr.exists().then_some(hdr.as_path()),
                );
            }
            dir => whole(Path::new(dir), &adapters, &devices),
        }
    }

    fn whole(dir: &Path, adapters: &[Adapter], devices: &[&D3d]) {
        println!("# Graphics cards");
        if adapters.is_empty() {
            println!("  none found (DXGI lists no hardware adapter)");
        }
        for a in adapters {
            let d3d = match &a.d3d {
                Ok(_) => "D3D11 video device: made".to_string(),
                Err(e) => format!("D3D11 video device: failed {}", hr(e)),
            };
            println!(
                "  {} (vendor {:04X}, LUID {:016X}); {d3d}",
                a.name, a.vendor, a.luid
            );
        }

        println!();
        println!("# Video decoders Media Foundation lists");
        decoders();

        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect())
            .unwrap_or_default();
        files.retain(|p| {
            matches!(
                p.extension()
                    .and_then(|e| e.to_str())
                    .map(str::to_ascii_lowercase)
                    .as_deref(),
                Some("mp4" | "mkv" | "mov" | "m4v" | "webm")
            )
        });
        files.sort();
        for f in &files {
            println!();
            child(&["--file".into(), f.to_string_lossy().into_owned()]);
        }

        println!();
        println!("# Encoders");
        for (codec, arg, name) in [
            (MFVideoFormat_H264, "h264", "H.264"),
            (MFVideoFormat_HEVC, "hevc", "HEVC"),
        ] {
            println!("## {name}");
            let mut listings = vec![(
                Listing::Hardware,
                "hw".to_string(),
                "Hardware, MFTEnumEx (HARDWARE|ASYNCMFT|SORTANDFILTER)".to_string(),
            )];
            for d in devices {
                listings.push((
                    Listing::Card(d.luid),
                    format!("{:016X}", d.luid),
                    format!(
                        "Hardware on {}, MFTEnum2 with its LUID (same flags)",
                        short(&d.name)
                    ),
                ));
            }
            listings.push((
                Listing::Software,
                "sw".to_string(),
                "Software, MFTEnumEx (SYNCMFT|LOCALMFT|SORTANDFILTER)".to_string(),
            ));
            for (listing, key, label) in listings {
                let count = encoder_list(codec, listing).len();
                println!("  {label}: {count} found");
                for i in 0..count {
                    child(&[
                        "--encoder".into(),
                        arg.into(),
                        key.clone(),
                        i.to_string(),
                        dir.to_string_lossy().into_owned(),
                    ]);
                }
            }
        }

        println!();
        println!("# AMD encoders activated from a fresh process");
        for mode in ["clean", "amd-device"] {
            child(&["--amd".into(), mode.into()]);
        }

        println!();
        println!("# Video processor (CLSID_VideoProcessorMFT)");
        let mut cards = vec!["none".to_string()];
        cards.extend(devices.iter().map(|d| format!("{:016X}", d.luid)));
        for c in cards {
            child(&["--processor".into(), c, dir.to_string_lossy().into_owned()]);
        }
    }

    /// How long one part may run before it is taken as hung and killed.
    const CHILD_DEADLINE: Duration = Duration::from_secs(60);

    /// Runs this program on one part and prints what it printed, and how
    /// it died if it did. A part still running after [`CHILD_DEADLINE`] is
    /// killed, and what it printed until then is kept.
    fn child(args: &[String]) {
        use std::io::Read as _;
        use std::process::{Command, Stdio};
        use std::sync::{Arc, Mutex};
        let exe = match std::env::current_exe() {
            Ok(e) => e,
            Err(e) => {
                println!("  can't find this program to run a part: {e}");
                return;
            }
        };
        let mut proc = match Command::new(exe)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(p) => p,
            Err(e) => {
                println!("  can't run a part: {e}");
                return;
            }
        };
        // Each pipe is drained by a thread of its own into a shared buffer, so
        // a part that hangs still leaves what it wrote.
        let drain = |pipe: Option<Box<dyn std::io::Read + Send>>| {
            let buf = Arc::new(Mutex::new(Vec::new()));
            let into = buf.clone();
            let handle = std::thread::spawn(move || {
                if let Some(mut pipe) = pipe {
                    let mut chunk = [0u8; 4096];
                    while let Ok(n) = pipe.read(&mut chunk) {
                        if n == 0 {
                            break;
                        }
                        into.lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .extend_from_slice(&chunk[..n]);
                    }
                }
            });
            (buf, handle)
        };
        let (out, out_thread) = drain(
            proc.stdout
                .take()
                .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
        );
        let (err, err_thread) = drain(
            proc.stderr
                .take()
                .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
        );
        let started = Instant::now();
        let status = loop {
            match proc.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if started.elapsed() > CHILD_DEADLINE => {
                    let _ = proc.kill();
                    let _ = proc.wait();
                    break None;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                Err(e) => {
                    println!("  can't wait for a part: {e}");
                    let _ = proc.kill();
                    break None;
                }
            }
        };
        let _ = out_thread.join();
        let _ = err_thread.join();
        let text = |b: &Arc<Mutex<Vec<u8>>>| {
            String::from_utf8_lossy(&b.lock().unwrap_or_else(|p| p.into_inner())).into_owned()
        };
        let printed = text(&out);
        print!("{printed}");
        if !printed.is_empty() && !printed.ends_with('\n') {
            println!();
        }
        let failed = match status {
            None => {
                println!(
                    "      !! timed out: the part was still running after {} s and was killed, after the lines above",
                    CHILD_DEADLINE.as_secs()
                );
                true
            }
            Some(status) if !status.success() => {
                let code = status
                    .code()
                    .map_or("none".into(), |c| format!("{:#010X}", c as u32));
                let what = match status.code().map(|c| c as u32) {
                    Some(0xC000_0094) => " (STATUS_INTEGER_DIVIDE_BY_ZERO)",
                    Some(0xC000_0005) => " (STATUS_ACCESS_VIOLATION)",
                    Some(0xC000_0409) => " (STATUS_STACK_BUFFER_OVERRUN / fail-fast)",
                    Some(0xC000_00FD) => " (STATUS_STACK_OVERFLOW)",
                    Some(101) => " (a Rust panic)",
                    Some(2) => " (bad arguments)",
                    _ => "",
                };
                println!("      !! the process running this part died, exit code {code}{what}, after the lines above");
                true
            }
            Some(_) => false,
        };
        if failed {
            for l in text(&err).lines().take(5) {
                println!("      !! {l}");
            }
        }
    }

    // -----------------------------------------------------------------------
    // Small helpers

    /// An HRESULT, named where it's one of the usual ones.
    fn hr(e: &windows::core::Error) -> String {
        let c = e.code();
        let known = [
            (MF_E_INVALIDMEDIATYPE, "MF_E_INVALIDMEDIATYPE"),
            (MF_E_TOPO_CODEC_NOT_FOUND, "MF_E_TOPO_CODEC_NOT_FOUND"),
            (
                MF_E_UNSUPPORTED_BYTESTREAM_TYPE,
                "MF_E_UNSUPPORTED_BYTESTREAM_TYPE",
            ),
            (MF_E_INVALIDSTREAMNUMBER, "MF_E_INVALIDSTREAMNUMBER"),
            (MF_E_TRANSFORM_TYPE_NOT_SET, "MF_E_TRANSFORM_TYPE_NOT_SET"),
            (MF_E_NO_MORE_TYPES, "MF_E_NO_MORE_TYPES"),
            (MF_E_ATTRIBUTENOTFOUND, "MF_E_ATTRIBUTENOTFOUND"),
            (MF_E_NOTACCEPTING, "MF_E_NOTACCEPTING"),
            (MF_E_UNSUPPORTED_D3D_TYPE, "MF_E_UNSUPPORTED_D3D_TYPE"),
            (MF_E_TRANSFORM_ASYNC_LOCKED, "MF_E_TRANSFORM_ASYNC_LOCKED"),
            (MF_E_INVALIDREQUEST, "MF_E_INVALIDREQUEST"),
            (MF_E_UNEXPECTED, "MF_E_UNEXPECTED"),
            (MF_E_UNSUPPORTED_SERVICE, "MF_E_UNSUPPORTED_SERVICE"),
            (windows::Win32::Foundation::E_NOTIMPL, "E_NOTIMPL"),
            (windows::Win32::Foundation::E_INVALIDARG, "E_INVALIDARG"),
            (windows::Win32::Foundation::E_FAIL, "E_FAIL"),
            (windows::Win32::Foundation::E_ACCESSDENIED, "E_ACCESSDENIED"),
            (windows::Win32::Foundation::E_POINTER, "E_POINTER"),
        ];
        let name = known.iter().find(|(k, _)| *k == c).map(|(_, n)| *n);
        match name {
            Some(n) => format!("{n} ({:#010X})", c.0 as u32),
            None => {
                let m = e.message();
                let m = m.trim();
                if m.is_empty() {
                    format!("{:#010X}", c.0 as u32)
                } else {
                    format!("{:#010X} \"{m}\"", c.0 as u32)
                }
            }
        }
    }

    fn fail() -> windows::core::Error {
        E_FAIL.into()
    }

    fn subtype_name(g: &GUID) -> String {
        let known = [
            (MFVideoFormat_H264, "H264"),
            (MFVideoFormat_H264_ES, "H264_ES"),
            (MFVideoFormat_HEVC, "HEVC"),
            (MFVideoFormat_HEVC_ES, "HEVC_ES"),
            (MFVideoFormat_AV1, "AV1"),
            (MFVideoFormat_VP90, "VP9"),
            (MFVideoFormat_MP4V, "MP4V"),
            (MFVideoFormat_MJPG, "MJPG"),
            (MFVideoFormat_NV12, "NV12"),
            (MFVideoFormat_P010, "P010"),
            (MFVideoFormat_YUY2, "YUY2"),
            (MFVideoFormat_IYUV, "IYUV"),
            (MFVideoFormat_I420, "I420"),
            (MFVideoFormat_ARGB32, "ARGB32"),
            (MFVideoFormat_RGB32, "RGB32"),
            (MFAudioFormat_AAC, "AAC"),
            (MFAudioFormat_Opus, "Opus"),
            (MFAudioFormat_Float, "Float"),
            (MFAudioFormat_PCM, "PCM"),
            (MFMediaType_Video, "Video"),
            (MFMediaType_Audio, "Audio"),
        ];
        if let Some((_, n)) = known.iter().find(|(k, _)| k == g) {
            return n.to_string();
        }
        let four = g.data1.to_le_bytes();
        if four.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
            format!("'{}'", String::from_utf8_lossy(&four))
        } else {
            format!("{g:?}")
        }
    }

    fn get_u32(a: &IMFAttributes, k: &GUID) -> Option<u32> {
        // SAFETY: a plain attribute read.
        unsafe { a.GetUINT32(k) }.ok()
    }

    fn get_u64(a: &IMFAttributes, k: &GUID) -> Option<u64> {
        // SAFETY: a plain attribute read.
        unsafe { a.GetUINT64(k) }.ok()
    }

    fn get_guid(a: &IMFAttributes, k: &GUID) -> Option<GUID> {
        // SAFETY: a plain attribute read.
        unsafe { a.GetGUID(k) }.ok()
    }

    fn get_blob(a: &IMFAttributes, k: &GUID) -> Option<Vec<u8>> {
        // SAFETY: the buffer is sized from GetBlobSize before GetBlob fills it.
        unsafe {
            let n = a.GetBlobSize(k).ok()?;
            let mut v = vec![0u8; n as usize];
            a.GetBlob(k, &mut v, None).ok()?;
            Some(v)
        }
    }

    fn get_string(a: &IMFAttributes, k: &GUID) -> Option<String> {
        // SAFETY: the string is CoTaskMem the caller frees, after copying.
        unsafe {
            let mut p = PWSTR::null();
            let mut len = 0u32;
            a.GetAllocatedString(k, &mut p, &mut len).ok()?;
            let s = p.to_string().ok();
            CoTaskMemFree(Some(p.0 as *const _));
            s
        }
    }

    fn hex(b: &[u8], n: usize) -> String {
        let mut s: Vec<String> = b.iter().take(n).map(|x| format!("{x:02X}")).collect();
        if b.len() > n {
            s.push("..".into());
        }
        s.join(" ")
    }

    fn hi_lo(v: u64) -> (u32, u32) {
        ((v >> 32) as u32, v as u32)
    }

    fn secs(hns: i64) -> String {
        format!("{:.3}", hns as f64 / HNS)
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    /// MF_MT_MINIMUM_DISPLAY_APERTURE and kin: an MFVideoArea.
    fn area(b: &[u8]) -> Option<String> {
        if b.len() < 16 {
            return None;
        }
        let x = i16::from_le_bytes([b[2], b[3]]);
        let y = i16::from_le_bytes([b[6], b[7]]);
        let w = i32::from_le_bytes([b[8], b[9], b[10], b[11]]);
        let h = i32::from_le_bytes([b[12], b[13], b[14], b[15]]);
        Some(format!("{w}x{h} at ({x},{y})"))
    }

    fn media_type(major: &GUID, subtype: &GUID) -> WinResult<IMFMediaType> {
        // SAFETY: plain calls on a new media type.
        unsafe {
            let t = MFCreateMediaType()?;
            t.SetGUID(&MF_MT_MAJOR_TYPE, major)?;
            t.SetGUID(&MF_MT_SUBTYPE, subtype)?;
            Ok(t)
        }
    }

    /// A full uncompressed or compressed video type at `w`x`h`, 30 fps.
    fn video_type(subtype: &GUID, w: u32, h: u32) -> WinResult<IMFMediaType> {
        let t = media_type(&MFMediaType_Video, subtype)?;
        // SAFETY: plain attribute calls.
        unsafe {
            t.SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | h as u64)?;
            t.SetUINT64(&MF_MT_FRAME_RATE, (30u64 << 32) | 1)?;
            t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
            t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        }
        Ok(t)
    }

    /// The colour facts a video type carries, as text.
    fn colour(t: &IMFAttributes) -> String {
        let mut parts = Vec::new();
        let names = [
            (&MF_MT_TRANSFER_FUNCTION, "transfer"),
            (&MF_MT_VIDEO_PRIMARIES, "primaries"),
            (&MF_MT_YUV_MATRIX, "matrix"),
            (&MF_MT_VIDEO_NOMINAL_RANGE, "range"),
            (&MF_MT_MAX_LUMINANCE_LEVEL, "max CLL"),
            (&MF_MT_MAX_FRAME_AVERAGE_LUMINANCE_LEVEL, "max FALL"),
            (&MF_MT_MAX_MASTERING_LUMINANCE, "mastering max"),
        ];
        for (k, n) in names {
            if let Some(v) = get_u32(t, k) {
                let v = match n {
                    "transfer" => match v {
                        15 => "PQ (2084)".to_string(),
                        16 => "HLG".to_string(),
                        5 => "709".to_string(),
                        x => x.to_string(),
                    },
                    "primaries" => match v {
                        9 => "BT.2020".to_string(),
                        2 => "BT.709".to_string(),
                        x => x.to_string(),
                    },
                    "matrix" => match v {
                        4 => "BT.2020".to_string(),
                        1 => "BT.709".to_string(),
                        2 => "BT.601".to_string(),
                        x => x.to_string(),
                    },
                    "range" => match v {
                        2 => "16-235".to_string(),
                        1 => "0-255".to_string(),
                        x => x.to_string(),
                    },
                    _ => v.to_string(),
                };
                parts.push(format!("{n} {v}"));
            }
        }
        if parts.is_empty() {
            "no colour attributes".into()
        } else {
            parts.join(", ")
        }
    }

    // -----------------------------------------------------------------------
    // Graphics cards

    struct Adapter {
        name: String,
        vendor: u32,
        luid: u64,
        d3d: WinResult<D3d>,
    }

    pub struct D3d {
        name: String,
        vendor: u32,
        luid: u64,
        device: ID3D11Device,
        manager: IMFDXGIDeviceManager,
    }

    fn adapters() -> Vec<Adapter> {
        dxgi_adapters()
            .into_iter()
            .map(|(a, name, vendor, luid)| {
                let d3d = device_on(&a, &name, vendor, luid);
                Adapter {
                    name,
                    vendor,
                    luid,
                    d3d,
                }
            })
            .collect()
    }

    /// The hardware adapters DXGI lists: each with its name, vendor ID and
    /// LUID. No device is made.
    fn dxgi_adapters() -> Vec<(IDXGIAdapter1, String, u32, u64)> {
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
                let name = String::from_utf16_lossy(&desc.Description[..end]);
                let luid = ((desc.AdapterLuid.HighPart as u32 as u64) << 32)
                    | desc.AdapterLuid.LowPart as u64;
                out.push((a, name, desc.VendorId, luid));
            }
        }
        out
    }

    /// A D3D11 device with video support and multithread protection on
    /// `adapter`, and a DXGI device manager over it.
    fn device_on(adapter: &IDXGIAdapter1, name: &str, vendor: u32, luid: u64) -> WinResult<D3d> {
        // SAFETY: plain device creation; the out pointers are locals.
        unsafe {
            let adapter: IDXGIAdapter = adapter.cast()?;
            let mut device = None;
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_VIDEO_SUPPORT | D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )?;
            let device = device.ok_or_else(fail)?;
            let mt: ID3D11Multithread = device.cast()?;
            let _ = mt.SetMultithreadProtected(true);
            let mut token = 0u32;
            let mut manager = None;
            MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
            let manager = manager.ok_or_else(fail)?;
            manager.ResetDevice(&device, token)?;
            Ok(D3d {
                name: name.to_string(),
                vendor,
                luid,
                device,
                manager,
            })
        }
    }

    fn short(name: &str) -> &str {
        if name.contains("NVIDIA") {
            "NVIDIA"
        } else if name.contains("AMD") || name.contains("Radeon") {
            "AMD"
        } else if name.contains("Intel") {
            "Intel"
        } else {
            name
        }
    }

    // -----------------------------------------------------------------------
    // Decoders

    fn activate_name(a: &IMFAttributes) -> String {
        get_string(a, &MFT_FRIENDLY_NAME_Attribute).unwrap_or_else(|| "(no name)".into())
    }

    fn decoders() {
        let (hw, sw) = (hw_flags(), sw_flags());
        let every = MFT_ENUM_FLAG_ALL | MFT_ENUM_FLAG_UNTRUSTED_STOREMFT;
        for (sub, name) in [
            (MFVideoFormat_H264, "H.264"),
            (MFVideoFormat_HEVC, "HEVC"),
            (MFVideoFormat_AV1, "AV1"),
            (MFVideoFormat_VP90, "VP9"),
            (MFVideoFormat_MP4V, "MPEG-4 Part 2"),
            (MFVideoFormat_MJPG, "MJPEG"),
        ] {
            let list = |flags| -> Vec<String> {
                enum_transforms(
                    MFT_CATEGORY_VIDEO_DECODER,
                    flags,
                    Some((MFMediaType_Video, sub)),
                    None,
                    None,
                )
                .iter()
                .map(|a| activate_name(a))
                .collect()
            };
            let (h, s, all) = (list(hw), list(sw), list(every));
            println!("  {name}:");
            println!(
                "    hardware (HARDWARE|ASYNCMFT|SORTANDFILTER): {}",
                join_or_none(&h)
            );
            println!(
                "    software (SYNCMFT|LOCALMFT|SORTANDFILTER): {}",
                join_or_none(&s)
            );
            println!(
                "    every flag (ALL|UNTRUSTED_STOREMFT): {}",
                join_or_none(&all)
            );
        }
    }

    fn join_or_none(v: &[String]) -> String {
        if v.is_empty() {
            "none".into()
        } else {
            v.join("; ")
        }
    }

    // -----------------------------------------------------------------------
    // Reading a file

    /// The MIME type the first bytes suggest, which helps Media Foundation
    /// pick its source.
    fn content_type(path: &Path) -> Option<&'static str> {
        let mut head = [0u8; 12];
        let n = File::open(path).ok()?.read(&mut head).ok()?;
        let h = &head[..n];
        if h.len() >= 12 && &h[4..8] == b"ftyp" {
            Some(if &h[8..12] == b"qt  " {
                "video/quicktime"
            } else {
                "video/mp4"
            })
        } else if h.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
            Some("video/x-matroska")
        } else {
            None
        }
    }

    /// A source reader over the file through Kiln's byte stream (Media
    /// Foundation never sees the path), told the content type when `hint`.
    fn open_reader(
        path: &Path,
        attrs: Option<&IMFAttributes>,
        hint: bool,
    ) -> WinResult<IMFSourceReader> {
        let file = File::open(path).map_err(|_| fail())?;
        let (stream, _) = byte_stream(file, false).map_err(|_| fail())?;
        if hint {
            if let Some(ct) = content_type(path) {
                let w = wide(ct);
                // SAFETY: the string is NUL-terminated and copied by the call.
                unsafe {
                    stream
                        .cast::<IMFAttributes>()?
                        .SetString(&MF_BYTESTREAM_CONTENT_TYPE, PCWSTR(w.as_ptr()))?;
                }
            }
        }
        // SAFETY: the byte stream is live; the reader keeps its own reference.
        unsafe { MFCreateSourceReaderFromByteStream(&stream, attrs) }
    }

    fn file_report(path: &Path, devices: &[&D3d]) {
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        println!(
            "## {} ({} bytes, {})",
            path.file_name().unwrap_or_default().to_string_lossy(),
            size,
            content_type(path).unwrap_or("unknown type")
        );
        match open_reader(path, None, false) {
            Ok(_) => println!("  opens with no content type given: yes"),
            Err(e) => println!("  opens with no content type given: no, {}", hr(&e)),
        }
        println!("  Compressed mode (native types, no decoder):");
        if let Err(e) = compressed(path) {
            println!("    failed: {}", hr(&e));
        }
        println!("  Decoded mode:");
        for d in devices {
            for sub in [MFVideoFormat_NV12, MFVideoFormat_P010] {
                decoded_line(path, Some(d), sub);
            }
        }
        for sub in [MFVideoFormat_NV12, MFVideoFormat_P010] {
            decoded_line(path, None, sub);
        }
    }

    fn describe_type(t: &IMFMediaType) -> String {
        let major = get_guid(t, &MF_MT_MAJOR_TYPE).unwrap_or_default();
        let sub = get_guid(t, &MF_MT_SUBTYPE).unwrap_or_default();
        let mut s = format!("{} {}", subtype_name(&major), subtype_name(&sub));
        if major == MFMediaType_Video {
            if let Some((w, h)) = get_u64(t, &MF_MT_FRAME_SIZE).map(hi_lo) {
                s += &format!(" {w}x{h}");
            }
            if let Some((n, d)) = get_u64(t, &MF_MT_FRAME_RATE).map(hi_lo) {
                s += &format!(" {n}/{d} fps");
            }
            if let Some(p) = get_u32(t, &MF_MT_MPEG2_PROFILE) {
                s += &format!(", profile {p}");
            }
            if let Some(l) = get_u32(t, &MF_MT_MPEG2_LEVEL) {
                s += &format!(" level {l}");
            }
            match get_u32(t, &MF_MT_VIDEO_ROTATION) {
                Some(r) => s += &format!(", MF_MT_VIDEO_ROTATION {r}"),
                None => s += ", no MF_MT_VIDEO_ROTATION",
            }
            if let Some(a) = get_blob(t, &MF_MT_MINIMUM_DISPLAY_APERTURE).and_then(|b| area(&b)) {
                s += &format!(", display aperture {a}");
            }
            if let Some(b) = get_u32(t, &MF_MT_AVG_BITRATE) {
                s += &format!(", {} kbps", b / 1000);
            }
            s += &format!(", {}", colour(t));
        } else if major == MFMediaType_Audio {
            if let Some(r) = get_u32(t, &MF_MT_AUDIO_SAMPLES_PER_SECOND) {
                s += &format!(" {r} Hz");
            }
            if let Some(c) = get_u32(t, &MF_MT_AUDIO_NUM_CHANNELS) {
                s += &format!(" {c} ch");
            }
            if let Some(b) = get_u32(t, &MF_MT_AUDIO_AVG_BYTES_PER_SECOND) {
                s += &format!(", {} kbps", b * 8 / 1000);
            }
        }
        for (k, n) in [
            (&MF_MT_MPEG_SEQUENCE_HEADER, "MF_MT_MPEG_SEQUENCE_HEADER"),
            (&MF_MT_USER_DATA, "MF_MT_USER_DATA"),
        ] {
            match get_blob(t, k) {
                Some(b) => s += &format!("\n        {n}: {} B [{}]", b.len(), hex(&b, 24)),
                None => s += &format!("\n        {n}: none"),
            }
        }
        s
    }

    #[derive(Default)]
    struct Stats {
        count: u32,
        keys: u32,
        bytes: u64,
        with_dts: u32,
        dur_sum: i64,
        pts: Vec<i64>,
        first: Vec<String>,
        first_bytes: Option<Vec<u8>>,
        ended: bool,
        ticks: u32,
    }

    fn sample_bytes(s: &IMFSample, max: usize) -> Option<Vec<u8>> {
        // SAFETY: the buffer is locked while its bytes are copied out.
        unsafe {
            let b = s.ConvertToContiguousBuffer().ok()?;
            let mut p = std::ptr::null_mut();
            let mut len = 0u32;
            b.Lock(&mut p, None, Some(&mut len)).ok()?;
            let v = std::slice::from_raw_parts(p, (len as usize).min(max)).to_vec();
            let _ = b.Unlock();
            Some(v)
        }
    }

    fn compressed(path: &Path) -> WinResult<()> {
        let reader = open_reader(path, None, true)?;
        let mut types = Vec::new();
        for i in 0..16u32 {
            // SAFETY: a plain call on the reader.
            match unsafe { reader.GetNativeMediaType(i, 0) } {
                Ok(t) => types.push(t),
                Err(_) => break,
            }
        }
        // SAFETY: plain calls on the reader; the PROPVARIANT is a copy.
        let duration = unsafe {
            reader
                .GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)
                .ok()
                .and_then(|v| u64::try_from(&v).ok())
        };
        println!(
            "    {} streams, duration {}",
            types.len(),
            duration.map_or("unknown".into(), |d| format!("{} s", secs(d as i64)))
        );
        // SAFETY: plain calls on the reader with types it gave.
        unsafe {
            reader.SetStreamSelection(ALL, true)?;
            for (i, t) in types.iter().enumerate() {
                println!("    stream {i}: {}", describe_type(t));
                if let Err(e) = reader.SetCurrentMediaType(i as u32, None, t) {
                    println!(
                        "        setting the native type as current failed: {}",
                        hr(&e)
                    );
                }
            }
        }
        let mut stats: Vec<Stats> = types.iter().map(|_| Stats::default()).collect();
        let started = Instant::now();
        let mut error = None;
        for _ in 0..20_000 {
            let mut index = 0u32;
            let mut flags = 0u32;
            let mut ts = 0i64;
            let mut sample = None;
            // SAFETY: a synchronous read; the out pointers are locals.
            let r = unsafe {
                reader.ReadSample(
                    ANY,
                    0,
                    Some(&mut index),
                    Some(&mut flags),
                    Some(&mut ts),
                    Some(&mut sample),
                )
            };
            if let Err(e) = r {
                error = Some(hr(&e));
                break;
            }
            if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
                error = Some("MF_SOURCE_READERF_ERROR".into());
                break;
            }
            let Some(st) = stats.get_mut(index as usize) else {
                continue;
            };
            if flags & MF_SOURCE_READERF_STREAMTICK.0 as u32 != 0 {
                st.ticks += 1;
            }
            if let Some(s) = sample {
                // SAFETY: plain reads from a sample the reader gave.
                unsafe {
                    let pts = s.GetSampleTime().unwrap_or(ts);
                    let dur = s.GetSampleDuration().unwrap_or(0);
                    let key = get_u32(&s, &MFSampleExtension_CleanPoint) == Some(1);
                    let dts = get_u64(&s, &MFSampleExtension_DecodeTimestamp).map(|d| d as i64);
                    let len = s.GetTotalLength().unwrap_or(0);
                    st.count += 1;
                    st.keys += key as u32;
                    st.bytes += len as u64;
                    st.with_dts += dts.is_some() as u32;
                    st.dur_sum += dur;
                    st.pts.push(pts);
                    if st.first.len() < 3 {
                        let head = sample_bytes(&s, 16).unwrap_or_default();
                        st.first.push(format!(
                            "pts {} dur {} dts {} {}{} B [{}]",
                            secs(pts),
                            secs(dur),
                            dts.map_or("-".into(), secs),
                            if key { "key " } else { "" },
                            len,
                            hex(&head, 12)
                        ));
                        if st.first_bytes.is_none() {
                            st.first_bytes = Some(head);
                        }
                    }
                }
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                st.ended = true;
                if stats.iter().all(|s| s.ended) {
                    break;
                }
            }
        }
        let ms = started.elapsed().as_millis();
        for (i, st) in stats.iter().enumerate() {
            let mut pts = st.pts.clone();
            pts.sort_unstable();
            let range = match (pts.first(), pts.last()) {
                (Some(a), Some(b)) => format!("pts {}..{} s", secs(*a), secs(*b)),
                _ => "no samples".into(),
            };
            let start_code = st.first_bytes.as_deref().map(|b| {
                if b.starts_with(&[0, 0, 0, 1]) || b.starts_with(&[0, 0, 1]) {
                    "starts with an Annex B start code"
                } else {
                    "no start code (length-prefixed or not NAL)"
                }
            });
            println!(
                "    stream {i} read: {} samples, {} key, {} B, {range}, {} with a decode time, sum of durations {} s{}{}",
                st.count,
                st.keys,
                st.bytes,
                st.with_dts,
                secs(st.dur_sum),
                if st.ended { ", reached its end" } else { ", did NOT reach its end" },
                if st.ticks > 0 { format!(", {} stream ticks", st.ticks) } else { String::new() }
            );
            if let Some(sc) = start_code {
                println!("      first sample {sc}");
            }
            for f in &st.first {
                println!("      {f}");
            }
            let mut deltas: Vec<i64> = pts.windows(2).map(|w| (w[1] - w[0] + 500) / 1000).collect();
            deltas.sort_unstable();
            let mut runs: Vec<(i64, u32)> = Vec::new();
            for d in deltas {
                match runs.last_mut() {
                    Some((v, n)) if *v == d => *n += 1,
                    _ => runs.push((d, 1)),
                }
            }
            runs.sort_by_key(|r| std::cmp::Reverse(r.1));
            if !runs.is_empty() {
                let shown: Vec<String> = runs
                    .iter()
                    .take(4)
                    .map(|(d, n)| format!("{:.1} ms x{n}", *d as f64 / 10.0))
                    .collect();
                println!(
                    "      pts steps (sorted order): {}{}",
                    shown.join(", "),
                    if runs.len() > 4 { ", .." } else { "" }
                );
            }
        }
        match error {
            Some(e) => println!("    reading stopped: {e} ({ms} ms)"),
            None => println!("    read in {ms} ms"),
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Decoded mode

    /// A reader that decodes the first video stream to `subtype`, on the
    /// graphics card when `d3d` is given.
    fn decode_reader(path: &Path, d3d: Option<&D3d>, subtype: &GUID) -> WinResult<IMFSourceReader> {
        // SAFETY: plain attribute and reader calls on objects made here.
        unsafe {
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 2)?;
            let attrs = attrs.ok_or_else(fail)?;
            if let Some(d) = d3d {
                attrs.SetUnknown(&MF_SOURCE_READER_D3D_MANAGER, &d.manager)?;
                attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
            }
            let reader = open_reader(path, Some(&attrs), true)?;
            reader.SetStreamSelection(ALL, false)?;
            reader.SetStreamSelection(VIDEO, true)?;
            let t = media_type(&MFMediaType_Video, subtype)?;
            reader.SetCurrentMediaType(VIDEO, None, &t)?;
            Ok(reader)
        }
    }

    /// The transforms the reader put between the source and the output.
    fn chain(reader: &IMFSourceReader) -> String {
        let Ok(ex) = reader.cast::<IMFSourceReaderEx>() else {
            return "no IMFSourceReaderEx".into();
        };
        let mut parts = Vec::new();
        for k in 0..6 {
            let mut cat = GUID::zeroed();
            let mut t = None;
            // SAFETY: a plain call; the out pointers are locals.
            if unsafe { ex.GetTransformForStream(VIDEO, k, Some(&mut cat), &mut t) }.is_err() {
                break;
            }
            let Some(t) = t else { break };
            let kind = if cat == MFT_CATEGORY_VIDEO_DECODER {
                "decoder"
            } else if cat == MFT_CATEGORY_VIDEO_PROCESSOR {
                "processor"
            } else {
                "transform"
            };
            // SAFETY: a plain call on the transform.
            let attrs = unsafe { t.GetAttributes() }.ok();
            let mut what = vec![kind.to_string()];
            if let Some(a) = &attrs {
                if let Some(n) = get_string(a, &MFT_FRIENDLY_NAME_Attribute) {
                    what.push(format!("\"{n}\""));
                }
                if let Some(u) = get_string(a, &MFT_ENUM_HARDWARE_URL_Attribute) {
                    what.push(format!("hw {u}"));
                }
                if get_u32(a, &MF_TRANSFORM_ASYNC) == Some(1) {
                    what.push("async".into());
                }
                if get_u32(a, &MF_SA_D3D11_AWARE) == Some(1) {
                    what.push("D3D11-aware".into());
                }
            }
            parts.push(what.join(" "));
        }
        if parts.is_empty() {
            "no transforms".into()
        } else {
            parts.join(" -> ")
        }
    }

    /// Where a sample's first buffer lives, as text.
    fn buffer_place(s: &IMFSample) -> String {
        // SAFETY: plain calls on the sample and its buffer; the texture
        // reference GetResource hands back is taken over by `from_raw`.
        unsafe {
            let Ok(b) = s.GetBufferByIndex(0) else {
                return "no buffer".into();
            };
            let n = s.GetBufferCount().unwrap_or(0);
            match b.cast::<IMFDXGIBuffer>() {
                Ok(dx) => {
                    let mut p = std::ptr::null_mut();
                    if dx.GetResource(&ID3D11Texture2D::IID, &mut p).is_err() || p.is_null() {
                        return "DXGI buffer, no texture".into();
                    }
                    let tex = ID3D11Texture2D::from_raw(p);
                    let mut d = D3D11_TEXTURE2D_DESC::default();
                    tex.GetDesc(&mut d);
                    let sub = dx.GetSubresourceIndex().unwrap_or(0);
                    format!(
                        "GPU texture {} {}x{} array {} (index {sub}), bind {:#X}, {n} buffer(s)",
                        dxgi_name(d.Format),
                        d.Width,
                        d.Height,
                        d.ArraySize,
                        d.BindFlags
                    )
                }
                Err(_) => format!("system memory, {n} buffer(s)"),
            }
        }
    }

    fn dxgi_name(f: DXGI_FORMAT) -> String {
        if f == DXGI_FORMAT_NV12 {
            "NV12".into()
        } else if f == DXGI_FORMAT_P010 {
            "P010".into()
        } else {
            format!("DXGI_FORMAT {}", f.0)
        }
    }

    /// The Y plane of a frame, `w`x`h`, as 8-bit (P010 shifted down), read
    /// through IMF2DBuffer when the buffer has it, else with `stride`.
    fn y_plane(s: &IMFSample, w: u32, h: u32, p010: bool, stride: u32) -> WinResult<Vec<u8>> {
        let px = if p010 { 2 } else { 1 };
        // SAFETY: the buffer is locked while rows within its pitch are read.
        unsafe {
            let b = s.GetBufferByIndex(0)?;
            let mut out = Vec::with_capacity((w * h) as usize);
            let mut read = |p: *const u8, pitch: isize| {
                for y in 0..h as isize {
                    let row = p.offset(y * pitch);
                    for x in 0..w as usize {
                        let v = if p010 {
                            u16::from_le_bytes([*row.add(x * 2), *row.add(x * 2 + 1)]) >> 8
                        } else {
                            *row.add(x) as u16
                        };
                        out.push(v as u8);
                    }
                }
            };
            if let Ok(b2) = b.cast::<IMF2DBuffer>() {
                let mut p = std::ptr::null_mut();
                let mut pitch = 0i32;
                b2.Lock2D(&mut p, &mut pitch)?;
                read(p, pitch as isize);
                let _ = b2.Unlock2D();
            } else {
                let mut p = std::ptr::null_mut();
                b.Lock(&mut p, None, None)?;
                let pitch = if stride > 0 { stride } else { w * px };
                read(p, pitch as isize);
                let _ = b.Unlock();
            }
            Ok(out)
        }
    }

    fn mean(v: &[u8]) -> f64 {
        if v.is_empty() {
            return 0.0;
        }
        v.iter().map(|&x| x as f64).sum::<f64>() / v.len() as f64
    }

    fn decoded_line(path: &Path, d3d: Option<&D3d>, subtype: GUID) {
        let place = match d3d {
            Some(d) => format!("D3D11 on {} + hardware transforms", short(&d.name)),
            None => "no D3D".into(),
        };
        let label = format!("    {place}, {}:", subtype_name(&subtype));
        let started = Instant::now();
        let reader = match decode_reader(path, d3d, &subtype) {
            Ok(r) => r,
            Err(e) => {
                println!("{label} refused, {}", hr(&e));
                return;
            }
        };
        let (mut w, mut h, mut stride, mut cur_sub, info) = current_video(&reader);
        let mut changed_to = Vec::new();
        let mut frames = 0u32;
        let mut first: Option<(i64, String, f64)> = None;
        let mut last_pts = 0i64;
        let mut changes = 0u32;
        let mut stop = None;
        for _ in 0..5000 {
            let mut flags = 0u32;
            let mut sample = None;
            // SAFETY: a synchronous read; the out pointers are locals.
            let r = unsafe {
                reader.ReadSample(VIDEO, 0, None, Some(&mut flags), None, Some(&mut sample))
            };
            if let Err(e) = r {
                stop = Some(hr(&e));
                break;
            }
            if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
                stop = Some("MF_SOURCE_READERF_ERROR".into());
                break;
            }
            if flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32 != 0 {
                changes += 1;
                let (nw, nh, ns, nsub, ninfo) = current_video(&reader);
                (w, h, stride, cur_sub) = (nw, nh, ns, nsub);
                changed_to.push(ninfo);
            }
            if let Some(s) = sample {
                frames += 1;
                // SAFETY: a plain read from the sample.
                let pts = unsafe { s.GetSampleTime() }.unwrap_or(0);
                last_pts = pts;
                if first.is_none() {
                    let p010 = cur_sub == MFVideoFormat_P010;
                    let y = y_plane(&s, w, h, p010, stride)
                        .map(|y| mean(&y))
                        .unwrap_or(-1.0);
                    first = Some((pts, buffer_place(&s), y));
                }
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
        }
        let ms = started.elapsed().as_millis();
        let chain = chain(&reader);
        println!("{label} {info}");
        match first {
            Some((pts, place, y)) => println!(
                "        {frames} frames, pts {}..{} s, first frame in {place}, Y mean {y:.1} (8-bit scale){}{}, {ms} ms",
                secs(pts),
                secs(last_pts),
                if changes > 0 { format!(", {changes} type changes") } else { String::new() },
                stop.as_deref().map(|s| format!(", stopped: {s}")).unwrap_or_default(),
            ),
            None => println!(
                "        no frames{}",
                stop.as_deref().map(|s| format!(", stopped: {s}")).unwrap_or_default()
            ),
        }
        for c in changed_to {
            println!("        type changed mid-stream to: {c}");
        }
        println!("        chain: {chain}");
    }

    /// The reader's current video type: size, stride, subtype, and a line.
    fn current_video(reader: &IMFSourceReader) -> (u32, u32, u32, GUID, String) {
        // SAFETY: a plain call on the reader.
        match unsafe { reader.GetCurrentMediaType(VIDEO) } {
            Ok(t) => {
                let (w, h) = get_u64(&t, &MF_MT_FRAME_SIZE).map(hi_lo).unwrap_or((0, 0));
                let stride = get_u32(&t, &MF_MT_DEFAULT_STRIDE).unwrap_or(0);
                let sub = get_guid(&t, &MF_MT_SUBTYPE).unwrap_or_default();
                let mut info = format!(
                    "{} {w}x{h}, stride {}",
                    subtype_name(&sub),
                    if stride == 0 {
                        "not set".to_string()
                    } else {
                        stride.to_string()
                    }
                );
                if let Some(a) =
                    get_blob(&t, &MF_MT_MINIMUM_DISPLAY_APERTURE).and_then(|b| area(&b))
                {
                    info += &format!(", display aperture {a}");
                }
                if let Some(a) = get_blob(&t, &MF_MT_GEOMETRIC_APERTURE).and_then(|b| area(&b)) {
                    info += &format!(", geometric aperture {a}");
                }
                if let Some(r) = get_u32(&t, &MF_MT_VIDEO_ROTATION) {
                    info += &format!(", rotation {r}");
                }
                info += &format!(", {}", colour(&t));
                (w, h, stride, sub, info)
            }
            Err(e) => (
                0,
                0,
                0,
                GUID::zeroed(),
                format!("no current type: {}", hr(&e)),
            ),
        }
    }

    // -----------------------------------------------------------------------
    // Encoders

    fn hw_flags() -> MFT_ENUM_FLAG {
        MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_ASYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER
    }

    fn sw_flags() -> MFT_ENUM_FLAG {
        MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_LOCALMFT | MFT_ENUM_FLAG_SORTANDFILTER
    }

    /// How a list of encoders is made.
    #[derive(Clone, Copy)]
    enum Listing {
        /// MFTEnumEx with HARDWARE|ASYNCMFT|SORTANDFILTER.
        Hardware,
        /// MFTEnum2 with the same flags, for one graphics card (its LUID).
        Card(u64),
        /// MFTEnumEx with SYNCMFT|LOCALMFT|SORTANDFILTER.
        Software,
    }

    fn encoder_list(codec: GUID, listing: Listing) -> Vec<IMFActivate> {
        let (flags, adapter) = match listing {
            Listing::Hardware => (hw_flags(), None),
            Listing::Card(l) => (hw_flags(), Some(l)),
            Listing::Software => (sw_flags(), None),
        };
        enum_transforms(
            MFT_CATEGORY_VIDEO_ENCODER,
            flags,
            None,
            Some((MFMediaType_Video, codec)),
            adapter,
        )
    }

    fn luid_of(a: &IMFAttributes) -> Option<u64> {
        get_u64(a, &MFT_ENUM_ADAPTER_LUID).or_else(|| {
            get_blob(a, &MFT_ENUM_ADAPTER_LUID)
                .filter(|b| b.len() >= 8)
                .map(|b| {
                    let lo = u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as u64;
                    let hi = u32::from_le_bytes([b[4], b[5], b[6], b[7]]) as u64;
                    (hi << 32) | lo
                })
        })
    }

    fn var_str(v: &VARIANT) -> String {
        if let Ok(x) = u32::try_from(v) {
            x.to_string()
        } else if let Ok(x) = i32::try_from(v) {
            x.to_string()
        } else if let Ok(x) = u64::try_from(v) {
            x.to_string()
        } else {
            format!("{v:?}")
        }
    }

    fn rc_name(v: u32) -> &'static str {
        match v {
            0 => "CBR",
            1 => "PeakConstrainedVBR",
            2 => "UnconstrainedVBR",
            3 => "Quality",
            4 => "LowDelayVBR",
            5 => "GlobalVBR",
            6 => "GlobalLowDelayVBR",
            _ => "?",
        }
    }

    /// The ICodecAPI properties an encoder may offer for its coding
    /// efficiency, asked of each encoder alongside its rate control.
    const TUNING_KNOBS: [(&GUID, &str); 16] = [
        (&CODECAPI_AVEncMPVDefaultBPictureCount, "BPictureCount"),
        (&CODECAPI_AVEncMPVGOPSize, "GOPSize"),
        (&CODECAPI_AVEncH264CABACEnable, "H264CABACEnable"),
        (&CODECAPI_AVEncVideoMaxNumRefFrame, "MaxNumRefFrame"),
        (&CODECAPI_AVEncAdaptiveMode, "AdaptiveMode"),
        (&CODECAPI_AVEncVideoContentType, "VideoContentType"),
        (&CODECAPI_AVEncMPVSceneDetection, "SceneDetection"),
        (&CODECAPI_AVEncCommonMultipassMode, "MultipassMode"),
        (&CODECAPI_AVEncCommonLowLatency, "CommonLowLatency"),
        (&CODECAPI_AVLowLatencyMode, "LowLatencyMode"),
        (&CODECAPI_AVEncVideoMinQP, "VideoMinQP"),
        (&CODECAPI_AVEncVideoMaxQP, "VideoMaxQP"),
        (&CODECAPI_AVEncNumWorkerThreads, "NumWorkerThreads"),
        (&CODECAPI_AVEncMPVProfile, "MPVProfile"),
        (&CODECAPI_AVEncMPVLevel, "MPVLevel"),
        (&CODECAPI_AVEncVideoLTRBufferControl, "LTRBufferControl"),
    ];

    /// What ICodecAPI says about one property, before any type is set.
    fn api_facts(api: &ICodecAPI, key: &GUID, name: &str, enum_values: bool) -> String {
        // SAFETY: plain ICodecAPI calls; the value array is CoTaskMem whose
        // VARIANTs are moved out (and so cleared) before it is freed.
        unsafe {
            if let Err(e) = api.IsSupported(key) {
                return format!("{name}: not supported ({})", hr(&e));
            }
            let m = api.IsModifiable(key);
            let modifiable = if m == windows::Win32::Foundation::S_OK {
                "modifiable (S_OK)".to_string()
            } else if m == windows::Win32::Foundation::S_FALSE {
                "NOT modifiable (S_FALSE)".to_string()
            } else {
                format!("IsModifiable says {}", hr(&windows::core::Error::from(m)))
            };
            let mut s = format!("{name}: supported, {modifiable}");
            if let Ok(v) = api.GetDefaultValue(key) {
                s += &format!(", default {}", var_str(&v));
            }
            if let Ok(v) = api.GetValue(key) {
                s += &format!(", current {}", var_str(&v));
            }
            if enum_values {
                let mut values: *mut VARIANT = std::ptr::null_mut();
                let mut count = 0u32;
                let r = api.GetParameterValues(key, &mut values, &mut count);
                if r.is_ok() && !values.is_null() {
                    let list: Vec<String> = (0..count as usize)
                        .map(|i| {
                            let v = std::ptr::read(values.add(i));
                            let n = u32::try_from(&v).unwrap_or(u32::MAX);
                            format!("{n} {}", rc_name(n))
                        })
                        .collect();
                    CoTaskMemFree(Some(values as *const _));
                    s += &format!(", values [{}]", list.join(", "));
                } else {
                    s += &format!(
                        ", no value list ({})",
                        r.err().map_or("empty".into(), |e| hr(&e))
                    );
                }
            } else {
                let (mut lo, mut hi, mut step) =
                    (VARIANT::default(), VARIANT::default(), VARIANT::default());
                match api.GetParameterRange(key, &mut lo, &mut hi, &mut step) {
                    Ok(()) => {
                        s += &format!(
                            ", range {}..{} step {}",
                            var_str(&lo),
                            var_str(&hi),
                            var_str(&step)
                        )
                    }
                    Err(e) => s += &format!(", no range ({})", hr(&e)),
                }
            }
            s
        }
    }

    #[derive(Clone, Copy)]
    enum Rc {
        /// eAVEncCommonRateControlMode_Quality with CODECAPI_AVEncCommonQuality.
        Quality(u32),
        /// eAVEncCommonRateControlMode_UnconstrainedVBR with a mean bitrate.
        Uvbr(u32),
        /// Constant QP: Quality mode with CODECAPI_AVEncVideoEncodeQP.
        Qp(u32),
        /// Whatever the encoder starts with.
        Default,
    }

    impl Rc {
        fn label(self) -> String {
            match self {
                Rc::Quality(q) => format!("Quality {q}"),
                Rc::Uvbr(k) => format!("UnconstrainedVBR {k} kbps"),
                Rc::Qp(q) => format!("constant QP {q}"),
                Rc::Default => "default rate control".into(),
            }
        }

        fn sets(self) -> Vec<(GUID, &'static str, VARIANT)> {
            let mode = |m: eAVEncCommonRateControlMode| VARIANT::from(m.0 as u32);
            match self {
                Rc::Quality(q) => vec![
                    (
                        CODECAPI_AVEncCommonRateControlMode,
                        "mode",
                        mode(eAVEncCommonRateControlMode_Quality),
                    ),
                    (CODECAPI_AVEncCommonQuality, "quality", VARIANT::from(q)),
                ],
                Rc::Uvbr(k) => vec![
                    (
                        CODECAPI_AVEncCommonRateControlMode,
                        "mode",
                        mode(eAVEncCommonRateControlMode_UnconstrainedVBR),
                    ),
                    (
                        CODECAPI_AVEncCommonMeanBitRate,
                        "bitrate",
                        VARIANT::from(k * 1000),
                    ),
                ],
                Rc::Qp(q) => vec![
                    (
                        CODECAPI_AVEncCommonRateControlMode,
                        "mode",
                        mode(eAVEncCommonRateControlMode_Quality),
                    ),
                    (CODECAPI_AVEncVideoEncodeQP, "QP", VARIANT::from(q as u64)),
                ],
                Rc::Default => Vec::new(),
            }
        }
    }

    /// Where the frames for an encode come from.
    enum Frames {
        /// Made here, in system memory: a moving pattern with noise.
        Memory { n: u32, total: u32 },
        /// Decoded on the graphics card by a reader.
        Gpu {
            reader: IMFSourceReader,
            n: u32,
            total: u32,
        },
    }

    const W: u32 = 320;
    const H: u32 = 240;
    const FRAME_HNS: i64 = 333_333;
    /// Frames per test encode: two seconds.
    const FRAMES: u32 = 60;

    impl Frames {
        fn next(&mut self) -> Option<IMFSample> {
            match self {
                Frames::Memory { n, total } => {
                    if *n >= *total {
                        return None;
                    }
                    let s = memory_sample(&nv12_frame(W, H, *n), *n as i64 * FRAME_HNS).ok();
                    *n += 1;
                    s
                }
                Frames::Gpu { reader, n, total } => {
                    if *n >= *total {
                        return None;
                    }
                    loop {
                        let mut flags = 0u32;
                        let mut s = None;
                        // SAFETY: a synchronous read; the out pointers are locals.
                        let r = unsafe {
                            reader.ReadSample(VIDEO, 0, None, Some(&mut flags), None, Some(&mut s))
                        };
                        if r.is_err() || flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
                            return None;
                        }
                        if let Some(s) = s {
                            *n += 1;
                            return Some(s);
                        }
                        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                            return None;
                        }
                    }
                }
            }
        }
    }

    /// An NV12 frame: a gradient that moves with `n`, plus fixed noise, so
    /// an encoder's output size follows its quality setting.
    fn nv12_frame(w: u32, h: u32, n: u32) -> Vec<u8> {
        let mut v = Vec::with_capacity((w * h * 3 / 2) as usize);
        let mut seed = 0x1234_5678u32;
        let mut noise = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed % 41) as i32 - 20
        };
        for y in 0..h {
            for x in 0..w {
                let base = ((x + 3 * n) % 220 + y / 4) as i32;
                v.push((base + 16 + noise()).clamp(16, 235) as u8);
            }
        }
        for y in 0..h / 2 {
            for x in 0..w / 2 {
                v.push((128 + ((x + n) % 64) as i32 - 32) as u8);
                v.push((128 + ((y + n) % 64) as i32 - 32) as u8);
            }
        }
        v
    }

    fn memory_sample(bytes: &[u8], time: i64) -> WinResult<IMFSample> {
        // SAFETY: the buffer is locked only while it is filled.
        unsafe {
            let buf = MFCreateMemoryBuffer(bytes.len() as u32)?;
            let mut p = std::ptr::null_mut();
            buf.Lock(&mut p, None, None)?;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
            buf.Unlock()?;
            buf.SetCurrentLength(bytes.len() as u32)?;
            let s = MFCreateSample()?;
            s.AddBuffer(&buf)?;
            s.SetSampleTime(time)?;
            s.SetSampleDuration(FRAME_HNS)?;
            Ok(s)
        }
    }

    #[derive(Default)]
    struct Encoded {
        log: Vec<String>,
        frames_in: u32,
        packets: u32,
        bytes: u64,
        keys: u32,
        first: Option<Vec<u8>>,
        seq_header: Option<usize>,
        error: Option<String>,
    }

    /// Takes every packet the transform has ready (one, for an asynchronous
    /// transform, which signals each).
    fn pull(t: &IMFTransform, out: &mut Encoded) -> WinResult<()> {
        // SAFETY: plain transform calls; the output buffer array has one
        // entry, and the sample and events it holds are taken back.
        unsafe {
            let info = t.GetOutputStreamInfo(0)?;
            let provides = info.dwFlags
                & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32
                    | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0 as u32)
                != 0;
            for _ in 0..1000 {
                let sample = if provides {
                    None
                } else {
                    let s = MFCreateSample()?;
                    let b = MFCreateMemoryBuffer(info.cbSize.max(1 << 20))?;
                    s.AddBuffer(&b)?;
                    Some(s)
                };
                let mut db = [MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: std::mem::ManuallyDrop::new(sample),
                    dwStatus: 0,
                    pEvents: std::mem::ManuallyDrop::new(None),
                }];
                let mut status = 0u32;
                let r = t.ProcessOutput(0, &mut db, &mut status);
                let sample = std::mem::ManuallyDrop::take(&mut db[0].pSample);
                drop(std::mem::ManuallyDrop::take(&mut db[0].pEvents));
                match r {
                    Ok(()) => {
                        if let Some(s) = sample {
                            let len = s.GetTotalLength().unwrap_or(0);
                            if len > 0 {
                                out.packets += 1;
                                out.bytes += len as u64;
                                out.keys +=
                                    (get_u32(&s, &MFSampleExtension_CleanPoint) == Some(1)) as u32;
                                if out.first.is_none() {
                                    out.first = sample_bytes(&s, 16);
                                }
                            }
                        }
                    }
                    Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(()),
                    Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                        let nt = t.GetOutputAvailableType(0, 0)?;
                        t.SetOutputType(0, &nt, 0)?;
                        out.log.push("output stream change, new type set".into());
                    }
                    Err(e) => return Err(e),
                }
                if provides && t.cast::<IMFMediaEventGenerator>().is_ok() {
                    // An asynchronous transform gives one output per event.
                    return Ok(());
                }
            }
            Ok(())
        }
    }

    fn run_sync(t: &IMFTransform, frames: &mut Frames, out: &mut Encoded) -> WinResult<()> {
        // SAFETY: plain transform calls with samples made or read here.
        unsafe {
            t.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            t.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
            while let Some(s) = frames.next() {
                loop {
                    match t.ProcessInput(0, &s, 0) {
                        Ok(()) => break,
                        Err(e) if e.code() == MF_E_NOTACCEPTING => pull(t, out)?,
                        Err(e) => return Err(e),
                    }
                }
                out.frames_in += 1;
                pull(t, out)?;
            }
            t.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0)?;
            t.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0)?;
            pull(t, out)?;
            let _ = t.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
        }
        Ok(())
    }

    fn run_async(t: &IMFTransform, frames: &mut Frames, out: &mut Encoded) -> WinResult<()> {
        let events: IMFMediaEventGenerator = t.cast()?;
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut draining = false;
        // SAFETY: plain transform and event calls with samples made or read here.
        unsafe {
            t.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            t.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
            loop {
                if Instant::now() > deadline {
                    out.log
                        .push("timed out after 15 s waiting for events".into());
                    break;
                }
                let ev = match events.GetEvent(MF_EVENT_FLAG_NO_WAIT) {
                    Ok(e) => e,
                    Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => {
                        std::thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(e) => return Err(e),
                };
                let kind = ev.GetType()?;
                if kind == METransformNeedInput.0 as u32 {
                    if draining {
                        continue;
                    }
                    match frames.next() {
                        Some(s) => {
                            t.ProcessInput(0, &s, 0)?;
                            out.frames_in += 1;
                        }
                        None => {
                            t.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0)?;
                            t.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0)?;
                            draining = true;
                        }
                    }
                } else if kind == METransformHaveOutput.0 as u32 {
                    pull(t, out)?;
                } else if kind == METransformDrainComplete.0 as u32 {
                    break;
                } else if kind == MEError.0 as u32 {
                    let status = ev.GetStatus().unwrap_or_default();
                    return Err(windows::core::Error::from(status));
                }
            }
            let _ = t.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
        }
        Ok(())
    }

    /// The input type the encoder offers in `subtype`, sized, or one made here.
    fn encoder_input(t: &IMFTransform, subtype: &GUID) -> WinResult<IMFMediaType> {
        for i in 0..64 {
            // SAFETY: a plain call on the transform.
            let Ok(a) = (unsafe { t.GetInputAvailableType(0, i) }) else {
                break;
            };
            if get_guid(&a, &MF_MT_SUBTYPE) == Some(*subtype) {
                // SAFETY: plain attribute calls on the offered type.
                unsafe {
                    a.SetUINT64(&MF_MT_FRAME_SIZE, ((W as u64) << 32) | H as u64)?;
                    a.SetUINT64(&MF_MT_FRAME_RATE, (30u64 << 32) | 1)?;
                    a.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
                    a.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
                }
                return Ok(a);
            }
        }
        video_type(subtype, W, H)
    }

    fn set_values(
        api: &ICodecAPI,
        rc: Rc,
        when: &str,
        only_failed: &[&str],
        log: &mut Vec<String>,
    ) -> Vec<&'static str> {
        let mut failed = Vec::new();
        for (key, name, value) in rc.sets() {
            if !only_failed.is_empty() && !only_failed.contains(&name) {
                continue;
            }
            // SAFETY: a plain ICodecAPI call with a VARIANT that outlives it.
            match unsafe { api.SetValue(&key, &value) } {
                Ok(()) => log.push(format!("set {name} {when}: ok")),
                Err(e) => {
                    log.push(format!("set {name} {when}: {}", hr(&e)));
                    failed.push(name);
                }
            }
        }
        failed
    }

    fn read_back(api: &ICodecAPI, rc: Rc) -> String {
        let get = |k: &GUID| {
            // SAFETY: a plain ICodecAPI call.
            unsafe { api.GetValue(k) }
                .map(|v| var_str(&v))
                .unwrap_or_else(|e| hr(&e))
        };
        let mode = get(&CODECAPI_AVEncCommonRateControlMode);
        let mode = mode
            .parse::<u32>()
            .map(|m| format!("{m} {}", rc_name(m)))
            .unwrap_or(mode);
        match rc {
            Rc::Quality(_) => format!(
                "reads back mode {mode}, quality {}",
                get(&CODECAPI_AVEncCommonQuality)
            ),
            Rc::Uvbr(_) => format!(
                "reads back mode {mode}, bitrate {}",
                get(&CODECAPI_AVEncCommonMeanBitRate)
            ),
            Rc::Qp(_) => format!(
                "reads back mode {mode}, QP {}",
                get(&CODECAPI_AVEncVideoEncodeQP)
            ),
            Rc::Default => format!("reads back mode {mode}"),
        }
    }

    /// One encode of [`FRAMES`] frames with `rc`, from memory or from the graphics card.
    ///
    /// `avg_bitrate` is put on the output type as `MF_MT_AVG_BITRATE`, or left
    /// off when None.
    fn encode(
        act: &IMFActivate,
        codec: &GUID,
        rc: Rc,
        gpu: Option<(&D3d, &Path)>,
        avg_bitrate: Option<u32>,
    ) -> Encoded {
        let mut out = Encoded::default();
        let r = (|| -> WinResult<()> {
            // SAFETY: plain activation and transform calls.
            unsafe {
                let t: IMFTransform = act.ActivateObject()?;
                let attrs = t.GetAttributes().ok();
                let is_async =
                    attrs.as_ref().and_then(|a| get_u32(a, &MF_TRANSFORM_ASYNC)) == Some(1);
                if is_async {
                    if let Some(a) = &attrs {
                        a.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)?;
                    }
                }
                let mut frames = match gpu {
                    Some((d, path)) => {
                        match t.ProcessMessage(
                            MFT_MESSAGE_SET_D3D_MANAGER,
                            d.manager.as_raw() as usize,
                        ) {
                            Ok(()) => out
                                .log
                                .push(format!("D3D manager ({}) taken", short(&d.name))),
                            Err(e) => {
                                out.log.push(format!("D3D manager refused: {}", hr(&e)));
                                let _ = act.ShutdownObject();
                                return Err(e);
                            }
                        }
                        Frames::Gpu {
                            reader: decode_reader(path, Some(d), &MFVideoFormat_NV12)?,
                            n: 0,
                            total: FRAMES,
                        }
                    }
                    None => Frames::Memory {
                        n: 0,
                        total: FRAMES,
                    },
                };
                let api = t.cast::<ICodecAPI>().ok();
                let mut failed = Vec::new();
                if let Some(api) = &api {
                    failed = set_values(api, rc, "before types", &[], &mut out.log);
                } else if !matches!(rc, Rc::Default) {
                    out.log.push("no ICodecAPI".into());
                }
                let ot = video_type(codec, W, H)?;
                if let Some(b) = avg_bitrate {
                    ot.SetUINT32(&MF_MT_AVG_BITRATE, b)?;
                }
                if let Err(e) = t.SetOutputType(0, &ot, 0) {
                    out.log.push(format!("output type refused: {}", hr(&e)));
                    let _ = act.ShutdownObject();
                    return Err(e);
                }
                let it = encoder_input(&t, &MFVideoFormat_NV12)?;
                if let Err(e) = t.SetInputType(0, &it, 0) {
                    out.log.push(format!("NV12 input type refused: {}", hr(&e)));
                    let _ = act.ShutdownObject();
                    return Err(e);
                }
                if let Some(api) = &api {
                    if !failed.is_empty() {
                        set_values(api, rc, "after types", &failed, &mut out.log);
                    }
                    if !matches!(rc, Rc::Default) {
                        out.log.push(read_back(api, rc));
                    }
                }
                let ran = if is_async {
                    run_async(&t, &mut frames, &mut out)
                } else {
                    run_sync(&t, &mut frames, &mut out)
                };
                if let Ok(cur) = t.GetOutputCurrentType(0) {
                    out.seq_header = get_blob(&cur, &MF_MT_MPEG_SEQUENCE_HEADER).map(|b| b.len());
                }
                drop(frames);
                let _ = act.ShutdownObject();
                ran
            }
        })();
        if let Err(e) = r {
            out.error = Some(hr(&e));
        }
        out
    }

    fn encoded_line(label: &str, e: &Encoded) {
        let head = e.first.as_deref().map(|b| {
            if b.starts_with(&[0, 0, 0, 1]) || b.starts_with(&[0, 0, 1]) {
                "Annex B"
            } else {
                "not Annex B"
            }
        });
        let result = match &e.error {
            Some(err) => format!("FAILED {err}"),
            None => format!(
                "{} frames in, {} packets, {} B ({} kbps), {} key{}{}",
                e.frames_in,
                e.packets,
                e.bytes,
                (e.bytes * 8 * 30) / (e.frames_in.max(1) as u64 * 1000),
                e.keys,
                head.map(|h| format!(", packets {h}")).unwrap_or_default(),
                e.seq_header
                    .map(|n| format!(", output type has a {n} B sequence header"))
                    .unwrap_or_else(|| ", no sequence header on the output type".into()),
            ),
        };
        println!("      {label}: {result}");
        if !e.log.is_empty() {
            println!("        {}", e.log.join("; "));
        }
    }

    fn encoder_report(
        codec: GUID,
        listing: Listing,
        index: usize,
        act: &IMFActivate,
        devices: &[&D3d],
        gpu_source: Option<&Path>,
    ) {
        let name = activate_name(act);
        let luid = luid_of(act);
        let vendor = get_string(act, &MFT_ENUM_HARDWARE_VENDOR_ID_Attribute);
        let luid = luid.or(match listing {
            Listing::Card(l) => Some(l),
            _ => None,
        });
        let vendor_id = vendor
            .as_deref()
            .and_then(|v| v.strip_prefix("VEN_"))
            .and_then(|v| u32::from_str_radix(v, 16).ok());
        // The encoder's own card: by LUID when known, else by vendor.
        let device = luid
            .and_then(|l| devices.iter().copied().find(|d| d.luid == l))
            .or_else(|| vendor_id.and_then(|v| devices.iter().copied().find(|d| d.vendor == v)));
        println!(
            "  [{index}] {name}{}{}{}",
            vendor.map(|v| format!(", vendor {v}")).unwrap_or_default(),
            luid.map(|l| format!(", adapter LUID {l:016X}"))
                .unwrap_or_default(),
            device.map(|d| format!(" ({})", d.name)).unwrap_or_default()
        );
        // A fresh activation from a fresh list for each run, so one run's
        // state never leaks into the next.
        let fresh = || encoder_list(codec, listing).into_iter().nth(index);
        // The facts, from one activation.
        let started = Instant::now();
        let facts = (|| -> WinResult<Vec<String>> {
            let act = fresh().ok_or_else(fail)?;
            // SAFETY: plain activation and attribute calls.
            unsafe {
                let t: IMFTransform = act.ActivateObject()?;
                let mut lines = vec![format!("activated in {} ms", started.elapsed().as_millis())];
                match t.GetAttributes() {
                    Ok(a) => {
                        let is_async = get_u32(&a, &MF_TRANSFORM_ASYNC) == Some(1);
                        if is_async {
                            a.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)?;
                        }
                        lines.push(format!(
                            "async {}, MF_SA_D3D11_AWARE {}, MF_SA_D3D_AWARE {}",
                            if is_async { "yes" } else { "no" },
                            get_u32(&a, &MF_SA_D3D11_AWARE)
                                .map_or("not set".into(), |v| v.to_string()),
                            get_u32(&a, &MF_SA_D3D_AWARE)
                                .map_or("not set".into(), |v| v.to_string()),
                        ));
                    }
                    Err(e) => lines.push(format!("no attributes: {}", hr(&e))),
                }
                match t.cast::<ICodecAPI>() {
                    Ok(api) => {
                        lines.push(api_facts(
                            &api,
                            &CODECAPI_AVEncCommonRateControlMode,
                            "RateControlMode",
                            true,
                        ));
                        lines.push(api_facts(
                            &api,
                            &CODECAPI_AVEncCommonQuality,
                            "CommonQuality",
                            false,
                        ));
                        lines.push(api_facts(
                            &api,
                            &CODECAPI_AVEncVideoEncodeQP,
                            "VideoEncodeQP",
                            false,
                        ));
                        lines.push(api_facts(
                            &api,
                            &CODECAPI_AVEncCommonMeanBitRate,
                            "MeanBitRate",
                            false,
                        ));
                        lines.push(api_facts(
                            &api,
                            &CODECAPI_AVEncCommonQualityVsSpeed,
                            "QualityVsSpeed",
                            false,
                        ));
                        // The knobs that trade speed for quality (R85).
                        for (key, name) in TUNING_KNOBS {
                            lines.push(api_facts(&api, key, name, false));
                        }
                        for (m, n) in [
                            (eAVEncCommonRateControlMode_Quality, "Quality"),
                            (
                                eAVEncCommonRateControlMode_UnconstrainedVBR,
                                "UnconstrainedVBR",
                            ),
                            (eAVEncCommonRateControlMode_CBR, "CBR"),
                        ] {
                            let v = VARIANT::from(m.0 as u32);
                            let r = api.SetValue(&CODECAPI_AVEncCommonRateControlMode, &v);
                            lines.push(format!(
                                "SetValue(RateControlMode = {n}) before types: {}",
                                r.map_or_else(|e| hr(&e), |_| "ok".into())
                            ));
                        }
                    }
                    Err(e) => lines.push(format!("no ICodecAPI: {}", hr(&e))),
                }
                let _ = act.ShutdownObject();
                Ok(lines)
            }
        })();
        match facts {
            Ok(lines) => {
                for l in lines {
                    println!("      {l}");
                }
            }
            Err(e) => {
                println!("      won't activate: {}", hr(&e));
                return;
            }
        }
        // Encodes from memory, two settings each so a setting that is taken
        // but ignored shows as two equal sizes. The bitrate on the output
        // type is the mode's own for the default (CBR) and UnconstrainedVBR.
        for (rc, avg) in [
            (Rc::Default, 1_000_000),
            (Rc::Uvbr(200), 200_000),
            (Rc::Uvbr(2000), 2_000_000),
            (Rc::Qp(40), 1_000_000),
        ] {
            let Some(act) = fresh() else { continue };
            let e = encode(&act, &codec, rc, None, Some(avg));
            encoded_line(
                &format!(
                    "memory NV12, {}, MF_MT_AVG_BITRATE {}",
                    rc.label(),
                    bps(Some(avg))
                ),
                &e,
            );
        }
        // Quality and constant QP against the output type's MF_MT_AVG_BITRATE:
        // 1 Mbps, 20 Mbps and none at all, to see whether it caps them. QP 12
        // asks for far more than 1 Mbps.
        for avg in [Some(1_000_000), Some(20_000_000), None] {
            for rc in [
                Rc::Quality(30),
                Rc::Quality(65),
                Rc::Quality(90),
                Rc::Qp(20),
                Rc::Qp(12),
            ] {
                let Some(act) = fresh() else { continue };
                let e = encode(&act, &codec, rc, None, avg);
                encoded_line(
                    &format!(
                        "memory NV12, {}, MF_MT_AVG_BITRATE {}",
                        rc.label(),
                        bps(avg)
                    ),
                    &e,
                );
            }
        }
        // From the graphics card: frames decoded on the encoder's own card
        // (or, for a software encoder, on each card).
        let Some(src) = gpu_source else {
            println!("      (no h264.mp4 to decode for the D3D input test)");
            return;
        };
        let cards: Vec<&D3d> = match device {
            Some(d) => vec![d],
            None => devices.to_vec(),
        };
        for d in cards {
            for rc in [Rc::Default, Rc::Qp(26)] {
                let Some(act) = fresh() else { continue };
                let e = encode(&act, &codec, rc, Some((d, src)), Some(1_000_000));
                encoded_line(
                    &format!(
                        "D3D11 NV12 decoded on {}, {}, MF_MT_AVG_BITRATE 1 Mbps",
                        short(&d.name),
                        rc.label()
                    ),
                    &e,
                );
            }
        }
    }

    /// An MF_MT_AVG_BITRATE for a label.
    fn bps(b: Option<u32>) -> String {
        match b {
            Some(b) if b % 1_000_000 == 0 => format!("{} Mbps", b / 1_000_000),
            Some(b) => format!("{} kbps", b / 1000),
            None => "unset".into(),
        }
    }

    /// Activates every AMD H.264 and HEVC encoder in a process that has made
    /// no D3D11 device ("clean") or only the AMD card's ("amd-device"), and
    /// encodes from memory with each one that activates.
    fn amd_activation(mode: &str) {
        let cards = dxgi_adapters();
        let Some((adapter, name, vendor, luid)) = cards.into_iter().find(|c| c.2 == 0x1002) else {
            println!("  no AMD card listed");
            return;
        };
        let _device = match mode {
            "clean" => {
                println!("  {mode}: this process has made no D3D11 device");
                None
            }
            "amd-device" => match device_on(&adapter, &name, vendor, luid) {
                Ok(d) => {
                    println!("  {mode}: this process has made only the AMD card's D3D11 device");
                    Some(d)
                }
                Err(e) => {
                    println!(
                        "  {mode}: the AMD card's D3D11 device wasn't made: {}",
                        hr(&e)
                    );
                    return;
                }
            },
            other => {
                eprintln!("bad --amd mode {other:?}: clean or amd-device");
                std::process::exit(2);
            }
        };
        for (codec, cname) in [(MFVideoFormat_H264, "H.264"), (MFVideoFormat_HEVC, "HEVC")] {
            for (listing, how) in [
                (Listing::Hardware, "MFTEnumEx"),
                (Listing::Card(luid), "MFTEnum2 with the AMD LUID"),
            ] {
                for (i, act) in encoder_list(codec, listing).iter().enumerate() {
                    if get_string(act, &MFT_ENUM_HARDWARE_VENDOR_ID_Attribute).as_deref()
                        != Some("VEN_1002")
                    {
                        continue;
                    }
                    let started = Instant::now();
                    // SAFETY: plain activation calls.
                    let r = unsafe { act.ActivateObject::<IMFTransform>() };
                    let ms = started.elapsed().as_millis();
                    match r {
                        Ok(_) => {
                            println!(
                                "    {cname} [{i}] {} via {how}: activated in {ms} ms",
                                activate_name(act)
                            );
                            // SAFETY: a plain call on the activation.
                            let _ = unsafe { act.ShutdownObject() };
                            if let Some(fresh) = encoder_list(codec, listing).into_iter().nth(i) {
                                let e = encode(&fresh, &codec, Rc::Quality(65), None, None);
                                encoded_line(
                                    "memory NV12, Quality 65, MF_MT_AVG_BITRATE unset",
                                    &e,
                                );
                            }
                        }
                        Err(e) => println!(
                            "    {cname} [{i}] {} via {how}: won't activate: {}",
                            activate_name(act),
                            hr(&e)
                        ),
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // The video processor

    fn processor(d3d: Option<&D3d>) -> WinResult<IMFTransform> {
        // SAFETY: plain COM creation and a message with a live manager.
        unsafe {
            let t: IMFTransform =
                CoCreateInstance(&CLSID_VideoProcessorMFT, None, CLSCTX_INPROC_SERVER)?;
            if let Some(d) = d3d {
                t.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, d.manager.as_raw() as usize)?;
            }
            Ok(t)
        }
    }

    /// A sample of `bytes` on the graphics card: a texture made with the
    /// bytes as its initial data.
    fn texture_sample(
        d: &D3d,
        bytes: &[u8],
        w: u32,
        h: u32,
        format: DXGI_FORMAT,
        pitch: u32,
        bind: u32,
    ) -> WinResult<IMFSample> {
        // SAFETY: the initial data points at `bytes`, which outlives the call;
        // the texture is wrapped in a buffer that keeps its own reference.
        unsafe {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: w,
                Height: h,
                MipLevels: 1,
                ArraySize: 1,
                Format: format,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: bind,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let init = D3D11_SUBRESOURCE_DATA {
                pSysMem: bytes.as_ptr() as *const _,
                SysMemPitch: pitch,
                SysMemSlicePitch: 0,
            };
            let mut tex = None;
            d.device
                .CreateTexture2D(&desc, Some(&init), Some(&mut tex))?;
            let tex = tex.ok_or_else(fail)?;
            let buf = MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, &tex, 0, false)?;
            if let Ok(b2) = buf.cast::<IMF2DBuffer>() {
                let len = b2.GetContiguousLength().unwrap_or(0);
                let _ = buf.SetCurrentLength(len);
            }
            let s = MFCreateSample()?;
            s.AddBuffer(&buf)?;
            s.SetSampleTime(0)?;
            s.SetSampleDuration(FRAME_HNS)?;
            Ok(s)
        }
    }

    /// A failed step, named.
    fn at(step: &str) -> impl Fn(windows::core::Error) -> String + '_ {
        move |e| format!("{step}: {}", hr(&e))
    }

    /// Runs one sample through the processor, whose types are set. Gives
    /// the output sample and where it came out.
    fn process_sample(t: &IMFTransform, input: &IMFSample) -> Result<(IMFSample, String), String> {
        // SAFETY: plain transform calls; the output buffer array has one
        // entry, and the sample and events it holds are taken back.
        unsafe {
            t.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .map_err(at("BEGIN_STREAMING"))?;
            t.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .map_err(at("START_OF_STREAM"))?;
            t.ProcessInput(0, input, 0).map_err(at("ProcessInput"))?;
            let info = t
                .GetOutputStreamInfo(0)
                .map_err(at("GetOutputStreamInfo"))?;
            let provides = info.dwFlags
                & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32
                    | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0 as u32)
                != 0;
            let sample = if provides {
                None
            } else {
                let s = MFCreateSample().map_err(at("MFCreateSample"))?;
                let b = MFCreateMemoryBuffer(info.cbSize.max(1 << 20))
                    .map_err(at("MFCreateMemoryBuffer"))?;
                s.AddBuffer(&b).map_err(at("AddBuffer"))?;
                Some(s)
            };
            let mut db = [MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: std::mem::ManuallyDrop::new(sample),
                dwStatus: 0,
                pEvents: std::mem::ManuallyDrop::new(None),
            }];
            let mut status = 0u32;
            let r = t.ProcessOutput(0, &mut db, &mut status);
            let sample = std::mem::ManuallyDrop::take(&mut db[0].pSample);
            drop(std::mem::ManuallyDrop::take(&mut db[0].pEvents));
            r.map_err(at("ProcessOutput"))?;
            let sample = sample.ok_or("ProcessOutput gave no sample")?;
            let how = format!(
                "{}, output in {}",
                if provides {
                    "output sample from the processor"
                } else {
                    "output sample given"
                },
                buffer_place(&sample)
            );
            Ok((sample, how))
        }
    }

    /// Runs one frame of `bytes` through a processor `make` gives, its types
    /// set: as a system-memory sample, or on a graphics card, when that is
    /// refused, as a texture made with the bytes as its initial data, trying
    /// each of three bind flags on a fresh processor until one goes through.
    fn process_one(
        make: &dyn Fn() -> WinResult<IMFTransform>,
        d3d: Option<&D3d>,
        bytes: &[u8],
        w: u32,
        h: u32,
        p010: bool,
    ) -> Result<(IMFSample, String), String> {
        let t = make().map_err(at("processor"))?;
        let s = memory_sample(bytes, 0).map_err(at("memory sample"))?;
        let e = match process_sample(&t, &s) {
            Ok((out, how)) => return Ok((out, format!("system-memory input, {how}"))),
            Err(e) => e,
        };
        let Some(d) = d3d else { return Err(e) };
        let mut notes = vec![format!("system-memory input refused ({e})")];
        let (format, pitch) = if p010 {
            (DXGI_FORMAT_P010, w * 2)
        } else {
            (DXGI_FORMAT_NV12, w)
        };
        for (bind, name) in [
            (D3D11_BIND_SHADER_RESOURCE.0 as u32, "SHADER_RESOURCE"),
            (
                (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                "RENDER_TARGET|SHADER_RESOURCE",
            ),
            (D3D11_BIND_DECODER.0 as u32, "DECODER"),
        ] {
            let t = make().map_err(at("processor"))?;
            let s = match texture_sample(d, bytes, w, h, format, pitch, bind) {
                Ok(s) => s,
                Err(e) => {
                    notes.push(format!("{name} texture not made ({})", hr(&e)));
                    continue;
                }
            };
            match process_sample(&t, &s) {
                Ok((out, how)) => {
                    notes.push(format!("{name} texture input, {how}"));
                    return Ok((out, notes.join("; ")));
                }
                Err(e) => notes.push(format!("{name} texture refused ({e})")),
            }
        }
        Err(notes.join("; "))
    }

    /// PQ's EOTF: a signal in 0..1 to nits.
    fn pq_nits(e: f64) -> f64 {
        let (m1, m2) = (0.159_301_757_812_5, 78.843_75);
        let (c1, c2, c3) = (0.835_937_5, 18.851_562_5, 18.687_5);
        let p = e.powf(1.0 / m2);
        10000.0 * ((p - c1).max(0.0) / (c2 - c3 * p)).powf(1.0 / m1)
    }

    /// A P010 frame whose luma steps across 32 columns of PQ code values,
    /// from 64 (black) to 940 (10 000 nits), with neutral chroma.
    fn pq_ramp(w: u32, h: u32) -> (Vec<u8>, Vec<u16>) {
        let steps = 32u32;
        let codes: Vec<u16> = (0..steps)
            .map(|i| (64 + i * 876 / (steps - 1)) as u16)
            .collect();
        let mut v = Vec::with_capacity((w * h * 3) as usize);
        for _ in 0..h {
            for x in 0..w {
                let c = codes[(x * steps / w) as usize];
                v.extend_from_slice(&(c << 6).to_le_bytes());
            }
        }
        for _ in 0..h / 2 {
            for _ in 0..w / 2 {
                v.extend_from_slice(&(512u16 << 6).to_le_bytes());
                v.extend_from_slice(&(512u16 << 6).to_le_bytes());
            }
        }
        (v, codes)
    }

    fn hdr_test(d3d: Option<&D3d>) -> WinResult<()> {
        let (w, h) = (W, H);
        let it = video_type(&MFVideoFormat_P010, w, h)?;
        let ot = video_type(&MFVideoFormat_NV12, w, h)?;
        // SAFETY: plain attribute calls.
        unsafe {
            it.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_2084.0 as u32)?;
            it.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT2020.0 as u32)?;
            it.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT2020_10.0 as u32)?;
            it.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
            it.SetUINT32(&MF_MT_MAX_LUMINANCE_LEVEL, 1000)?;
            it.SetUINT32(&MF_MT_MAX_FRAME_AVERAGE_LUMINANCE_LEVEL, 400)?;
            it.SetUINT32(&MF_MT_MAX_MASTERING_LUMINANCE, 1000)?;
            it.SetUINT32(&MF_MT_DEFAULT_STRIDE, w * 2)?;
            ot.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32)?;
            ot.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32)?;
            ot.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32)?;
            ot.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
            ot.SetUINT32(&MF_MT_DEFAULT_STRIDE, w)?;
        }
        let make = || -> WinResult<IMFTransform> {
            let t = processor(d3d)?;
            // SAFETY: plain transform calls with types made here.
            unsafe {
                t.SetInputType(0, &it, 0)?;
                t.SetOutputType(0, &ot, 0)?;
            }
            Ok(t)
        };
        let t = processor(d3d)?;
        // SAFETY: plain transform calls with types made here.
        unsafe {
            if let Err(e) = t.SetInputType(0, &it, 0) {
                println!("    HDR: P010 PQ/BT.2020 input type refused: {}", hr(&e));
                return Ok(());
            }
            if let Err(e) = t.SetOutputType(0, &ot, 0) {
                println!("    HDR: NV12 BT.709 output type refused: {}", hr(&e));
                return Ok(());
            }
        }
        drop(t);
        println!(
            "    HDR: P010 PQ/BT.2020 (1000-nit metadata) in, NV12 BT.709 out: types accepted"
        );
        let (bytes, codes) = pq_ramp(w, h);
        let (out, how) = match process_one(&make, d3d, &bytes, w, h, true) {
            Ok(x) => x,
            Err(e) => {
                println!("      processing failed: {e}");
                return Ok(());
            }
        };
        println!("      {how}");
        let y = y_plane(&out, w, h, false, w)?;
        let steps = codes.len() as u32;
        // Three models of what could come out at each step:
        // plain: the PQ code passed through as if it were already SDR video;
        // clip: PQ decoded to light, SDR white at 100 nits, everything above clipped;
        // the processor's own value, read from the middle row.
        let mut rows = Vec::new();
        let (mut d_plain, mut d_clip) = (0.0, 0.0);
        let mut clipped_from = None;
        for (i, &c) in codes.iter().enumerate() {
            let x = ((i as u32 * w + w / 2) / steps).min(w - 1);
            let got = y[(h / 2 * w + x) as usize] as f64;
            let e = (c as f64 - 64.0) / 876.0;
            let nits = pq_nits(e.clamp(0.0, 1.0));
            let plain = 16.0 + 219.0 * e;
            let clip = 16.0 + 219.0 * (nits / 100.0).min(1.0).powf(1.0 / 2.4);
            d_plain += (got - plain).powi(2);
            d_clip += (got - clip).powi(2);
            if clipped_from.is_none() && got >= 234.0 {
                clipped_from = Some(nits);
            }
            if i % 3 == 0 || i == codes.len() - 1 {
                rows.push(format!(
                    "        PQ code {c:4} ({nits:7.1} nits): out Y {got:5.1}  [plain {plain:5.1}, clip-at-100-nits {clip:5.1}]"
                ));
            }
        }
        for r in rows {
            println!("{r}");
        }
        let n = codes.len() as f64;
        let (rp, rc) = ((d_plain / n).sqrt(), (d_clip / n).sqrt());
        println!(
            "      RMS distance from 'plain' {rp:.1}, from 'clip at 100 nits' {rc:.1}; output first reaches white at {}",
            clipped_from.map_or("no step (never reaches 234)".into(), |n| format!("{n:.0} nits"))
        );
        let verdict = if rp < 6.0 {
            "looks like the PQ code passed through with only a matrix change (no transfer conversion, no tone mapping)"
        } else if rc < 6.0 {
            "looks like PQ decoded to light and clipped at SDR white (a transfer conversion, no tone mapping)"
        } else {
            "matches neither simple model: a roll-off, so some tone mapping (see the curve above)"
        };
        println!("      verdict: {verdict}");
        Ok(())
    }

    /// An NV12 frame, dark, with a white block in the top-left corner.
    fn marked_frame(w: u32, h: u32) -> Vec<u8> {
        let mut v = Vec::with_capacity((w * h * 3 / 2) as usize);
        for y in 0..h {
            for x in 0..w {
                v.push(if x < w / 4 && y < h / 4 { 235 } else { 16 });
            }
        }
        v.resize((w * h * 3 / 2) as usize, 128);
        v
    }

    /// Where the white block went: the box around the bright pixels.
    fn corners(y: &[u8], w: u32, h: u32) -> String {
        let (mut x0, mut y0, mut x1, mut y1, mut n) = (u32::MAX, u32::MAX, 0, 0, 0u64);
        for yy in 0..h {
            for xx in 0..w {
                if y[(yy * w + xx) as usize] > 128 {
                    x0 = x0.min(xx);
                    y0 = y0.min(yy);
                    x1 = x1.max(xx);
                    y1 = y1.max(yy);
                    n += 1;
                }
            }
        }
        if n == 0 {
            return format!("no bright pixels at all (frame mean {:.1})", mean(y));
        }
        let (cx, cy) = ((x0 + x1) / 2, (y0 + y1) / 2);
        let place = match (cx < w / 2, cy < h / 2) {
            (true, true) => "top-left: not turned",
            (false, true) => "top-right: turned 90 degrees clockwise",
            (true, false) => "bottom-left: turned 90 degrees counterclockwise",
            (false, false) => "bottom-right: turned 180 degrees",
        };
        format!("bright block at x {x0}..{x1}, y {y0}..{y1} of {w}x{h} ({n} px), {place}")
    }

    fn rotation_test(d3d: Option<&D3d>, method: &str) -> WinResult<()> {
        let (w, h) = (W, H);
        let it = video_type(&MFVideoFormat_NV12, w, h)?;
        let ot = video_type(&MFVideoFormat_NV12, h, w)?;
        // SAFETY: plain attribute calls.
        unsafe {
            it.SetUINT32(&MF_MT_DEFAULT_STRIDE, w)?;
            ot.SetUINT32(&MF_MT_DEFAULT_STRIDE, h)?;
            if method != "override" {
                it.SetUINT32(&MF_MT_VIDEO_ROTATION, MFVideoRotationFormat_90.0 as u32)?;
            }
        }
        let setup = match method {
            "attribute" => "input MF_MT_VIDEO_ROTATION 90, no control call",
            "attribute+SetRotation" => {
                "input MF_MT_VIDEO_ROTATION 90 + SetRotation(ROTATION_NORMAL)"
            }
            _ => "no rotation attribute, IMFVideoProcessorControl2::SetRotationOverride(90)",
        };
        // A processor with the control call made and the types set; the
        // control call's own result is kept for the report.
        let control = std::cell::RefCell::new(String::new());
        let make = || -> WinResult<IMFTransform> {
            let t = processor(d3d)?;
            // SAFETY: plain control and transform calls with types made here.
            unsafe {
                let r = match method {
                    "attribute" => Ok(()),
                    "attribute+SetRotation" => t
                        .cast::<IMFVideoProcessorControl>()?
                        .SetRotation(ROTATION_NORMAL),
                    _ => t
                        .cast::<IMFVideoProcessorControl2>()?
                        .SetRotationOverride(90),
                };
                *control.borrow_mut() = r.map_or_else(
                    |e| format!(" (control call: {})", hr(&e)),
                    |_| String::new(),
                );
                t.SetInputType(0, &it, 0)?;
                t.SetOutputType(0, &ot, 0)?;
            }
            Ok(t)
        };
        if let Err(e) = make() {
            println!(
                "    rotation, {setup}{}: types refused: {}",
                control.borrow(),
                hr(&e)
            );
            return Ok(());
        }
        println!(
            "    rotation, {setup}{}; {w}x{h} in, {h}x{w} out:",
            control.borrow()
        );
        match process_one(&make, d3d, &marked_frame(w, h), w, h, false) {
            Ok((out, how)) => {
                println!("      {how}");
                let y = y_plane(&out, h, w, false, h)?;
                println!("      {}", corners(&y, h, w));
            }
            Err(e) => println!("      processing failed: {e}"),
        }
        Ok(())
    }

    fn processor_report(d3d: Option<&D3d>, rot90: Option<&Path>, hdr: Option<&Path>) {
        println!(
            "  {}:",
            match d3d {
                Some(d) => format!("With a D3D11 manager on {}", d.name),
                None => "Without D3D (software)".into(),
            }
        );
        if let Err(e) = processor(d3d) {
            println!("    won't start: {}", hr(&e));
            return;
        }
        if let Err(e) = hdr_test(d3d) {
            println!("    HDR test failed: {}", hr(&e));
        }
        for method in ["attribute", "attribute+SetRotation", "override"] {
            if let Err(e) = rotation_test(d3d, method) {
                println!("    rotation test ({method}) failed: {}", hr(&e));
            }
        }
        if let Some(p) = rot90 {
            real_rotation(d3d, p);
        }
        if let Some(p) = hdr {
            real_hdr(d3d, p);
        }
    }

    /// The first decoded frame of `path` in `subtype`, with the reader's
    /// type as it stands once that frame is out.
    fn first_frame(
        path: &Path,
        d3d: Option<&D3d>,
        subtype: &GUID,
    ) -> Result<(IMFSample, IMFMediaType), String> {
        let reader = decode_reader(path, d3d, subtype).map_err(at("decode reader"))?;
        for _ in 0..100 {
            let mut flags = 0u32;
            let mut s = None;
            // SAFETY: a synchronous read; the out pointers are locals.
            unsafe { reader.ReadSample(VIDEO, 0, None, Some(&mut flags), None, Some(&mut s)) }
                .map_err(at("ReadSample"))?;
            if let Some(s) = s {
                // SAFETY: a plain call on the reader.
                let t = unsafe { reader.GetCurrentMediaType(VIDEO) }
                    .map_err(at("GetCurrentMediaType"))?;
                return Ok((s, t));
            }
            if flags & (MF_SOURCE_READERF_ENDOFSTREAM.0 | MF_SOURCE_READERF_ERROR.0) as u32 != 0 {
                break;
            }
        }
        Err("no frame".into())
    }

    /// A copy of a media type, to change without touching the original.
    fn copy_type(t: &IMFMediaType) -> WinResult<IMFMediaType> {
        // SAFETY: plain calls on a new media type.
        unsafe {
            let c = MFCreateMediaType()?;
            t.CopyAllItems(&c)?;
            Ok(c)
        }
    }

    fn turn(y: &[u8], w: u32, h: u32, clockwise: bool) -> Vec<u8> {
        // Out is h wide and w tall.
        let mut out = vec![0u8; (w * h) as usize];
        for yy in 0..h {
            for xx in 0..w {
                let (ox, oy) = if clockwise {
                    (h - 1 - yy, xx)
                } else {
                    (yy, w - 1 - xx)
                };
                out[(oy * h + ox) as usize] = y[(yy * w + xx) as usize];
            }
        }
        out
    }

    fn mad(a: &[u8], b: &[u8]) -> f64 {
        let n = a.len().min(b.len()).max(1);
        a.iter()
            .zip(b)
            .map(|(x, y)| (*x as f64 - *y as f64).abs())
            .sum::<f64>()
            / n as f64
    }

    /// The real case: the first frame of the rotated clip, as its decoder
    /// gives it (with MF_MT_VIDEO_ROTATION from the file), straight into
    /// the processor, against the same frame turned here each way.
    fn real_rotation(d3d: Option<&D3d>, path: &Path) {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let r = (|| -> Result<String, String> {
            let (frame, t) = first_frame(path, d3d, &MFVideoFormat_NV12)?;
            let (w, h) = get_u64(&t, &MF_MT_FRAME_SIZE)
                .map(hi_lo)
                .ok_or("no frame size")?;
            let stride = get_u32(&t, &MF_MT_DEFAULT_STRIDE).unwrap_or(w);
            let rot = get_u32(&t, &MF_MT_VIDEO_ROTATION);
            let input = y_plane(&frame, w, h, false, stride).map_err(at("reading the frame"))?;
            let proc = processor(d3d).map_err(at("processor"))?;
            let it = copy_type(&t).map_err(at("copying the type"))?;
            let ot = video_type(&MFVideoFormat_NV12, h, w).map_err(at("output type"))?;
            // SAFETY: plain attribute and transform calls.
            unsafe {
                ot.SetUINT32(&MF_MT_DEFAULT_STRIDE, h)
                    .map_err(at("stride"))?;
                proc.SetInputType(0, &it, 0)
                    .map_err(at("SetInputType (the decoder's type)"))?;
                proc.SetOutputType(0, &ot, 0)
                    .map_err(at("SetOutputType NV12 turned"))?;
            }
            let (out, how) = process_sample(&proc, &frame)?;
            let y = y_plane(&out, h, w, false, h).map_err(at("reading the output"))?;
            let (cw, ccw) = (
                mad(&y, &turn(&input, w, h, true)),
                mad(&y, &turn(&input, w, h, false)),
            );
            let verdict = if cw < ccw / 3.0 {
                "matches the frame turned clockwise"
            } else if ccw < cw / 3.0 {
                "matches the frame turned counterclockwise"
            } else {
                "matches neither turn"
            };
            Ok(format!(
                "decoder type has MF_MT_VIDEO_ROTATION {}, {w}x{h} in, {h}x{w} out; {how}\n      mean difference from the frame turned clockwise {cw:.1}, counterclockwise {ccw:.1}: {verdict}",
                rot.map_or("unset".into(), |r| r.to_string())
            ))
        })();
        match r {
            Ok(s) => println!(
                "    real frame, {name} decoded {} into the processor: {s}",
                if d3d.is_some() {
                    "on this card"
                } else {
                    "in memory"
                }
            ),
            Err(e) => println!("    real frame, {name}: failed at {e}"),
        }
    }

    /// The real case for HDR: the HDR clip's first frame, decoded to P010
    /// with the decoder's own colour attributes, into the processor for
    /// NV12 BT.709.
    fn real_hdr(d3d: Option<&D3d>, path: &Path) {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let r = (|| -> Result<String, String> {
            let (frame, t) = first_frame(path, d3d, &MFVideoFormat_P010)?;
            let (w, h) = get_u64(&t, &MF_MT_FRAME_SIZE)
                .map(hi_lo)
                .ok_or("no frame size")?;
            let stride = get_u32(&t, &MF_MT_DEFAULT_STRIDE).unwrap_or(w * 2);
            let input = y_plane(&frame, w, h, true, stride).map_err(at("reading the frame"))?;
            let proc = processor(d3d).map_err(at("processor"))?;
            let it = copy_type(&t).map_err(at("copying the type"))?;
            let ot = video_type(&MFVideoFormat_NV12, w, h).map_err(at("output type"))?;
            // SAFETY: plain attribute and transform calls.
            unsafe {
                ot.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32)
                    .map_err(at("attr"))?;
                ot.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32)
                    .map_err(at("attr"))?;
                ot.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32)
                    .map_err(at("attr"))?;
                ot.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)
                    .map_err(at("attr"))?;
                ot.SetUINT32(&MF_MT_DEFAULT_STRIDE, w)
                    .map_err(at("stride"))?;
                proc.SetInputType(0, &it, 0)
                    .map_err(at("SetInputType (the decoder's type)"))?;
                proc.SetOutputType(0, &ot, 0)
                    .map_err(at("SetOutputType NV12 BT.709"))?;
            }
            let (out, how) = process_sample(&proc, &frame)?;
            let y = y_plane(&out, w, h, false, w).map_err(at("reading the output"))?;
            Ok(format!(
                "decoder type: {}; {how}\n      Y mean in {:.1} (P010 as 8-bit), out {:.1}",
                colour(&t),
                mean(&input),
                mean(&y)
            ))
        })();
        match r {
            Ok(s) => println!("    real frame, {name} decoded {} to P010, into the processor for NV12 BT.709: {s}", if d3d.is_some() { "on this card" } else { "in memory" }),
            Err(e) => println!("    real frame, {name}: failed at {e}"),
        }
    }
}
