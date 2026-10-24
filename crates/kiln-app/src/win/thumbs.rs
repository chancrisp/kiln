//! Thumbnails from Windows' own thumbnail service, the one File Explorer
//! uses, fetched one at a time on a worker thread so the window never
//! waits on the Shell or the disk.

use std::collections::VecDeque;
use std::os::windows::fs::MetadataExt;
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
    IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_CROPTOSQUARE, SIIGBF_INCACHEONLY,
    SIIGBF_THUMBNAILONLY,
};

use super::{PostTarget, WM_KILN_PREVIEW};
use crate::previews::{self, fix_alpha, premultiply, Ask, Pixels, Preview, PreviewKey};

/// Windows' thumbnail of `path`, asked for as `ask` says, as
/// premultiplied BGRA, top row first. With `cached_only` it is only one
/// Windows already has, so the file itself is never read. `None` when the
/// file has no thumbnail of its own (only an icon), can't be read, or
/// anything else goes wrong.
///
/// It calls the Shell, which can be slow, so never on the UI thread; the
/// thread must have COM initialised apartment-threaded.
pub fn fetch(path: &Path, ask: Ask, cached_only: bool) -> Option<Pixels> {
    let side = ask.side.clamp(1, i32::MAX as u32) as i32;
    // A thumbnail, never an icon; cropped to the square when asked; and
    // only from Windows' cache when the file mustn't be read.
    let mut flags = SIIGBF_THUMBNAILONLY;
    if ask.square {
        flags |= SIIGBF_CROPTOSQUARE;
    }
    if cached_only {
        flags |= SIIGBF_INCACHEONLY;
    }
    let name = HSTRING::from(path.as_os_str());
    let factory: IShellItemImageFactory =
        unsafe { SHCreateItemFromParsingName(&name, None) }.ok()?;
    let bitmap = unsafe { factory.GetImage(SIZE { cx: side, cy: side }, flags) }.ok()?;
    let read = read_bitmap(bitmap);
    // The bitmap is ours to delete, whether or not it could be read.
    unsafe {
        let _ = DeleteObject(bitmap.into());
    }
    let mut px = read?;
    fix_alpha(&mut px.bgra);
    premultiply(&mut px.bgra);
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
        fetch: impl Fn(&Path, Ask, bool) -> Option<Pixels> + Send + 'static,
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
    fetch: impl Fn(&Path, Ask, bool) -> Option<Pixels>,
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
        let read = std::fs::metadata(&key.path)
            .and_then(|m| Ok((m.modified()?, m.file_attributes())))
            .ok();
        let (modified, preview) = match read {
            // The file is gone, or its share is offline: asking the Shell
            // would only wait on it again. There is no picture, and the
            // answer keeps the time it was asked with, so the window
            // caches it under the key it asked for and doesn't ask again.
            None => (key.modified, Preview::None),
            Some((modified, attributes)) => {
                let ask = previews::ask(key.size, key.pixels);
                let preview = match fetch(&key.path, ask, cloud_only(attributes)) {
                    Some(px) => Preview::Ready(Arc::new(px)),
                    None => Preview::None,
                };
                (Some(modified), preview)
            }
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

/// The data is kept somewhere else, as by hierarchical storage.
const FILE_ATTRIBUTE_OFFLINE: u32 = 0x1000;
/// The file has no data here: opening it fetches it, as a cloud-only file
/// may.
const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x4_0000;
/// The file's data isn't all here: reading it fetches it from its remote
/// store, as reading a OneDrive placeholder does.
const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;

/// Whether a file's attributes say its data is somewhere else, so that
/// making a thumbnail would read it from the network or the cloud. Such a
/// file is shown only from Windows' cache of thumbnails, so a preview
/// never brings a cloud-only file down.
fn cloud_only(attributes: u32) -> bool {
    attributes
        & (FILE_ATTRIBUTE_OFFLINE
            | FILE_ATTRIBUTE_RECALL_ON_OPEN
            | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS)
        != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::time::{Duration, SystemTime};

    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};

    use crate::previews::{cover_source, pixel_size, PreviewKey, PreviewSize};

    const WAIT: Duration = Duration::from_secs(10);

    /// `fetch` on a thread of its own with COM apartment-threaded, as the
    /// worker runs it.
    fn fetch_on_sta(path: PathBuf, ask: Ask, cached_only: bool) -> Option<Pixels> {
        std::thread::spawn(move || {
            let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
            let px = fetch(&path, ask, cached_only);
            if com {
                unsafe { CoUninitialize() };
            }
            px
        })
        .join()
        .expect("the fetch thread panicked")
    }

    /// A row's square at 100%, as the worker asks for it.
    fn row_ask() -> Ask {
        previews::ask(PreviewSize::Row, pixel_size(PreviewSize::Row, 1.0))
    }

    /// A PNG of one opaque colour, `w` x `h`, at `path`.
    fn png(path: &Path, w: u32, h: u32) {
        let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h).unwrap();
        pixmap.fill(resvg::tiny_skia::Color::from_rgba8(0x7F, 0xB5, 0xA0, 0xFF));
        pixmap.save_png(path).unwrap();
    }

    #[test]
    fn a_png_gives_a_row_square_the_right_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("celadon.png");
        png(&path, 64, 48);

        let px = fetch_on_sta(path, row_ask(), false).expect("Windows makes a thumbnail of a PNG");

        // Cropped to the square by Windows, so the row draws it as made.
        assert_eq!((px.width, px.height), (34, 34));
        assert_eq!(px.bgra.len(), (px.width * px.height * 4) as usize);
        assert!(px.bgra.chunks_exact(4).any(|p| p[3] != 0), "all clear");
    }

    #[test]
    fn wide_and_upright_pictures_are_made_big_enough_to_fill_their_box() {
        // A 16:9 frame and an upright phone video's, bigger than any box,
        // at a row's size and the inspector's, at 125%.
        let dir = tempfile::tempdir().unwrap();
        for (w, h) in [(1600, 900), (900, 1600)] {
            let path = dir.path().join(format!("frame {w}x{h}.png"));
            png(&path, w, h);
            for size in [PreviewSize::Row, PreviewSize::Hero] {
                let bx = pixel_size(size, 1.25);
                let ask = previews::ask(size, bx);
                let px = fetch_on_sta(path.clone(), ask, false).expect("a thumbnail");
                let (_, _, cw, ch) =
                    cover_source((px.width, px.height), (bx.0 as f32, bx.1 as f32));
                let scale = (bx.0 as f32 / cw).max(bx.1 as f32 / ch);
                assert!(
                    scale <= 1.0,
                    "{w}x{h} as {size:?}: made {} x {} for {bx:?}, drawn at {scale}",
                    px.width,
                    px.height
                );
            }
        }
    }

    #[test]
    fn a_translucent_png_arrives_premultiplied() {
        // Windows gives straight alpha; Direct2D draws premultiplied. A
        // translucent bone half and a clear half: each colour is scaled by
        // its alpha once, or the edge glows on the dark theme, and only
        // once, or the picture goes dark.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("glaze.png");
        let mut pixmap = resvg::tiny_skia::Pixmap::new(64, 64).unwrap();
        let bone = resvg::tiny_skia::ColorU8::from_rgba(0xED, 0xE6, 0xDA, 64).premultiply();
        for y in 0..64 {
            for x in 0..32 {
                pixmap.pixels_mut()[y * 64 + x] = bone;
            }
        }
        pixmap.save_png(&path).unwrap();

        let px = fetch_on_sta(path, row_ask(), false).expect("Windows makes a thumbnail of a PNG");

        assert!(px.bgra.chunks_exact(4).any(|p| p[3] == 0), "clear");
        let translucent: Vec<&[u8]> = px.bgra.chunks_exact(4).filter(|p| p[3] == 64).collect();
        assert!(!translucent.is_empty(), "translucent");
        // Bone at a quarter: 0xDA, 0xE6, 0xED times 64/255 is about 55, 58
        // and 59. Twice over it would be about 14.
        for p in translucent {
            for (got, want) in p[..3].iter().zip([55u8, 58, 59]) {
                assert!(got.abs_diff(want) <= 3, "{p:?}");
            }
        }
        assert!(
            px.bgra
                .chunks_exact(4)
                .all(|p| p[..3].iter().all(|&c| c <= p[3])),
            "straight alpha reached the renderer"
        );
    }

    #[test]
    fn a_missing_file_gives_none() {
        let path = PathBuf::from("C:\\no\\such\\file.png");
        assert!(fetch_on_sta(path, row_ask(), false).is_none());
    }

    #[test]
    fn a_file_windows_has_no_thumbnail_of_yet_gives_none_from_the_cache() {
        // A picture made just now has never been shown: asked for only
        // what Windows already has, there is nothing.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("never shown.png");
        png(&path, 64, 48);
        assert!(fetch_on_sta(path.clone(), row_ask(), true).is_none());
        // Asked for in full, it has one.
        assert!(fetch_on_sta(path, row_ask(), false).is_some());
    }

    #[test]
    fn a_file_whose_data_is_elsewhere_is_one_to_take_from_the_cache_only() {
        const OFFLINE: u32 = 0x1000;
        const RECALL_ON_OPEN: u32 = 0x4_0000;
        const RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;
        const READONLY: u32 = 0x1;
        const ARCHIVE: u32 = 0x20;
        for cloud in [OFFLINE, RECALL_ON_OPEN, RECALL_ON_DATA_ACCESS] {
            assert!(cloud_only(cloud), "{cloud:#x}");
            assert!(cloud_only(cloud | ARCHIVE), "{cloud:#x}");
        }
        assert!(!cloud_only(0));
        assert!(!cloud_only(READONLY | ARCHIVE));
    }

    /// Files named `names` in a new folder, each a few bytes.
    fn files(names: &[&str]) -> (tempfile::TempDir, Vec<PathBuf>) {
        let dir = tempfile::tempdir().unwrap();
        let paths = names
            .iter()
            .map(|n| {
                let p = dir.path().join(n);
                std::fs::write(&p, b"x").unwrap();
                p
            })
            .collect();
        (dir, paths)
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

    /// What a test worker's fetch was asked: the file, how, and whether
    /// from the cache only.
    type Started = mpsc::Receiver<(PathBuf, Ask, bool)>;

    /// A worker whose fetch waits for a go on the returned sender (one per
    /// file, in order) and says which file it started on; what it posts
    /// comes out of the last receiver.
    fn gated() -> (Thumbs, mpsc::Sender<()>, Started, Posted) {
        let (release, gate) = mpsc::channel::<()>();
        let (started_tx, started) = mpsc::channel();
        let (done_tx, posted) = mpsc::channel();
        let gate = Mutex::new(gate);
        let thumbs = Thumbs::spawn(
            move |path, ask, cached_only| {
                let _ = started_tx.send((path.to_path_buf(), ask, cached_only));
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
        let (_dir, paths) = files(&["a", "b", "c", "d"]);
        let [a, b, c, d] = [0, 1, 2, 3].map(|i| paths[i].clone());
        let (thumbs, release, started, posted) = gated();
        thumbs.ask(vec![key(&a), key(&b), key(&c)]);
        assert_eq!(started.recv_timeout(WAIT).unwrap().0, a);

        // Scrolled on while a is under way: b and c are no longer wanted,
        // and a, asked again, is not fetched twice.
        thumbs.ask(vec![key(&a), key(&d)]);
        for _ in 0..2 {
            let _ = release.send(());
        }

        let (first, preview) = posted.recv_timeout(WAIT).unwrap();
        assert_eq!(first.path, a);
        assert_eq!(preview, Preview::None, "no picture: nothing to show");
        assert_eq!(started.recv_timeout(WAIT).unwrap().0, d);
        assert_eq!(posted.recv_timeout(WAIT).unwrap().0.path, d);
        drop(thumbs);
        drop(release);
        assert!(started.try_recv().is_err(), "b and c were never fetched");
        assert!(posted.recv_timeout(Duration::from_millis(200)).is_err());
    }

    #[test]
    fn each_size_is_asked_for_as_it_is_drawn() {
        let (_dir, paths) = files(&["a.png"]);
        let (thumbs, release, started, _posted) = gated();
        let hero = PreviewKey {
            size: PreviewSize::Hero,
            pixels: (335, 150),
            ..key(&paths[0])
        };
        thumbs.ask(vec![key(&paths[0]), hero]);
        let _ = release.send(());
        let _ = release.send(());
        let (_, row, cached) = started.recv_timeout(WAIT).unwrap();
        assert_eq!(row, previews::ask(PreviewSize::Row, (34, 34)));
        assert!(row.square && !cached);
        let (_, hero, _) = started.recv_timeout(WAIT).unwrap();
        assert_eq!(hero, previews::ask(PreviewSize::Hero, (335, 150)));
        assert!(!hero.square);
    }

    #[test]
    fn a_file_kept_offline_is_asked_for_from_the_cache_only() {
        use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
        // Its data is somewhere else, as a cloud-only file's is: making a
        // preview must never bring it down.
        const OFFLINE: u32 = 0x1000;
        let (dir, paths) = files(&["here.png"]);
        let away = dir.path().join("away.png");
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .attributes(OFFLINE)
            .open(&away)
            .unwrap();
        assert_ne!(
            std::fs::metadata(&away).unwrap().file_attributes() & OFFLINE,
            0,
            "the test file is marked offline"
        );
        let (thumbs, release, started, _posted) = gated();
        thumbs.ask(vec![key(&away), key(&paths[0])]);
        let _ = release.send(());
        let _ = release.send(());
        let (path, _, cached) = started.recv_timeout(WAIT).unwrap();
        assert_eq!((path, cached), (away, true));
        let (path, _, cached) = started.recv_timeout(WAIT).unwrap();
        assert_eq!((path, cached), (paths[0].clone(), false));
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
    fn a_file_whose_time_cant_be_read_isnt_fetched_and_keeps_the_time_it_was_asked_with() {
        // Gone since it was listed, or on a share that went offline: it
        // costs no fetch, which would only wait on the share again, and
        // its "no picture" answer is kept under the key the window asked
        // for, so it is cached once instead of asked for again and again.
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("gone.png");
        let listed = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let (thumbs, release, started, posted) = gated();
        let asked = PreviewKey {
            modified: Some(listed),
            ..key(&gone)
        };
        thumbs.ask(vec![asked.clone()]);
        let _ = release.send(());

        let (answer, preview) = posted.recv_timeout(WAIT).unwrap();
        assert_eq!(answer, asked);
        assert_eq!(preview, Preview::None);
        assert!(started.try_recv().is_err(), "it was fetched");
    }

    #[test]
    fn nothing_is_posted_once_the_worker_is_dropped() {
        let (_dir, paths) = files(&["a", "b"]);
        let (thumbs, release, started, posted) = gated();
        thumbs.ask(vec![key(&paths[0]), key(&paths[1])]);
        assert_eq!(started.recv_timeout(WAIT).unwrap().0, paths[0]);
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
