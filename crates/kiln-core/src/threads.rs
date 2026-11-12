//! Threads that parse user files.
//!
//! A hostile file can nest deeper than the default 2 MiB thread stack holds,
//! and a stack overflow ends the whole process. Every thread that parses
//! user files starts here, with a stack large enough for any file Kiln
//! reads in good faith.

use std::io;
use std::thread::{Builder, JoinHandle};

/// The stack of a thread that parses user files, 64 MiB. It is reserved
/// address space, not memory in use.
pub const PARSER_STACK: usize = 64 << 20;

/// Runs `f` on a new thread called `name`, with a [`PARSER_STACK`] stack.
pub fn spawn_parser<F, T>(name: &str, f: F) -> io::Result<JoinHandle<T>>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    Builder::new()
        .name(name.into())
        .stack_size(PARSER_STACK)
        .spawn(f)
}

/// [`spawn_parser`] for a pool thread: it runs below normal priority from
/// its first instruction, so Kiln's work yields to the rest of the PC.
pub fn spawn_worker<F, T>(name: &str, f: F) -> io::Result<JoinHandle<T>>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    spawn_parser(name, move || {
        crate::process::lower_thread_priority();
        f()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recurses `target` levels, each holding a 4 KiB local that stays live
    /// across the call, and returns the depth reached.
    fn dive(depth: usize, target: usize) -> usize {
        let pad = [0u8; 4096];
        std::hint::black_box(&pad);
        if depth == target {
            return depth;
        }
        let reached = dive(depth + 1, target);
        std::hint::black_box(&pad);
        reached
    }

    #[test]
    fn a_parser_thread_has_room_for_deep_recursion() {
        // 4,096 levels of 4 KiB is 16 MiB, eight times a default thread's stack.
        let handle = spawn_parser("t", || dive(0, 4096)).unwrap();
        assert_eq!(handle.join().unwrap(), 4096);
    }

    #[cfg(windows)]
    #[test]
    fn a_worker_thread_runs_below_normal() {
        use windows_sys::Win32::System::Threading::{
            GetCurrentThread, GetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
        };
        let priority = |_: ()| {
            // SAFETY: GetCurrentThread returns a pseudo-handle that needs no
            // closing, and the call only reads this thread's priority.
            unsafe { GetThreadPriority(GetCurrentThread()) }
        };
        let handle = spawn_worker("kiln-test", move || priority(())).unwrap();
        assert_eq!(handle.join().unwrap(), THREAD_PRIORITY_BELOW_NORMAL);
    }

    #[test]
    fn a_parser_thread_has_its_name() {
        let handle = spawn_parser("kiln-test", || {
            std::thread::current().name().map(str::to_owned)
        })
        .unwrap();
        assert_eq!(handle.join().unwrap().as_deref(), Some("kiln-test"));
    }
}
