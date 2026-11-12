//! This run's holding folders, as the window knows them.
//!
//! Results wait on their source's drive (`kiln_core::HeldRuns`): a run
//! folder in `%LOCALAPPDATA%\Kiln\held` for the system drive, network
//! paths and drives Kiln can't hold on, and one in the user's own folder
//! in `<drive>\.kiln-held` for each other local drive a job uses.
//!
//! Opening a drive's folder the first time does disk work (folders, their
//! permissions, the record of roots), and a slow drive can take a while,
//! so it is never done on the window's thread. The window keeps its own
//! copy of what is open, by drive ([`Held::folder_now`], the answer
//! `HeldRuns::cached` gives), and a worker opens the rest
//! ([`Held::open_for`]), answering with `Event::HeldOpened`. Only those
//! workers, and the end of the run, lock the runs themselves.
//!
//! What a run notes in its folders (where each result came from, that one
//! has left, that runs from last time are done with), and the held results
//! a Discard throws away, are written and removed by one notes worker
//! ([`Notes`]), in the order asked, so the window never waits on another
//! drive.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use kiln_core::{DriveKind, HeldRuns, HoldKey, Places, Prepared, StaleRun, SystemVolumes, Volumes};

use super::channel::{Event, Wake};
use super::{inside, log};

/// What a worker opening a holding folder found, for `Event::HeldOpened`:
/// the run folder for `key`'s results, or `None` if none could be opened
/// (they are then held in the local one).
pub(super) struct Opened {
    pub key: HoldKey,
    pub dir: Option<PathBuf>,
}

/// This run's holding folders.
pub(super) struct Held {
    /// The runs, for the workers that open folders and for the end of the
    /// run. Never locked on the window's thread before then.
    runs: Arc<Mutex<Option<HeldRuns>>>,
    places: Places,
    /// The run folder in `%LOCALAPPDATA%\Kiln\held`, open from the start.
    local: PathBuf,
    /// The run folder each drive's results go to, as the runs opened it.
    by_key: HashMap<HoldKey, PathBuf>,
    /// Every run folder open, in the order opened.
    dirs: Vec<PathBuf>,
    /// Drives a worker is opening a folder for now.
    opening: HashSet<HoldKey>,
}

impl Held {
    /// This run's holding folders on `volumes`: the local run folder is
    /// opened now, as phase 1 opened its one folder, and is where the
    /// system drive's and network paths' results go. Other drives' are
    /// opened when a job first needs them. An error if the local one
    /// can't be opened: results then go next to each file, as before.
    pub fn open(places: Places, volumes: Box<dyn Volumes + Send>) -> kiln_core::Result<Held> {
        let system = volumes.system_root();
        let mut runs = HeldRuns::new(places.clone(), volumes);
        let local_held = places.local_held().to_path_buf();
        let local = runs.folder_for(&local_held)?.dir().to_path_buf();
        let mut held = Held {
            runs: Arc::new(Mutex::new(None)),
            places,
            local: local.clone(),
            by_key: HashMap::from([(HoldKey::of(&local_held), local.clone())]),
            dirs: vec![local],
            opening: HashSet::new(),
        };
        // The drives whose results are held locally whatever they are: no
        // job of theirs waits for a worker. Neither looks at a network.
        let unc = PathBuf::from(r"\\?\UNC\");
        for source in std::iter::once(unc).chain(system) {
            if let Ok(h) = runs.folder_for(&source) {
                held.note(HoldKey::of(&source), h.dir().to_path_buf());
            }
        }
        held.runs = Arc::new(Mutex::new(Some(runs)));
        Ok(held)
    }

    /// The local run folder.
    pub fn local(&self) -> &Path {
        &self.local
    }

    /// The run folder results made from `source` go to, if it is open.
    /// No disk or network access.
    pub fn folder_now(&self, source: &Path) -> Option<&Path> {
        self.by_key.get(&HoldKey::of(source)).map(PathBuf::as_path)
    }

    /// The run folder `file` is directly in, if it is one of this run's.
    pub fn folder_of(&self, file: &Path) -> Option<&Path> {
        let parent = file.parent()?;
        self.dirs
            .iter()
            .find(|d| d.as_path() == parent)
            .map(PathBuf::as_path)
    }

    /// Whether `path` is no place to save to: inside a place Kiln holds
    /// results in, or in one of this run's folders.
    pub fn holds(&self, path: &Path) -> bool {
        kiln_core::in_holding_place(path, &self.places) || self.dirs.iter().any(|d| inside(path, d))
    }

    /// Have a worker open the run folder for `source`'s results, unless it
    /// is open or being opened. It answers with `Event::HeldOpened`.
    /// False if no worker could be started: `source`'s drive is then held
    /// locally, and its folder is known at once.
    pub fn open_for(&mut self, source: &Path, wake: &Wake) -> bool {
        let key = HoldKey::of(source);
        if self.by_key.contains_key(&key) || self.opening.contains(&key) {
            return true;
        }
        let runs = self.runs.clone();
        let wake = wake.clone();
        let source = source.to_path_buf();
        let started = std::thread::Builder::new()
            .name("kiln-held-open".to_owned())
            .spawn(move || {
                // A panic opening it still answers, with nothing opened, so
                // the drive isn't left waiting: its results are held locally.
                let opened = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    match lock(&runs).as_mut().map(|r| r.folder_for(&source)) {
                        Some(Ok(h)) => Some(Some(h.dir().to_path_buf())),
                        Some(Err(e)) => {
                            let s = source.display();
                            log(&format!("couldn't hold results from {s}: {e}"));
                            Some(None)
                        }
                        // The run has ended.
                        None => None,
                    }
                }));
                let dir = match opened {
                    Ok(Some(dir)) => dir,
                    Ok(None) => return,
                    Err(_) => {
                        log(&format!(
                            "opening a holding folder for {} failed",
                            source.display()
                        ));
                        None
                    }
                };
                wake.send(Event::HeldOpened(Opened { key, dir }));
            });
        match started {
            Ok(_) => {
                self.opening.insert(key);
                true
            }
            Err(e) => {
                log(&format!("couldn't start opening a holding folder: {e}"));
                self.note(key, self.local.clone());
                false
            }
        }
    }

    /// A worker has opened a folder: from now on `key`'s results go there,
    /// or to the local folder if none could be opened.
    pub fn opened(&mut self, o: Opened) {
        self.opening.remove(&o.key);
        let dir = o.dir.unwrap_or_else(|| self.local.clone());
        self.note(o.key, dir);
    }

    fn note(&mut self, key: HoldKey, dir: PathBuf) {
        if !self.dirs.contains(&dir) {
            self.dirs.push(dir.clone());
        }
        self.by_key.insert(key, dir);
    }

    /// The end of the run: each run folder with no result in it goes, and
    /// then each drive's root Kiln made, if that left it empty
    /// (`kiln_core::HeldRuns::close_empty`). A folder with a result still
    /// in it, one that couldn't be saved, is kept whole and let go, for the
    /// next start to deliver. Waits up to `limit` for the runs (a worker
    /// opening a folder, or the start's look); past it, every folder is
    /// kept as it is, their locks going as the process ends. Nothing a
    /// folder holds is ever removed here. The folders kept.
    pub fn close_empty(self, limit: Duration) -> Vec<PathBuf> {
        let until = Instant::now() + limit;
        loop {
            match self.runs.try_lock() {
                Ok(mut r) => return r.take().map(HeldRuns::close_empty).unwrap_or_default(),
                Err(TryLockError::Poisoned(p)) => {
                    return p
                        .into_inner()
                        .take()
                        .map(HeldRuns::close_empty)
                        .unwrap_or_default();
                }
                Err(TryLockError::WouldBlock) if Instant::now() < until => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(TryLockError::WouldBlock) => {
                    log("the holding folders were still in use; kept for the next start");
                    return self.dirs;
                }
            }
        }
    }
    /// Signing out or shutting down: every run folder is kept as it is,
    /// for the next start to offer, and its lock let go. Never waits: with
    /// a worker still opening a folder, the locks go as the process ends.
    /// The folders kept.
    pub fn keep(self) -> Vec<PathBuf> {
        let runs = match self.runs.try_lock() {
            Ok(mut r) => r.take(),
            Err(TryLockError::Poisoned(p)) => p.into_inner().take(),
            Err(TryLockError::WouldBlock) => None,
        };
        match runs {
            Some(runs) => runs.keep(),
            None => self.dirs,
        }
    }
}

fn lock(runs: &Mutex<Option<HeldRuns>>) -> MutexGuard<'_, Option<HeldRuns>> {
    // A worker that panicked left the runs as they were.
    runs.lock().unwrap_or_else(|e| e.into_inner())
}

/// Set for a test Kiln, to the drives (`;` between) that stand in for the
/// mounted ones recovery looks on, so a test never looks in a real
/// drive's `.kiln-held`. Empty: none. Read only by a debug or test-hooks
/// build.
#[cfg_attr(not(any(debug_assertions, feature = "test-hooks")), allow(dead_code))]
pub(super) const TEST_DRIVES: &str = "KILN_TEST_DRIVES";

/// Set for a test Kiln: every delivery of a held result is a copy, as to
/// another drive, slowed, so a test can end Kiln part-way through one.
/// Read only by a debug or test-hooks build.
#[cfg_attr(not(any(debug_assertions, feature = "test-hooks")), allow(dead_code))]
pub(super) const TEST_SLOW_COPY: &str = "KILN_TEST_SLOW_COPY";

/// This PC's volumes, with the mounted drives `KILN_TEST_DRIVES` names
/// instead of the real ones when it is set, in a debug or test-hooks build
/// (as the tests run). A shipped build always looks on the real ones.
pub(super) fn volumes() -> Drives {
    #[cfg(any(debug_assertions, feature = "test-hooks"))]
    let instead = {
        // Keeps the build's mark in the program, for a test to find.
        std::hint::black_box(&crate::test_build::MARK);
        std::env::var_os(TEST_DRIVES).map(|v| {
            std::env::split_paths(&v)
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        })
    };
    #[cfg(not(any(debug_assertions, feature = "test-hooks")))]
    let instead = None;
    Drives { instead }
}

/// [`SystemVolumes`], whose mounted drives can be stood in for.
#[derive(Clone)]
pub(super) struct Drives {
    instead: Option<Vec<PathBuf>>,
}

impl Volumes for Drives {
    fn volume_root(&self, path: &Path) -> Option<PathBuf> {
        SystemVolumes.volume_root(path)
    }

    fn kind(&self, root: &Path) -> DriveKind {
        SystemVolumes.kind(root)
    }

    fn system_root(&self) -> Option<PathBuf> {
        SystemVolumes.system_root()
    }

    fn local_drive_roots(&self) -> Vec<PathBuf> {
        match &self.instead {
            Some(drives) => drives.clone(),
            None => SystemVolumes.local_drive_roots(),
        }
    }

    fn user_sid(&self) -> Option<String> {
        SystemVolumes.user_sid()
    }

    fn prepare(&self, root: &Path) -> std::io::Result<Prepared> {
        SystemVolumes.prepare(root)
    }
}

/// What the notes worker writes, each in the order asked.
pub(super) enum Note {
    /// `result`, in the run folder `dir`, was made from a file in `origin`
    /// (`kiln_core::record_source`).
    Source {
        dir: PathBuf,
        result: PathBuf,
        origin: PathBuf,
    },
    /// The held `file` has left the run folder `dir`
    /// (`kiln_core::forget_source`).
    Left { dir: PathBuf, file: PathBuf },

    /// Tests: the worker waits here until the gate is let go.
    #[cfg(test)]
    Hold(mpsc::Receiver<()>),
}

/// Write `note` now, on this thread.
pub(super) fn write(note: Note) {
    match note {
        Note::Source {
            dir,
            result,
            origin,
        } => {
            if let Err(e) = kiln_core::record_source(&dir, &result, &origin) {
                let r = result.display();
                log(&format!("couldn't note where {r} came from: {e}"));
            }
        }
        Note::Left { dir, file } => {
            if let Err(e) = kiln_core::forget_source(&dir, &file) {
                log(&format!("couldn't note that {} left: {e}", file.display()));
            }
        }

        #[cfg(test)]
        Note::Hold(gate) => {
            let _ = gate.recv();
        }
    }
}

/// What the notes worker takes, in turn.
enum Line {
    Note(Note),
    /// Answered once every note before it is written.
    Mark(Sender<()>),
    /// Write what is left, and end.
    Stop,
}

/// The way to the worker while it runs, shared with [`NotesFlusher`].
type Slot = Arc<Mutex<Option<Sender<Line>>>>;

/// The notes worker: one thread, started with the first note, that writes
/// each in turn. `Line::Stop` on its line ends it.
#[derive(Default)]
pub(super) struct Notes {
    tx: Option<Sender<Line>>,
    /// `tx` again, for the end of a session, which may come while `App`,
    /// and this with it, is borrowed.
    slot: Slot,
    thread: Option<JoinHandle<()>>,
    /// It couldn't be started: notes are written where they are asked.
    inline: bool,
}

/// A way to the notes worker for another thread, such as an export's. A
/// note it sends once the worker has ended is written on that thread.
#[derive(Clone)]
pub(super) struct NoteSender(Option<Sender<Line>>);

impl NoteSender {
    pub fn send(&self, note: Note) {
        match &self.0 {
            Some(tx) => {
                if let Err(mpsc::SendError(Line::Note(note))) = tx.send(Line::Note(note)) {
                    write(note);
                }
            }
            None => write(note),
        }
    }
}

/// A wait for the notes asked so far, for the end of a session: Windows
/// may end Kiln once its message is answered. Works whether or not `App`
/// is borrowed.
pub(super) struct NotesFlusher(Slot);

/// The notes asked before [`NotesFlusher::mark`], to wait for.
pub(super) struct Marked(Option<mpsc::Receiver<()>>);

impl NotesFlusher {
    /// Mark the notes asked so far, to wait for with [`Marked::wait`].
    pub fn mark(&self) -> Marked {
        let tx = self.0.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let Some(tx) = tx else {
            // No worker: every note was written where it was asked.
            return Marked(None);
        };
        let (done, wait) = mpsc::channel();
        match tx.send(Line::Mark(done)) {
            Ok(()) => Marked(Some(wait)),
            // The worker has ended, and wrote them.
            Err(_) => Marked(None),
        }
    }
}

impl Marked {
    /// Wait, until `until` at most, for the notes marked to be written.
    /// False if they weren't by then: the worker writes on.
    pub fn wait(self, until: Instant) -> bool {
        let Some(wait) = self.0 else {
            return true;
        };
        let limit = until.saturating_duration_since(Instant::now());
        match wait.recv_timeout(limit) {
            Ok(()) => true,
            // The worker ended without answering: nothing is left for it.
            Err(mpsc::RecvTimeoutError::Disconnected) => true,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                log("the notes worker didn't finish in time");
                false
            }
        }
    }
}

impl Notes {
    fn tx(&mut self) -> Option<&Sender<Line>> {
        if self.tx.is_none() && !self.inline {
            let (tx, rx) = mpsc::channel::<Line>();
            let started = std::thread::Builder::new()
                .name("kiln-held-notes".to_owned())
                .spawn(move || {
                    let each = |line: Line| match line {
                        Line::Note(note) => {
                            let wrote =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    write(note)
                                }));
                            if wrote.is_err() {
                                log("writing a holding folder's note failed");
                            }
                        }
                        Line::Mark(done) => {
                            let _ = done.send(());
                        }
                        Line::Stop => {}
                    };
                    while let Ok(line) = rx.recv() {
                        if matches!(line, Line::Stop) {
                            break;
                        }
                        each(line);
                    }
                    // Asked to stop: what other threads sent before the
                    // stop arrived, or after it, is still written.
                    for line in rx.try_iter() {
                        each(line);
                    }
                });
            match started {
                Ok(t) => {
                    *self.slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx.clone());
                    self.tx = Some(tx);
                    self.thread = Some(t);
                }
                Err(e) => {
                    log(&format!("couldn't start the notes worker: {e}"));
                    self.inline = true;
                }
            }
        }
        self.tx.as_ref()
    }

    /// Have `note` written, after every note asked before it.
    pub fn send(&mut self, note: Note) {
        self.sender().send(note);
    }

    /// A way to the worker for another thread.
    pub fn sender(&mut self) -> NoteSender {
        NoteSender(self.tx().cloned())
    }

    /// A way to wait for the worker at the end of a session.
    pub fn flusher(&self) -> NotesFlusher {
        NotesFlusher(self.slot.clone())
    }

    /// The run is ending: write what is asked so far, waiting up to
    /// `limit`. A note sent after this is written where it is sent.
    pub fn finish(&mut self, limit: Duration) {
        self.slot.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(Line::Stop);
        }
        if let Some(t) = self.thread.take() {
            let until = Instant::now() + limit;
            while !t.is_finished() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(5));
            }
            if t.is_finished() {
                let _ = t.join();
            } else {
                log("the notes worker didn't finish in time");
            }
        }
    }
}

/// Open this run's local holding folder, as phase 1 opened its one
/// folder: before the message loop, on the window's thread, and only in
/// `%LOCALAPPDATA%`. What crashed or killed runs left, on every drive, is
/// looked for after, on a worker ([`look_for_stale`]).
pub(super) fn open_holding() -> (Option<Held>, Option<Places>) {
    #[cfg(any(debug_assertions, feature = "test-hooks"))]
    if std::env::var_os(TEST_SLOW_COPY).is_some() {
        kiln_core::slow_copies_for_tests(|done, total| {
            log(&format!("slowed copy: {done} of {total}"));
        });
    }
    let Some(places) = Places::from_env() else {
        log("LOCALAPPDATA isn't set");
        return (None, None);
    };
    let held = match Held::open(places.clone(), Box::new(volumes())) {
        Ok(h) => Some(h),
        Err(e) => {
            log(&format!(
                "couldn't open {}: {e}",
                places.local_held().display()
            ));
            None
        }
    };
    (held, Some(places))
}

/// Set for a test Kiln, to a folder that stands in for Documents, so a
/// test never writes into the real one. Read only by a debug or
/// test-hooks build.
#[cfg_attr(not(any(debug_assertions, feature = "test-hooks")), allow(dead_code))]
pub(super) const TEST_DOCUMENTS: &str = "KILN_TEST_DOCUMENTS";

/// The folder results go to when they can't go where they were meant
/// to: `Kiln results` in Documents (the known folder, wherever it has
/// been moved to). `None` if Documents can't be found. A debug or
/// test-hooks build with `KILN_TEST_DOCUMENTS` set uses that folder as
/// Documents instead. Tests never use the real Documents: unit tests, and
/// a Kiln a test started (one with `KILN_TEST_RUN_KEY` or
/// `KILN_TEST_DRIVES` set), have no such folder without the variable.
pub(super) fn kiln_results() -> Option<PathBuf> {
    #[cfg(any(debug_assertions, feature = "test-hooks"))]
    {
        if let Some(docs) = std::env::var_os(TEST_DOCUMENTS) {
            return Some(kiln_core::kiln_results_in(Path::new(&docs)));
        }
        let under_test = ["KILN_TEST_RUN_KEY", "KILN_TEST_DRIVES"]
            .iter()
            .any(|v| std::env::var_os(v).is_some());
        if under_test {
            return None;
        }
    }
    if cfg!(test) {
        return None;
    }
    super::system::documents().map(|d| kiln_core::kiln_results_in(&d))
}

/// Find what crashed, killed or older runs left, on every drive, and
/// deliver every result, on a worker: next to its original, or into the
/// folder this version noted for it, else into `fallback`
/// (`kiln_core::deliver_left`). Their run folders without results are
/// cleared, and so are the empty drive roots Kiln made. The worker answers
/// twice: `Event::Swept` once the look and the sweep are done, and
/// `Event::Delivered` with what became of the results once every one has
/// been tried. A slow or sleeping drive keeps only the worker waiting.
/// This run's own folders are never among them: each is locked from the
/// moment it is made. While it looks it holds `held`'s runs, so no folder
/// is opened on a drive it may be clearing; it lets them go before it
/// delivers, so new jobs never wait on a long copy. `cancel` stops the
/// delivery, keeping what is left, whole, for the next start. If no
/// worker could be started, nothing is looked at: what they left stays
/// for the next start. `volumes` are the drives to look on ([`volumes`]).
pub(super) fn look_for_stale(
    held: Option<&Held>,
    places: Places,
    volumes: Drives,
    fallback: Option<PathBuf>,
    cancel: kiln_core::CancelToken,
    wake: &Wake,
) -> Option<JoinHandle<()>> {
    let runs = held.map(|h| h.runs.clone());
    let answer = wake.clone();
    let started = std::thread::Builder::new()
        .name("kiln-held-stale".to_owned())
        .spawn(move || {
            let found = {
                let _opening = runs.as_deref().map(lock);
                let look = || find_stale(&places, &volumes);
                std::panic::catch_unwind(look).unwrap_or_else(|_| {
                    log("looking for results from last time failed");
                    vec![]
                })
            };
            answer.send(Event::Swept);
            let deliver = std::panic::AssertUnwindSafe(|| {
                kiln_core::deliver_left(found, &places, fallback.as_deref(), &cancel)
            });
            let report = std::panic::catch_unwind(deliver).unwrap_or_else(|_| {
                log("saving the results from last time failed");
                kiln_core::Report::default()
            });
            if !report.is_empty() {
                // A result kept by the quit's stop is no failure; one kept
                // otherwise couldn't be saved.
                let why = match report.kept {
                    0 => "",
                    _ if cancel.is_cancelled() => ", stopped by the quit",
                    _ => ", as they couldn't be saved",
                };
                log(&format!(
                    "from last time: {} saved beside, {} in Kiln results, {} kept{why}",
                    report.beside, report.fallback, report.kept
                ));
            }
            answer.send(Event::Delivered(report));
        });
    match started {
        Ok(t) => Some(t),
        Err(e) => {
            log(&format!("couldn't look for results from last time: {e}"));
            wake.send(Event::Swept);
            None
        }
    }
}

/// What crashed, killed or older runs left, on every drive: their run
/// folders with results, locked; the others cleared.
fn find_stale(places: &Places, volumes: &Drives) -> Vec<StaleRun> {
    let roots = kiln_core::recovery_roots(places, volumes);
    // Locked first: from here no sweep, and no other Kiln, touches them.
    let stale = kiln_core::stale_runs_in(&roots, places);
    for run in &stale {
        log(&format!(
            "{} holds {} results from last time",
            run.dir.display(),
            run.files.len()
        ));
    }
    for dir in kiln_core::sweep_stale_in(&roots, places) {
        log(&format!("cleared {}", dir.display()));
    }
    stale
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_local_folder_is_open_from_the_start_and_takes_the_system_drives() {
        let dir = tempfile::tempdir().unwrap();
        let places = Places::new(dir.path());
        let held = Held::open(
            places,
            Box::new(Drives {
                instead: Some(vec![]),
            }),
        )
        .unwrap();
        assert!(held.local().starts_with(dir.path().join("held")));
        assert!(held.local().is_dir());
        assert_eq!(held.dirs, [held.local().to_path_buf()]);
        // The folder LOCALAPPDATA is on, and network paths: no worker.
        let near = dir.path().join("photo.png");
        assert_eq!(held.folder_now(&near), Some(held.local()));
        let unc = Path::new(r"\\server\share\photo.png");
        assert_eq!(held.folder_now(unc), Some(held.local()));
        // A file directly in it is held; one deeper, or elsewhere, isn't.
        let f = held.local().join("a.jpg");
        assert_eq!(held.folder_of(&f), Some(held.local()));
        assert_eq!(held.folder_of(&held.local().join("x").join("a.jpg")), None);
        assert_eq!(held.folder_of(&near), None);
        // Nothing inside a holding place takes an export.
        assert!(held.holds(held.local()));
        assert!(held.holds(&dir.path().join("held").join("other")));
        assert!(held.holds(Path::new(r"E:\.kiln-held\S-1-5-21-1\9-9")));
        assert!(!held.holds(dir.path()));
        let local = held.local().to_path_buf();
        assert!(held.close_empty(Duration::from_secs(10)).is_empty());
        assert!(!local.exists(), "with nothing in it, it goes");
    }

    #[test]
    fn a_folder_a_worker_opened_is_used_from_then_on() {
        let dir = tempfile::tempdir().unwrap();
        let mut held =
            Held::open(Places::new(dir.path()), Box::new(Drives { instead: None })).unwrap();
        let other = Path::new(r"Q:\pictures\photo.png");
        assert_eq!(held.folder_now(other), None);
        let q = dir.path().join("q-run");
        held.opened(Opened {
            key: HoldKey::of(other),
            dir: Some(q.clone()),
        });
        assert_eq!(held.folder_now(other), Some(q.as_path()));
        assert_eq!(held.folder_of(&q.join("a.jpg")), Some(q.as_path()));
        // None could be opened: held locally.
        let r = Path::new(r"R:\photo.png");
        held.opened(Opened {
            key: HoldKey::of(r),
            dir: None,
        });
        assert_eq!(held.folder_now(r), Some(held.local()));
        assert_eq!(held.dirs.len(), 2);
        let kept = held.keep();
        assert_eq!(kept.len(), 1, "only folders the runs opened are theirs");
    }

    #[test]
    fn notes_are_written_in_the_order_asked_and_all_by_the_end() {
        let root = tempfile::tempdir().unwrap();
        let origin = tempfile::tempdir().unwrap();
        let h = kiln_core::Holding::open(root.path()).unwrap();
        let mut notes = Notes::default();
        let other = notes.sender();
        for name in ["a.jpg", "b.jpg"] {
            let result = h.dir().join(name);
            std::fs::write(&result, name).unwrap();
            notes.send(Note::Source {
                dir: h.dir().to_path_buf(),
                result: result.clone(),
                origin: origin.path().to_path_buf(),
            });
            other.send(Note::Left {
                dir: h.dir().to_path_buf(),
                file: result,
            });
        }
        notes.finish(Duration::from_secs(10));
        let o = origin.path().display();
        assert_eq!(
            std::fs::read_to_string(h.dir().join(kiln_core::SOURCES_NAME)).unwrap(),
            format!("a.jpg\t{o}\na.jpg\t\nb.jpg\t{o}\nb.jpg\t\n")
        );
        // Once it has ended, a note is written where it is sent.
        let c = h.dir().join("c.jpg");
        other.send(Note::Left {
            dir: h.dir().to_path_buf(),
            file: c,
        });
        assert!(
            std::fs::read_to_string(h.dir().join(kiln_core::SOURCES_NAME))
                .unwrap()
                .ends_with("c.jpg\t\n")
        );
    }

    #[test]
    fn a_quit_never_removes_a_result_still_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let held = Held::open(Places::new(dir.path()), Box::new(SystemVolumes)).unwrap();
        let local = held.local().to_path_buf();
        let result = local.join("a.jpg");
        std::fs::write(&result, "a").unwrap();
        // A result that couldn't be saved anywhere: kept whole, let go.
        assert_eq!(held.close_empty(Duration::from_secs(10)), [local]);
        assert_eq!(std::fs::read_to_string(&result).unwrap(), "a");
        // The next start finds it, to deliver it.
        let stale = kiln_core::stale_runs(&dir.path().join("held"));
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].files[0].path, result);
    }

    #[test]
    fn a_quit_never_waits_long_on_a_folder_being_opened() {
        let dir = tempfile::tempdir().unwrap();
        let held = Held::open(Places::new(dir.path()), Box::new(SystemVolumes)).unwrap();
        let local = held.local().to_path_buf();
        let result = local.join("a.jpg");
        std::fs::write(&result, "a").unwrap();
        let runs = held.runs.clone();
        let busy = runs.lock().unwrap();
        let started = Instant::now();
        // Past its limit everything is kept as it is: nothing removed.
        assert_eq!(held.close_empty(Duration::from_millis(200)), [local]);
        assert!(started.elapsed() < Duration::from_secs(5));
        drop(busy);
        assert!(result.is_file());
        // Its lock goes as the process ends; the next start delivers it.
        drop(runs);
        let stale = kiln_core::stale_runs(&dir.path().join("held"));
        assert_eq!(stale.len(), 1);
    }

    #[test]
    fn last_times_results_are_found_on_a_worker_that_waits_for_the_runs_and_delivered() {
        let dir = tempfile::tempdir().unwrap();
        let docs = tempfile::tempdir().unwrap();
        let places = Places::new(dir.path());
        let none = || Drives {
            instead: Some(vec![]),
        };
        let held = Held::open(places.clone(), Box::new(none())).unwrap();
        // Two runs from last time: one with a result, one without.
        let left = places.local_held().join("1-1");
        let empty = places.local_held().join("2-2");
        for d in [&left, &empty] {
            std::fs::create_dir_all(d).unwrap();
            std::fs::write(d.join("lock"), "").unwrap();
        }
        let origin = dir.path().join("Photos");
        std::fs::create_dir(&origin).unwrap();
        std::fs::write(left.join("a.jpg"), "a").unwrap();
        std::fs::write(
            left.join(kiln_core::SOURCES_NAME),
            format!("a.jpg\t{}\n", origin.display()),
        )
        .unwrap();
        // No window: the events wait in the inbox.
        let (wake, inbox) = super::super::channel::channel::<Event>(Default::default());
        // A worker is opening a folder: the look waits for it, so it never
        // clears a drive's root that folder is being made in.
        let busy = lock(&held.runs);
        let fallback = kiln_core::kiln_results_in(docs.path());
        let worker = look_for_stale(
            Some(&held),
            places.clone(),
            none(),
            Some(fallback),
            kiln_core::CancelToken::new(),
            &wake,
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert!(inbox.queued().is_empty(), "waits for the runs");
        assert!(empty.exists());
        drop(busy);
        worker.join().unwrap();
        let events = inbox.queued();
        assert!(matches!(events.first(), Some(Event::Swept)));
        let Some(Event::Delivered(report)) = events.get(1) else {
            panic!("the delivery answers");
        };
        assert_eq!((report.beside, report.fallback, report.kept), (1, 0, 0));
        assert_eq!(std::fs::read_to_string(origin.join("a.jpg")).unwrap(), "a");
        assert!(!left.exists(), "its run goes once delivered");
        assert!(!empty.exists(), "a run with no results is cleared");
        // This run's own folder is live: never delivered, never cleared.
        assert!(held.local().is_dir());
        held.close_empty(Duration::from_secs(10));
    }

    #[test]
    fn a_folder_being_opened_waits_only_for_the_look_never_the_delivery() {
        let dir = tempfile::tempdir().unwrap();
        let places = Places::new(dir.path());
        let none = || Drives {
            instead: Some(vec![]),
        };
        let held = Held::open(places.clone(), Box::new(none())).unwrap();
        let left = places.local_held().join("1-1");
        std::fs::create_dir_all(&left).unwrap();
        std::fs::write(left.join("lock"), "").unwrap();
        std::fs::write(left.join("a.jpg"), "a").unwrap();
        let (wake, inbox) = super::super::channel::channel::<Event>(Default::default());
        // A delivery that is cancelled before it starts keeps everything.
        let cancel = kiln_core::CancelToken::new();
        cancel.cancel();
        let worker =
            look_for_stale(Some(&held), places.clone(), none(), None, cancel, &wake).unwrap();
        worker.join().unwrap();
        // The runs were let go before the delivery: a folder can be opened.
        assert!(held.runs.try_lock().is_ok());
        let Some(Event::Delivered(report)) = inbox.queued().into_iter().nth(1) else {
            panic!("the delivery answers");
        };
        assert_eq!(report.kept, 1);
        assert!(left.join("a.jpg").is_file());
    }
    #[test]
    fn test_drives_stand_in_for_the_mounted_ones() {
        let d = Drives {
            instead: Some(vec![PathBuf::from(r"C:\scratch\one")]),
        };
        assert_eq!(d.local_drive_roots(), [PathBuf::from(r"C:\scratch\one")]);
        let none = Drives {
            instead: Some(vec![]),
        };
        assert!(none.local_drive_roots().is_empty());
    }
}
