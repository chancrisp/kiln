//! Thumbnails from Windows' own thumbnail service, the one File Explorer
//! uses, fetched one at a time on a worker thread so the window never
//! waits on the Shell or the disk.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use windows::core::HSTRING;
use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Shell::{
    IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_THUMBNAILONLY,
};

use super::{PostTarget, WM_KILN_PREVIEW};
use crate::previews::{fix_alpha, Pixels, Preview, PreviewKey};

/// Windows' thumbnail of `path`, made to fit a square as wide as the
/// larger side of `pixels`, as premultiplied BGRA, top row first. `None`
/// when the file has no thumbnail of its own (only an icon), can't be
/// read, or anything else goes wrong.
///
/// It calls the Shell, which can be slow, so never on the UI thread; the
/// thread must have COM initialised apartment-threaded.
pub fn fetch(path: &Path, pixels: (u32, u32)) -> Option<Pixels> {
    let side = pixels.0.max(pixels.1).clamp(1, i32::MAX as u32) as i32;
    let name = HSTRING::from(path.as_os_str());
    let factory: IShellItemImageFactory =
        unsafe { SHCreateItemFromParsingName(&name, None) }.ok()?;
    let bitmap =
        unsafe { factory.GetImage(SIZE { cx: side, cy: side }, SIIGBF_THUMBNAILONLY) }.ok()?;
    let read = read_bitmap(bitmap);
    // The bitmap is ours to delete, whether or not it could be read.
    unsafe {
        let _ = DeleteObject(bitmap.into());
    }
    let mut px = read?;
    fix_alpha(&mut px.bgra);
    Some(px)
}

/// The pixels of `bitmap` as 32-bit BGRA, top row first. The bitmap is
/// left for the caller to delete.
fn read_bitmap(bitmap: HBITMAP) -> Option<Pixels> {
    let mut info = BITMAP::default();
    let got = unsafe {
        GetObjectW(
            bitmap.into(),
            size_of::<BITMAP>() as i32,
            Some((&mut info as *mut BITMAP).cast()),
        )
    };
    let (w, h) = (info.bmWidth, info.bmHeight.saturating_abs());
    if got == 0 || w <= 0 || h <= 0 {
        return None;
    }
    let (width, height) = (w as u32, h as u32);
    let len = (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)?;
    let mut bgra = vec![0u8; len];
    let mut header = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            // Negative: the top row comes first.
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let dc = unsafe { CreateCompatibleDC(None) };
    if dc.is_invalid() {
        return None;
    }
    // 32 bits a pixel leaves no padding, so `bgra` holds every row.
    let lines = unsafe {
        GetDIBits(
            dc,
            bitmap,
            0,
            height,
            Some(bgra.as_mut_ptr().cast()),
            &mut header,
            DIB_RGB_COLORS,
        )
    };
    unsafe {
        let _ = DeleteDC(dc);
    }
    (lines == h).then_some(Pixels {
        width,
        height,
        bgra,
    })
}

#[derive(Default)]
struct State {
    /// What to fetch, first to last. Each ask replaces it.
    queue: VecDeque<PreviewKey>,
    /// The key being fetched now, as it was asked for.
    current: Option<PreviewKey>,
    stop: bool,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        // A panic in a fetch must not take the queue with it.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The thumbnail worker: one thread, COM apartment-threaded, that fetches
/// the previews it was last asked for, in order, and posts each one to
/// the window. It stops with the window.
pub struct Thumbs {
    shared: Arc<Shared>,
}

impl Thumbs {
    /// Start the worker. Each answer is posted to `post` as
    /// `WM_KILN_PREVIEW` with a boxed `(PreviewKey, Preview)`; the key
    /// carries the file's time as it was when the preview was made.
    pub fn start(post: PostTarget) -> Thumbs {
        Thumbs::spawn(fetch, move |key, preview| {
            post.post(WM_KILN_PREVIEW, (key, preview))
        })
    }

    /// The worker, fetching with `fetch` and handing each answer to
    /// `done`.
    fn spawn(
        fetch: impl Fn(&Path, (u32, u32)) -> Option<Pixels> + Send + 'static,
        done: impl Fn(PreviewKey, Preview) + Send + 'static,
    ) -> Thumbs {
        let shared = Arc::new(Shared::default());
        let worker = shared.clone();
        // Detached: `shutdown` doesn't wait for it, so a slow file never
        // holds the window up as it closes.
        let spawned = std::thread::Builder::new()
            .name("kiln-thumbs".into())
            .spawn(move || {
                // Background work, like inspecting: it gives way to the
                // window and to conversions.
                super::inspect::below_normal();
                // The Shell's thumbnail handlers expect an apartment.
                let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
                work(&worker, fetch, done);
                if com {
                    unsafe { CoUninitialize() };
                }
            });
        if spawned.is_err() {
            super::log("couldn't start the thumbnail thread");
        }
        Thumbs { shared }
    }

    /// Fetch `keys`, in order, in place of whatever was still waiting:
    /// what was asked before and not yet fetched is dropped at once. The
    /// key being fetched now isn't queued again, as its answer is on the
    /// way.
    pub fn ask(&self, keys: Vec<PreviewKey>) {
        let mut s = self.shared.lock();
        if s.stop {
            return;
        }
        let queue: VecDeque<PreviewKey> = keys
            .into_iter()
            .filter(|k| s.current.as_ref() != Some(k))
            .collect();
        s.queue = queue;
        self.shared.wake.notify_one();
    }

    /// Stop without waiting for a fetch under way, which may be slow.
    /// Once this returns nothing more is posted. Safe to call more than
    /// once.
    pub fn shutdown(&self) {
        {
            let mut s = self.shared.lock();
            s.stop = true;
            s.queue.clear();
        }
        self.shared.wake.notify_all();
    }
}

impl Drop for Thumbs {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn work(
    shared: &Shared,
    fetch: impl Fn(&Path, (u32, u32)) -> Option<Pixels>,
    done: impl Fn(PreviewKey, Preview),
) {
    loop {
        // The queue is read afresh for every key, so a newer ask takes
        // over as soon as the fetch under way is done.
        let key = {
            let mut s = shared.lock();
            loop {
                if s.stop {
                    return;
                }
                if let Some(key) = s.queue.pop_front() {
                    s.current = Some(key.clone());
                    break key;
                }
                s = shared.wake.wait(s).unwrap_or_else(|e| e.into_inner());
            }
        };
        // The file's time now, read before it is fetched: the answer is
        // about the file as it is, which may have changed since it was
        // listed. The window sorts out which is newer.
        let modified = std::fs::metadata(&key.path).and_then(|m| m.modified()).ok();
        let preview = match fetch(&key.path, key.pixels) {
            Some(px) => Preview::Ready(Arc::new(px)),
            None => Preview::None,
        };
        let answer = PreviewKey { modified, ..key };
        // Posted under the lock, so nothing is posted once `shutdown`
        // has returned.
        let mut s = shared.lock();
        s.current = None;
        if s.stop {
            return;
        }
        done(answer, preview);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::time::{Duration, SystemTime};

    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};

    use crate::previews::{PreviewKey, PreviewSize};

    const WAIT: Duration = Duration::from_secs(10);

    /// `fetch` on a thread of its own with COM apartment-threaded, as the
    /// worker runs it.
    fn fetch_on_sta(path: PathBuf, pixels: (u32, u32)) -> Option<Pixels> {
        std::thread::spawn(move || {
            let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
            let px = fetch(&path, pixels);
            if com {
                unsafe { CoUninitialize() };
            }
            px
        })
        .join()
        .expect("the fetch thread panicked")
    }

    #[test]
    fn a_png_gives_a_thumbnail_the_right_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("celadon.png");
        let mut pixmap = tiny_skia::Pixmap::new(64, 48).unwrap();
        pixmap.fill(tiny_skia::Color::from_rgba8(0x7F, 0xB5, 0xA0, 0xFF));
        pixmap.save_png(&path).unwrap();

        let px = fetch_on_sta(path, (34, 34)).expect("Windows makes a thumbnail of a PNG");

        assert!(
            px.width <= 34 && px.height <= 34,
            "{} x {}",
            px.width,
            px.height
        );
        assert!(px.width > 0 && px.height > 0);
        assert_eq!(px.bgra.len(), (px.width * px.height * 4) as usize);
        assert!(px.bgra.chunks_exact(4).any(|p| p[3] != 0), "all clear");
    }

    #[test]
    fn a_missing_file_gives_none() {
        let path = PathBuf::from("C:\\no\\such\\file.png");
        assert!(fetch_on_sta(path, (34, 34)).is_none());
    }

    fn key(path: &Path) -> PreviewKey {
        PreviewKey {
            path: path.to_path_buf(),
            modified: None,
            size: PreviewSize::Row,
            pixels: (34, 34),
        }
    }

    /// What a test worker posted.
    type Posted = mpsc::Receiver<(PreviewKey, Preview)>;

    /// A worker whose fetch waits for a go on the returned sender (one per
    /// file, in order) and says which file it started on; what it posts
    /// comes out of the last receiver.
    fn gated() -> (Thumbs, mpsc::Sender<()>, mpsc::Receiver<PathBuf>, Posted) {
        let (release, gate) = mpsc::channel::<()>();
        let (started_tx, started) = mpsc::channel();
        let (done_tx, posted) = mpsc::channel();
        let gate = Mutex::new(gate);
        let thumbs = Thumbs::spawn(
            move |path, _| {
                let _ = started_tx.send(path.to_path_buf());
                let _ = gate.lock().unwrap().recv();
                None
            },
            move |key, preview| {
                let _ = done_tx.send((key, preview));
            },
        );
        (thumbs, release, started, posted)
    }

    #[test]
    fn a_new_ask_replaces_the_queue_at_once() {
        let (thumbs, release, started, posted) = gated();
        let [a, b, c, d] = ["a", "b", "c", "d"].map(PathBuf::from);
        thumbs.ask(vec![key(&a), key(&b), key(&c)]);
        assert_eq!(started.recv_timeout(WAIT).unwrap(), a);

        // Scrolled on while a is under way: b and c are no longer wanted,
        // and a, asked again, is not fetched twice.
        thumbs.ask(vec![key(&a), key(&d)]);
        for _ in 0..2 {
            let _ = release.send(());
        }

        let (first, preview) = posted.recv_timeout(WAIT).unwrap();
        assert_eq!(first.path, a);
        assert_eq!(preview, Preview::None, "no picture: nothing to show");
        assert_eq!(started.recv_timeout(WAIT).unwrap(), d);
        assert_eq!(posted.recv_timeout(WAIT).unwrap().0.path, d);
        drop(thumbs);
        drop(release);
        assert!(started.try_recv().is_err(), "b and c were never fetched");
        assert!(posted.recv_timeout(Duration::from_millis(200)).is_err());
    }

    #[test]
    fn an_answer_carries_the_files_time_as_it_was_fetched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.png");
        std::fs::write(&path, b"x").unwrap();
        let now = std::fs::metadata(&path).unwrap().modified().unwrap();
        let (thumbs, release, _started, posted) = gated();

        // Asked with the time the list read when the file was added, which
        // has gone stale since.
        let stale = PreviewKey {
            modified: Some(SystemTime::UNIX_EPOCH),
            ..key(&path)
        };
        thumbs.ask(vec![stale]);
        let _ = release.send(());

        let (answer, _) = posted.recv_timeout(WAIT).unwrap();
        assert_eq!(answer.modified, Some(now));
        assert_eq!(answer.path, path);
        assert_eq!(answer.pixels, (34, 34));
    }

    #[test]
    fn nothing_is_posted_once_the_worker_is_dropped() {
        let (thumbs, release, started, posted) = gated();
        thumbs.ask(vec![key(Path::new("a")), key(Path::new("b"))]);
        assert_eq!(started.recv_timeout(WAIT).unwrap(), PathBuf::from("a"));
        let t = std::time::Instant::now();
        drop(thumbs);
        assert!(
            t.elapsed() < Duration::from_millis(100),
            "{:?}",
            t.elapsed()
        );
        // Let a finish: the thread ends without posting it or starting b.
        let _ = release.send(());
        drop(release);
        assert_eq!(
            posted.recv_timeout(WAIT),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
        assert!(started.try_recv().is_err());
    }
}
