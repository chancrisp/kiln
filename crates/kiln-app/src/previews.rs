//! Previews: the pictures Windows' own thumbnail service makes for the
//! files in the list.
//!
//! This is the platform-free half: a cache capped by bytes, the order
//! previews are asked for, the centred crop that fills a box, and the fix
//! for thumbnails Windows hands back fully transparent. The Windows half
//! (`win::thumbs`) fetches on a worker thread, and the window draws what
//! the cache holds.

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use crate::layout::{HERO, PREVIEW_ROW};

/// The most bytes of pictures the cache holds.
pub const CACHE_CAP: usize = 64 * 1024 * 1024;

/// What a `Preview::None` counts toward the cap: it holds no pixels, but
/// it isn't free.
const NONE_BYTES: usize = 64;

/// How many rows past the visible ones are asked for, each way.
const AHEAD: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PreviewSize {
    /// The square beside a row.
    Row,
    /// The wide picture at the top of the inspector.
    Hero,
}

/// Which picture: a file as it was, at one size on one screen. A file
/// that changes, or a window that moves to a screen of another scale,
/// makes a new key and so is asked for again.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PreviewKey {
    pub path: PathBuf,
    pub modified: Option<SystemTime>,
    pub size: PreviewSize,
    /// The box it is drawn in, in device pixels: see [`pixel_size`]. What
    /// is asked of Windows follows from it and `size` ([`ask`]).
    pub pixels: (u32, u32),
}

impl PreviewKey {
    /// A hash of the whole key, for the renderer to key its bitmap by.
    /// It is stable for one run of the app (equal keys always give the
    /// same id), not between Rust releases, so never store it.
    pub fn id(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.hash(&mut hasher);
        hasher.finish()
    }
}

/// A picture in device pixels: premultiplied BGRA, top row first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Preview {
    Ready(Arc<Pixels>),
    /// Windows has no thumbnail for this file: draw the glaze tile.
    None,
}

impl Preview {
    /// What this counts toward the cache's cap.
    fn cost(&self) -> usize {
        match self {
            Preview::Ready(pixels) => pixels.bgra.len(),
            Preview::None => NONE_BYTES,
        }
    }
}

struct Entry {
    preview: Preview,
    arrived: Instant,
    cost: usize,
    /// When this was last used, as a count of uses: bigger is newer.
    last_used: u64,
}

/// Previews held by key, up to a number of bytes. When a new one doesn't
/// fit, the one used longest ago goes first.
pub struct PreviewCache {
    cap: usize,
    bytes: usize,
    entries: HashMap<PreviewKey, Entry>,
    /// Counts every `get` and `put`; an entry takes the count as its
    /// `last_used`. Reading is constant time however many are held.
    uses: u64,
    /// Keys the worker has been asked for and has not yet answered.
    pending: HashSet<PreviewKey>,
}

impl PreviewCache {
    pub fn new(cap: usize) -> Self {
        PreviewCache {
            cap,
            bytes: 0,
            entries: HashMap::new(),
            uses: 0,
            pending: HashSet::new(),
        }
    }

    /// The preview for `key` and when it arrived, which counts as using
    /// it. `now` is the frame's clock; the order of use doesn't read it.
    pub fn get(&mut self, key: &PreviewKey, _now: Instant) -> Option<(&Preview, Instant)> {
        let entry = self.entries.get_mut(key)?;
        self.uses += 1;
        entry.last_used = self.uses;
        Some((&entry.preview, entry.arrived))
    }

    /// Stores a preview that arrived at `now`, replacing any for the same
    /// key, and makes room by dropping the least recently used. One
    /// preview bigger than the whole cap is kept alone rather than lost
    /// as it arrives.
    pub fn put(&mut self, key: PreviewKey, preview: Preview, now: Instant) {
        self.pending.remove(&key);
        self.remove(&key);
        let cost = preview.cost();
        self.bytes += cost;
        self.uses += 1;
        self.entries.insert(
            key,
            Entry {
                preview,
                arrived: now,
                cost,
                last_used: self.uses,
            },
        );
        // The one just put is the newest, so it is only ever the oldest
        // when it is the only one left.
        while self.bytes > self.cap && self.entries.len() > 1 {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone());
            match oldest.and_then(|key| self.entries.remove(&key)) {
                Some(entry) => self.bytes -= entry.cost,
                None => break,
            }
        }
    }

    /// Whether to ask the worker for `key` now. It's true once for a key
    /// the cache doesn't hold, and stays false until the answer arrives
    /// through [`put`](Self::put), so each picture is asked for once.
    pub fn pending(&mut self, key: &PreviewKey) -> bool {
        if self.entries.contains_key(key) || self.pending.contains(key) {
            return false;
        }
        self.pending.insert(key.clone());
        true
    }

    /// Forgets that anything is being waited for; the previews held stay.
    /// The worker's new queue replaces its old one, so the keys it dropped
    /// will never be answered. The window calls this just before each new
    /// ask, so what is pending is what was last asked, and a picture
    /// scrolled away from and back to is asked for again.
    pub fn clear_pending(&mut self) {
        self.pending.clear();
    }

    /// Drops the previews, and the waiting for previews, of every path
    /// `listed` turns down: the rows that are gone.
    pub fn keep_paths(&mut self, listed: &dyn Fn(&Path) -> bool) {
        self.pending.retain(|key| listed(&key.path));
        let mut freed = 0;
        self.entries.retain(|key, entry| {
            let keep = listed(&key.path);
            if !keep {
                freed += entry.cost;
            }
            keep
        });
        self.bytes -= freed;
    }

    /// The bytes of pictures held now.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    fn remove(&mut self, key: &PreviewKey) {
        if let Some(entry) = self.entries.remove(key) {
            self.bytes -= entry.cost;
        }
    }
}

/// The shapes of picture the inspector's ask is sized for: wide video, a
/// landscape photo, a portrait photo and an upright phone video.
pub const SHAPES: [(u32, u32); 4] = [(16, 9), (4, 3), (3, 4), (9, 16)];

/// How to ask Windows for a picture: the side of the square it is made
/// for, and whether Windows crops it to that square
/// (`SIIGBF_CROPTOSQUARE`) rather than fitting the whole of it inside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ask {
    pub side: u32,
    pub square: bool,
}

/// How to ask Windows for a picture of `size` drawn in a box of `pixels`
/// device pixels ([`pixel_size`]), so that once it is cropped to fill the
/// box it is never drawn larger than it was made.
///
/// Windows fits a thumbnail inside the square it is asked for, so a wide
/// picture asked for at a row's size would come back a strip, and filling
/// the row's square would stretch it. A row's picture is cropped to the
/// square by Windows instead. The inspector's wide box is asked for as a
/// square big enough that each of [`SHAPES`], fitted inside it, still
/// covers the box: an upright phone video fitted inside it is as wide as
/// the box.
pub fn ask(size: PreviewSize, pixels: (u32, u32)) -> Ask {
    let (w, h) = (pixels.0.max(1), pixels.1.max(1));
    match size {
        PreviewSize::Row => Ask {
            side: w.max(h),
            square: true,
        },
        PreviewSize::Hero => {
            // Fitted inside a square of `side`, a picture `sw` by `sh` is
            // side * sw / long wide and side * sh / long tall, and both
            // must reach the box.
            let side = SHAPES
                .iter()
                .map(|&(sw, sh)| {
                    let long = sw.max(sh);
                    let wide = w.saturating_mul(long).div_ceil(sw);
                    let tall = h.saturating_mul(long).div_ceil(sh);
                    wide.max(tall)
                })
                .fold(w.max(h), u32::max);
            Ask {
                side,
                square: false,
            }
        }
    }
}

/// The box a picture is drawn in, in device pixels: a row's square or the
/// inspector's picture, in DIPs, times the screen's scale, rounded up so
/// it is never smaller than the box drawn. What is asked of Windows
/// follows from it: see [`ask`].
pub fn pixel_size(size: PreviewSize, px_per_dip: f32) -> (u32, u32) {
    let (w, h) = match size {
        PreviewSize::Row => (PREVIEW_ROW, PREVIEW_ROW),
        PreviewSize::Hero => HERO,
    };
    (device_pixels(w, px_per_dip), device_pixels(h, px_per_dip))
}

/// `dips` at `scale`, rounded up. A hair is taken off first so a product
/// that is whole in exact sums (120 at 1.15) doesn't round up because a
/// float came out a little over.
fn device_pixels(dips: f32, scale: f32) -> u32 {
    (dips * scale - 0.001).ceil().max(1.0) as u32
}

/// The previews to ask for, first to last: the selection's picture for
/// the inspector (`Hero`) and beside its row, then the visible rows from
/// the top, then up to 12 rows below them and up to 12 above, each nearest
/// first. No row is asked for twice at one size; only the selection has
/// a `Hero`.
pub fn request_order(
    selected: Option<usize>,
    visible: Range<usize>,
    total: usize,
) -> Vec<(usize, PreviewSize)> {
    let selected = selected.filter(|&i| i < total);
    let first = visible.start.min(total);
    let end = visible.end.min(total).max(first);
    let mut order = Vec::with_capacity(2 + (end - first) + 2 * AHEAD);
    if let Some(i) = selected {
        order.push((i, PreviewSize::Hero));
        order.push((i, PreviewSize::Row));
    }
    let rows = (first..end)
        .chain(end..end.saturating_add(AHEAD).min(total))
        .chain((first.saturating_sub(AHEAD)..first).rev());
    order.extend(
        rows.filter(|&i| Some(i) != selected)
            .map(|i| (i, PreviewSize::Row)),
    );
    order
}

/// The part of a picture to draw so that it fills `dst` without
/// stretching: the largest rectangle with `dst`'s proportions that fits
/// inside the picture, centred. It is `(x, y, width, height)` in the
/// picture's pixels, each rounded to the nearest half pixel.
pub fn cover_source(img: (u32, u32), dst: (f32, f32)) -> (f32, f32, f32, f32) {
    let (iw, ih) = (img.0 as f32, img.1 as f32);
    let usable = |v: f32| v.is_finite() && v > 0.0;
    if iw < 1.0 || ih < 1.0 || !usable(dst.0) || !usable(dst.1) {
        return (0.0, 0.0, iw, ih);
    }
    let half = |v: f32| (v * 2.0).round() / 2.0;
    let fit = (iw / dst.0).min(ih / dst.1);
    let (w, h) = (dst.0 * fit, dst.1 * fit);
    let (rw, rh) = (half(w).clamp(0.5, iw), half(h).clamp(0.5, ih));
    let x = half((iw - w) / 2.0).clamp(0.0, iw - rw);
    let y = half((ih - h) / 2.0).clamp(0.0, ih - rh);
    (x, y, rw, rh)
}

/// Windows sometimes returns a thumbnail whose every alpha is 0 though
/// its colours are right. When that is so, makes every alpha 255; a
/// bitmap with any transparency of its own is left alone.
pub fn fix_alpha(bgra: &mut [u8]) {
    if bgra.chunks_exact(4).all(|px| px[3] == 0) {
        for px in bgra.chunks_exact_mut(4) {
            px[3] = 255;
        }
    }
}

/// Whether a file of `kind` shows a picture from Windows. Audio never
/// does: it always shows its glaze tile with the waveform, so album art
/// isn't asked for or drawn.
pub fn wants_preview(kind: Option<kiln_core::Kind>) -> bool {
    kind != Some(kiln_core::Kind::Audio)
}

/// Windows hands thumbnails back with straight alpha: a pixel's colour is
/// its full colour whatever its alpha. Direct2D draws premultiplied
/// pixels, so each colour is scaled by its alpha here, once, as the
/// picture arrives. Drawn straight, a translucent edge would glow.
pub fn premultiply(bgra: &mut [u8]) {
    for px in bgra.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a == 255 {
            continue;
        }
        for c in &mut px[..3] {
            *c = ((*c as u32 * a + 127) / 255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn key(name: &str) -> PreviewKey {
        PreviewKey {
            path: PathBuf::from(name),
            modified: None,
            size: PreviewSize::Row,
            pixels: (34, 34),
        }
    }

    /// A preview that counts `bytes` bytes toward the cache's cap.
    fn ready(bytes: usize) -> Preview {
        Preview::Ready(Arc::new(Pixels {
            width: (bytes / 4) as u32,
            height: 1,
            bgra: vec![0; bytes],
        }))
    }

    #[test]
    fn the_cache_holds_its_cap_and_drops_the_least_recently_used() {
        let now = Instant::now();
        let mut cache = PreviewCache::new(300);
        let (a, b, c, d) = (key("a"), key("b"), key("c"), key("d"));
        cache.put(a.clone(), ready(100), now);
        cache.put(b.clone(), ready(100), now);
        cache.put(c.clone(), ready(100), now);
        assert_eq!(cache.bytes(), 300);

        // Reading a makes b the one that has gone longest unused.
        assert!(cache.get(&a, now).is_some());
        cache.put(d.clone(), ready(100), now);

        assert!(cache.get(&b, now).is_none(), "b was least recently used");
        assert!(cache.get(&a, now).is_some());
        assert!(cache.get(&c, now).is_some());
        assert!(cache.get(&d, now).is_some());
        assert_eq!(cache.bytes(), 300);
    }

    #[test]
    fn a_big_arrival_drops_as_many_as_it_needs() {
        let now = Instant::now();
        let mut cache = PreviewCache::new(300);
        for name in ["a", "b", "c"] {
            cache.put(key(name), ready(100), now);
        }
        cache.put(key("big"), ready(250), now);
        assert!(cache.bytes() <= 300);
        assert!(cache.get(&key("big"), now).is_some());
        assert!(cache.get(&key("a"), now).is_none());
        assert!(cache.get(&key("b"), now).is_none());
        assert!(cache.get(&key("c"), now).is_none());
    }

    #[test]
    fn putting_a_key_again_replaces_it_without_counting_it_twice() {
        let now = Instant::now();
        let mut cache = PreviewCache::new(1000);
        cache.put(key("a"), ready(100), now);
        cache.put(key("a"), ready(40), now);
        assert_eq!(cache.bytes(), 40);
    }

    #[test]
    fn a_preview_bigger_than_the_cap_is_kept_alone() {
        let now = Instant::now();
        let mut cache = PreviewCache::new(100);
        cache.put(key("a"), ready(60), now);
        cache.put(key("huge"), ready(500), now);
        assert!(cache.get(&key("a"), now).is_none());
        assert!(cache.get(&key("huge"), now).is_some());
        assert_eq!(cache.bytes(), 500);
    }

    #[test]
    fn the_cache_remembers_when_a_preview_arrived() {
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_secs(5);
        let mut cache = PreviewCache::new(1000);
        cache.put(key("a"), ready(100), t0);
        let (_, arrived) = cache.get(&key("a"), t1).expect("cached");
        assert_eq!(arrived, t0);
    }

    #[test]
    fn no_thumbnail_is_remembered_too_and_counts_a_little() {
        let now = Instant::now();
        let mut cache = PreviewCache::new(1000);
        cache.put(key("a"), Preview::None, now);
        assert_eq!(cache.bytes(), 64);
        assert!(matches!(
            cache.get(&key("a"), now),
            Some((Preview::None, _))
        ));
        assert!(!cache.pending(&key("a")), "no need to ask Windows again");
    }

    #[test]
    fn the_cache_key_carries_the_pixel_size() {
        assert_eq!(pixel_size(PreviewSize::Row, 1.0), (34, 34));
        assert_eq!(pixel_size(PreviewSize::Row, 1.25), (43, 43));
        assert_eq!(pixel_size(PreviewSize::Hero, 2.0), (536, 240));
        assert_eq!(pixel_size(PreviewSize::Hero, 1.0), (268, 120));
        // A scale whose product is a whole number in exact sums stays put.
        assert_eq!(pixel_size(PreviewSize::Hero, 1.2), (322, 144));
        assert_eq!(pixel_size(PreviewSize::Hero, 1.15), (309, 138));

        // Same path and time at two sizes: two keys, two ids.
        let row = PreviewKey {
            size: PreviewSize::Row,
            pixels: pixel_size(PreviewSize::Row, 1.0),
            ..key("a")
        };
        let hero = PreviewKey {
            size: PreviewSize::Hero,
            pixels: pixel_size(PreviewSize::Hero, 1.0),
            ..key("a")
        };
        assert_ne!(row, hero);
        assert_ne!(row.id(), hero.id());
        // The same screen at another scale is another key too.
        let row2 = PreviewKey {
            pixels: pixel_size(PreviewSize::Row, 2.0),
            ..row.clone()
        };
        assert_ne!(row, row2);
        assert_ne!(row.id(), row2.id());
        // And equal keys share one id.
        assert_eq!(row.id(), row.clone().id());
    }

    /// What Windows makes of a picture of `shape`, bigger than asked, for
    /// `ask`: cropped to the square, or fitted inside it with its short
    /// side rounded down.
    fn made(shape: (u32, u32), ask: Ask) -> (u32, u32) {
        if ask.square {
            return (ask.side, ask.side);
        }
        let long = shape.0.max(shape.1);
        (ask.side * shape.0 / long, ask.side * shape.1 / long)
    }

    /// How many device pixels one pixel of a picture `img` covers once it
    /// is cropped to fill a box of `bx` device pixels, as the window draws
    /// it: past 1, it is stretched.
    fn drawn_scale(img: (u32, u32), bx: (f32, f32)) -> f32 {
        let (_, _, w, h) = cover_source(img, bx);
        (bx.0 / w).max(bx.1 / h)
    }

    #[test]
    fn no_picture_is_drawn_larger_than_windows_made_it() {
        // Several's cards, drawn from the inspector's picture in a box a
        // little smaller than its own.
        const CARD: (f32, f32) = (244.0, 104.0);
        for scale in [1.0, 1.15, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0] {
            for size in [PreviewSize::Row, PreviewSize::Hero] {
                let bx = pixel_size(size, scale);
                let a = ask(size, bx);
                for shape in SHAPES {
                    let img = made(shape, a);
                    let s = drawn_scale(img, (bx.0 as f32, bx.1 as f32));
                    assert!(
                        s <= 1.0,
                        "{size:?} at {scale}: {shape:?} made {img:?} for {bx:?}, drawn at {s}"
                    );
                }
            }
            let hero = ask(PreviewSize::Hero, pixel_size(PreviewSize::Hero, scale));
            let card = (CARD.0 * scale, CARD.1 * scale);
            for shape in SHAPES {
                let s = drawn_scale(made(shape, hero), card);
                assert!(s <= 1.0, "a card at {scale}: {shape:?} drawn at {s}");
            }
        }
    }

    #[test]
    fn a_row_is_asked_as_a_square_and_the_hero_big_enough_for_an_upright_video() {
        assert_eq!(
            ask(PreviewSize::Row, (43, 43)),
            Ask {
                side: 43,
                square: true
            }
        );
        // 268 x 120 at 125% is 335 x 150. An upright phone video fitted
        // in a square of 596 is 335 wide, just as wide as the box.
        assert_eq!(
            ask(PreviewSize::Hero, (335, 150)),
            Ask {
                side: 596,
                square: false
            }
        );
        assert_eq!(ask(PreviewSize::Hero, (268, 120)).side, 477);
        // Its biggest picture, a square photo at 200%, for each of
        // Several's three cards, is still a small share of the cap.
        let side = ask(PreviewSize::Hero, pixel_size(PreviewSize::Hero, 2.0)).side as usize;
        assert_eq!(side, 953);
        assert!(3 * side * side * 4 < CACHE_CAP / 4);
    }

    #[test]
    fn a_changed_file_is_asked_for_again() {
        let now = Instant::now();
        let t1 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let t2 = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000);
        let old = PreviewKey {
            modified: Some(t1),
            ..key("a")
        };
        let new = PreviewKey {
            modified: Some(t2),
            ..key("a")
        };
        assert_ne!(old, new);

        let mut cache = PreviewCache::new(1000);
        cache.put(old.clone(), ready(100), now);
        assert!(cache.get(&new, now).is_none());
        assert!(cache.pending(&new));
        // The old picture is still there for the old key.
        assert!(cache.get(&old, now).is_some());
    }

    #[test]
    fn pending_asks_once() {
        let now = Instant::now();
        let mut cache = PreviewCache::new(1000);
        let k = key("a");
        assert!(cache.pending(&k), "the first time: ask the worker");
        assert!(!cache.pending(&k), "it is already on its way");
        assert!(!cache.pending(&k));
        cache.put(k.clone(), ready(100), now);
        assert!(!cache.pending(&k), "it has arrived");
        // Other keys are asked for on their own.
        assert!(cache.pending(&key("b")));
    }

    #[test]
    fn a_dropped_preview_can_be_asked_for_again() {
        let now = Instant::now();
        let mut cache = PreviewCache::new(100);
        assert!(cache.pending(&key("a")));
        cache.put(key("a"), ready(100), now);
        cache.put(key("b"), ready(100), now); // pushes a out
        assert!(cache.get(&key("a"), now).is_none());
        assert!(cache.pending(&key("a")));
    }

    #[test]
    fn clearing_the_pending_asks_again_but_keeps_what_arrived() {
        let now = Instant::now();
        let mut cache = PreviewCache::new(1000);
        let (waiting, held) = (key("waiting"), key("held"));
        cache.put(held.clone(), ready(100), now);
        assert!(cache.pending(&waiting));
        assert!(!cache.pending(&waiting));

        cache.clear_pending();

        assert!(cache.pending(&waiting), "the worker dropped it: ask again");
        assert!(!cache.pending(&held), "a held preview is never asked for");
        assert!(cache.get(&held, now).is_some());
        assert_eq!(cache.bytes(), 100);
    }

    #[test]
    fn five_thousand_previews_still_drop_the_least_recently_used() {
        // 5,000 four-byte previews fill a cap of 20,000 bytes exactly.
        let now = Instant::now();
        let mut cache = PreviewCache::new(5_000 * 4);
        for i in 0..5_000 {
            cache.put(key(&format!("k{i}")), ready(4), now);
        }
        assert_eq!(cache.bytes(), 20_000);

        // The oldest is read, so the second-oldest is now the one to go.
        assert!(cache.get(&key("k0"), now).is_some());
        cache.put(key("one more"), ready(4), now);

        assert!(cache.get(&key("k0"), now).is_some(), "k0 was just used");
        assert!(cache.get(&key("k1"), now).is_none(), "k1 was the oldest");
        assert!(cache.get(&key("k2"), now).is_some());
        assert!(cache.get(&key("one more"), now).is_some());
        assert_eq!(cache.bytes(), 20_000);
    }

    #[test]
    fn a_preview_for_a_file_no_longer_listed_is_dropped() {
        let now = Instant::now();
        let mut cache = PreviewCache::new(1000);
        let (a, b, waiting) = (key("a"), key("b"), key("waiting"));
        cache.put(a.clone(), ready(100), now);
        cache.put(b.clone(), ready(200), now);
        assert!(cache.pending(&waiting));

        cache.keep_paths(&|p| p == a.path);

        assert!(cache.get(&b, now).is_none());
        assert!(cache.get(&a, now).is_some());
        assert_eq!(cache.bytes(), 100);
        assert!(cache.pending(&waiting), "its pending mark went with it");
    }

    #[test]
    fn keeping_every_path_changes_nothing() {
        let now = Instant::now();
        let mut cache = PreviewCache::new(1000);
        cache.put(key("a"), ready(100), now);
        assert!(cache.pending(&key("b")));
        cache.keep_paths(&|_| true);
        assert_eq!(cache.bytes(), 100);
        assert!(cache.get(&key("a"), now).is_some());
        assert!(!cache.pending(&key("b")));
    }

    #[test]
    fn request_order_puts_the_selection_first_and_drops_stale_heroes() {
        let o = request_order(Some(250), 240..252, 500);
        assert_eq!(o[0], (250, PreviewSize::Hero));
        assert_eq!(o[1], (250, PreviewSize::Row));
        // The other eleven visible rows, top down.
        assert!(o[2..13]
            .iter()
            .all(|(i, s)| (240..252).contains(i) && *s == PreviewSize::Row));
        assert_eq!(
            o[2..13].iter().map(|(i, _)| *i).collect::<Vec<_>>(),
            [240, 241, 242, 243, 244, 245, 246, 247, 248, 249, 251]
        );
        // Then twelve below the visible rows, then twelve above.
        assert_eq!(o[13], (252, PreviewSize::Row));
        assert_eq!(o[24], (263, PreviewSize::Row));
        assert_eq!(o[25], (239, PreviewSize::Row));
        assert_eq!(o[36], (228, PreviewSize::Row));
        assert_eq!(o.len(), 37);
        assert_eq!(o.iter().filter(|(_, s)| *s == PreviewSize::Hero).count(), 1);
        assert!(o.len() <= 2 + 12 + 24);
        // No row twice.
        let mut rows: Vec<usize> = o.iter().map(|(i, _)| *i).collect();
        rows.sort_unstable();
        rows.dedup();
        assert_eq!(
            rows.len(),
            o.len() - 1,
            "only the selection is listed twice"
        );
    }

    #[test]
    fn request_order_without_a_selection_has_no_hero() {
        let o = request_order(None, 0..3, 100);
        assert_eq!(
            &o[..3],
            [
                (0, PreviewSize::Row),
                (1, PreviewSize::Row),
                (2, PreviewSize::Row)
            ]
        );
        assert_eq!(o[3], (3, PreviewSize::Row));
        assert!(o.iter().all(|(_, s)| *s == PreviewSize::Row));
        // Nothing above the first row, twelve below.
        assert_eq!(o.len(), 3 + 12);
    }

    #[test]
    fn request_order_stays_inside_the_list() {
        // A short list: no row past the end, none before the start.
        let o = request_order(None, 0..4, 6);
        assert_eq!(
            o.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
            [0, 1, 2, 3, 4, 5]
        );
        // The visible range may run past the end while the list shrinks.
        let o = request_order(None, 3..9, 5);
        assert_eq!(
            o.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
            [3, 4, 2, 1, 0]
        );
        // A selection that is gone asks for nothing of its own.
        let o = request_order(Some(9), 0..2, 5);
        assert!(o.iter().all(|(i, s)| *i < 5 && *s == PreviewSize::Row));
        // An empty list asks for nothing.
        assert!(request_order(Some(0), 0..0, 0).is_empty());
    }

    #[test]
    fn request_order_does_not_repeat_a_selection_below_the_window() {
        // Selected row 5 is just under the visible rows 0..3.
        let o = request_order(Some(5), 0..3, 100);
        assert_eq!(o[0], (5, PreviewSize::Hero));
        assert_eq!(o[1], (5, PreviewSize::Row));
        assert_eq!(o.iter().filter(|(i, _)| *i == 5).count(), 2);
    }

    #[test]
    fn cover_source_crops_the_centre() {
        assert_eq!(
            cover_source((400, 300), (268.0, 120.0)),
            (0.0, 60.5, 400.0, 179.0)
        ); // rounded to 0.5
        assert_eq!(cover_source((34, 34), (34.0, 34.0)), (0.0, 0.0, 34.0, 34.0));
    }

    #[test]
    fn cover_source_crops_the_sides_of_a_tall_picture() {
        // 300 wide, 600 tall into a square: the middle 300 x 300.
        assert_eq!(
            cover_source((300, 600), (34.0, 34.0)),
            (0.0, 150.0, 300.0, 300.0)
        );
        // A wide picture into a square keeps the middle of its width.
        assert_eq!(
            cover_source((600, 300), (34.0, 34.0)),
            (150.0, 0.0, 300.0, 300.0)
        );
    }

    #[test]
    fn cover_source_never_leaves_the_picture() {
        for (img, dst) in [
            ((401, 299), (268.0, 120.0)),
            ((1, 1), (268.0, 120.0)),
            ((7, 1000), (34.0, 34.0)),
            ((536, 240), (268.0, 120.0)),
        ] {
            let (x, y, w, h) = cover_source(img, dst);
            assert!(x >= 0.0 && y >= 0.0, "{img:?}");
            assert!(w > 0.0 && h > 0.0, "{img:?}");
            assert!(x + w <= img.0 as f32 && y + h <= img.1 as f32, "{img:?}");
        }
        // A destination with no area shows the whole picture.
        assert_eq!(cover_source((40, 30), (0.0, 10.0)), (0.0, 0.0, 40.0, 30.0));
    }

    #[test]
    fn an_all_clear_bitmap_is_made_opaque() {
        let mut px = vec![10, 20, 30, 0, 40, 50, 60, 0];
        fix_alpha(&mut px);
        assert_eq!(px, [10, 20, 30, 255, 40, 50, 60, 255]);
        let mut keep = vec![10, 20, 30, 0, 40, 50, 60, 128];
        fix_alpha(&mut keep);
        assert_eq!(keep[3], 0);
        assert_eq!(keep, [10, 20, 30, 0, 40, 50, 60, 128]);
        // Nothing to fix is not an error.
        let mut none: Vec<u8> = Vec::new();
        fix_alpha(&mut none);
        assert!(none.is_empty());
    }

    #[test]
    fn audio_keeps_its_tile_and_every_other_kind_asks_for_a_picture() {
        use kiln_core::Kind;
        assert!(!wants_preview(Some(Kind::Audio)));
        for kind in [
            Kind::Image,
            Kind::Video,
            Kind::Document,
            Kind::Spreadsheet,
            Kind::Presentation,
            Kind::Subtitle,
        ] {
            assert!(wants_preview(Some(kind)), "{kind:?}");
        }
        // A type Kiln has no kind for may still have a Windows thumbnail.
        assert!(wants_preview(None));
    }

    #[test]
    fn premultiplying_scales_each_colour_by_its_alpha() {
        let mut px = vec![
            219, 231, 239, 64, // bone at a quarter
            200, 100, 50, 255, // opaque: unchanged
            90, 80, 70, 0, // clear: no colour left
            255, 255, 255, 128, // half
        ];
        premultiply(&mut px);
        assert_eq!(
            px,
            [55, 58, 60, 64, 200, 100, 50, 255, 0, 0, 0, 0, 128, 128, 128, 128]
        );
        // No colour is ever brighter than its alpha allows.
        assert!(px
            .chunks_exact(4)
            .all(|p| p[..3].iter().all(|&c| c <= p[3])));
    }
}
