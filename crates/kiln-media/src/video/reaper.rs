//! Holding a shut-down hardware encoder's last references until it is safe
//! to let them go (R42).
//!
//! NVIDIA's HEVC transform (nvEncMFThevcx.dll) runs a work item on an RTWorkQ
//! thread after its shutdown, which enters a lock inside the transform; if
//! the last reference has gone by then, that lock is in freed memory and the
//! process comes down. How long the work item can lag is not documented and
//! grows under load, so no fixed wait is safe on its own.
//!
//! So one long-lived thread, the reaper, holds every such encoder. Each is
//! let go once it is both settled (its `IMFShutdown` says the shutdown has
//! completed, or it has none to ask) and at least [`Rules::floor`] old; one
//! that never says so is let go at [`Rules::cap`]. Settled says little on
//! its own: many asynchronous transforms report `MFSHUTDOWN_COMPLETED` as
//! soon as `Shutdown()` returns, with their own work still to run, so the
//! floor is the real protection. The decision is
//! [`Graveyard`], a plain structure kept apart from Media Foundation so it is
//! tested without hardware. The thread is never joined: at process exit
//! whatever it still holds ends with the process, never released.

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// When a held encoder may be let go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Rules {
    /// The least time one is held, settled or not: the real protection,
    /// since many transforms say they are settled at once.
    pub floor: Duration,
    /// The most time one is held: past this it is let go even if it never
    /// said its shutdown completed, so a long run in the tray doesn't keep
    /// every encoder it ever used.
    pub cap: Duration,
}

/// The rules Kiln uses: held at least 30 seconds, at most 10 minutes.
pub(crate) const RULES: Rules = Rules {
    floor: Duration::from_secs(30),
    cap: Duration::from_secs(600),
};

/// How often the reaper looks at what it holds while it holds anything.
pub(crate) const TICK: Duration = Duration::from_secs(1);

/// True when one held for `age` may be let go. `settled` is only asked once
/// the floor has passed, so nothing is asked of a transform sooner than it
/// need be.
pub(crate) fn due(age: Duration, rules: Rules, settled: impl FnOnce() -> bool) -> bool {
    if age >= rules.cap {
        return true;
    }
    age >= rules.floor && settled()
}

/// What the reaper holds, each with when it came.
pub(crate) struct Graveyard<T> {
    rules: Rules,
    held: Vec<(T, Instant)>,
}

impl<T> Graveyard<T> {
    pub(crate) fn new(rules: Rules) -> Graveyard<T> {
        Graveyard {
            rules,
            held: Vec::new(),
        }
    }

    /// Holds `item` from `now`.
    pub(crate) fn bury(&mut self, item: T, now: Instant) {
        self.held.push((item, now));
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.held.len()
    }

    /// Takes out, oldest first, everything [`due`] at `now`; the caller lets
    /// go of what comes back.
    pub(crate) fn take_due(&mut self, now: Instant, mut settled: impl FnMut(&T) -> bool) -> Vec<T> {
        let rules = self.rules;
        let mut out = Vec::new();
        let mut kept = Vec::with_capacity(self.held.len());
        for (item, since) in self.held.drain(..) {
            if due(now.saturating_duration_since(since), rules, || {
                settled(&item)
            }) {
                out.push(item);
            } else {
                kept.push((item, since));
            }
        }
        self.held = kept;
        out
    }
}

/// The reaper: a thread of its own with a [`Graveyard`]. Built once and kept
/// for the life of the process.
pub(crate) struct Reaper<T: Send + 'static> {
    to: Mutex<Sender<T>>,
}

impl<T: Send + 'static> Reaper<T> {
    /// Starts the reaper's thread, `None` if none can be started. `settled`
    /// is asked on that thread; `tick` is how often it looks while holding
    /// anything.
    pub(crate) fn start(
        name: &str,
        rules: Rules,
        tick: Duration,
        setup: impl FnOnce() + Send + 'static,
        settled: impl Fn(&T) -> bool + Send + 'static,
    ) -> Option<Reaper<T>> {
        let (to, from) = mpsc::channel::<T>();
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                setup();
                let mut yard = Graveyard::new(rules);
                loop {
                    let came = if yard.is_empty() {
                        match from.recv() {
                            Ok(t) => Some(t),
                            // Nothing held and nothing can come: done.
                            Err(_) => return,
                        }
                    } else {
                        match from.recv_timeout(tick) {
                            Ok(t) => Some(t),
                            Err(RecvTimeoutError::Timeout) => None,
                            Err(RecvTimeoutError::Disconnected) => {
                                std::thread::sleep(tick);
                                None
                            }
                        }
                    };
                    let now = Instant::now();
                    if let Some(t) = came {
                        yard.bury(t, now);
                    }
                    // Let go oldest first, each in turn.
                    for t in yard.take_due(now, &settled) {
                        drop(t);
                    }
                }
            })
            .ok()?;
        Some(Reaper { to: Mutex::new(to) })
    }

    /// Hands `item` to the reaper. If the reaper has stopped, `item` comes
    /// back for the caller to keep.
    pub(crate) fn bury(&self, item: T) -> Result<(), T> {
        let to = self.to.lock().unwrap_or_else(|p| p.into_inner());
        to.send(item).map_err(|e| e.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    const R: Rules = Rules {
        floor: Duration::from_secs(30),
        cap: Duration::from_secs(600),
    };

    #[test]
    fn kiln_holds_an_encoder_for_tens_of_seconds_at_least_and_minutes_at_most() {
        assert!(RULES.floor >= Duration::from_secs(10), "{RULES:?}");
        assert!(RULES.cap > RULES.floor, "{RULES:?}");
        assert!(RULES.cap <= Duration::from_secs(3600), "{RULES:?}");
    }

    #[test]
    fn one_settled_is_not_let_go_before_the_floor() {
        assert!(!due(Duration::ZERO, R, || true));
        assert!(!due(Duration::from_millis(500), R, || true));
        assert!(!due(Duration::from_secs(29), R, || true));
        assert!(due(Duration::from_secs(30), R, || true));
    }

    #[test]
    fn one_not_settled_is_held_until_the_cap() {
        assert!(!due(Duration::from_secs(30), R, || false));
        assert!(!due(Duration::from_secs(599), R, || false));
        assert!(due(Duration::from_secs(600), R, || false));
    }

    #[test]
    fn settled_is_not_asked_before_the_floor_nor_at_the_cap() {
        let asked = AtomicBool::new(false);
        let ask = || {
            asked.store(true, Ordering::SeqCst);
            true
        };
        assert!(!due(Duration::from_secs(5), R, ask));
        assert!(!asked.load(Ordering::SeqCst));
        assert!(due(Duration::from_secs(700), R, || panic!(
            "asked at the cap"
        )));
    }

    #[test]
    fn the_graveyard_gives_back_only_what_is_due_and_keeps_the_rest() {
        let t0 = Instant::now();
        let mut y = Graveyard::new(R);
        y.bury(("old settled", true), t0);
        y.bury(("old unsettled", false), t0);
        y.bury(("new settled", true), t0 + Duration::from_secs(20));
        let out = y.take_due(t0 + Duration::from_secs(31), |(_, s)| *s);
        assert_eq!(out, vec![("old settled", true)]);
        assert_eq!(y.len(), 2);
        let out = y.take_due(t0 + Duration::from_secs(51), |(_, s)| *s);
        assert_eq!(out, vec![("new settled", true)]);
        let out = y.take_due(t0 + Duration::from_secs(600), |(_, s)| *s);
        assert_eq!(out, vec![("old unsettled", false)]);
        assert!(y.is_empty());
    }

    #[test]
    fn a_clock_that_seems_to_go_back_holds_rather_than_lets_go() {
        let t0 = Instant::now() + Duration::from_secs(100);
        let mut y = Graveyard::new(R);
        y.bury(1, t0);
        assert!(y
            .take_due(t0 - Duration::from_secs(50), |_| true)
            .is_empty());
        assert_eq!(y.len(), 1);
    }

    /// Counts its drops.
    struct Body(Arc<AtomicUsize>, bool);
    impl Drop for Body {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn one_reaper_thread_lets_go_of_each_once_settled_and_past_the_floor() {
        // A floor ten times the look before it, so a loaded machine that
        // runs late still looks before the floor.
        let rules = Rules {
            floor: Duration::from_secs(1),
            cap: Duration::from_secs(60),
        };
        let set_up = Arc::new(AtomicBool::new(false));
        let s = Arc::clone(&set_up);
        let r = Reaper::start(
            "test-reaper",
            rules,
            Duration::from_millis(20),
            move || s.store(true, Ordering::SeqCst),
            |b: &Body| b.1,
        )
        .expect("a thread");
        let dropped = Arc::new(AtomicUsize::new(0));
        for settled in [true, true, false] {
            assert!(r.bury(Body(Arc::clone(&dropped), settled)).is_ok());
        }
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(dropped.load(Ordering::SeqCst), 0, "before the floor");
        let until = Instant::now() + Duration::from_secs(10);
        while !set_up.load(Ordering::SeqCst) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(set_up.load(Ordering::SeqCst));
        let until = Instant::now() + Duration::from_secs(10);
        while dropped.load(Ordering::SeqCst) < 2 && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(dropped.load(Ordering::SeqCst), 2, "the settled two");
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(
            dropped.load(Ordering::SeqCst),
            2,
            "the unsettled one waits for the cap"
        );
    }

    #[test]
    fn a_reaper_lets_go_of_one_never_settled_at_the_cap() {
        let rules = Rules {
            floor: Duration::from_millis(20),
            cap: Duration::from_millis(150),
        };
        let r = Reaper::start(
            "test-reaper-cap",
            rules,
            Duration::from_millis(10),
            || {},
            |_: &Body| false,
        )
        .expect("a thread");
        let dropped = Arc::new(AtomicUsize::new(0));
        assert!(r.bury(Body(Arc::clone(&dropped), false)).is_ok());
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(dropped.load(Ordering::SeqCst), 0);
        let until = Instant::now() + Duration::from_secs(5);
        while dropped.load(Ordering::SeqCst) < 1 && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }
}
