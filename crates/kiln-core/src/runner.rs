//! A small worker pool that runs conversions and tools off the UI thread
//! and reports each job's progress and result through a callback.

use std::any::Any;
use std::collections::HashMap;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use crate::converter::{CancelToken, Options, Progress};
use crate::error::Error;
use crate::job::Kiln;
use crate::plain;
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
}

impl JobError {
    /// What `e` tells the user, for a job that read `source` files and
    /// wrote `target` ones (extensions). An engine failure Kiln recognises
    /// becomes a plain sentence with a next step; the engine's own words
    /// go to the details.
    pub fn new(e: Error, source: &str, target: &str) -> Self {
        let (message, details, engine) = match &e {
            Error::EngineFailed {
                engine,
                summary,
                details,
            } => {
                let raw = format!(
                    "{summary}
{details}"
                );
                let shown = if details.trim().is_empty() {
                    summary.clone()
                } else {
                    details.clone()
                };
                (
                    plain::plain_failure_for(engine, source, target, &raw),
                    shown,
                    Some(*engine),
                )
            }
            Error::EngineMissing { engine } => (plain::missing_engine(engine), String::new(), None),
            _ => (e.to_string(), String::new(), None),
        };
        JobError {
            cancelled: matches!(e, Error::Cancelled),
            no_room: e.no_room().map(Into::into),
            message,
            details,
            engine,
        }
    }
}

impl From<Error> for JobError {
    fn from(e: Error) -> Self {
        JobError::new(e, "", "")
    }
}

impl JobKind {
    /// The extensions a job reads and writes, for its error sentences.
    fn exts(&self) -> (String, String) {
        let ext = |p: &PathBuf| {
            p.extension()
                .map(|e| e.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        match self {
            JobKind::Convert { input, target } => (ext(input), target.clone()),
            JobKind::Tool { inputs, .. } => {
                let source = inputs.first().map(ext).unwrap_or_default();
                (source.clone(), source)
            }
        }
    }
}

/// What a job that hit a fault in Kiln itself says.
const PANICKED: &str = "Kiln stopped this job because of a problem in Kiln itself.";

#[derive(Debug, Clone, PartialEq)]
pub enum JobEvent {
    Progress(JobId, Progress),
    Finished(JobId, Result<Vec<PathBuf>, JobError>),
}

struct Job {
    id: JobId,
    kind: JobKind,
    options: Options,
    cancel: CancelToken,
}

pub struct Runner {
    tx: Option<Sender<Job>>,
    cancels: Arc<Mutex<HashMap<JobId, CancelToken>>>,
    workers: Vec<JoinHandle<()>>,
    next: AtomicU64,
}

impl Runner {
    /// Start `workers` threads (at least one). `on_event` is called from
    /// the worker threads.
    pub fn new(
        kiln: Arc<Kiln>,
        workers: usize,
        on_event: impl Fn(JobEvent) + Send + Sync + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        let cancels: Arc<Mutex<HashMap<JobId, CancelToken>>> = Arc::default();
        let on_event = Arc::new(on_event);
        let workers = (0..workers.max(1))
            .map(|_| {
                let (kiln, rx, cancels, on_event) =
                    (kiln.clone(), rx.clone(), cancels.clone(), on_event.clone());
                thread::spawn(move || work(&kiln, &rx, &cancels, &*on_event))
            })
            .collect();
        Self {
            tx: Some(tx),
            cancels,
            workers,
            next: AtomicU64::new(1),
        }
    }

    /// A sensible pool size: engines are multi-threaded themselves, so use
    /// about half the cores, between one and four.
    pub fn default_workers() -> usize {
        let cores = thread::available_parallelism().map_or(2, |n| n.get());
        (cores / 2).clamp(1, 4)
    }

    pub fn submit(&self, kind: JobKind, options: Options) -> JobId {
        let id = JobId(self.next.fetch_add(1, Ordering::Relaxed));
        let cancel = CancelToken::new();
        self.cancels.lock().unwrap().insert(id, cancel.clone());
        let job = Job {
            id,
            kind,
            options,
            cancel,
        };
        if let Some(tx) = &self.tx {
            let _ = tx.send(job);
        }
        id
    }

    /// Stop a queued or running job. It finishes with a cancelled error and
    /// leaves nothing behind.
    pub fn cancel(&self, id: JobId) {
        if let Some(c) = self.cancels.lock().unwrap().get(&id) {
            c.cancel();
        }
    }

    pub fn cancel_all(&self) {
        for c in self.cancels.lock().unwrap().values() {
            c.cancel();
        }
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        self.cancel_all();
        self.tx = None;
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}

fn work(
    kiln: &Kiln,
    rx: &Mutex<Receiver<Job>>,
    cancels: &Mutex<HashMap<JobId, CancelToken>>,
    on_event: &(dyn Fn(JobEvent) + Send + Sync),
) {
    loop {
        let job = match rx.lock().unwrap().recv() {
            Ok(job) => job,
            Err(_) => return,
        };
        let id = job.id;
        let result = if job.cancel.is_cancelled() {
            Err(JobError::from(Error::Cancelled))
        } else {
            let mut progress = |p: Progress| on_event(JobEvent::Progress(id, p));
            let (source, target) = job.kind.exts();
            caught(&job.cancel, (&source, &target), || match &job.kind {
                JobKind::Convert { input, target } => {
                    kiln.convert_outputs(input, target, &job.options, &mut progress, &job.cancel)
                }
                JobKind::Tool { tool, inputs } => {
                    kiln.run_tool_with(*tool, inputs, &job.options, &mut progress, &job.cancel)
                }
            })
        };
        cancels.lock().unwrap().remove(&id);
        on_event(JobEvent::Finished(id, result));
    }
}

/// Run a job's work. A panic in it is a failed job, not a lost worker:
/// the row gets its result and the pool keeps its size. A job the user
/// had already cancelled when it panicked is cancelled, not failed.
fn caught(
    cancel: &CancelToken,
    (source, target): (&str, &str),
    f: impl FnOnce() -> crate::Result<Vec<PathBuf>>,
) -> Result<Vec<PathBuf>, JobError> {
    panic::catch_unwind(AssertUnwindSafe(f))
        .map(|r| r.map_err(|e| JobError::new(e, source, target)))
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NO_EXT: (&str, &str) = ("", "");

    #[test]
    fn a_panicking_job_is_a_failed_job() {
        let e = caught(&CancelToken::new(), NO_EXT, || {
            panic!("handles() checked the type")
        })
        .unwrap_err();
        assert_eq!(e.message, PANICKED);
        assert_eq!(e.details, "handles() checked the type");
        assert!(!e.cancelled);
        let n = 3;
        let e = caught(&CancelToken::new(), NO_EXT, || panic!("{n} went wrong")).unwrap_err();
        assert_eq!(e.details, "3 went wrong");
        assert_eq!(
            caught(&CancelToken::new(), NO_EXT, || Ok(vec![])),
            Ok(vec![])
        );
        let e = caught(&CancelToken::new(), NO_EXT, || Err(Error::Cancelled)).unwrap_err();
        assert!(e.cancelled);
    }

    #[test]
    fn a_job_that_panics_after_a_cancel_is_cancelled() {
        let cancel = CancelToken::new();
        let e = caught(&cancel, NO_EXT, || {
            cancel.cancel();
            panic!("the engine went down while stopping")
        })
        .unwrap_err();
        assert!(e.cancelled);
        assert_eq!(e, JobError::from(Error::Cancelled));
    }

    #[test]
    fn an_engine_failure_becomes_a_plain_sentence() {
        let e = JobError::new(
            Error::EngineFailed {
                engine: "FFmpeg",
                summary: "Error opening input: Invalid data found when processing input".into(),
                details: "ffmpeg version 7
Error opening input: Invalid data found when processing input"
                    .into(),
            },
            "mp4",
            "gif",
        );
        assert_eq!(
            e.message,
            "This file looks damaged, or isn't really a MP4. Try opening it in another app."
        );
        assert!(e.details.starts_with("ffmpeg version 7"));
        assert_eq!(e.engine, Some("FFmpeg"));

        // Kiln's own PDF failures have only a summary: it becomes the
        // details.
        let e = JobError::new(
            Error::EngineFailed {
                engine: "Kiln",
                summary: "the PDF has no pages.".into(),
                details: String::new(),
            },
            "pdf",
            "png",
        );
        assert_eq!(e.message, "Kiln couldn't convert the file.");
        assert_eq!(e.details, "the PDF has no pages.");
    }

    #[test]
    fn a_missing_engine_says_where_to_look() {
        let e = JobError::new(Error::EngineMissing { engine: "Pandoc" }, "md", "docx");
        assert_eq!(
            e.message,
            "Needs Pandoc, which isn't installed. Settings > Engines says where to get it."
        );
        assert!(e.details.is_empty());
        assert_eq!(e.engine, None);
    }

    #[test]
    fn a_job_names_what_it_reads_and_writes() {
        let convert = JobKind::Convert {
            input: "a/clip.MOV".into(),
            target: "mp4".into(),
        };
        assert_eq!(convert.exts(), ("MOV".to_owned(), "mp4".to_owned()));
        let tool = JobKind::Tool {
            tool: Tool::Merge,
            inputs: vec!["a.pdf".into(), "b.pdf".into()],
        };
        assert_eq!(tool.exts(), ("pdf".to_owned(), "pdf".to_owned()));
    }
}
