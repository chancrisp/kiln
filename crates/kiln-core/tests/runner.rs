//! The worker pool.

#[path = "../../../tests/support/mod.rs"]
mod support;

use std::sync::mpsc;
use std::time::Duration;

use kiln_core::{JobEvent, JobId, JobKind, Kiln, Options, Runner, Tool};
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
    let (_wav_dir, wav) = stage(&sample_for("wav"));
    let (dir, avi) = stage(&sample_for("avi"));
    let first = runner.submit(
        JobKind::Tool {
            tool: Tool::CutInHalf,
            inputs: vec![wav],
        },
        Options::default(),
    );
    // In the same pool as the tool, so it waits behind it.
    let second = runner.submit(
        JobKind::Convert {
            input: avi,
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
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("corrupt.avi");
    std::fs::copy(support::fixture("corrupt.wav"), &bad).unwrap();
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
        "This file looks damaged, or isn't really an AVI. Try opening it in another app."
    );
    assert_eq!(err.engine, Some("FFmpeg"));
    assert!(!err.details.is_empty());
    assert!(!err.cancelled);

    // Kiln's own sound engine says what is wrong in one line of its own.
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
    assert_eq!(err.message, "This WAV is damaged and can't be read.");
    assert_eq!(err.engine, None);
    assert!(!err.cancelled);
}

#[test]
fn a_running_audio_job_cancels() {
    let (tx, rx) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let runner = Runner::new(Kiln::new().into(), 1, move |e| {
        let _ = tx.lock().unwrap().send(e);
    });
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("long.wav");
    write_silence(&wav, 600);
    let opts = Options {
        temp_root: Some(dir.path().join("work")),
        ..Options::default()
    };
    let id = runner.submit(
        JobKind::Convert {
            input: wav,
            target: "mp3".into(),
        },
        opts,
    );
    // Cancel once it is under way.
    let first = rx.recv_timeout(Duration::from_secs(60)).unwrap();
    assert!(
        matches!(first, JobEvent::Progress(i, _) if i == id),
        "{first:?}"
    );
    runner.cancel(id);
    let events = collect(&rx, 1);
    let JobEvent::Finished(_, Err(err)) = events.last().unwrap() else {
        panic!("expected a cancelled job, got {events:?}");
    };
    assert!(err.cancelled, "{err:?}");
    assert!(!dir.path().join("long.mp3").exists());
    let work = dir.path().join("work");
    assert!(!work.exists() || std::fs::read_dir(&work).unwrap().next().is_none());
}

/// A 16-bit stereo WAV of `secs` seconds of silence at 48 kHz.
fn write_silence(path: &std::path::Path, secs: u32) {
    let data = 48_000 * 4 * secs;
    let mut b = Vec::with_capacity(data as usize + 44);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    for v in [16u32, 0x0002_0001, 48_000, 48_000 * 4, 0x0010_0004] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    b.resize(b.len() + data as usize, 0);
    std::fs::write(path, b).unwrap();
}

/// Runs `kind` on a runner of one worker and gives every event up to and
/// including its `Finished`.
fn run_job(kind: JobKind) -> (JobId, Vec<JobEvent>) {
    let (tx, rx) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let runner = Runner::new(Kiln::new().into(), 1, move |e| {
        let _ = tx.lock().unwrap().send(e);
    });
    let id = runner.submit(kind, Options::default());
    (id, collect(&rx, 1))
}

#[test]
fn a_job_reports_what_it_drops() {
    // A PDF's text laid out again as a DOCX: the text goes through
    // LibreOffice.
    if !Kiln::new()
        .availability(&Options::default())
        .has("libreoffice")
    {
        eprintln!("a_job_reports_what_it_drops: skipped, LibreOffice isn't installed");
        return;
    }
    let (_dir, pdf) = stage(&sample_for("pdf"));
    let (id, events) = run_job(JobKind::Convert {
        input: pdf,
        target: "docx".into(),
    });
    let notes = events
        .iter()
        .position(|e| matches!(e, JobEvent::Notes(i, _) if *i == id))
        .unwrap_or_else(|| panic!("no notes in {events:?}"));
    assert_eq!(
        events[notes],
        JobEvent::Notes(
            id,
            vec!["Keeps the text only: no pictures, tables or layout.".to_owned()]
        )
    );
    let finished = events
        .iter()
        .position(|e| matches!(e, JobEvent::Finished(i, _) if *i == id))
        .unwrap();
    assert!(notes < finished, "{events:?}");
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, JobEvent::Notes(..)))
            .count(),
        1
    );
    assert!(matches!(&events[finished], JobEvent::Finished(_, Ok(_))));
}

#[test]
fn a_video_job_reports_the_sound_it_drops() {
    if !support::have_ffmpeg("a_video_job_reports_the_sound_it_drops") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mkv = support::video_with_tracks(dir.path(), "two.mkv", "srt");
    let (id, events) = run_job(JobKind::Convert {
        input: mkv,
        target: "mp3".into(),
    });
    // The subtitles aren't said: a sound file has none to lose.
    assert_eq!(
        events.first(),
        Some(&JobEvent::Notes(
            id,
            vec!["Keeps the first of 2 sound tracks.".to_owned()]
        )),
        "{events:?}"
    );
    assert!(
        matches!(events.last(), Some(JobEvent::Finished(_, Ok(_)))),
        "{events:?}"
    );
}

#[test]
fn a_job_that_drops_nothing_sends_no_notes() {
    let (_dir, wav) = stage(&sample_for("wav"));
    let (_, events) = run_job(JobKind::Convert {
        input: wav,
        target: "mp3".into(),
    });
    assert!(
        !events.iter().any(|e| matches!(e, JobEvent::Notes(..))),
        "{events:?}"
    );
}
