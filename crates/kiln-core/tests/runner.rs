//! The worker pool.

#[path = "../../../tests/support/mod.rs"]
mod support;

use std::sync::mpsc;
use std::time::Duration;

use kiln_core::{JobEvent, JobKind, Kiln, Options, Runner, Tool};
use support::{assert_output, sample_for, stage};

fn collect(rx: &mpsc::Receiver<JobEvent>, finished: usize) -> Vec<JobEvent> {
    let mut events = vec![];
    let mut done = 0;
    while done < finished {
        let e = rx
            .recv_timeout(Duration::from_secs(60))
            .expect("job events arrive");
        if matches!(e, JobEvent::Finished(..)) {
            done += 1;
        }
        events.push(e);
    }
    events
}

#[test]
fn runs_jobs_in_parallel_and_reports_each() {
    let (tx, rx) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let runner = Runner::new(Kiln::new().into(), 3, move |e| {
        let _ = tx.lock().unwrap().send(e);
    });
    let (_d1, wav) = stage(&sample_for("wav"));
    let (_d2, md) = stage(&sample_for("md"));
    let (_d3, png) = stage(&sample_for("png"));
    let ids = [
        runner.submit(
            JobKind::Convert {
                input: wav,
                target: "mp3".into(),
            },
            Options::default(),
        ),
        runner.submit(
            JobKind::Convert {
                input: md,
                target: "html".into(),
            },
            Options::default(),
        ),
        runner.submit(
            JobKind::Convert {
                input: png,
                target: "jpg".into(),
            },
            Options::default(),
        ),
    ];
    let events = collect(&rx, 3);
    for (id, ext) in ids.iter().zip(["mp3", "html", "jpg"]) {
        let out = events
            .iter()
            .find_map(|e| match e {
                JobEvent::Finished(i, r) if i == id => Some(r.clone()),
                _ => None,
            })
            .unwrap()
            .unwrap();
        assert_output(&out[0], ext);
        assert!(events
            .iter()
            .any(|e| matches!(e, JobEvent::Progress(i, _) if i == id)));
    }
}

#[test]
fn cancelling_a_queued_job_never_runs_it() {
    let (tx, rx) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let runner = Runner::new(Kiln::new().into(), 1, move |e| {
        let _ = tx.lock().unwrap().send(e);
    });
    let (dir, wav) = stage(&sample_for("wav"));
    let first = runner.submit(
        JobKind::Tool {
            tool: Tool::Split,
            inputs: vec![wav.clone()],
        },
        Options::default(),
    );
    let second = runner.submit(
        JobKind::Convert {
            input: wav,
            target: "flac".into(),
        },
        Options::default(),
    );
    runner.cancel(second);
    let events = collect(&rx, 2);
    for e in &events {
        match e {
            JobEvent::Finished(id, r) if *id == first => assert_eq!(r.as_ref().unwrap().len(), 2),
            JobEvent::Finished(id, r) if *id == second => {
                let err = r.as_ref().unwrap_err();
                assert!(err.cancelled);
                assert_eq!(err.message, "Cancelled.");
            }
            _ => {}
        }
    }
    assert!(!dir.path().join("sample.flac").exists());
}

#[test]
fn failures_carry_a_line_and_the_details() {
    let (tx, rx) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let runner = Runner::new(Kiln::new().into(), 1, move |e| {
        let _ = tx.lock().unwrap().send(e);
    });
    let (_dir, bad) = stage(&support::fixture("corrupt.wav"));
    runner.submit(
        JobKind::Convert {
            input: bad,
            target: "mp3".into(),
        },
        Options::default(),
    );
    let events = collect(&rx, 1);
    let JobEvent::Finished(_, Err(err)) = events.last().unwrap() else {
        panic!("expected a failure");
    };
    // A plain sentence with a next step; FFmpeg's own words are in the
    // details.
    assert_eq!(
        err.message,
        "This file looks damaged, or isn't really a WAV. Try opening it in another app."
    );
    assert_eq!(err.engine, Some("FFmpeg"));
    assert!(!err.details.is_empty());
    assert!(!err.cancelled);
}
