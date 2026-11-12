//! The one way worker threads, UI Automation and the floating wheel reach
//! the window: a typed [`Event`] on a channel, and one payload-free
//! [`WM_KILN_WAKE`] that tells the window to read it.
//!
//! - **Sending** ([`Wake::send`]) pushes the event, then posts
//!   `WM_KILN_WAKE` unless one is already pending: the flag is set before
//!   the post and cleared by the window before it reads ([`Inbox::woken`]),
//!   so many sends make one wake, and none is missed.
//! - **No pointers in messages.** The wake carries nothing: its `wparam`
//!   and `lparam` are never read, so a post of it from anywhere only makes
//!   the window look at its own queue.
//! - **Shutdown.** Events still queued when the [`Inbox`] goes are dropped
//!   with it, which runs their destructors. A send once it has gone fails,
//!   and the value is dropped on the sender's side.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::a11y;
use crate::previews::{Preview, PreviewKey};
use crate::update::Reply;

use super::float_wheel::FloatNote;
use super::held::Opened;
use super::workers::{EnginesFound, GoneFound};
use super::{
    CardsFound, Copying, Exported, ExtensionsLooked, Inspected, JobNews, MfLooked, Probed, Walked,
};

/// Posted to the window when events wait on its channel. Its `wparam` and
/// `lparam` are never read. A number no message of Kiln's had before, so
/// nothing posted for an older Kiln is taken for it.
pub(crate) const WM_KILN_WAKE: u32 = WM_APP + 21;

/// What reaches the window from elsewhere, one variant per kind of news.
pub(crate) enum Event {
    /// A job's news from the runner's thread.
    Job(JobNews),
    /// The update check was answered.
    Update(Reply),
    /// The options sheet's files have been read.
    Probed(Probed),
    /// The video sheet's graphics card encoders have been looked for.
    Cards(CardsFound),
    /// The decoders of the Store extensions rows wait on were looked for.
    Extensions(ExtensionsLooked),
    /// The inspect thread has looked inside a listed file.
    Inspected(Inspected),
    /// An export thread has copied part of a held file to another drive.
    Copying(Copying),
    /// An export thread has finished.
    Exported(Exported),
    /// A worker has opened the holding folder for a drive's results.
    HeldOpened(Opened),
    /// The start's look for what crashed, killed or older runs left, and
    /// its sweep, are done; the delivery of their results goes on.
    Swept,
    /// Every result earlier runs left has been tried: what became of them.
    Delivered(kiln_core::Report),
    /// Windows' Text size changed; the window reads the new factor itself.
    TextScale,
    /// The thumbnail worker made a preview, or found there is none.
    Preview(PreviewKey, Preview),
    /// Media Foundation's codecs have been looked for again.
    Mf(MfLooked),
    /// The engines thread has looked for the helper apps.
    Engines(EnginesFound),
    /// The held results removed by hand have been looked for.
    Gone(GoneFound),
    /// A walk of dropped or pasted folders has more to add.
    Walked(Walked),
    /// The window has more of a walk's files to add, after a pause for
    /// its other messages: the walk's id.
    WalkOn(u64),
    /// The settings writer has written, or failed to.
    SettingsWritten,
    /// The floating wheel's news.
    Float(FloatNote),
    /// A UI Automation client asked an element to do something.
    A11y(a11y::Request),
    /// The Make smaller dialog's estimate was worked out: the dialog it
    /// was for (`App::tool_dialog`), the options, and what was found.
    Estimated(u64, kiln_core::ToolOptions, crate::smaller::Estimate),
}

// Every event crosses from another thread.
const _: () = {
    const fn is_send<T: Send>() {}
    is_send::<Event>();
    is_send::<Wake>();
};

/// A way to the window, for any thread: clone it for each sender.
pub(crate) struct Wake<E = Event> {
    tx: Sender<E>,
    /// The window, as a number: it is only ever handed to `PostMessageW`.
    hwnd: usize,
    /// Set from a send until the window takes the wake.
    pending: Arc<AtomicBool>,
}

// Not derived: that would ask `E: Clone`.
impl<E> Clone for Wake<E> {
    fn clone(&self) -> Self {
        Wake {
            tx: self.tx.clone(),
            hwnd: self.hwnd,
            pending: self.pending.clone(),
        }
    }
}

/// The window's end: it lives in `App`, and goes with it.
pub(crate) struct Inbox<E = Event> {
    rx: Receiver<E>,
    pending: Arc<AtomicBool>,
}

/// A channel to the window `hwnd`.
pub(crate) fn channel<E>(hwnd: HWND) -> (Wake<E>, Inbox<E>) {
    let (tx, rx) = mpsc::channel();
    let pending = Arc::new(AtomicBool::new(false));
    let wake = Wake {
        tx,
        hwnd: hwnd.0 as usize,
        pending: pending.clone(),
    };
    (wake, Inbox { rx, pending })
}

impl<E> Wake<E> {
    /// Queue `event` for the window and wake it. False when the window
    /// won't hear it: the inbox has gone, and `event` with it, or the wake
    /// couldn't be posted, and `event` waits for the next one.
    pub fn send(&self, event: E) -> bool {
        // On failure the error holds the event, and drops it here.
        if self.tx.send(event).is_err() {
            return false;
        }
        self.ring()
    }

    /// Post the window a wake, unless one is pending already. False when
    /// it couldn't be posted.
    pub fn ring(&self) -> bool {
        if self.pending.swap(true, Ordering::SeqCst) {
            return true;
        }
        let hwnd = HWND(self.hwnd as *mut core::ffi::c_void);
        // SAFETY: `PostMessageW` takes any window handle; one that is no
        // longer a window makes it fail. The message carries no pointer.
        let posted = unsafe { PostMessageW(Some(hwnd), WM_KILN_WAKE, WPARAM(0), LPARAM(0)) };
        if posted.is_err() {
            // Not posted (the window is gone, or its queue is full): the
            // next send tries again rather than waiting on a wake that
            // never comes.
            self.pending.store(false, Ordering::SeqCst);
            return false;
        }
        true
    }
}

impl<E> Inbox<E> {
    /// For a `WM_KILN_WAKE`: the wake is taken, so a send from now on
    /// posts another, then every event queued, oldest first.
    pub fn woken(&self) -> Vec<E> {
        self.pending.store(false, Ordering::SeqCst);
        self.queued()
    }

    /// Every event queued now, oldest first, leaving the wake as it is.
    pub fn queued(&self) -> Vec<E> {
        self.rx.try_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    use windows::core::w;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, PeekMessageW, HWND_MESSAGE, MSG, PM_REMOVE,
        WINDOW_EX_STYLE, WINDOW_STYLE,
    };

    /// A message-only window on this test's thread, to post wakes to.
    struct Window(HWND);

    impl Window {
        fn new() -> Window {
            // SAFETY: a plain message-only window of a system class, made
            // and destroyed on this thread.
            let hwnd = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!("kiln-wake-test"),
                    WINDOW_STYLE::default(),
                    0,
                    0,
                    0,
                    0,
                    Some(HWND_MESSAGE),
                    None,
                    None,
                    None,
                )
            }
            .expect("a message-only window");
            Window(hwnd)
        }

        /// How many wakes are queued for it, taken off the queue.
        fn wakes(&self) -> usize {
            let mut msg = MSG::default();
            let mut n = 0;
            // SAFETY: `msg` is ours to write; nothing is dispatched.
            while unsafe {
                PeekMessageW(
                    &mut msg,
                    Some(self.0),
                    WM_KILN_WAKE,
                    WM_KILN_WAKE,
                    PM_REMOVE,
                )
            }
            .as_bool()
            {
                n += 1;
            }
            n
        }
    }

    impl Drop for Window {
        fn drop(&mut self) {
            // SAFETY: the window is this thread's own.
            let _ = unsafe { DestroyWindow(self.0) };
        }
    }

    /// A payload that counts its drops.
    struct Counted(Arc<AtomicUsize>);

    impl Drop for Counted {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn many_sends_post_one_wake_until_it_is_taken() {
        let window = Window::new();
        let (wake, inbox) = channel::<u32>(window.0);
        for n in 0..100 {
            assert!(wake.send(n));
        }
        assert_eq!(window.wakes(), 1, "one wake for all of them");
        assert_eq!(inbox.woken(), (0..100).collect::<Vec<_>>(), "in order");
        // Taken: the next send wakes the window again, once.
        assert!(wake.clone().send(100));
        assert!(wake.send(101));
        assert_eq!(window.wakes(), 1);
        assert_eq!(inbox.woken(), vec![100, 101]);
        assert_eq!(window.wakes(), 0);
    }

    #[test]
    fn reading_without_taking_the_wake_posts_none() {
        let window = Window::new();
        let (wake, inbox) = channel::<u32>(window.0);
        assert!(wake.send(1));
        assert_eq!(inbox.queued(), vec![1]);
        // The wake is still pending: a send posts no second one.
        assert!(wake.send(2));
        assert_eq!(window.wakes(), 1);
        assert_eq!(inbox.woken(), vec![2]);
    }

    #[test]
    fn sends_from_other_threads_all_arrive() {
        let window = Window::new();
        let (wake, inbox) = channel::<usize>(window.0);
        let senders: Vec<_> = (0..4)
            .map(|t| {
                let wake = wake.clone();
                std::thread::spawn(move || {
                    for n in 0..250 {
                        assert!(wake.send(t * 1000 + n));
                    }
                })
            })
            .collect();
        for s in senders {
            s.join().unwrap();
        }
        assert_eq!(window.wakes(), 1);
        let mut got = inbox.woken();
        got.sort_unstable();
        let mut want: Vec<usize> = (0..4)
            .flat_map(|t| (0..250).map(move |n| t * 1000 + n))
            .collect();
        want.sort_unstable();
        assert_eq!(got, want);
    }

    #[test]
    fn dropping_the_inbox_frees_what_was_queued() {
        let window = Window::new();
        let drops = Arc::new(AtomicUsize::new(0));
        let (wake, inbox) = channel::<Counted>(window.0);
        for _ in 0..5 {
            assert!(wake.send(Counted(drops.clone())));
        }
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        drop(inbox);
        assert_eq!(drops.load(Ordering::SeqCst), 5, "each queued event dropped");
    }

    #[test]
    fn a_send_once_the_inbox_has_gone_drops_the_value() {
        let window = Window::new();
        let drops = Arc::new(AtomicUsize::new(0));
        let (wake, inbox) = channel::<Counted>(window.0);
        drop(inbox);
        assert!(
            !wake.send(Counted(drops.clone())),
            "the window won't hear it"
        );
        assert_eq!(drops.load(Ordering::SeqCst), 1, "dropped by the sender");
        assert_eq!(window.wakes(), 0, "and no wake posted");
    }

    #[test]
    fn a_wake_that_cannot_be_posted_is_tried_again() {
        let window = Window::new();
        let gone = window.0;
        drop(window);
        let (wake, inbox) = channel::<u32>(gone);
        assert!(!wake.send(1), "no window to wake");
        // Not left pending: the next send tries to post again.
        assert!(!inbox.pending.load(Ordering::SeqCst));
        assert!(!wake.send(2));
        assert_eq!(inbox.queued(), vec![1, 2]);
    }
}
