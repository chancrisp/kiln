//! The plumbing every Media Foundation module shares: COM and MF start-up
//! guards, a Media Foundation byte stream over a Rust [`File`] (so Media
//! Foundation never sees a path), and a driver for a synchronous transform
//! (an MFT).

use crate::error::{MediaError, Result};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use windows::core::{implement, w, GUID, HRESULT, PCWSTR};
use windows::Win32::Foundation::{E_FAIL, E_POINTER, S_OK};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_MULTITHREADED,
};
use windows::Win32::System::LibraryLoader::{LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};

// ---------------------------------------------------------------------------
// Start-up

/// COM for the current thread: joined to the multithreaded apartment the
/// first time a thread touches Media Foundation, and left when the thread ends.
struct ComThread {
    /// True when `CoInitializeEx` succeeded and so must be balanced.
    joined: bool,
}

impl Drop for ComThread {
    fn drop(&mut self) {
        if self.joined {
            // SAFETY: balances this thread's successful CoInitializeEx.
            unsafe { CoUninitialize() };
        }
    }
}

thread_local! {
    static COM: ComThread = ComThread {
        // SAFETY: a plain COM start-up call. RPC_E_CHANGED_MODE (the thread is
        // already a single-threaded apartment, as a UI thread is) leaves COM
        // usable, so only a success is balanced.
        joined: unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok(),
    };
}

/// Makes sure COM is initialised on this thread. Called by every entry point
/// that may run on a new thread: open, read, write, finish.
pub fn com() {
    COM.with(|_| {});
}

/// The DLLs Kiln's programs delay-load (DIST-04): Windows N and KN
/// editions have none of them until the Media Feature Pack is added.
const MF_DLLS: [PCWSTR; 3] = [w!("mfplat.dll"), w!("mfreadwrite.dll"), w!("mf.dll")];

/// Set once all of [`MF_DLLS`] have been found; they stay loaded after.
static FOUND: AtomicBool = AtomicBool::new(false);

/// Whether this Windows has Media Foundation. `kiln-app.exe` and `kiln.exe`
/// delay-load its DLLs, so they start without them, and a call into a
/// missing one would raise an exception instead of returning an error.
/// Every way into Media Foundation asks this first. A missing DLL is looked
/// for again on the next call, for when the Media Feature Pack is added
/// while Kiln runs. In debug builds `KILN_HIDE_MF` says no.
pub fn installed() -> bool {
    #[cfg(debug_assertions)]
    if std::env::var_os("KILN_HIDE_MF").is_some() {
        return false;
    }
    if FOUND.load(Ordering::Acquire) {
        return true;
    }
    let found = all_in_system32(&MF_DLLS);
    if found {
        FOUND.store(true, Ordering::Release);
    }
    found
}

/// Whether every DLL in `names` loads from System32 (and so is loaded).
fn all_in_system32(names: &[PCWSTR]) -> bool {
    names.iter().all(|&name| {
        // SAFETY: a plain library load of a system DLL by name, from
        // System32 only; the module is kept, as the delay-load would keep it.
        unsafe { LoadLibraryExW(name, None, LOAD_LIBRARY_SEARCH_SYSTEM32) }.is_ok()
    })
}

/// Media Foundation, started for as long as this guard lives. `MFStartup` and
/// `MFShutdown` count, so every source and sink holds its own.
pub struct Mf(());

impl Mf {
    pub fn start() -> Result<Mf> {
        if !installed() {
            return Err(MediaError::NeedsMediaFoundation);
        }
        com();
        // SAFETY: balanced by `Drop`.
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET) }
            .map_err(|_| MediaError::NeedsMediaFoundation)?;
        Ok(Mf(()))
    }
}

impl Drop for Mf {
    fn drop(&mut self) {
        com();
        // SAFETY: balances the MFStartup in `start`.
        unsafe {
            let _ = MFShutdown();
        }
    }
}

/// A Media Foundation failure while encoding, named. A full disk, which
/// reaches Media Foundation from the byte stream as HRESULT_FROM_WIN32 of
/// ERROR_DISK_FULL or ERROR_HANDLE_DISK_FULL, is the I/O error it was, so
/// the row can say there is no room.
pub(crate) fn failed(what: &str, e: windows::core::Error) -> MediaError {
    const DISK_FULL: u32 = 0x8007_0070;
    const HANDLE_DISK_FULL: u32 = 0x8007_0027;
    let code = e.code().0 as u32;
    if matches!(code, DISK_FULL | HANDLE_DISK_FULL) {
        return MediaError::Io(std::io::Error::from_raw_os_error((code & 0xFFFF) as i32));
    }
    MediaError::Encoder(format!("{what} ({code:#010X})"))
}

// ---------------------------------------------------------------------------
// A byte stream over a File

fn io_error(e: std::io::Error) -> windows::core::Error {
    match e.raw_os_error() {
        Some(code) => HRESULT::from_win32(code as u32).into(),
        None => E_FAIL.into(),
    }
}

/// A Media Foundation byte stream over a Rust file, for reading or writing.
///
/// Media Foundation's own wrapper over an `IStream` caches in 64 KiB blocks
/// and reads and writes from work-queue threads, each a Seek and then a Read
/// or Write; under load those interleave. Writing, the ASF writer then put
/// its header over a block of garbage and lost the audio, and every file
/// ended padded with junk to the next 64 KiB. This stream has no cache, keeps
/// its own position, and does every call, the asynchronous ones too, at once,
/// whole, and in the order it is made.
///
/// It is also an attribute store, as Media Foundation's byte streams are, so
/// a reader can be told the content type (`MF_BYTESTREAM_CONTENT_TYPE`).
#[implement(IMFByteStream, IMFAttributes)]
struct FileByteStream {
    state: Mutex<StreamState>,
    writable: bool,
    attrs: IMFAttributes,
    /// The furthest any read has reached, which a source reports as progress.
    furthest: Arc<AtomicU64>,
    /// Every byte read so far, counted once per read.
    total: Arc<AtomicU64>,
    /// Box types read as `free` at these file offsets: what a reader is
    /// not to see (an MP4's edit lists, which the video reader applies
    /// itself).
    hidden: Vec<u64>,
}

struct StreamState {
    /// `None` once the stream is closed, so the handle goes with `Close`
    /// rather than with the last COM reference.
    file: Option<File>,
    pos: u64,
}

/// Where an asynchronous call's byte count travels in its result.
const DONE_BYTES: GUID = GUID::from_u128(0x6b1f_57c2_4a0e_4d3b_9a51_2c7e_0d84_e913);

impl FileByteStream {
    fn lock(&self) -> std::sync::MutexGuard<'_, StreamState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Reads at the stream's position into `buf`, moving the position on.
    fn read_into(&self, buf: &mut [u8]) -> windows::core::Result<u32> {
        let mut guard = self.lock();
        let s = &mut *guard;
        let file = s.file.as_mut().ok_or_else(closed)?;
        file.seek(SeekFrom::Start(s.pos)).map_err(io_error)?;
        let mut done = 0;
        while done < buf.len() {
            match file.read(&mut buf[done..]) {
                Ok(0) => break,
                Ok(n) => done += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(io_error(e)),
            }
        }
        let from = s.pos;
        s.pos += done as u64;
        for &at in &self.hidden {
            for (i, b) in b"free".iter().enumerate() {
                let p = at + i as u64;
                if p >= from && p < s.pos {
                    buf[(p - from) as usize] = *b;
                }
            }
        }
        self.furthest.fetch_max(s.pos, Ordering::Relaxed);
        self.total.fetch_add(done as u64, Ordering::Relaxed);
        Ok(done as u32)
    }

    /// Writes `buf` at the stream's position, moving the position on.
    fn write_from(&self, buf: &[u8]) -> windows::core::Result<u32> {
        if !self.writable {
            return Err(windows::Win32::Foundation::STG_E_ACCESSDENIED.into());
        }
        let mut guard = self.lock();
        let s = &mut *guard;
        let file = s.file.as_mut().ok_or_else(closed)?;
        file.seek(SeekFrom::Start(s.pos)).map_err(io_error)?;
        file.write_all(buf).map_err(io_error)?;
        s.pos += buf.len() as u64;
        Ok(buf.len() as u32)
    }

    fn len(&self) -> windows::core::Result<u64> {
        let s = self.lock();
        let file = s.file.as_ref().ok_or_else(closed)?;
        Ok(file.metadata().map_err(io_error)?.len())
    }
}

/// What a call on a closed stream gets.
fn closed() -> windows::core::Error {
    MF_E_SHUTDOWN.into()
}

/// Hands the outcome of an asynchronous call, already done, to its callback.
fn complete(
    outcome: windows::core::Result<u32>,
    callback: windows::core::Ref<IMFAsyncCallback>,
    state: windows::core::Ref<windows::core::IUnknown>,
) -> windows::core::Result<()> {
    // SAFETY: plain Media Foundation calls on objects made here; the result
    // keeps its own references to the callback, the state and the count.
    unsafe {
        let mut attrs = None;
        MFCreateAttributes(&mut attrs, 1)?;
        let attrs = attrs.ok_or_else(|| windows::core::Error::from(E_FAIL))?;
        let status = match &outcome {
            Ok(n) => {
                attrs.SetUINT32(&DONE_BYTES, *n)?;
                S_OK
            }
            Err(e) => e.code(),
        };
        let result = MFCreateAsyncResult(&attrs, callback.as_ref(), state.as_ref())?;
        result.SetStatus(status)?;
        MFInvokeCallback(&result)
    }
}

/// The byte count of a finished asynchronous call, or its failure.
fn finished(result: windows::core::Ref<IMFAsyncResult>) -> windows::core::Result<u32> {
    let result = result.ok()?;
    // SAFETY: plain calls on a result `complete` made.
    unsafe {
        result.GetStatus()?;
        let attrs: IMFAttributes = windows::core::Interface::cast(&result.GetObject()?)?;
        attrs.GetUINT32(&DONE_BYTES)
    }
}

impl IMFByteStream_Impl for FileByteStream_Impl {
    fn GetCapabilities(&self) -> windows::core::Result<u32> {
        let write = if self.writable {
            MFBYTESTREAM_IS_WRITABLE
        } else {
            0
        };
        Ok(MFBYTESTREAM_IS_READABLE | MFBYTESTREAM_IS_SEEKABLE | write)
    }

    fn GetLength(&self) -> windows::core::Result<u64> {
        self.len()
    }

    fn SetLength(&self, qwlength: u64) -> windows::core::Result<()> {
        if !self.writable {
            return Err(windows::Win32::Foundation::STG_E_ACCESSDENIED.into());
        }
        let s = self.lock();
        let file = s.file.as_ref().ok_or_else(closed)?;
        file.set_len(qwlength).map_err(io_error)
    }

    fn GetCurrentPosition(&self) -> windows::core::Result<u64> {
        Ok(self.lock().pos)
    }

    fn SetCurrentPosition(&self, qwposition: u64) -> windows::core::Result<()> {
        self.lock().pos = qwposition;
        Ok(())
    }

    fn IsEndOfStream(&self) -> windows::core::Result<windows::core::BOOL> {
        let len = self.len()?;
        Ok((self.lock().pos >= len).into())
    }

    fn Read(&self, pb: *mut u8, cb: u32, pcbread: *mut u32) -> windows::core::Result<()> {
        if pb.is_null() || pcbread.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: Media Foundation promises `pb` points at `cb` writable bytes.
        let buf = unsafe { std::slice::from_raw_parts_mut(pb, cb as usize) };
        let n = self.read_into(buf)?;
        // SAFETY: checked non-null above.
        unsafe { *pcbread = n };
        Ok(())
    }

    fn BeginRead(
        &self,
        pb: *mut u8,
        cb: u32,
        pcallback: windows::core::Ref<IMFAsyncCallback>,
        punkstate: windows::core::Ref<windows::core::IUnknown>,
    ) -> windows::core::Result<()> {
        if pb.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: Media Foundation promises `pb` points at `cb` writable
        // bytes until EndRead.
        let buf = unsafe { std::slice::from_raw_parts_mut(pb, cb as usize) };
        complete(self.read_into(buf), pcallback, punkstate)
    }

    fn EndRead(&self, presult: windows::core::Ref<IMFAsyncResult>) -> windows::core::Result<u32> {
        finished(presult)
    }

    fn Write(&self, pb: *const u8, cb: u32) -> windows::core::Result<u32> {
        if pb.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: Media Foundation promises `pb` points at `cb` readable bytes.
        self.write_from(unsafe { std::slice::from_raw_parts(pb, cb as usize) })
    }

    fn BeginWrite(
        &self,
        pb: *const u8,
        cb: u32,
        pcallback: windows::core::Ref<IMFAsyncCallback>,
        punkstate: windows::core::Ref<windows::core::IUnknown>,
    ) -> windows::core::Result<()> {
        if pb.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: Media Foundation promises `pb` points at `cb` readable
        // bytes until EndWrite.
        let buf = unsafe { std::slice::from_raw_parts(pb, cb as usize) };
        complete(self.write_from(buf), pcallback, punkstate)
    }

    fn EndWrite(&self, presult: windows::core::Ref<IMFAsyncResult>) -> windows::core::Result<u32> {
        finished(presult)
    }

    fn Seek(
        &self,
        seekorigin: MFBYTESTREAM_SEEK_ORIGIN,
        llseekoffset: i64,
        _dwseekflags: u32,
    ) -> windows::core::Result<u64> {
        let mut s = self.lock();
        let from = if seekorigin == msoBegin {
            0
        } else if seekorigin == msoCurrent {
            s.pos as i64
        } else {
            return Err(windows::Win32::Foundation::E_INVALIDARG.into());
        };
        s.pos = from.saturating_add(llseekoffset).max(0) as u64;
        Ok(s.pos)
    }

    fn Flush(&self) -> windows::core::Result<()> {
        match self.lock().file.as_mut() {
            Some(f) if self.writable => f.flush().map_err(io_error),
            _ => Ok(()),
        }
    }

    fn Close(&self) -> windows::core::Result<()> {
        // The handle goes now, so the file can be removed straight after.
        self.lock().file = None;
        Ok(())
    }
}

/// The attribute calls, passed to a store Media Foundation made.
impl IMFAttributes_Impl for FileByteStream_Impl {
    fn GetItem(&self, k: *const GUID, v: *mut PROPVARIANT) -> windows::core::Result<()> {
        // SAFETY (this and every call below): the arguments are passed on
        // as Media Foundation gave them, to an attribute store of its own.
        unsafe { self.attrs.GetItem(k, Some(v)) }
    }
    fn GetItemType(&self, k: *const GUID) -> windows::core::Result<MF_ATTRIBUTE_TYPE> {
        unsafe { self.attrs.GetItemType(k) }
    }
    fn CompareItem(
        &self,
        k: *const GUID,
        v: *const PROPVARIANT,
    ) -> windows::core::Result<windows::core::BOOL> {
        unsafe { self.attrs.CompareItem(k, v) }
    }
    fn Compare(
        &self,
        theirs: windows::core::Ref<IMFAttributes>,
        m: MF_ATTRIBUTES_MATCH_TYPE,
    ) -> windows::core::Result<windows::core::BOOL> {
        unsafe { self.attrs.Compare(theirs.as_ref(), m) }
    }
    fn GetUINT32(&self, k: *const GUID) -> windows::core::Result<u32> {
        unsafe { self.attrs.GetUINT32(k) }
    }
    fn GetUINT64(&self, k: *const GUID) -> windows::core::Result<u64> {
        unsafe { self.attrs.GetUINT64(k) }
    }
    fn GetDouble(&self, k: *const GUID) -> windows::core::Result<f64> {
        unsafe { self.attrs.GetDouble(k) }
    }
    fn GetGUID(&self, k: *const GUID) -> windows::core::Result<GUID> {
        unsafe { self.attrs.GetGUID(k) }
    }
    fn GetStringLength(&self, k: *const GUID) -> windows::core::Result<u32> {
        unsafe { self.attrs.GetStringLength(k) }
    }
    fn GetString(
        &self,
        k: *const GUID,
        buf: windows::core::PWSTR,
        size: u32,
        len: *mut u32,
    ) -> windows::core::Result<()> {
        if buf.is_null() {
            return Err(E_POINTER.into());
        }
        let out = unsafe { std::slice::from_raw_parts_mut(buf.0, size as usize) };
        unsafe { self.attrs.GetString(k, out, Some(len)) }
    }
    fn GetAllocatedString(
        &self,
        k: *const GUID,
        out: *mut windows::core::PWSTR,
        len: *mut u32,
    ) -> windows::core::Result<()> {
        unsafe { self.attrs.GetAllocatedString(k, out, len) }
    }
    fn GetBlobSize(&self, k: *const GUID) -> windows::core::Result<u32> {
        unsafe { self.attrs.GetBlobSize(k) }
    }
    fn GetBlob(
        &self,
        k: *const GUID,
        buf: *mut u8,
        size: u32,
        len: *mut u32,
    ) -> windows::core::Result<()> {
        if buf.is_null() {
            return Err(E_POINTER.into());
        }
        let out = unsafe { std::slice::from_raw_parts_mut(buf, size as usize) };
        unsafe { self.attrs.GetBlob(k, out, Some(len)) }
    }
    fn GetAllocatedBlob(
        &self,
        k: *const GUID,
        out: *mut *mut u8,
        len: *mut u32,
    ) -> windows::core::Result<()> {
        unsafe { self.attrs.GetAllocatedBlob(k, out, len) }
    }
    fn GetUnknown(
        &self,
        k: *const GUID,
        iid: *const GUID,
        out: *mut *mut core::ffi::c_void,
    ) -> windows::core::Result<()> {
        if iid.is_null() || out.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe {
            let unknown: windows::core::IUnknown = self.attrs.GetUnknown(k)?;
            windows::core::Interface::query(&unknown, iid, out).ok()
        }
    }
    fn SetItem(&self, k: *const GUID, v: *const PROPVARIANT) -> windows::core::Result<()> {
        unsafe { self.attrs.SetItem(k, v) }
    }
    fn DeleteItem(&self, k: *const GUID) -> windows::core::Result<()> {
        unsafe { self.attrs.DeleteItem(k) }
    }
    fn DeleteAllItems(&self) -> windows::core::Result<()> {
        unsafe { self.attrs.DeleteAllItems() }
    }
    fn SetUINT32(&self, k: *const GUID, v: u32) -> windows::core::Result<()> {
        unsafe { self.attrs.SetUINT32(k, v) }
    }
    fn SetUINT64(&self, k: *const GUID, v: u64) -> windows::core::Result<()> {
        unsafe { self.attrs.SetUINT64(k, v) }
    }
    fn SetDouble(&self, k: *const GUID, v: f64) -> windows::core::Result<()> {
        unsafe { self.attrs.SetDouble(k, v) }
    }
    fn SetGUID(&self, k: *const GUID, v: *const GUID) -> windows::core::Result<()> {
        unsafe { self.attrs.SetGUID(k, v) }
    }
    fn SetString(&self, k: *const GUID, v: &windows::core::PCWSTR) -> windows::core::Result<()> {
        unsafe { self.attrs.SetString(k, *v) }
    }
    fn SetBlob(&self, k: *const GUID, buf: *const u8, size: u32) -> windows::core::Result<()> {
        if buf.is_null() && size > 0 {
            return Err(E_POINTER.into());
        }
        let data: &[u8] = if size == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(buf, size as usize) }
        };
        unsafe { self.attrs.SetBlob(k, data) }
    }
    fn SetUnknown(
        &self,
        k: *const GUID,
        v: windows::core::Ref<windows::core::IUnknown>,
    ) -> windows::core::Result<()> {
        unsafe { self.attrs.SetUnknown(k, v.as_ref()) }
    }
    fn LockStore(&self) -> windows::core::Result<()> {
        unsafe { self.attrs.LockStore() }
    }
    fn UnlockStore(&self) -> windows::core::Result<()> {
        unsafe { self.attrs.UnlockStore() }
    }
    fn GetCount(&self) -> windows::core::Result<u32> {
        unsafe { self.attrs.GetCount() }
    }
    fn GetItemByIndex(
        &self,
        i: u32,
        k: *mut GUID,
        v: *mut PROPVARIANT,
    ) -> windows::core::Result<()> {
        unsafe { self.attrs.GetItemByIndex(i, k, Some(v)) }
    }
    fn CopyAllItems(&self, dest: windows::core::Ref<IMFAttributes>) -> windows::core::Result<()> {
        unsafe { self.attrs.CopyAllItems(dest.as_ref()) }
    }
}

/// A handle on the same file that can read as well as write: Media
/// Foundation reads back what it writes.
fn read_write(file: File) -> Result<File> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, HANDLE};
    use windows::Win32::Storage::FileSystem::{
        ReOpenFile, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    // SAFETY: reopens the open file's own handle; on success the new handle is
    // owned by the returned File and the old one is closed when `file` drops.
    match unsafe {
        ReOpenFile(
            HANDLE(file.as_raw_handle()),
            GENERIC_READ.0 | GENERIC_WRITE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_FLAGS_AND_ATTRIBUTES(0),
        )
    } {
        Ok(h) => Ok(unsafe { File::from_raw_handle(h.0 as _) }),
        Err(e) => Err(MediaError::Encoder(format!(
            "the output can't be opened for reading too ({:#010X})",
            e.code().0 as u32
        ))),
    }
}

/// A second handle on `file` for reading, with a position of its own: what
/// Kiln reads through it never moves a byte stream's reads.
pub(crate) fn reopen_for_reading(file: &File) -> Option<File> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use windows::Win32::Foundation::{GENERIC_READ, HANDLE};
    use windows::Win32::Storage::FileSystem::{
        ReOpenFile, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    // SAFETY: reopens the open file's own handle, which stays `file`'s; on
    // success the new handle is owned by the returned File.
    let h = unsafe {
        ReOpenFile(
            HANDLE(file.as_raw_handle()),
            GENERIC_READ.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_FLAGS_AND_ATTRIBUTES(0),
        )
    }
    .ok()?;
    // SAFETY: `h` is a new handle that nothing else owns.
    Some(unsafe { File::from_raw_handle(h.0 as _) })
}

/// A Media Foundation byte stream over `file`, read from its start (or
/// written, when `writable`), and a counter of how far it has been read.
pub fn byte_stream(file: File, writable: bool) -> Result<(IMFByteStream, Arc<AtomicU64>)> {
    byte_stream_counted(file, writable).map(|(stream, furthest, _)| (stream, furthest))
}

/// As [`byte_stream`], with a counter of every byte read too: what a reader
/// that looks through the whole file first (a fragmented MP4's index) has
/// read since, as it goes through the samples.
pub(crate) fn byte_stream_counted(
    file: File,
    writable: bool,
) -> Result<(IMFByteStream, Arc<AtomicU64>, Arc<AtomicU64>)> {
    byte_stream_hiding(file, writable, Vec::new())
}

/// As [`byte_stream_counted`], with the 4-byte box types at each offset in
/// `hidden` read as `free`, so the box is skipped by whoever reads it.
pub(crate) fn byte_stream_hiding(
    file: File,
    writable: bool,
    hidden: Vec<u64>,
) -> Result<(IMFByteStream, Arc<AtomicU64>, Arc<AtomicU64>)> {
    let file = if writable { read_write(file)? } else { file };
    // SAFETY: a plain call making an empty attribute store.
    let attrs = unsafe {
        let mut attrs = None;
        MFCreateAttributes(&mut attrs, 2).map_err(|_| MediaError::NeedsMediaFoundation)?;
        attrs.ok_or(MediaError::NeedsMediaFoundation)?
    };
    let furthest = Arc::new(AtomicU64::new(0));
    let total = Arc::new(AtomicU64::new(0));
    let stream: IMFByteStream = FileByteStream {
        state: Mutex::new(StreamState {
            file: Some(file),
            pos: 0,
        }),
        writable,
        attrs,
        furthest: furthest.clone(),
        total: total.clone(),
        hidden,
    }
    .into();
    Ok((stream, furthest, total))
}

// ---------------------------------------------------------------------------
// Media types and transforms

/// An audio media type with the common fields set.
pub(crate) fn audio_type(subtype: &GUID, rate: u32, channels: u16) -> Result<IMFMediaType> {
    // SAFETY: plain attribute calls on a new media type.
    unsafe {
        let t = MFCreateMediaType().map_err(|e| failed("no media type", e))?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
            .and_then(|_| t.SetGUID(&MF_MT_SUBTYPE, subtype))
            .and_then(|_| t.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate))
            .and_then(|_| t.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels as u32))
            .map_err(|e| failed("can't describe the audio", e))?;
        Ok(t)
    }
}

/// Uncompressed interleaved audio: 32-bit float, or 16-bit integer.
pub(crate) fn pcm_type(float: bool, rate: u32, channels: u16, mask: u32) -> Result<IMFMediaType> {
    let (subtype, bits) = if float {
        (MFAudioFormat_Float, 32u32)
    } else {
        (MFAudioFormat_PCM, 16)
    };
    let t = audio_type(&subtype, rate, channels)?;
    let align = bits / 8 * channels as u32;
    // SAFETY: plain attribute calls.
    unsafe {
        t.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, bits)
            .and_then(|_| t.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, align))
            .and_then(|_| t.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, align * rate))
            .and_then(|_| t.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1))
            .map_err(|e| failed("can't describe the audio", e))?;
        if mask != 0 {
            t.SetUINT32(&MF_MT_AUDIO_CHANNEL_MASK, mask)
                .map_err(|e| failed("can't describe the speakers", e))?;
        }
    }
    Ok(t)
}

pub(crate) fn get_u32(t: &IMFAttributes, key: &GUID) -> Option<u32> {
    // SAFETY: a plain attribute read.
    unsafe { t.GetUINT32(key) }.ok()
}

pub(crate) fn get_blob(t: &IMFAttributes, key: &GUID) -> Option<Vec<u8>> {
    // SAFETY: the buffer is sized from GetBlobSize before GetBlob fills it.
    unsafe {
        let n = t.GetBlobSize(key).ok()?;
        let mut v = vec![0u8; n as usize];
        t.GetBlob(key, &mut v, None).ok()?;
        Some(v)
    }
}

/// Every synchronous local transform in `category` that makes `output`.
pub(crate) fn transforms(category: GUID, output: &GUID) -> Vec<IMFActivate> {
    enum_transforms(
        category,
        MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_LOCALMFT | MFT_ENUM_FLAG_SORTANDFILTER,
        None,
        Some((MFMediaType_Audio, *output)),
        None,
    )
}

/// Every transform in `category` that `flags` admits, taking `input` and
/// making `output` (each a major type and a subtype) when they are given.
/// Hardware and asynchronous transforms come back only when `flags` asks.
/// With `adapter` (a graphics card's LUID, high part above low), only the
/// hardware transforms on that card, through `MFTEnum2`.
pub fn enum_transforms(
    category: GUID,
    flags: MFT_ENUM_FLAG,
    input: Option<(GUID, GUID)>,
    output: Option<(GUID, GUID)>,
    adapter: Option<u64>,
) -> Vec<IMFActivate> {
    let info = |(major, subtype): (GUID, GUID)| MFT_REGISTER_TYPE_INFO {
        guidMajorType: major,
        guidSubtype: subtype,
    };
    let input = input.map(info);
    let output = output.map(info);
    let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    // SAFETY: the type-info pointers point at locals that outlive the call;
    // on success the enumeration hands back a CoTaskMem array of `count`
    // activation objects; each is moved out once, then the array is freed.
    unsafe {
        let input = input.as_ref().map(|i| i as *const _);
        let output = output.as_ref().map(|o| o as *const _);
        let found = match adapter {
            None => MFTEnumEx(category, flags, input, output, &mut activates, &mut count),
            Some(luid) => {
                // MFTEnum2 takes the LUID as a blob: LowPart, then HighPart.
                let mut bytes = [0u8; 8];
                bytes[..4].copy_from_slice(&(luid as u32).to_le_bytes());
                bytes[4..].copy_from_slice(&((luid >> 32) as u32).to_le_bytes());
                let mut attrs = None;
                MFCreateAttributes(&mut attrs, 1).and_then(|_| {
                    let attrs = attrs.ok_or_else(|| windows::core::Error::from(E_FAIL))?;
                    attrs.SetBlob(&MFT_ENUM_ADAPTER_LUID, &bytes)?;
                    MFTEnum2(
                        category,
                        flags,
                        input,
                        output,
                        &attrs,
                        &mut activates,
                        &mut count,
                    )
                })
            }
        };
        if found.is_err() || activates.is_null() {
            return Vec::new();
        }
        let v = (0..count as usize)
            .filter_map(|i| std::ptr::read(activates.add(i)))
            .collect();
        CoTaskMemFree(Some(activates as *const _));
        v
    }
}

/// The first encoder that makes `output`, made ready to use.
pub(crate) fn encoder(output: &GUID) -> Option<IMFTransform> {
    transforms(MFT_CATEGORY_AUDIO_ENCODER, output)
        .iter()
        // SAFETY: activates the transform the enumeration described.
        .find_map(|a| unsafe { a.ActivateObject::<IMFTransform>() }.ok())
}

/// Drives a synchronous transform: interleaved samples in, packets out.
pub(crate) struct Transform {
    t: IMFTransform,
    /// The size of buffer to hand the transform for each output, when it
    /// doesn't allocate its own.
    out_size: u32,
    provides: bool,
    /// Bytes per input frame, and the input rate, for the time stamps.
    frame_bytes: u32,
    rate: u32,
    frames_in: u64,
    streaming: bool,
}

impl Transform {
    /// Starts streaming on a transform whose types are already set.
    pub(crate) fn new(t: IMFTransform, frame_bytes: u32, rate: u32) -> Result<Transform> {
        // SAFETY: plain calls on a transform with both types set.
        unsafe {
            let info = t
                .GetOutputStreamInfo(0)
                .map_err(|e| failed("the encoder has no output", e))?;
            t.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .and_then(|_| t.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0))
                .map_err(|e| failed("the encoder won't start", e))?;
            Ok(Transform {
                t,
                out_size: info.cbSize.max(64 * 1024),
                provides: info.dwFlags
                    & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32
                        | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0 as u32)
                    != 0,
                frame_bytes,
                rate,
                frames_in: 0,
                streaming: true,
            })
        }
    }

    /// Hands `bytes` (whole frames) to the transform, collecting every packet
    /// it gives back into `out`.
    pub(crate) fn push(&mut self, bytes: &[u8], out: &mut Vec<Vec<u8>>) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let frames = (bytes.len() / self.frame_bytes as usize) as u64;
        let sample = sample(bytes, self.frames_in, frames, self.rate)?;
        self.frames_in += frames;
        loop {
            // SAFETY: a sample we own, given to the transform.
            match unsafe { self.t.ProcessInput(0, &sample, 0) } {
                Ok(()) => break,
                Err(e) if e.code() == MF_E_NOTACCEPTING => self.pull(out)?,
                Err(e) => return Err(failed("the encoder refused the audio", e)),
            }
        }
        self.pull(out)
    }

    /// Collects every packet the transform has ready.
    fn pull(&mut self, out: &mut Vec<Vec<u8>>) -> Result<()> {
        loop {
            // SAFETY: the output buffer array has one entry, for stream 0. The
            // sample and events it holds afterwards are taken back and dropped.
            let got = unsafe {
                let sample = if self.provides {
                    None
                } else {
                    let s = MFCreateSample().map_err(|e| failed("out of memory", e))?;
                    let b = MFCreateMemoryBuffer(self.out_size)
                        .map_err(|e| failed("out of memory", e))?;
                    s.AddBuffer(&b).map_err(|e| failed("out of memory", e))?;
                    Some(s)
                };
                let mut db = [MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: std::mem::ManuallyDrop::new(sample),
                    dwStatus: 0,
                    pEvents: std::mem::ManuallyDrop::new(None),
                }];
                let mut status = 0u32;
                let r = self.t.ProcessOutput(0, &mut db, &mut status);
                let sample = std::mem::ManuallyDrop::take(&mut db[0].pSample);
                drop(std::mem::ManuallyDrop::take(&mut db[0].pEvents));
                match r {
                    Ok(()) => sample,
                    Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(()),
                    Err(e) => return Err(failed("the encoder failed", e)),
                }
            };
            if let Some(s) = got {
                out.push(sample_bytes(&s)?);
            }
        }
    }

    /// Tells the transform the audio has ended and collects what it still holds.
    pub(crate) fn drain(&mut self, out: &mut Vec<Vec<u8>>) -> Result<()> {
        // SAFETY: plain messages to a streaming transform.
        unsafe {
            self.t
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0)
                .and_then(|_| self.t.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0))
                .map_err(|e| failed("the encoder won't finish", e))?;
        }
        self.pull(out)?;
        self.stop();
        Ok(())
    }

    fn stop(&mut self) {
        if self.streaming {
            self.streaming = false;
            // SAFETY: a plain message to the transform.
            unsafe {
                let _ = self.t.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
            }
        }
    }
}

impl Drop for Transform {
    fn drop(&mut self) {
        com();
        self.stop();
    }
}

/// A sample holding `bytes`: `frames` frames of audio at `rate`, starting at
/// frame `first`, with its time and duration in 100 ns units.
pub(crate) fn sample(bytes: &[u8], first: u64, frames: u64, rate: u32) -> Result<IMFSample> {
    let hns = |f: u64| (f as u128 * 10_000_000 / rate as u128) as i64;
    let oom = |e| failed("out of memory", e);
    // SAFETY: the buffer is locked only while it is filled, and its length is
    // set to what was copied in.
    unsafe {
        let buf = MFCreateMemoryBuffer(bytes.len() as u32).map_err(oom)?;
        let mut p = std::ptr::null_mut();
        buf.Lock(&mut p, None, None).map_err(oom)?;
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        buf.Unlock().map_err(oom)?;
        buf.SetCurrentLength(bytes.len() as u32).map_err(oom)?;
        let s = MFCreateSample().map_err(oom)?;
        s.AddBuffer(&buf).map_err(oom)?;
        s.SetSampleTime(hns(first))
            .and_then(|_| s.SetSampleDuration(hns(first + frames) - hns(first)))
            .map_err(oom)?;
        Ok(s)
    }
}

/// The bytes a sample holds, in one piece.
pub(crate) fn sample_bytes(s: &IMFSample) -> Result<Vec<u8>> {
    // SAFETY: the buffer is locked while its bytes are copied out.
    unsafe {
        let b = s
            .ConvertToContiguousBuffer()
            .map_err(|e| failed("can't read the encoder's output", e))?;
        let mut p = std::ptr::null_mut();
        let mut len = 0u32;
        b.Lock(&mut p, None, Some(&mut len))
            .map_err(|e| failed("can't read the encoder's output", e))?;
        let v = std::slice::from_raw_parts(p, len as usize).to_vec();
        let _ = b.Unlock();
        Ok(v)
    }
}

/// Interleaved samples held to [-1, 1], as little-endian f32 bytes.
pub(crate) fn float_bytes(samples: &[f32], out: &mut Vec<u8>) {
    out.clear();
    out.reserve(samples.len() * 4);
    for s in samples {
        out.extend_from_slice(&s.clamp(-1.0, 1.0).to_le_bytes());
    }
}

/// Interleaved samples held to [-1, 1], as little-endian 16-bit bytes.
pub(crate) fn i16_bytes(samples: &[f32], out: &mut Vec<u8>) {
    out.clear();
    out.reserve(samples.len() * 2);
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dll_windows_lacks_is_not_found_and_a_system_one_is() {
        assert!(!all_in_system32(&[w!("kiln-no-such-library.dll")]));
        assert!(!all_in_system32(&[
            w!("kernel32.dll"),
            w!("kiln-no-such-library.dll")
        ]));
        assert!(all_in_system32(&[w!("kernel32.dll")]));
    }

    /// The PC running the tests has Media Foundation, as CI says when it
    /// hasn't (KILN_CI_NO_MF).
    #[test]
    fn this_windows_has_media_foundation() {
        if std::env::var_os("KILN_CI_NO_MF").is_some() || std::env::var_os("KILN_HIDE_MF").is_some()
        {
            return;
        }
        assert!(installed());
    }
    use std::os::windows::fs::OpenOptionsExt;

    #[test]
    fn a_full_disk_is_an_io_error() {
        for (hr, os) in [(0x8007_0070u32, 112), (0x8007_0027, 39)] {
            match failed("writing", HRESULT(hr as i32).into()) {
                MediaError::Io(e) => assert_eq!(e.raw_os_error(), Some(os)),
                other => panic!("{hr:#X}: {other:?}"),
            }
        }
        // Anything else is still the encoder's.
        assert!(matches!(
            failed("writing", E_FAIL.into()),
            MediaError::Encoder(w) if w == "writing (0x80004005)"
        ));
    }

    #[test]
    fn close_lets_go_of_the_file_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.bin");
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        let _mf = Mf::start().unwrap();
        let (stream, _) = byte_stream(file, true).unwrap();
        // SAFETY: plain calls on a stream made here.
        unsafe {
            assert_eq!(stream.Write(b"abc").unwrap(), 3);
            stream.SetCurrentPosition(0).unwrap();
            let mut back = [0u8; 3];
            let mut n = 0;
            stream.Read(&mut back, &mut n).unwrap();
            assert_eq!((&back, n), (b"abc", 3));
            stream.Close().unwrap();
            assert!(stream.Write(b"d").is_err());
        }
        // The stream is still referenced, but no handle is open: the file
        // opens with no sharing at all.
        let only = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path);
        assert!(only.is_ok(), "{only:?}");
        drop(stream);
    }

    #[test]
    fn a_read_only_stream_says_so_and_takes_a_content_type() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("in.bin");
        std::fs::write(&path, b"0123456789").unwrap();
        let _mf = Mf::start().unwrap();
        let (stream, furthest) = byte_stream(File::open(&path).unwrap(), false).unwrap();
        // SAFETY: plain calls on a stream made here.
        unsafe {
            let caps = stream.GetCapabilities().unwrap();
            assert_eq!(caps & MFBYTESTREAM_IS_WRITABLE, 0);
            assert_ne!(caps & MFBYTESTREAM_IS_READABLE, 0);
            assert!(stream.Write(b"x").is_err());
            assert_eq!(stream.Seek(msoBegin, 4, 0).unwrap(), 4);
            let mut buf = [0u8; 4];
            let mut n = 0;
            stream.Read(&mut buf, &mut n).unwrap();
            assert_eq!(&buf, b"4567");
            assert_eq!(furthest.load(Ordering::Relaxed), 8);
            let attrs: IMFAttributes = windows::core::Interface::cast(&stream).unwrap();
            attrs
                .SetString(&MF_BYTESTREAM_CONTENT_TYPE, windows::core::w!("audio/aac"))
                .unwrap();
            assert_eq!(
                attrs.GetStringLength(&MF_BYTESTREAM_CONTENT_TYPE).unwrap(),
                9
            );
        }
    }
}
