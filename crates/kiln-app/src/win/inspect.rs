//! The inspect thread: one low-priority thread, separate from the job
//! pool, that looks inside each listed file in turn so a conversion never
//! waits on it.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;

use kiln_core::{CancelToken, Facts, Options};

/// One file waiting to be inspected, with the options it is read with.
struct Job {
    path: PathBuf,
    opts: Options,
    token: CancelToken,
}

#[derive(Default)]
struct State {
    /// First in, first out.
    queue: VecDeque<Job>,
    /// The file being inspected now, and its token.
    current: Option<(PathBuf, CancelToken)>,
    stop: bool,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        // A panic in `run` or `done` must not take the queue with it.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Inspects queued files one at a time and hands each result to `done`.
/// Once `cancel` or `cancel_all` returns, `done` is never called for the
/// files they cancelled.
pub struct Inspector {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Inspector {
    pub fn new(
        run: impl Fn(&Path, &Options, &CancelToken) -> Option<Facts> + Send + 'static,
        done: impl Fn(PathBuf, Option<Facts>) + Send + 'static,
    ) -> Inspector {
        let shared = Arc::new(Shared::default());
        let thread = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("kiln-inspect".into())
                .spawn(move || {
                    below_normal();
                    work(&shared, run, done);
                })
                .ok()
        };
        Inspector { shared, thread }
    }

    /// Inspect `path` with `opts` once the files before it are done. A
    /// path already waiting, or being inspected, is not queued twice.
    pub fn queue(&mut self, path: PathBuf, opts: Options) {
        let mut s = self.shared.lock();
        let waiting = s.queue.iter().any(|j| j.path == path);
        let running = s
            .current
            .as_ref()
            .is_some_and(|(p, t)| *p == path && !t.is_cancelled());
        if waiting || running || s.stop {
            return;
        }
        s.queue.push_back(Job {
            path,
            opts,
            token: CancelToken::new(),
        });
        self.shared.wake.notify_one();
    }

    /// Stop inspecting `path`, whether it is waiting or under way.
    pub fn cancel(&mut self, path: &Path) {
        let mut s = self.shared.lock();
        s.queue.retain(|j| j.path != path);
        if let Some((p, t)) = &s.current {
            if p == path {
                t.cancel();
            }
        }
    }

    /// Stop inspecting everything.
    pub fn cancel_all(&mut self) {
        let mut s = self.shared.lock();
        s.queue.clear();
        if let Some((_, t)) = &s.current {
            t.cancel();
        }
    }

    /// Cancel everything and wait for the thread to finish. Safe to call
    /// more than once.
    pub fn shutdown(&mut self) {
        {
            let mut s = self.shared.lock();
            s.stop = true;
            s.queue.clear();
            if let Some((_, t)) = &s.current {
                t.cancel();
            }
        }
        self.shared.wake.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Inspector {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn work(
    shared: &Shared,
    run: impl Fn(&Path, &Options, &CancelToken) -> Option<Facts>,
    done: impl Fn(PathBuf, Option<Facts>),
) {
    loop {
        let job = {
            let mut s = shared.lock();
            loop {
                if s.stop {
                    return;
                }
                if let Some(job) = s.queue.pop_front() {
                    s.current = Some((job.path.clone(), job.token.clone()));
                    break job;
                }
                s = shared.wake.wait(s).unwrap_or_else(|e| e.into_inner());
            }
        };
        let facts = run(&job.path, &job.opts, &job.token);
        // Checked and reported under the lock, so a cancel that has
        // returned can't be followed by a report for its file.
        let mut s = shared.lock();
        s.current = None;
        if !job.token.is_cancelled() && !s.stop {
            done(job.path, facts);
        }
    }
}

/// Inspecting is background work: below normal, so it gives way to the
/// window and to conversions.
fn below_normal() {
    use windows::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
    };
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    /// An inspector whose `run` waits for a go on `release` (one per file,
    /// in order) and whose `done` reports to the receiver.
    fn gated() -> (
        Inspector,
        mpsc::Sender<()>,
        mpsc::Receiver<PathBuf>,
        mpsc::Receiver<PathBuf>,
    ) {
        let (release, gate) = mpsc::channel::<()>();
        let (started_tx, started) = mpsc::channel();
        let (done_tx, reported) = mpsc::channel();
        let gate = Mutex::new(gate);
        let started_tx = Mutex::new(started_tx);
        let done_tx = Mutex::new(done_tx);
        let inspector = Inspector::new(
            move |path, _, _| {
                let _ = started_tx.lock().unwrap().send(path.to_path_buf());
                let _ = gate.lock().unwrap().recv();
                None
            },
            move |path, _| {
                let _ = done_tx.lock().unwrap().send(path);
            },
        );
        (inspector, release, started, reported)
    }

    const WAIT: Duration = Duration::from_secs(5);

    #[test]
    fn cancelled_paths_never_report() {
        let (mut inspector, release, started, reported) = gated();
        for p in ["a", "b", "c"] {
            inspector.queue(p.into(), Options::default());
        }
        // Queued again while still waiting: one job.
        inspector.queue("c".into(), Options::default());
        assert_eq!(started.recv_timeout(WAIT).unwrap(), PathBuf::from("a"));
        inspector.cancel(Path::new("b"));
        for _ in 0..3 {
            let _ = release.send(());
        }
        assert_eq!(reported.recv_timeout(WAIT).unwrap(), PathBuf::from("a"));
        assert_eq!(reported.recv_timeout(WAIT).unwrap(), PathBuf::from("c"));
        drop(inspector);
        assert!(reported.try_recv().is_err());
        let seen: Vec<PathBuf> = started.try_iter().collect();
        assert_eq!(seen, [PathBuf::from("c")]);
    }

    #[test]
    fn a_file_under_way_can_be_cancelled() {
        let (mut inspector, release, started, reported) = gated();
        inspector.queue("a".into(), Options::default());
        inspector.queue("b".into(), Options::default());
        assert_eq!(started.recv_timeout(WAIT).unwrap(), PathBuf::from("a"));
        inspector.cancel(Path::new("a"));
        let _ = release.send(());
        let _ = release.send(());
        assert_eq!(reported.recv_timeout(WAIT).unwrap(), PathBuf::from("b"));
        drop(inspector);
        assert!(reported.try_recv().is_err());
    }

    #[test]
    fn cancel_all_stops_the_queue() {
        let (mut inspector, release, started, reported) = gated();
        for p in ["a", "b", "c"] {
            inspector.queue(p.into(), Options::default());
        }
        assert_eq!(started.recv_timeout(WAIT).unwrap(), PathBuf::from("a"));
        inspector.cancel_all();
        let _ = release.send(());
        // Still working after a cancel: a new file goes through.
        inspector.queue("d".into(), Options::default());
        assert_eq!(started.recv_timeout(WAIT).unwrap(), PathBuf::from("d"));
        let _ = release.send(());
        assert_eq!(reported.recv_timeout(WAIT).unwrap(), PathBuf::from("d"));
        drop(inspector);
        assert!(reported.try_recv().is_err());
        assert!(started.try_recv().is_err());
    }

    #[test]
    fn dropping_stops_the_thread() {
        let (mut inspector, release, started, reported) = gated();
        inspector.queue("a".into(), Options::default());
        assert_eq!(started.recv_timeout(WAIT).unwrap(), PathBuf::from("a"));
        // The run in progress finishes once let go; the drop waits for it.
        let _ = release.send(());
        inspector.shutdown();
        inspector.queue("b".into(), Options::default());
        drop(inspector);
        assert!(started.try_recv().is_err());
        // "a" may or may not have reported before the stop; nothing after.
        let after: Vec<PathBuf> = reported.try_iter().collect();
        assert!(after.iter().all(|p| p == Path::new("a")), "{after:?}");
    }
}
