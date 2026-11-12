//! The threads that look at the disk for the window, so its own thread
//! never waits on a drive, a search of `PATH` or Media Foundation.
//!
//! - **Engines** ([`Engines`]): which helper apps are installed
//!   (`Availability::find`), with the line Settings shows for each
//!   (`settings::engine_status`), and Media Foundation's codecs, looked for
//!   again after Get Media Feature Pack. The first look also starts Media Foundation.
//! - **Gone** ([`Gone`]): which held results are no longer on disk, when
//!   the window is activated.
//!
//! Each is one thread, started with the window, that takes what was asked
//! since it last looked and answers the latest of it on the window's
//! channel. A thread still looking when the window goes finishes its look
//! and ends; its answer is dropped on the way.

use std::collections::HashSet;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};

use kiln_core::Availability;

use crate::settings::{self, Engine, EngineStatus, Settings};

use super::channel::{Event, Wake};
use super::{log, MfLooked};

/// One thread that is handed requests and works through them, all those
/// waiting at once.
struct Worker<R> {
    tx: Sender<R>,
}

impl<R: Send + 'static> Worker<R> {
    /// Start the thread `name`, which calls `work` with every request
    /// waiting, oldest first, until the worker goes. A panic in `work` is
    /// logged, and the thread goes on to the next requests.
    fn start(name: &str, work: impl FnMut(Vec<R>) + Send + 'static) -> Option<Worker<R>> {
        Self::start_on(name, work, |name, f| {
            std::thread::Builder::new().name(name.to_owned()).spawn(f)
        })
    }

    /// As `start`, on a thread that reads user files: below normal
    /// priority, with a parser's stack (`kiln_core::spawn_worker`).
    fn start_low(name: &str, work: impl FnMut(Vec<R>) + Send + 'static) -> Option<Worker<R>> {
        Self::start_on(name, work, kiln_core::spawn_worker)
    }

    fn start_on(
        name: &str,
        mut work: impl FnMut(Vec<R>) + Send + 'static,
        spawn: impl FnOnce(
            &str,
            Box<dyn FnOnce() + Send>,
        ) -> std::io::Result<std::thread::JoinHandle<()>>,
    ) -> Option<Worker<R>> {
        let (tx, rx) = mpsc::channel::<R>();
        let spawned = spawn(
            name,
            Box::new(move || {
                while let Ok(first) = rx.recv() {
                    let mut asked = vec![first];
                    asked.extend(rx.try_iter());
                    if catch_unwind(AssertUnwindSafe(|| work(asked))).is_err() {
                        log("a worker's look failed; it goes on with the next");
                    }
                }
            }),
        );
        match spawned {
            Ok(_) => Some(Worker { tx }),
            Err(e) => {
                log(&format!("couldn't start the {name} thread: {e}"));
                None
            }
        }
    }

    /// Hand the thread `r`. It comes back if the thread has gone.
    fn send(&self, r: R) -> Result<(), R> {
        self.tx.send(r).map_err(|e| e.0)
    }
}

/// What the engines thread is asked.
pub(super) enum EnginesAsk {
    /// Which engines are installed with `settings`, and where each is
    /// found: answered as `Event::Engines` with `id`.
    Look { id: u64, settings: Box<Settings> },
    /// Media Foundation's codecs again: answered as `Event::Mf`.
    Mf,
}

/// Which engines are installed, as a look `id` found them.
pub(super) struct EnginesFound {
    pub id: u64,
    pub available: Availability,
    /// The look failed and `available` is "nothing found": it stands in
    /// only until a real answer has come.
    pub failed: bool,
    /// Where each engine is found, in `Engine::ALL` order, for Settings'
    /// lines; `None` when the look failed.
    pub status: Option<[EngineStatus; 4]>,
    /// What was found of Microsoft Office, for Settings; `None` when the
    /// look failed.
    pub office: Option<settings::OfficeStatus>,
    /// The chosen folder (`save_to`) the look was asked with, and whether
    /// it is missing, for Settings' line: a folder on a sleeping network
    /// drive can take the drive's timeout to answer. `None` with no chosen
    /// folder, or when the look failed.
    pub save_to: Option<(PathBuf, bool)>,
    /// Where `winget.exe` was found, for Settings' Install with winget.
    pub winget: Option<PathBuf>,
}

/// The engines thread.
pub(super) struct Engines {
    worker: Option<Worker<EnginesAsk>>,
    wake: Wake,
}

impl Engines {
    pub(super) fn start(wake: Wake) -> Engines {
        let answer = wake.clone();
        let worker = Worker::start("kiln-engines", move |asked| {
            for found in engines_answered(asked) {
                answer.send(found);
            }
        });
        Engines { worker, wake }
    }

    /// Ask the thread. Without one (it couldn't be started), the look is
    /// made here: the window would otherwise never know its engines.
    pub(super) fn ask(&self, ask: EnginesAsk) {
        let ask = match &self.worker {
            Some(w) => match w.send(ask) {
                Ok(()) => return,
                Err(ask) => ask,
            },
            None => ask,
        };
        log("looking for engines on the window's thread: their thread has gone");
        for found in engines_answered(vec![ask]) {
            self.wake.send(found);
        }
    }
}

/// [`engines_answer`], or, if it panics, answers that let the window go
/// on: "nothing found" for the latest look asked, marked failed, so what
/// waits for the engines' first answer (a wheel, a menu) still goes ahead
/// and a known answer is kept; and for a Media Foundation re-check, the
/// codecs as they are, so the re-check is over.
fn engines_answered(asked: Vec<EnginesAsk>) -> Vec<Event> {
    let latest = asked.iter().rev().find_map(|a| match a {
        EnginesAsk::Look { id, .. } => Some(*id),
        EnginesAsk::Mf => None,
    });
    let mf = asked.iter().any(|a| matches!(a, EnginesAsk::Mf));
    match catch_unwind(AssertUnwindSafe(|| engines_answer(asked))) {
        Ok(found) => found,
        Err(_) => {
            log("the look for engines failed; nothing is taken as found");
            let mut out = vec![];
            if mf {
                let now = catch_unwind(kiln_media::mf_codecs).unwrap_or_default();
                out.push(Event::Mf(MfLooked {
                    before: now,
                    found: now,
                }));
            }
            if let Some(id) = latest {
                out.push(Event::Engines(EnginesFound {
                    id,
                    available: Availability::default(),
                    failed: true,
                    status: None,
                    office: None,
                    save_to: None,
                    winget: None,
                }));
            }
            out
        }
    }
}

/// What the engines thread says for `asked`, all of it waiting at once:
/// a look again at Media Foundation first, if one was asked, so a look for
/// engines after it sees what it found; then the latest look for engines,
/// which stands for those before it.
fn engines_answer(asked: Vec<EnginesAsk>) -> Vec<Event> {
    #[cfg(test)]
    if asked
        .iter()
        .any(|a| matches!(a, EnginesAsk::Look { id, .. } if *id == PANIC_ID))
    {
        panic!("a look made to fail");
    }
    let mut out = vec![];
    let mut look = None;
    let mut mf = false;
    for a in asked {
        match a {
            EnginesAsk::Look { id, settings } => look = Some((id, settings)),
            EnginesAsk::Mf => mf = true,
        }
    }
    if mf {
        let before = kiln_media::mf_codecs();
        let found = kiln_media::mf_recheck();
        out.push(Event::Mf(MfLooked { before, found }));
    }
    if let Some((id, settings)) = look {
        let available = Availability::find(&settings.options());
        // PATH as the registry holds it now: a helper installed since Kiln
        // started is found.
        let path_var = kiln_core::current_path();
        let status = Engine::ALL.map(|e| settings::engine_status(&settings, e, &path_var));
        // Office's three apps, from the registry: only what is there, and
        // only while Office to PDF is switched on.
        let office = kiln_core::office::OFFICE_TO_PDF
            .then(|| settings::office_status(&kiln_core::office::find_installed()));
        out.push(Event::Engines(EnginesFound {
            id,
            available,
            failed: false,
            status: Some(status),
            office,
            save_to: settings.save_to.clone().map(|d| {
                let missing = !d.is_dir();
                (d, missing)
            }),
            winget: settings::find_winget(&path_var, std::env::var_os("LOCALAPPDATA").as_deref()),
        }));
    }
    out
}

/// Which of the held results asked about are no longer on disk, by the
/// look `ask` numbers (`Gone::ask`).
pub(super) struct GoneFound {
    pub ask: u64,
    pub gone: HashSet<PathBuf>,
}

/// The thread that looks for held results removed by hand.
pub(super) struct Gone {
    worker: Option<Worker<(u64, Vec<PathBuf>)>>,
}

impl Gone {
    pub(super) fn start(wake: Wake) -> Gone {
        let worker = Worker::start("kiln-gone", move |asked: Vec<(u64, Vec<PathBuf>)>| {
            // Each ask names every held result there was then: the latest
            // stands for those before it.
            let Some((ask, paths)) = asked.into_iter().last() else {
                return;
            };
            let gone = paths.into_iter().filter(|p| !p.exists()).collect();
            wake.send(Event::Gone(GoneFound { ask, gone }));
        });
        Gone { worker }
    }

    /// Ask which of `paths` are gone, by the look numbered `ask`. False
    /// when there is no thread to ask: the window then doesn't look until
    /// it is next activated.
    pub(super) fn ask(&self, ask: u64, paths: Vec<PathBuf>) -> bool {
        self.worker
            .as_ref()
            .is_some_and(|w| w.send((ask, paths)).is_ok())
    }
}

/// What the estimate thread is asked: the size `path` would come to
/// with `options`, for dialog `dialog`'s choice `opts`. The window stops
/// `cancel` once the choice changes or the dialog goes.
pub(super) struct EstimateAsk {
    pub dialog: u64,
    pub path: PathBuf,
    pub options: kiln_core::Options,
    pub opts: kiln_core::ToolOptions,
    pub cancel: kiln_core::CancelToken,
}

/// The thread that works out the Make smaller dialog's estimate: one
/// thread, kept, below normal priority (spec 3.3), so a run of choices
/// never piles up decodes or slows the conversions running.
pub(super) struct Estimator {
    worker: Option<Worker<EstimateAsk>>,
}

impl Estimator {
    /// The thread, working out with `work` only the latest of the asks
    /// waiting that the window hasn't stopped: each stands for the
    /// dialog's choice at the time, and only the latest is still wanted.
    pub(super) fn start_with(work: impl Fn(&EstimateAsk) + Send + 'static) -> Estimator {
        let worker = Worker::start_low("kiln-estimate", move |asked: Vec<EstimateAsk>| {
            if let Some(a) = asked.iter().rev().find(|a| !a.cancel.is_cancelled()) {
                work(a);
            }
        });
        Estimator { worker }
    }

    /// No thread: every ask comes back.
    #[cfg(test)]
    pub(super) fn none() -> Estimator {
        Estimator { worker: None }
    }

    /// Hand the thread `a`; it comes back when there is no thread.
    pub(super) fn ask(&self, a: EstimateAsk) -> Result<(), Box<EstimateAsk>> {
        match &self.worker {
            Some(w) => w.send(a).map_err(Box::new),
            None => Err(Box::new(a)),
        }
    }
}

/// Tests: a look with this id panics.
#[cfg(test)]
const PANIC_ID: u64 = u64::MAX;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_look_that_panics_answers_nothing_found_and_the_worker_goes_on() {
        let look = |id| EnginesAsk::Look {
            id,
            settings: Box::default(),
        };
        let answers = engines_answered(vec![EnginesAsk::Mf, look(PANIC_ID)]);
        assert!(matches!(
            &answers[..],
            [Event::Mf(m), Event::Engines(f)]
                if m.before == m.found
                    && f.id == PANIC_ID
                    && f.failed
                    && f.available == Availability::default()
        ));
        // The thread itself survives a panic in its work.
        let (tx, rx) = mpsc::channel();
        let w = Worker::start("kiln-test-panics", move |asked: Vec<u32>| {
            if asked.contains(&0) {
                panic!("made to fail");
            }
            tx.send(asked).unwrap();
        })
        .unwrap();
        w.send(0).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        w.send(1).unwrap();
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap(),
            vec![1]
        );
    }

    #[test]
    fn a_worker_takes_everything_waiting_at_once() {
        let (tx, rx) = mpsc::channel();
        let (go_tx, go_rx) = mpsc::channel::<()>();
        let go = std::sync::Mutex::new(go_rx);
        let w = Worker::start("kiln-test-worker", move |asked: Vec<u32>| {
            // The first call waits until the rest are queued.
            let _ = go.lock().unwrap().recv();
            tx.send(asked).unwrap();
        })
        .unwrap();
        w.send(1).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        w.send(2).unwrap();
        w.send(3).unwrap();
        go_tx.send(()).unwrap();
        go_tx.send(()).unwrap();
        assert_eq!(rx.recv().unwrap(), vec![1]);
        assert_eq!(rx.recv().unwrap(), vec![2, 3]);
    }

    /// An estimate ask for dialog `dialog`, with a token of its own.
    fn est(dialog: u64) -> EstimateAsk {
        EstimateAsk {
            dialog,
            path: PathBuf::from(format!("{dialog}.jpg")),
            options: kiln_core::Options::default(),
            opts: kiln_core::ToolOptions::default(),
            cancel: kiln_core::CancelToken::new(),
        }
    }

    #[test]
    fn the_estimator_works_out_only_the_latest_ask_on_one_low_priority_thread() {
        use windows::Win32::System::Threading::{
            GetCurrentThread, GetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
        };
        let (tx, rx) = mpsc::channel();
        let (go_tx, go_rx) = mpsc::channel::<()>();
        let go = std::sync::Mutex::new(go_rx);
        let e = Estimator::start_with(move |a: &EstimateAsk| {
            // The first waits until the rest are asked.
            let _ = go.lock().unwrap().recv();
            // SAFETY: the pseudo-handle of this thread, read only.
            let priority = unsafe { GetThreadPriority(GetCurrentThread()) };
            tx.send((a.dialog, std::thread::current().id(), priority))
                .unwrap();
        });
        assert!(e.ask(est(1)).is_ok());
        std::thread::sleep(std::time::Duration::from_millis(50));
        // Two more choices while the first is worked out: only the last
        // is, and the one before was stopped by the window.
        let second = est(2);
        second.cancel.cancel();
        assert!(e.ask(second).is_ok());
        assert!(e.ask(est(3)).is_ok());
        go_tx.send(()).unwrap();
        go_tx.send(()).unwrap();
        let wait = std::time::Duration::from_secs(5);
        let (a, t1, p) = rx.recv_timeout(wait).unwrap();
        let (b, t2, _) = rx.recv_timeout(wait).unwrap();
        assert_eq!((a, b), (1, 3));
        assert_eq!(t1, t2, "one thread, kept");
        assert_eq!(p, THREAD_PRIORITY_BELOW_NORMAL.0, "below normal");
        assert!(rx
            .recv_timeout(std::time::Duration::from_millis(100))
            .is_err());
    }

    #[test]
    fn an_estimator_with_no_thread_hands_the_ask_back() {
        let e = Estimator::none();
        assert!(e.ask(est(1)).is_err());
    }

    #[test]
    fn a_look_says_where_each_engine_is_for_settings_lines() {
        let mut settings = crate::settings::Settings::default();
        let gone = std::env::temp_dir()
            .join("kiln-no-such-dir")
            .join("pandoc.exe");
        settings.engines.set(Engine::Pandoc, Some(gone.clone()));
        let answers = engines_answer(vec![EnginesAsk::Look {
            id: 1,
            settings: Box::new(settings),
        }]);
        let [Event::Engines(f)] = &answers[..] else {
            panic!("one answer");
        };
        let status = f.status.as_ref().expect("a status for each engine");
        let i = Engine::ALL
            .iter()
            .position(|e| *e == Engine::Pandoc)
            .unwrap();
        assert_eq!(
            status[i],
            EngineStatus::Chosen {
                path: gone,
                exists: false
            }
        );
    }

    #[test]
    fn a_look_says_whether_the_chosen_folder_is_there() {
        let dir = tempfile::tempdir().unwrap();
        let look = |save_to: Option<PathBuf>| {
            let settings = crate::settings::Settings {
                save_to,
                ..Default::default()
            };
            let answers = engines_answer(vec![EnginesAsk::Look {
                id: 1,
                settings: Box::new(settings),
            }]);
            let [Event::Engines(f)] = &answers[..] else {
                panic!("one answer");
            };
            f.save_to.clone()
        };
        let here = dir.path().to_path_buf();
        assert_eq!(look(Some(here.clone())), Some((here, false)));
        let gone = dir.path().join("Gone away");
        assert_eq!(look(Some(gone.clone())), Some((gone, true)));
        assert_eq!(look(None), None);
    }

    #[test]
    fn the_latest_look_for_engines_stands_for_those_before_it() {
        let answers = engines_answer(vec![
            EnginesAsk::Look {
                id: 1,
                settings: Box::default(),
            },
            EnginesAsk::Look {
                id: 2,
                settings: Box::default(),
            },
        ]);
        assert_eq!(answers.len(), 1);
        assert!(matches!(&answers[0], Event::Engines(f) if f.id == 2));
    }
}
