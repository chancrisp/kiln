//! One processor budget for the jobs that share the machine.
//!
//! The budget is the logical processor count. A job weighs what it takes
//! from the machine: a video encoded on the processor takes all of it,
//! LibreOffice two, anything else one. A job starts while the running
//! weights plus its own fit; one heavier than the budget starts only when
//! nothing else runs, so it never starves.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use crate::converter::CancelToken;

/// The weight of a job that takes the whole budget.
pub(crate) const WHOLE: usize = usize::MAX;

/// What is running against a capacity. Pure: no waiting, no threads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Budget {
    capacity: usize,
    running: usize,
}

impl Budget {
    pub(crate) fn new(capacity: usize) -> Self {
        Budget {
            capacity: capacity.max(1),
            running: 0,
        }
    }

    /// A weight as the budget counts it: no more than the capacity.
    fn weigh(&self, weight: usize) -> usize {
        weight.clamp(1, self.capacity)
    }

    /// Starts a job of `weight` if it fits, or if nothing runs.
    pub(crate) fn try_admit(&mut self, weight: usize) -> bool {
        let weight = self.weigh(weight);
        if self.running == 0 || self.running + weight <= self.capacity {
            self.running += weight;
            true
        } else {
            false
        }
    }

    /// Ends a job admitted with `weight`.
    pub(crate) fn release(&mut self, weight: usize) {
        self.running = self.running.saturating_sub(self.weigh(weight));
    }
}

/// A [`Budget`] that jobs wait on, one after another in the order they
/// asked, so a heavy job is never passed by light ones behind it.
pub(crate) struct Gate {
    state: Mutex<State>,
    changed: Condvar,
}

struct State {
    budget: Budget,
    line: VecDeque<u64>,
    next: u64,
}

impl Gate {
    pub(crate) fn new(capacity: usize) -> Self {
        Gate {
            state: Mutex::new(State {
                budget: Budget::new(capacity),
                line: VecDeque::new(),
                next: 0,
            }),
            changed: Condvar::new(),
        }
    }

    /// Waits for room for `weight`. `false` when `cancel` was set first,
    /// and then nothing is held.
    pub(crate) fn admit(&self, weight: usize, cancel: &CancelToken) -> bool {
        let mut s = self.state.lock().unwrap();
        let ticket = s.next;
        s.next += 1;
        s.line.push_back(ticket);
        loop {
            if cancel.is_cancelled() {
                s.line.retain(|&t| t != ticket);
                self.changed.notify_all();
                return false;
            }
            if s.line.front() == Some(&ticket) && s.budget.try_admit(weight) {
                s.line.pop_front();
                // The next in line may fit beside this one.
                self.changed.notify_all();
                return true;
            }
            s = self
                .changed
                .wait_timeout(s, Duration::from_millis(50))
                .unwrap()
                .0;
        }
    }

    /// [`Gate::admit`], with the weight held by the returned token and given
    /// back when it drops, however the job ends, a panic included.
    pub(crate) fn enter(&self, weight: usize, cancel: &CancelToken) -> Option<Admitted<'_>> {
        self.admit(weight, cancel)
            .then_some(Admitted { gate: self, weight })
    }

    /// Frees what a job admitted with `weight` held.
    pub(crate) fn release(&self, weight: usize) {
        self.state.lock().unwrap().budget.release(weight);
        self.changed.notify_all();
    }

    /// Wakes the jobs waiting, so each looks at its cancel token now and
    /// not at its next 50 ms check.
    pub(crate) fn wake(&self) {
        self.changed.notify_all();
    }
}

/// The room a job holds in the budget, freed when this drops.
pub(crate) struct Admitted<'a> {
    gate: &'a Gate,
    weight: usize,
}

impl Drop for Admitted<'_> {
    fn drop(&mut self) {
        self.gate.release(self.weight);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn a_job_that_fits_starts() {
        let mut b = Budget::new(8);
        assert!(b.try_admit(1));
        assert!(b.try_admit(2));
        assert!(b.try_admit(5));
    }

    #[test]
    fn a_full_budget_admits_nothing_more() {
        let mut b = Budget::new(4);
        assert!(b.try_admit(2));
        assert!(b.try_admit(2));
        assert!(!b.try_admit(1));
        assert!(!b.try_admit(2));
    }

    #[test]
    fn a_job_heavier_than_the_budget_starts_only_alone() {
        let mut b = Budget::new(4);
        assert!(b.try_admit(1));
        assert!(!b.try_admit(WHOLE));
        assert!(!b.try_admit(9));
        b.release(1);
        assert!(b.try_admit(9), "alone, it starts");
        assert!(!b.try_admit(1), "and nothing starts beside it");
        b.release(9);
        assert!(b.try_admit(1), "released, the budget is free again");
    }

    #[test]
    fn a_whole_budget_job_waits_for_the_others_to_end() {
        let mut b = Budget::new(4);
        assert!(b.try_admit(2));
        assert!(!b.try_admit(WHOLE));
        b.release(2);
        assert!(b.try_admit(WHOLE));
        assert!(!b.try_admit(1));
    }

    #[test]
    fn release_frees_room() {
        let mut b = Budget::new(2);
        assert!(b.try_admit(2));
        assert!(!b.try_admit(1));
        b.release(2);
        assert!(b.try_admit(1));
        assert!(b.try_admit(1));
        assert!(!b.try_admit(1));
    }

    #[test]
    fn a_light_job_does_not_pass_a_heavy_one_in_line() {
        let gate = Arc::new(Gate::new(4));
        assert!(gate.admit(1, &CancelToken::new()));
        let order = Arc::new(Mutex::new(vec![]));
        let heavy = {
            let (gate, order) = (gate.clone(), order.clone());
            std::thread::spawn(move || {
                assert!(gate.admit(WHOLE, &CancelToken::new()));
                order.lock().unwrap().push("heavy");
                gate.release(WHOLE);
            })
        };
        // Let the heavy job reach the line first.
        std::thread::sleep(Duration::from_millis(150));
        let light = {
            let (gate, order) = (gate.clone(), order.clone());
            std::thread::spawn(move || {
                assert!(gate.admit(1, &CancelToken::new()));
                order.lock().unwrap().push("light");
                gate.release(1);
            })
        };
        std::thread::sleep(Duration::from_millis(150));
        assert!(order.lock().unwrap().is_empty(), "both wait for the first");
        gate.release(1);
        heavy.join().unwrap();
        light.join().unwrap();
        assert_eq!(*order.lock().unwrap(), ["heavy", "light"]);
    }

    #[test]
    fn a_panic_after_admission_still_frees_the_weight() {
        let gate = Gate::new(2);
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _held = gate.enter(WHOLE, &CancelToken::new()).unwrap();
            panic!("a fault after admission");
        }));
        assert!(unwound.is_err());
        // Had the weight leaked, nothing would be admitted again.
        assert!(gate.enter(WHOLE, &CancelToken::new()).is_some());
    }

    #[test]
    fn a_job_cancelled_in_line_leaves_it() {
        let gate = Gate::new(1);
        assert!(gate.admit(1, &CancelToken::new()));
        let cancel = CancelToken::new();
        cancel.cancel();
        assert!(!gate.admit(1, &cancel));
        gate.release(1);
        assert!(gate.admit(1, &CancelToken::new()), "nothing was left held");
    }
}
