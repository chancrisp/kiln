//! A global allocator that counts what each thread holds, shared by the tests
//! that watch a reader's memory (`tests/fuzz_lite.rs`, `tests/fuzz_regressions.rs`).
//! Not part of the cargo-fuzz targets, which run under the sanitizer's allocator.

#![allow(dead_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

/// The most one input may hold at once. The fixtures are under 200 KB, so a
/// reader past this has believed a length field.
pub const MEMORY_LIMIT: isize = 256 << 20;

thread_local! {
    static LIVE: Cell<isize> = const { Cell::new(0) };
    static PEAK: Cell<isize> = const { Cell::new(0) };
}

/// The system allocator, counting what each thread holds.
struct Counting;

fn note(delta: isize) {
    let _ = LIVE.try_with(|live| {
        let now = live.get() + delta;
        live.set(now);
        let _ = PEAK.try_with(|peak| peak.set(peak.get().max(now)));
    });
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = System.alloc(layout);
        if !p.is_null() {
            note(layout.size() as isize);
            if layout.size() as isize > MEMORY_LIMIT / 4 {
                trace_big(layout.size());
            }
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = System.alloc_zeroed(layout);
        if !p.is_null() {
            note(layout.size() as isize);
            if layout.size() as isize > MEMORY_LIMIT / 4 {
                trace_big(layout.size());
            }
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
        note(-(layout.size() as isize));
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let p = System.realloc(ptr, layout, new_size);
        if !p.is_null() {
            note(new_size as isize - layout.size() as isize);
            if new_size as isize > MEMORY_LIMIT / 4 {
                trace_big(new_size);
            }
        }
        p
    }
}

/// With `KILN_FUZZ_ALLOCTRACE` set, says who asked for a big block.
fn trace_big(size: usize) {
    thread_local! {
        static BUSY: Cell<bool> = const { Cell::new(false) };
    }
    if std::env::var_os("KILN_FUZZ_ALLOCTRACE").is_none() || BUSY.with(|b| b.replace(true)) {
        return;
    }
    eprintln!(
        "big allocation: {size} bytes
{}",
        std::backtrace::Backtrace::force_capture()
    );
    BUSY.with(|b| b.set(false));
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// The most this thread has held at once, in bytes.
pub fn peak() -> isize {
    PEAK.with(|p| p.get())
}

/// What this thread holds now, in bytes.
pub fn live() -> isize {
    LIVE.with(|l| l.get())
}
