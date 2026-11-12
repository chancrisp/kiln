//! A small worker pool that runs conversions and tools off the UI thread
//! and reports each job's progress and result through a callback.

use std::any::Any;
use std::collections::{HashMap, VecDeque};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::budget::{Gate, WHOLE};
use crate::converter::{CancelToken, Options, Progress};
use crate::error::Error;
use crate::job::Kiln;
use crate::plain::{self, FailedJob};
use crate::registry::Kind;
use crate::threads::spawn_worker;
use crate::tools::Tool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct JobId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobKind {
    Convert { input: PathBuf, target: String },
    Tool { tool: Tool, inputs: Vec<PathBuf> },
}

/// What a failed job tells the user: one plain line, and the engine's
/// full output for "Show details".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobError {
    /// One plain sentence, whole: the row shows it, cut short if it must.
    pub message: String,
    pub details: String,
    /// Who wrote `details`: the engine, or `None` for Kiln itself.
    pub engine: Option<&'static str>,
    pub cancelled: bool,
    /// The file a write ran out of room for, when a full disk is why the
    /// job failed.
    pub no_room: Option<PathBuf>,
    /// The Store product ID of the Microsoft extension the job needs, when
    /// that is why it failed ("Needs Microsoft's HEVC Video Extensions to
    /// read HEVC video."), for a Get it link that opens
    /// `ms-windows-store://pdp/?ProductId=` and the ID.
    pub store_product: Option<&'static str>,
}

impl JobError {
    /// What `e` tells the user, for a job `doing` something. An engine
    /// failure Kiln recognises becomes a plain sentence with a next step;
    /// the engine's own words go to the details.
    pub(crate) fn new(e: Error, doing: &Doing) -> Self {
        let (message, details, engine) = match &e {
            Error::EngineFailed {
                engine,
                summary,
                details,
            } => {
                let raw = format!("{summary}\n{details}");
                let shown = if details.trim().is_empty() {
                    summary.clone()
                } else {
                    details.clone()
                };
                let paths: Vec<&Path> = doing.paths.iter().map(PathBuf::as_path).collect();
                let job = FailedJob {
                    engine,
                    verb: doing.verb,
                    source: &doing.source,
                    target: &doing.target,
                    paths: &paths,
                };
                // Kiln's own code writes some summaries as whole sentences.
                let own = (*engine == "Kiln")
                    .then(|| plain::own_sentence(summary))
                    .flatten();
                let plain = plain::plain_failure_for(&job, &raw);
                let message = match own {
                    Some(sentence) if plain == plain::plain_failure_for(&job, "") => sentence,
                    _ => plain,
                };
                (message, shown, Some(*engine))
            }
            Error::EngineMissing { engine } => (plain::missing_engine(engine), String::new(), None),
            // Kiln's own sound engine already wrote the row's sentence.
            Error::Media(sentence) => (sentence.clone(), String::new(), None),
            // So is a refusal of a hostile file.
            Error::Unsafe(sentence) => (sentence.clone(), String::new(), None),
            // The row names the folder, never its path: the path waits in
            // Show details.
            Error::CantSaveIn(dir) => (
                format!("Kiln can't save in {}.", crate::output::folder_name(dir)),
                e.to_string(),
                None,
            ),
            Error::NeedsExtension { message, .. } => (message.clone(), String::new(), None),
            // The row gets a sentence with a next step and never a path;
            // the path and the library's words wait in Show details.
            Error::WrongContent { code, .. } => (plain::damaged_as(code), e.to_string(), None),
            Error::CutShort { code, .. } => (plain::cut_short(code), e.to_string(), None),
            Error::Decode { reason, .. } => (plain::unreadable_image(reason), e.to_string(), None),
            Error::Encode { code, .. } => (plain::cannot_write(code), e.to_string(), None),
            Error::InputNotFound(_) => (plain::gone(), e.to_string(), None),
            Error::Io { source, .. } => (plain::io_failure(source.kind()), e.to_string(), None),
            _ => (e.to_string(), String::new(), None),
        };
        JobError {
            cancelled: matches!(e, Error::Cancelled),
            no_room: e.no_room().map(Into::into),
            store_product: e.store_product(),
            message,
            details,
            engine,
        }
    }
}

impl From<Error> for JobError {
    fn from(e: Error) -> Self {
        JobError::new(e, &Doing::default())
    }
}

/// What a job was doing, for its error sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Doing {
    /// "convert", or a tool's id.
    verb: &'static str,
    /// The extensions it read and was writing.
    source: String,
    target: String,
    /// The files it read, and the result a conversion was writing, whose
    /// names are kept out of the matching.
    paths: Vec<PathBuf>,
}

impl Default for Doing {
    fn default() -> Self {
        Doing {
            verb: "convert",
            source: String::new(),
            target: String::new(),
            paths: vec![],
        }
    }
}

impl JobKind {
    /// What the job does, for its error sentences.
    fn doing(&self) -> Doing {
        let ext = |p: &PathBuf| {
            p.extension()
                .map(|e| e.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        match self {
            JobKind::Convert { input, target } => Doing {
                verb: "convert",
                source: ext(input),
                target: target.clone(),
                paths: vec![input.clone(), input.with_extension(target)],
            },
            JobKind::Tool { tool, inputs } => {
                let source = inputs.first().map(ext).unwrap_or_default();
                Doing {
                    verb: tool.id(),
                    source: source.clone(),
                    target: source,
                    paths: inputs.clone(),
                }
            }
        }
    }
}

/// What a job that hit a fault in Kiln itself says.
const PANICKED: &str = "Kiln stopped this job because of a problem in Kiln itself.";

#[derive(Debug, Clone, PartialEq)]
pub enum JobEvent {
    Progress(JobId, Progress),
    /// What a conversion leaves out ([`Kiln::drops`]), sent once, when the
    /// job starts, and only when there is something to say.
    Notes(JobId, Vec<String>),
    Finished(JobId, Result<Vec<PathBuf>, JobError>),
}

struct Job {
    id: JobId,
    kind: JobKind,
    options: Options,
    cancel: CancelToken,
    /// Half slots of the audio pool the job holds while it runs: 2 for a
    /// lossy target, 1 for a lossless or PCM one. 0 outside that pool.
    halves: usize,
    /// What the job takes of the processor budget ([`crate::budget`]).
    weight: usize,
}

/// The worker pools. Documents, images, tools, FFmpeg's video and the
/// video Kiln writes on the processor share the first, sized by the
/// caller. Sound Kiln writes itself (the "audio" and "audio-mf" backends)
/// has its own, so it never waits behind a video; each of its encodes uses
/// one thread. Video Kiln writes on a graphics card runs one job at a time
/// per card, in the order sorted, on one worker per card, so the
/// processor pool keeps its threads for the rest (R53).
///
/// Video jobs ("video" backend) go to a sorting thread first, which reads
/// the file's headers and asks which card's encoder would write it
/// ([`crate::video::lane`]: up to about 400 ms the first time), then hands
/// the job to that card's pool or to the processor pool.
///
/// All of it shares one processor budget ([`crate::budget`]): a job starts
/// when its weight fits beside the running ones. Threads are made when
/// work is queued, up to each pool's size, and none exist before the first
/// job. They, and the engines they start, run below normal priority.
pub struct Runner {
    cpu: Arc<Pool>,
    audio: Arc<Pool>,
    /// The sorting thread, started by the first video job.
    sorter: Mutex<Sorter>,
    kiln: Arc<Kiln>,
    cancels: Arc<Mutex<HashMap<JobId, CancelToken>>>,
    /// Wakes audio jobs waiting for a slot, so a cancel is seen at once.
    audio_slots: Arc<Slots>,
    /// The graphics cards' lines, made as jobs need them.
    cards: Arc<Cards>,
    gate: Arc<Gate>,
    next: AtomicU64,
}

/// The sorting thread and its queue.
#[derive(Default)]
struct Sorter {
    tx: Option<Sender<Job>>,
    thread: Option<JoinHandle<()>>,
    closed: bool,
}

/// What a worker needs besides its queue.
#[derive(Clone)]
struct Shared {
    kiln: Arc<Kiln>,
    cancels: Arc<Mutex<HashMap<JobId, CancelToken>>>,
    on_event: Arc<dyn Fn(JobEvent) + Send + Sync>,
    gate: Arc<Gate>,
}

/// A line of jobs and the threads that work it: at most `limit`, started
/// when a job is queued and no thread is free for it.
struct Pool {
    tx: Mutex<Option<Sender<Job>>>,
    rx: Arc<Mutex<Receiver<Job>>>,
    limit: usize,
    /// Set for the audio pool, whose jobs also wait for half slots.
    slots: Option<Arc<Slots>>,
    shared: Shared,
    /// Jobs queued or running.
    outstanding: Arc<AtomicUsize>,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl Pool {
    fn new(shared: &Shared, limit: usize, slots: Option<Arc<Slots>>) -> Arc<Self> {
        let (tx, rx) = mpsc::channel::<Job>();
        Arc::new(Pool {
            tx: Mutex::new(Some(tx)),
            rx: Arc::new(Mutex::new(rx)),
            limit: limit.max(1),
            slots,
            shared: shared.clone(),
            outstanding: Arc::default(),
            threads: Mutex::default(),
        })
    }

    /// Queues `job`, starting a thread for it if every one is busy and the
    /// pool has room for another.
    fn send(&self, job: Job) {
        {
            let mut threads = self.threads.lock().unwrap();
            let queued = self.outstanding.fetch_add(1, Ordering::SeqCst) + 1;
            if queued > threads.len() && threads.len() < self.limit {
                let (me, rx) = (self.shared.clone(), self.rx.clone());
                let (slots, outstanding) = (self.slots.clone(), self.outstanding.clone());
                threads.push(
                    spawn_worker("kiln-worker", move || {
                        work(&me, &rx, slots.as_deref(), &outstanding)
                    })
                    .expect("start a worker"),
                );
            }
        }
        if let Some(tx) = &*self.tx.lock().unwrap() {
            let _ = tx.send(job);
        }
    }

    /// The threads started so far.
    #[cfg(test)]
    fn started(&self) -> usize {
        self.threads.lock().unwrap().len()
    }

    /// Ends the line and waits for the threads to finish what they hold.
    fn close(&self) {
        *self.tx.lock().unwrap() = None;
        let threads: Vec<_> = self.threads.lock().unwrap().drain(..).collect();
        for t in threads {
            let _ = t.join();
        }
    }
}

/// The graphics cards, by LUID: each card's hardware encoder takes one
/// video at a time, in the order the jobs were sorted (R60). Each card has
/// one worker and a line of jobs waiting for it, so the threads are one per
/// card however many wait. A job cancelled while it waits in line leaves it
/// and ends at once ([`Cards::withdraw`]), not when the card is free.
struct Cards {
    shared: Shared,
    cards: Mutex<HashMap<u64, Arc<Card>>>,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

/// One graphics card's line of jobs.
#[derive(Default)]
struct Card {
    line: Mutex<Line>,
    ready: Condvar,
}

#[derive(Default)]
struct Line {
    jobs: VecDeque<Job>,
    closed: bool,
}

impl Cards {
    /// Puts `job` at the back of the card `luid`'s line, starting the
    /// card's worker the first time.
    fn send(&self, luid: u64, job: Job) {
        let card = {
            let mut cards = self.cards.lock().unwrap();
            cards
                .entry(luid)
                .or_insert_with(|| {
                    let card = Arc::new(Card::default());
                    let (me, it) = (self.shared.clone(), card.clone());
                    let worker = spawn_worker("kiln-card", move || {
                        while let Some(job) = it.next() {
                            run(&me, job, None);
                        }
                    })
                    .expect("start a worker");
                    self.workers.lock().unwrap().push(worker);
                    card
                })
                .clone()
        };
        let (id, cancel) = (job.id, job.cancel.clone());
        card.line.lock().unwrap().jobs.push_back(job);
        card.ready.notify_one();
        // Cancelled on its way here, even between the look for it in the
        // lines and the push: it never waits. A cancel after the push finds
        // it in the line itself, and a second withdraw finds nothing.
        if cancel.is_cancelled() {
            self.withdraw(id);
        }
    }

    /// Takes the job `id` out of whichever line holds it and reports it
    /// cancelled. False when no line holds it (it is running, done, or
    /// elsewhere).
    fn withdraw(&self, id: JobId) -> bool {
        let cards: Vec<Arc<Card>> = self.cards.lock().unwrap().values().cloned().collect();
        for card in cards {
            let found = {
                let mut line = card.line.lock().unwrap();
                let at = line.jobs.iter().position(|j| j.id == id);
                at.and_then(|i| line.jobs.remove(i))
            };
            if found.is_some() {
                self.shared.cancels.lock().unwrap().remove(&id);
                (self.shared.on_event)(JobEvent::Finished(
                    id,
                    Err(JobError::from(Error::Cancelled)),
                ));
                return true;
            }
        }
        false
    }

    /// The ids of every job waiting in a line.
    fn waiting(&self) -> Vec<JobId> {
        let cards: Vec<Arc<Card>> = self.cards.lock().unwrap().values().cloned().collect();
        cards
            .iter()
            .flat_map(|c| {
                c.line
                    .lock()
                    .unwrap()
                    .jobs
                    .iter()
                    .map(|j| j.id)
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Lets every card's worker end once its line is empty (cancelled jobs
    /// end at once), and waits for them.
    fn close(&self) {
        for card in self.cards.lock().unwrap().values() {
            card.line.lock().unwrap().closed = true;
            card.ready.notify_all();
        }
        let workers: Vec<_> = self.workers.lock().unwrap().drain(..).collect();
        for w in workers {
            let _ = w.join();
        }
    }
}

impl Card {
    /// The next job in line, waiting for one; `None` once the line is
    /// closed and empty.
    fn next(&self) -> Option<Job> {
        let mut line = self.line.lock().unwrap();
        loop {
            if let Some(job) = line.jobs.pop_front() {
                return Some(job);
            }
            if line.closed {
                return None;
            }
            line = self.ready.wait(line).unwrap();
        }
    }
}

/// The audio pool's slots, counted in halves: a lossless or PCM job is
/// bound by the disk more than the processor, so two of them share a slot.
struct Slots {
    free: Mutex<usize>,
    freed: Condvar,
}

impl Slots {
    fn new(slots: usize) -> Self {
        Slots {
            free: Mutex::new(slots * 2),
            freed: Condvar::new(),
        }
    }

    /// Waits for `halves` half slots. `false` when the job was cancelled
    /// first, which is then never run.
    fn take(&self, halves: usize, cancel: &CancelToken) -> bool {
        let mut free = self.free.lock().unwrap();
        loop {
            if cancel.is_cancelled() {
                return false;
            }
            if *free >= halves {
                *free -= halves;
                return true;
            }
            free = self
                .freed
                .wait_timeout(free, Duration::from_millis(50))
                .unwrap()
                .0;
        }
    }

    fn give_back(&self, halves: usize) {
        *self.free.lock().unwrap() += halves;
        self.freed.notify_all();
    }
}

impl Runner {
    /// Start `workers` threads (at least one) for most jobs, and
    /// [`Runner::audio_workers`] slots for sound Kiln writes itself.
    /// `on_event` is called from the worker threads.
    pub fn new(
        kiln: Arc<Kiln>,
        workers: usize,
        on_event: impl Fn(JobEvent) + Send + Sync + 'static,
    ) -> Self {
        Self::with_audio_pool(kiln, workers, Self::audio_workers(), on_event)
    }

    /// [`Runner::new`] with `audio` slots (at least one) in the audio pool.
    pub fn with_audio_pool(
        kiln: Arc<Kiln>,
        workers: usize,
        audio: usize,
        on_event: impl Fn(JobEvent) + Send + Sync + 'static,
    ) -> Self {
        let cores = thread::available_parallelism().map_or(2, |n| n.get());
        Self::build(kiln, workers, audio, cores, on_event)
    }

    /// A runner whose jobs share a processor budget of `capacity`.
    fn build(
        kiln: Arc<Kiln>,
        workers: usize,
        audio: usize,
        capacity: usize,
        on_event: impl Fn(JobEvent) + Send + Sync + 'static,
    ) -> Self {
        let audio = audio.max(1);
        let cancels: Arc<Mutex<HashMap<JobId, CancelToken>>> = Arc::default();
        let gate = Arc::new(Gate::new(capacity));
        let shared = Shared {
            kiln: kiln.clone(),
            cancels: cancels.clone(),
            on_event: Arc::new(on_event),
            gate: gate.clone(),
        };
        let audio_slots = Arc::new(Slots::new(audio));
        let cpu = Pool::new(&shared, workers, None);
        // A thread per half slot, so lossless jobs can fill every slot twice.
        let audio_pool = Pool::new(&shared, audio * 2, Some(audio_slots.clone()));
        let cards = Arc::new(Cards {
            shared,
            cards: Mutex::default(),
            workers: Mutex::default(),
        });
        Self {
            cpu,
            audio: audio_pool,
            sorter: Mutex::default(),
            kiln,
            cancels,
            audio_slots,
            cards,
            gate,
            next: AtomicU64::new(1),
        }
    }

    /// How many threads the runner has started, in its pools, its sorter
    /// and its card lanes.
    #[cfg(test)]
    fn threads_started(&self) -> usize {
        self.cpu.started()
            + self.audio.started()
            + usize::from(self.sorter.lock().unwrap().thread.is_some())
            + self.cards.workers.lock().unwrap().len()
    }

    /// A sensible pool size: engines are multi-threaded themselves, so use
    /// about half the cores, between one and four.
    pub fn default_workers() -> usize {
        let cores = thread::available_parallelism().map_or(2, |n| n.get());
        (cores / 2).clamp(1, 4)
    }

    /// The audio pool's size: the logical cores less one, so the window
    /// stays smooth, and at least one.
    pub fn audio_workers() -> usize {
        let cores = thread::available_parallelism().map_or(2, |n| n.get());
        cores.saturating_sub(1).max(1)
    }

    pub fn submit(&self, kind: JobKind, options: Options) -> JobId {
        let id = JobId(self.next.fetch_add(1, Ordering::Relaxed));
        let cancel = CancelToken::new();
        self.cancels.lock().unwrap().insert(id, cancel.clone());
        let halves = audio_halves(&self.kiln, &kind);
        let video = halves == 0 && is_video(&self.kiln, &kind);
        let weight = if halves > 0 || video {
            1
        } else {
            weight_of(&self.kiln, &kind)
        };
        let job = Job {
            id,
            kind,
            options,
            cancel,
            halves,
            weight,
        };
        if halves > 0 {
            self.audio.send(job);
        } else if video {
            self.sort(job);
        } else {
            self.cpu.send(job);
        }
        id
    }

    /// Hands a video job to the sorting thread, starting it the first time.
    fn sort(&self, job: Job) {
        let mut s = self.sorter.lock().unwrap();
        if s.closed {
            return;
        }
        if s.tx.is_none() {
            let (tx, rx) = mpsc::channel::<Job>();
            let (cpu, cards) = (self.cpu.clone(), self.cards.clone());
            s.thread = Some(
                spawn_worker("kiln-sorter", move || sort_video(&rx, &cpu, &cards))
                    .expect("start a worker"),
            );
            s.tx = Some(tx);
        }
        if let Some(tx) = &s.tx {
            let _ = tx.send(job);
        }
    }

    /// Stop a queued or running job. It finishes with a cancelled error and
    /// leaves nothing behind.
    pub fn cancel(&self, id: JobId) {
        let token = self.cancels.lock().unwrap().get(&id).cloned();
        if let Some(c) = token {
            c.cancel();
            // One waiting for a graphics card ends now, and one waiting
            // for room in the budget looks at its token now.
            self.cards.withdraw(id);
            self.gate.wake();
        }
    }

    pub fn cancel_all(&self) {
        for c in self.cancels.lock().unwrap().values() {
            c.cancel();
        }
        for id in self.cards.waiting() {
            self.cards.withdraw(id);
        }
        self.gate.wake();
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        self.cancel_all();
        self.audio_slots.freed.notify_all();
        self.gate.wake();
        // The sorter hands on what it holds, then the cards' jobs end.
        let thread = {
            let mut s = self.sorter.lock().unwrap();
            s.closed = true;
            s.tx = None;
            s.thread.take()
        };
        if let Some(s) = thread {
            let _ = s.join();
        }
        self.cards.close();
        self.cpu.close();
        self.audio.close();
    }
}

/// The half slots of the audio pool `kind` takes, or 0 when it isn't a
/// conversion Kiln's own sound engine does. Lossless and PCM targets take
/// one, the rest two.
fn audio_halves(kiln: &Kiln, kind: &JobKind) -> usize {
    let JobKind::Convert { input, target } = kind else {
        return 0;
    };
    let r = kiln.registry();
    let (Some(from), Some(to)) = (r.format_of(input), r.format(target)) else {
        return 0;
    };
    let ours = r.route(from, to).is_some_and(|steps| {
        steps
            .iter()
            .all(|s| crate::audio::is_audio_backend(&s.backend))
    });
    match kiln_media::Target::from_ext(&to.ext) {
        Some(t) if ours && t.lossless() => 1,
        _ if ours => 2,
        _ => 0,
    }
}

/// What `kind` takes of the processor budget: the whole of it for a video
/// FFmpeg encodes, 2 for LibreOffice or Office, 1 for anything else. A video Kiln
/// writes itself is weighed by the sorter, once it knows the lane.
fn weight_of(kiln: &Kiln, kind: &JobKind) -> usize {
    let JobKind::Convert { input, target } = kind else {
        return 1;
    };
    let r = kiln.registry();
    let (Some(from), Some(to)) = (r.format_of(input), r.format(target)) else {
        return 1;
    };
    let Some(steps) = r.route(from, to) else {
        return 1;
    };
    let encodes_video = |s: &crate::registry::Step| {
        s.backend == "ffmpeg" && r.format(&s.to).is_some_and(|f| f.kind == Kind::Video)
    };
    if steps.iter().any(encodes_video) {
        WHOLE
    } else if steps
        .iter()
        .any(|s| s.backend == "libreoffice" || s.backend == "office")
    {
        2
    } else {
        1
    }
}

/// Whether `kind` is a conversion Kiln's own video engine does.
fn is_video(kiln: &Kiln, kind: &JobKind) -> bool {
    let JobKind::Convert { input, target } = kind else {
        return false;
    };
    let r = kiln.registry();
    let (Some(from), Some(to)) = (r.format_of(input), r.format(target)) else {
        return false;
    };
    r.route(from, to)
        .is_some_and(|steps| steps.iter().all(|s| s.backend == crate::video::VIDEO))
}

/// The sorting thread: hands each video job from `rx` to the pool of the
/// graphics card that will encode it, or to the processor pool `cpu`, until
/// the runner is dropped. A cancelled job goes straight to the processor
/// pool, which says so. A look at a file that panics sends it there too.
fn sort_video(rx: &Receiver<Job>, cpu: &Pool, cards: &Cards) {
    while let Ok(mut job) = rx.recv() {
        let lane = match &job.kind {
            JobKind::Convert { input, target } if !job.cancel.is_cancelled() => {
                panic::catch_unwind(AssertUnwindSafe(|| {
                    crate::video::lane(input, target, &job.options)
                }))
                .unwrap_or(crate::video::Lane::Processor)
            }
            _ => crate::video::Lane::Processor,
        };
        match lane {
            // A graphics card counts as one, its encode being the card's.
            crate::video::Lane::Card(luid) => {
                job.weight = 1;
                cards.send(luid, job);
            }
            // A copy encodes nothing, so it counts as one.
            crate::video::Lane::Copy => {
                job.weight = 1;
                cpu.send(job);
            }
            // An encode on the processor takes all of it.
            crate::video::Lane::Processor => {
                job.weight = WHOLE;
                cpu.send(job);
            }
        }
    }
}

/// One worker: takes jobs from `rx` until the runner is dropped. In the
/// audio pool (`slots`), a job waits for its half slots first.
fn work(me: &Shared, rx: &Mutex<Receiver<Job>>, slots: Option<&Slots>, outstanding: &AtomicUsize) {
    loop {
        let job = match rx.lock().unwrap().recv() {
            Ok(job) => job,
            Err(_) => return,
        };
        run(me, job, slots);
        outstanding.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Runs one job and reports it. With `slots`, the job waits for its
/// `halves` first, and every job waits for room in the processor budget;
/// one cancelled while it waits never runs.
fn run(me: &Shared, job: Job, slots: Option<&Slots>) {
    let (kiln, cancels, on_event) = (&*me.kiln, &*me.cancels, &*me.on_event);
    let id = job.id;
    let held = match slots {
        Some(s) if s.take(job.halves, &job.cancel) => Some((s, job.halves)),
        _ => None,
    };
    // Frees the job's room however this function ends, a panic included.
    let admitted = if job.cancel.is_cancelled() {
        None
    } else {
        me.gate.enter(job.weight, &job.cancel)
    };
    let result = if job.cancel.is_cancelled() {
        Err(JobError::from(Error::Cancelled))
    } else {
        let mut progress = |p: Progress| on_event(JobEvent::Progress(id, p));
        let doing = job.kind.doing();
        if let JobKind::Convert { input, target } = &job.kind {
            // Reading the file for its tracks must not take the worker down:
            // a file that panics it is a job with nothing to say.
            let notes = panic::catch_unwind(AssertUnwindSafe(|| {
                kiln.drops_reading_with(input, target, &job.options, &job.cancel)
            }))
            .unwrap_or_default();
            if !notes.is_empty() {
                on_event(JobEvent::Notes(id, notes));
            }
        }
        caught(&job.cancel, &doing, || match &job.kind {
            JobKind::Convert { input, target } => {
                kiln.convert_outputs(input, target, &job.options, &mut progress, &job.cancel)
            }
            JobKind::Tool { tool, inputs } => {
                kiln.run_tool_with(*tool, inputs, &job.options, &mut progress, &job.cancel)
            }
        })
    };
    drop(admitted);
    if let Some((s, halves)) = held {
        s.give_back(halves);
    }
    cancels.lock().unwrap().remove(&id);
    on_event(JobEvent::Finished(id, result));
}

/// Run a job's work. A panic in it is a failed job, not a lost worker:
/// the row gets its result and the pool keeps its size. A job the user
/// had already cancelled when it panicked is cancelled, not failed.
fn caught(
    cancel: &CancelToken,
    doing: &Doing,
    f: impl FnOnce() -> crate::Result<Vec<PathBuf>>,
) -> Result<Vec<PathBuf>, JobError> {
    panic::catch_unwind(AssertUnwindSafe(f))
        .map(|r| r.map_err(|e| JobError::new(e, doing)))
        .unwrap_or_else(|p| {
            Err(if cancel.is_cancelled() {
                JobError::from(Error::Cancelled)
            } else {
                panicked(p)
            })
        })
}

/// The failed job a panic becomes, with the panic's text in the details.
fn panicked(payload: Box<dyn Any + Send>) -> JobError {
    let details = payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_default();
    JobError {
        message: PANICKED.to_owned(),
        details,
        engine: None,
        cancelled: false,
        no_room: None,
        store_product: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> Doing {
        Doing::default()
    }

    fn failed(engine: &'static str, summary: &str, details: &str) -> Error {
        Error::EngineFailed {
            engine,
            summary: summary.into(),
            details: details.into(),
        }
    }

    #[test]
    fn a_panicking_job_is_a_failed_job() {
        let e = caught(&CancelToken::new(), &none(), || {
            panic!("handles() checked the type")
        })
        .unwrap_err();
        assert_eq!(e.message, PANICKED);
        assert_eq!(e.details, "handles() checked the type");
        assert!(!e.cancelled);
        let n = 3;
        let e = caught(&CancelToken::new(), &none(), || panic!("{n} went wrong")).unwrap_err();
        assert_eq!(e.details, "3 went wrong");
        assert_eq!(
            caught(&CancelToken::new(), &none(), || Ok(vec![])),
            Ok(vec![])
        );
        let e = caught(&CancelToken::new(), &none(), || Err(Error::Cancelled)).unwrap_err();
        assert!(e.cancelled);
    }

    #[test]
    fn a_job_that_panics_after_a_cancel_is_cancelled() {
        let cancel = CancelToken::new();
        let e = caught(&cancel, &none(), || {
            cancel.cancel();
            panic!("the engine went down while stopping")
        })
        .unwrap_err();
        assert!(e.cancelled);
        assert_eq!(e, JobError::from(Error::Cancelled));
    }

    #[test]
    fn a_folder_that_takes_no_files_is_named_in_its_row() {
        let dir = PathBuf::from("Users").join("Ana").join("Card");
        let e = JobError::from(Error::CantSaveIn(dir.clone()));
        assert_eq!(e.message, "Kiln can't save in Card.");
        // The whole path waits in Show details.
        assert_eq!(e.details, format!("Kiln can't save in {}.", dir.display()));
        assert_eq!(e.engine, None);
        assert!(!e.cancelled);
    }

    #[test]
    fn a_refused_file_keeps_its_sentence() {
        let e = JobError::from(Error::Unsafe("This PDF is built to loop.".into()));
        assert_eq!(e.message, "This PDF is built to loop.");
        assert_eq!(e.details, "");
        assert_eq!(e.engine, None);
        assert!(!e.cancelled);
    }

    #[test]
    fn an_engine_failure_becomes_a_plain_sentence() {
        let gif = JobKind::Convert {
            input: r"C:\clips\take.mp4".into(),
            target: "gif".into(),
        }
        .doing();
        let e = JobError::new(
            failed(
                "FFmpeg",
                "Error opening input: Invalid data found when processing input",
                "ffmpeg version 7\nError opening input: Invalid data found when processing input",
            ),
            &gif,
        );
        assert_eq!(
            e.message,
            "This file looks damaged, or isn't really an MP4. Try opening it in another app."
        );
        assert!(e.details.starts_with("ffmpeg version 7"));
        assert_eq!(e.engine, Some("FFmpeg"));

        // Kiln's own PDF failures have only a summary: when it is a plain
        // sentence it is the row line, and it is the details too.
        let e = JobError::new(failed("Kiln", "the PDF has no pages.", ""), &gif);
        assert_eq!(e.message, "The PDF has no pages.");
        assert_eq!(e.details, "the PDF has no pages.");
        // One with a library's words in it is not shown on the row.
        let e = JobError::new(
            failed("Kiln", "the PDF's text couldn't be read (bad font).", ""),
            &gif,
        );
        assert_eq!(
            e.message,
            "The conversion stopped before it finished. Try again."
        );
        // A PDF Kiln couldn't read at all is damaged (D33).
        let e = JobError::new(
            failed("Kiln", "the PDF couldn't be read (bad xref).", ""),
            &gif,
        );
        assert!(e.message.starts_with("This file looks damaged"), "{e:?}");
        assert_eq!(e.details, "the PDF couldn't be read (bad xref).");

        // Image and document failures never put a path on the row.
        let path = std::path::PathBuf::from("C:/x/photo.png");
        for err in [
            Error::WrongContent {
                path: path.clone(),
                code: "PNG".into(),
            },
            Error::Decode {
                path: path.clone(),
                reason: "unexpected end of file".into(),
            },
            Error::InputNotFound(path.clone()),
        ] {
            let shown = err.to_string();
            let e = JobError::new(err, &gif);
            assert!(
                !e.message.contains("photo.png") && !e.message.contains("C:"),
                "{}",
                e.message
            );
            assert_eq!(e.details, shown, "the path waits in the details");
        }
        let e = JobError::new(
            Error::WrongContent {
                path,
                code: "PNG".into(),
            },
            &gif,
        );
        assert_eq!(
            e.message,
            "This file looks damaged, or isn't really a PNG. Try opening it in another app."
        );
    }

    #[test]
    fn a_tool_says_what_it_couldnt_do() {
        let trim = JobKind::Tool {
            tool: Tool::RemoveQuietEnds,
            inputs: vec![r"C:\a\talk.wav".into()],
        }
        .doing();
        let e = JobError::new(failed("FFmpeg", "Conversion failed!", ""), &trim);
        assert_eq!(e.message, "The job stopped before it finished. Try again.");
    }

    #[test]
    fn a_files_name_is_kept_out_of_the_matching() {
        let mp3 = JobKind::Convert {
            input: r"C:\Encrypted\Password reset.mp4".into(),
            target: "mp3".into(),
        }
        .doing();
        let raw = "Input #0, mov from 'C:\\Encrypted\\Password reset.mp4':\n\
                   Output #0 to 'C:\\held\\1\\Password reset.mp3':\nConversion failed!";
        let e = JobError::new(failed("LibreOffice", "Conversion failed!", raw), &mp3);
        assert_eq!(
            e.message,
            "The conversion stopped before it finished. Try again."
        );
    }

    #[test]
    fn a_missing_engine_says_where_to_look() {
        let e = JobError::new(Error::EngineMissing { engine: "Pandoc" }, &none());
        assert_eq!(
            e.message,
            "Needs Pandoc, which isn't installed. Settings > Helper apps says where to get it."
        );
        assert!(e.details.is_empty());
        assert_eq!(e.engine, None);
    }

    #[test]
    fn a_job_says_what_it_does() {
        let convert = JobKind::Convert {
            input: "a/clip.MOV".into(),
            target: "mp4".into(),
        };
        let d = convert.doing();
        assert_eq!(
            (d.verb, d.source.as_str(), d.target.as_str()),
            ("convert", "MOV", "mp4")
        );
        assert_eq!(
            d.paths,
            [PathBuf::from("a/clip.MOV"), PathBuf::from("a/clip.mp4")]
        );
        let tool = JobKind::Tool {
            tool: Tool::Merge,
            inputs: vec!["a.pdf".into(), "b.pdf".into()],
        };
        let d = tool.doing();
        assert_eq!(
            (d.verb, d.source.as_str(), d.target.as_str()),
            ("merge", "pdf", "pdf")
        );
        assert_eq!(d.paths.len(), 2);
    }
    /// Stands in for an engine: holds its job until let go or cancelled,
    /// counting how many of its jobs run at once.
    struct Held {
        id: &'static str,
        go: Arc<std::sync::atomic::AtomicBool>,
        running: Arc<std::sync::atomic::AtomicUsize>,
        most: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl Held {
        fn new(id: &'static str) -> Self {
            Held {
                id,
                go: Arc::default(),
                running: Arc::default(),
                most: Arc::default(),
            }
        }

        fn handles(&self) -> Self {
            Held {
                id: self.id,
                go: self.go.clone(),
                running: self.running.clone(),
                most: self.most.clone(),
            }
        }
    }

    impl crate::converter::Converter for Held {
        fn id(&self) -> &'static str {
            self.id
        }

        fn convert(
            &self,
            req: &crate::converter::Request<'_>,
            ctx: &mut crate::converter::Context<'_>,
        ) -> crate::Result<()> {
            let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
            self.most.fetch_max(now, Ordering::SeqCst);
            let result = loop {
                if self.go.load(Ordering::SeqCst) {
                    break std::fs::write(req.output, b"x").map_err(|e| Error::io(req.output, e));
                }
                if ctx.cancel.is_cancelled() {
                    break Err(Error::Cancelled);
                }
                thread::sleep(std::time::Duration::from_millis(5));
            };
            self.running.fetch_sub(1, Ordering::SeqCst);
            result
        }
    }

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures")
            .join(name)
    }

    /// Waits up to ten seconds for `f` to hold.
    fn eventually(f: impl Fn() -> bool) -> bool {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < end {
            if f() {
                return true;
            }
            thread::sleep(std::time::Duration::from_millis(5));
        }
        f()
    }

    /// Options that write into `dir`.
    fn out_to(dir: &tempfile::TempDir) -> Options {
        Options {
            out_dir: Some(dir.path().to_path_buf()),
            temp_root: Some(dir.path().join("work")),
            ..Options::default()
        }
    }

    fn convert(input: &str, target: &str) -> JobKind {
        JobKind::Convert {
            input: fixture(input),
            target: target.into(),
        }
    }

    type Events = Arc<Mutex<Vec<JobEvent>>>;

    fn finished(events: &Events, id: JobId) -> Option<Result<Vec<PathBuf>, JobError>> {
        events.lock().unwrap().iter().find_map(|e| match e {
            JobEvent::Finished(i, r) if *i == id => Some(r.clone()),
            _ => None,
        })
    }

    #[test]
    fn audio_jobs_get_their_own_pool() {
        let cores = thread::available_parallelism().map_or(2, |n| n.get());
        assert_eq!(Runner::audio_workers(), cores.saturating_sub(1).max(1));

        // Every video worker is busy with a job that doesn't end.
        let video = Held::new("ffmpeg");
        let kiln = Kiln::new().with_converter(Box::new(video.handles()));
        let events: Events = Arc::default();
        let seen = events.clone();
        let workers = Runner::default_workers();
        let runner = Runner::build(Arc::new(kiln), workers, 1, workers + 1, move |e| {
            seen.lock().unwrap().push(e)
        });
        let dir = tempfile::tempdir().unwrap();
        for _ in 0..workers {
            runner.submit(convert("sample.mp4", "gif"), out_to(&dir));
        }
        assert!(eventually(
            || video.running.load(Ordering::SeqCst) == workers
        ));

        // A sound still converts.
        let audio = runner.submit(convert("sample.wav", "flac"), out_to(&dir));
        assert!(eventually(|| finished(&events, audio).is_some()));
        let out = finished(&events, audio).unwrap().unwrap();
        assert!(std::fs::read(&out[0]).unwrap().starts_with(b"fLaC"));
        assert_eq!(video.running.load(Ordering::SeqCst), workers);
        video.go.store(true, Ordering::SeqCst);
    }

    #[test]
    fn jobs_weigh_what_they_take_of_the_processor() {
        let ffmpeg = Held::new("ffmpeg");
        let kiln = Kiln::new().with_converter(Box::new(ffmpeg.handles()));
        // A video FFmpeg encodes takes the whole budget.
        assert_eq!(weight_of(&kiln, &convert("sample.mp4", "webm")), WHOLE);
        // Anything else FFmpeg does, and a tool, counts one.
        assert_eq!(weight_of(&kiln, &convert("sample.mp4", "gif")), 1);
        let tool = JobKind::Tool {
            tool: Tool::RemoveQuietEnds,
            inputs: vec![fixture("sample.mp4")],
        };
        assert_eq!(weight_of(&kiln, &tool), 1);
        // LibreOffice counts two.
        let office = Held::new("libreoffice");
        let kiln = Kiln::new().with_converter(Box::new(office.handles()));
        assert_eq!(weight_of(&kiln, &convert("sample.docx", "pdf")), 2);
    }

    #[test]
    fn no_pool_thread_exists_until_work_is_queued() {
        let video = Held::new("ffmpeg");
        let kiln = Kiln::new().with_converter(Box::new(video.handles()));
        let events: Events = Arc::default();
        let seen = events.clone();
        let runner = Runner::build(Arc::new(kiln), 4, 2, 8, move |e| {
            seen.lock().unwrap().push(e)
        });
        assert_eq!(runner.threads_started(), 0);

        // One job starts one thread, not the pool.
        let dir = tempfile::tempdir().unwrap();
        let id = runner.submit(convert("sample.mp4", "gif"), out_to(&dir));
        assert_eq!(runner.threads_started(), 1);
        assert!(eventually(|| video.running.load(Ordering::SeqCst) == 1));
        // A second, while the first holds its thread, starts another.
        runner.submit(convert("sample.mp4", "gif"), out_to(&dir));
        assert_eq!(runner.threads_started(), 2);
        video.go.store(true, Ordering::SeqCst);
        assert!(eventually(|| finished(&events, id).is_some()));
    }

    #[test]
    fn an_audio_job_waits_behind_a_running_whole_budget_encode() {
        let video = Held::new("ffmpeg");
        let audio = Held::new("audio");
        audio.go.store(true, Ordering::SeqCst);
        let kiln = Kiln::new()
            .with_converter(Box::new(video.handles()))
            .with_converter(Box::new(audio.handles()));
        let events: Events = Arc::default();
        let seen = events.clone();
        let runner = Runner::build(Arc::new(kiln), 2, 2, 8, move |e| {
            seen.lock().unwrap().push(e)
        });
        let dir = tempfile::tempdir().unwrap();
        let encode = runner.submit(convert("sample.mp4", "webm"), out_to(&dir));
        assert!(eventually(|| video.running.load(Ordering::SeqCst) == 1));
        let sound = runner.submit(convert("sample.wav", "flac"), out_to(&dir));
        thread::sleep(std::time::Duration::from_millis(300));
        assert_eq!(audio.most.load(Ordering::SeqCst), 0, "it shares the budget");
        assert!(finished(&events, sound).is_none());
        video.go.store(true, Ordering::SeqCst);
        assert!(eventually(|| finished(&events, encode).is_some()));
        assert!(eventually(|| finished(&events, sound).is_some()));
        assert!(finished(&events, sound).unwrap().is_ok());
    }

    #[test]
    fn two_processor_video_encodes_never_run_together() {
        let video = Held::new("ffmpeg");
        let kiln = Kiln::new().with_converter(Box::new(video.handles()));
        let events: Events = Arc::default();
        let seen = events.clone();
        let runner = Runner::build(Arc::new(kiln), 4, 1, 8, move |e| {
            seen.lock().unwrap().push(e)
        });
        let dir = tempfile::tempdir().unwrap();
        let ids = [
            runner.submit(convert("sample.mp4", "webm"), out_to(&dir)),
            runner.submit(convert("sample.mov", "webm"), out_to(&dir)),
        ];
        assert!(eventually(|| video.running.load(Ordering::SeqCst) == 1));
        // The second has a thread free and still waits for the first.
        thread::sleep(std::time::Duration::from_millis(300));
        assert_eq!(video.running.load(Ordering::SeqCst), 1);
        video.go.store(true, Ordering::SeqCst);
        assert!(eventually(|| ids
            .iter()
            .all(|&id| finished(&events, id).is_some())));
        assert_eq!(video.most.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn lossless_audio_jobs_take_half_a_slot() {
        let run_two = |target: &str| -> usize {
            let audio = Held::new("audio");
            let kiln = Kiln::new().with_converter(Box::new(audio.handles()));
            let events: Events = Arc::default();
            let seen = events.clone();
            let runner = Runner::with_audio_pool(Arc::new(kiln), 1, 1, move |e| {
                seen.lock().unwrap().push(e)
            });
            let dir = tempfile::tempdir().unwrap();
            let ids = [
                runner.submit(convert("sample.wav", target), out_to(&dir)),
                runner.submit(convert("sample.wav", target), out_to(&dir)),
            ];
            // The first starts; then give a second time to start if it may.
            assert!(eventually(|| audio.running.load(Ordering::SeqCst) >= 1));
            let end = std::time::Instant::now() + std::time::Duration::from_millis(500);
            while audio.running.load(Ordering::SeqCst) < 2 && std::time::Instant::now() < end {
                thread::sleep(std::time::Duration::from_millis(5));
            }
            let most = audio.most.load(Ordering::SeqCst);
            audio.go.store(true, Ordering::SeqCst);
            assert!(eventually(|| ids
                .iter()
                .all(|&id| finished(&events, id).is_some())));
            assert!(ids.iter().all(|&id| finished(&events, id).unwrap().is_ok()));
            most
        };
        assert_eq!(run_two("flac"), 2, "two lossless jobs share one slot");
        assert_eq!(run_two("mp3"), 1, "a lossy job takes a whole slot");
    }

    #[test]
    fn an_extension_failure_carries_its_store_product() {
        let e = JobError::new(
            Error::NeedsExtension {
                message: "Needs Microsoft's HEVC Video Extensions to read HEVC video.".into(),
                product: "9N4WGH0Z6VHQ",
            },
            &none(),
        );
        assert_eq!(
            e.message,
            "Needs Microsoft's HEVC Video Extensions to read HEVC video."
        );
        assert_eq!(e.store_product, Some("9N4WGH0Z6VHQ"));
        assert!(e.details.is_empty() && e.engine.is_none() && !e.cancelled);
        assert_eq!(JobError::from(Error::Cancelled).store_product, None);
    }

    #[test]
    fn video_jobs_are_sorted_and_run() {
        let video = Held::new("video");
        video.go.store(true, Ordering::SeqCst);
        let kiln = Kiln::new().with_converter(Box::new(video.handles()));
        assert!(is_video(&kiln, &convert("sample.mp4", "mkv")));
        assert!(!is_video(&kiln, &convert("sample.mp4", "webm")));
        assert!(!is_video(&kiln, &convert("sample.mp4", "mp3")));
        let events: Events = Arc::default();
        let seen = events.clone();
        let runner = Runner::new(Arc::new(kiln), 1, move |e| seen.lock().unwrap().push(e));
        let dir = tempfile::tempdir().unwrap();
        let copy = runner.submit(convert("sample.mp4", "mkv"), out_to(&dir));
        let mut encode = out_to(&dir);
        encode.quality.video.setting = Some(kiln_media::video::VideoSetting::Quality(50));
        let encoded = runner.submit(convert("sample.mov", "mp4"), encode);
        for id in [copy, encoded] {
            assert!(eventually(|| finished(&events, id).is_some()));
            let out = finished(&events, id).unwrap().unwrap();
            assert_eq!(std::fs::read(&out[0]).unwrap(), b"x");
        }
        // A video job cancelled, sorted or not, ends cancelled.
        video.go.store(false, Ordering::SeqCst);
        let id = runner.submit(convert("sample.mp4", "mov"), out_to(&dir));
        runner.cancel(id);
        assert!(eventually(|| finished(&events, id).is_some()));
        assert!(finished(&events, id).unwrap().unwrap_err().cancelled);
    }

    #[test]
    fn a_graphics_card_takes_one_video_encode_at_a_time() {
        let video = Held::new("video");
        let kiln = Kiln::new().with_converter(Box::new(video.handles()));
        let events: Events = Arc::default();
        let seen = events.clone();
        let runner = Runner::new(Arc::new(kiln), 4, move |e| seen.lock().unwrap().push(e));
        let dir = tempfile::tempdir().unwrap();
        let mut encode = out_to(&dir);
        encode.quality.video.setting = Some(kiln_media::video::VideoSetting::Quality(50));
        let ids = [
            runner.submit(convert("sample.mov", "mp4"), encode.clone()),
            runner.submit(convert("sample.mov", "mp4"), encode),
        ];
        assert!(eventually(|| video.running.load(Ordering::SeqCst) >= 1));
        let end = std::time::Instant::now() + std::time::Duration::from_millis(500);
        while video.running.load(Ordering::SeqCst) < 2 && std::time::Instant::now() < end {
            thread::sleep(std::time::Duration::from_millis(5));
        }
        let most = video.most.load(Ordering::SeqCst);
        let card = kiln_media::video::encoder_adapter(
            kiln_media::video::VCodec::H264,
            kiln_media::video::EncoderChoice::Automatic,
        );
        // A copy runs in the processor pool alongside a card's encode. An
        // encode on the processor takes the whole budget, so nothing runs
        // beside it.
        let mut jobs = ids.to_vec();
        if card.is_some() {
            let copy = runner.submit(convert("sample.mp4", "mkv"), out_to(&dir));
            assert!(eventually(
                || video.running.load(Ordering::SeqCst) == most + 1
            ));
            jobs.push(copy);
        }
        video.go.store(true, Ordering::SeqCst);
        for id in jobs {
            assert!(eventually(|| finished(&events, id).is_some()));
            assert!(finished(&events, id).unwrap().is_ok());
        }
        // One at a time on the card, and one at a time on the processor.
        assert_eq!(most, 1, "card: {card:?}");
    }

    #[test]
    fn a_video_job_waiting_for_its_card_cancels_at_once() {
        let video = Held::new("video");
        let kiln = Kiln::new().with_converter(Box::new(video.handles()));
        let events: Events = Arc::default();
        let seen = events.clone();
        let runner = Runner::new(Arc::new(kiln), 1, move |e| seen.lock().unwrap().push(e));
        let dir = tempfile::tempdir().unwrap();
        let mut encode = out_to(&dir);
        encode.quality.video.setting = Some(kiln_media::video::VideoSetting::Quality(50));
        let first = runner.submit(convert("sample.mov", "mp4"), encode.clone());
        assert!(eventually(|| video.running.load(Ordering::SeqCst) == 1));
        // On a card the second waits for it; on the processor (one worker)
        // it waits in the queue. Either way a cancel ends it at once.
        let second = runner.submit(convert("sample.mov", "mp4"), encode);
        thread::sleep(std::time::Duration::from_millis(200));
        let asked = std::time::Instant::now();
        runner.cancel(second);
        let end = asked + std::time::Duration::from_secs(1);
        while finished(&events, second).is_none() && std::time::Instant::now() < end {
            thread::sleep(std::time::Duration::from_millis(5));
        }
        let card = kiln_media::video::encoder_adapter(
            kiln_media::video::VCodec::H264,
            kiln_media::video::EncoderChoice::Automatic,
        );
        if card.is_some() {
            let done = finished(&events, second).expect("reported within a second");
            assert!(done.unwrap_err().cancelled);
            assert!(asked.elapsed() < std::time::Duration::from_millis(500));
            // The first is still running: the second never did.
            assert_eq!(video.running.load(Ordering::SeqCst), 1);
            assert_eq!(video.most.load(Ordering::SeqCst), 1);
        }
        video.go.store(true, Ordering::SeqCst);
        assert!(eventually(|| finished(&events, first).is_some()));
        assert!(eventually(|| finished(&events, second).is_some()));
        assert!(finished(&events, second).unwrap().unwrap_err().cancelled);
    }

    #[test]
    fn a_job_cancelled_on_its_way_to_a_busy_card_never_waits() {
        // Item 9 of the final review: a cancel that lands after the
        // runner's withdraw found nothing, but before the job is in the
        // line, is seen once it is there: the job ends at once, not when
        // the card is free.
        let video = Held::new("video");
        let kiln = Kiln::new().with_converter(Box::new(video.handles()));
        let events: Events = Arc::default();
        let seen = events.clone();
        let cancels: Arc<Mutex<HashMap<JobId, CancelToken>>> = Arc::default();
        let cards = Cards {
            shared: Shared {
                kiln: Arc::new(kiln),
                cancels: cancels.clone(),
                on_event: Arc::new(move |e| seen.lock().unwrap().push(e)),
                gate: Arc::new(Gate::new(8)),
            },
            cards: Mutex::default(),
            workers: Mutex::default(),
        };
        let dir = tempfile::tempdir().unwrap();
        let job = |n: u64| {
            let cancel = CancelToken::new();
            cancels.lock().unwrap().insert(JobId(n), cancel.clone());
            Job {
                id: JobId(n),
                kind: convert("sample.mov", "mp4"),
                options: out_to(&dir),
                cancel,
                halves: 0,
                weight: 1,
            }
        };
        cards.send(7, job(1));
        assert!(eventually(|| video.running.load(Ordering::SeqCst) == 1));
        let second = job(2);
        // The runner's cancel: nothing in a line yet, then the flag.
        assert!(!cards.withdraw(JobId(2)));
        second.cancel.cancel();
        cards.send(7, second);
        let done = finished(&events, JobId(2)).expect("reported at once");
        assert!(done.unwrap_err().cancelled);
        assert!(cards.waiting().is_empty());
        video.go.store(true, Ordering::SeqCst);
        assert!(eventually(|| finished(&events, JobId(1)).is_some()));
        cards.close();
    }

    /// Stands in for the video engine: notes the order jobs start in, by
    /// the page DPI each was given.
    struct Ordered(Arc<Mutex<Vec<u32>>>);

    impl crate::converter::Converter for Ordered {
        fn id(&self) -> &'static str {
            "video"
        }

        fn convert(
            &self,
            req: &crate::converter::Request<'_>,
            _ctx: &mut crate::converter::Context<'_>,
        ) -> crate::Result<()> {
            self.0.lock().unwrap().push(req.options.quality.page_dpi);
            thread::sleep(std::time::Duration::from_millis(20));
            std::fs::write(req.output, b"x").map_err(|e| Error::io(req.output, e))
        }
    }

    #[test]
    fn video_encodes_start_in_the_order_they_were_sent() {
        let order: Arc<Mutex<Vec<u32>>> = Arc::default();
        let kiln = Kiln::new().with_converter(Box::new(Ordered(order.clone())));
        let events: Events = Arc::default();
        let seen = events.clone();
        let runner = Runner::new(Arc::new(kiln), 1, move |e| seen.lock().unwrap().push(e));
        let dir = tempfile::tempdir().unwrap();
        let ids: Vec<JobId> = (1..=8)
            .map(|n| {
                let mut o = out_to(&dir);
                o.quality.video.setting = Some(kiln_media::video::VideoSetting::Quality(50));
                o.quality.page_dpi = 100 + n;
                runner.submit(convert("sample.mov", "mp4"), o)
            })
            .collect();
        assert!(eventually(|| ids
            .iter()
            .all(|&id| finished(&events, id).is_some())));
        assert_eq!(*order.lock().unwrap(), (101..=108).collect::<Vec<u32>>());
    }

    #[test]
    fn a_queued_audio_job_cancels_like_any_other() {
        let audio = Held::new("audio");
        let kiln = Kiln::new().with_converter(Box::new(audio.handles()));
        let events: Events = Arc::default();
        let seen = events.clone();
        let runner =
            Runner::with_audio_pool(Arc::new(kiln), 1, 1, move |e| seen.lock().unwrap().push(e));
        let dir = tempfile::tempdir().unwrap();
        let first = runner.submit(convert("sample.wav", "mp3"), out_to(&dir));
        assert!(eventually(|| audio.running.load(Ordering::SeqCst) == 1));
        let second = runner.submit(convert("sample.wav", "ogg"), out_to(&dir));
        runner.cancel(second);
        assert!(eventually(|| finished(&events, second).is_some()));
        let err = finished(&events, second).unwrap().unwrap_err();
        assert!(err.cancelled);
        assert_eq!(err.message, "Cancelled.");
        assert_eq!(audio.most.load(Ordering::SeqCst), 1, "it never ran");
        runner.cancel(first);
        assert!(eventually(|| finished(&events, first).is_some()));
        assert!(finished(&events, first).unwrap().unwrap_err().cancelled);
        assert!(!dir.path().join("sample.ogg").exists());
        assert!(!dir.path().join("sample.mp3").exists());
    }
}
