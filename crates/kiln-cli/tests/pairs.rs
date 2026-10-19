//! One `kiln convert` run for every pair in formats.toml, checking the
//! output's type and basic properties.
//!
//! Types no free tool can write (camera RAW, Pages, Visio and the like)
//! are tested only when KILN_EXTRA_SAMPLES names a folder holding a
//! sample of each; see tests/support.

#[path = "../../../tests/support/mod.rs"]
mod support;

use std::path::PathBuf;
use std::sync::Mutex;
use std::thread;

use assert_cmd::Command;
use kiln_core::Registry;
use support::{assert_converted, external_sample, paged, sample_for, stage, EXTERNAL};

#[test]
fn every_supported_pair_converts() {
    let all: Vec<(String, String)> = Registry::builtin()
        .pairs()
        .map(|(f, t, _)| (f.ext.clone(), t.ext.clone()))
        .collect();
    assert!(all.len() > 1000, "only {} pairs", all.len());
    let skipped: Vec<String> = EXTERNAL
        .iter()
        .filter(|e| external_sample(e).is_none())
        .map(|e| e.to_uppercase())
        .collect();
    if !skipped.is_empty() {
        eprintln!(
            "No samples for {}; set KILN_EXTRA_SAMPLES to test them.",
            skipped.join(", ")
        );
    }
    let pairs: Vec<(String, String)> = all
        .into_iter()
        .filter(|(from, _)| !EXTERNAL.contains(&from.as_str()) || external_sample(from).is_some())
        .collect();

    let queue = Mutex::new(pairs.clone());
    let failures = Mutex::new(Vec::new());
    let workers = thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(8);

    thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| loop {
                let Some((from, to)) = queue.lock().unwrap().pop() else {
                    break;
                };
                let result = std::panic::catch_unwind(|| {
                    let sample = external_sample(&from).unwrap_or_else(|| sample_for(&from));
                    let (dir, input) = stage(&sample);
                    let out = Command::cargo_bin("kiln")
                        .unwrap()
                        .arg("convert")
                        .arg(&input)
                        .arg(&to)
                        .output()
                        .unwrap();
                    assert!(
                        out.status.success(),
                        "stderr: {}",
                        String::from_utf8_lossy(&out.stderr)
                    );
                    let stem = input.file_stem().unwrap().to_string_lossy().into_owned();
                    let written: Vec<PathBuf> = String::from_utf8_lossy(&out.stdout)
                        .lines()
                        .map(PathBuf::from)
                        .collect();
                    match written.as_slice() {
                        [one] => assert_eq!(*one, dir.path().join(format!("{stem}.{to}"))),
                        many => {
                            assert!(
                                paged(&from) && many.len() > 1,
                                "{from} wrote {} files",
                                many.len()
                            );
                            let unit = if matches!(from.as_str(), "pptx" | "ppt" | "odp" | "key") {
                                "slide"
                            } else {
                                "page"
                            };
                            for (n, path) in many.iter().enumerate() {
                                let name = format!("{stem} {unit} {}.{to}", n + 1);
                                assert_eq!(*path, dir.path().join(name));
                            }
                        }
                    }
                    for path in &written {
                        assert_converted(path, Some(&from), &to);
                    }
                });
                if let Err(e) = result {
                    let msg = e
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                        .unwrap_or_default();
                    failures
                        .lock()
                        .unwrap()
                        .push(format!("{from} -> {to}: {msg}"));
                }
            });
        }
    });

    let mut failures = failures.into_inner().unwrap();
    failures.sort();
    assert!(
        failures.is_empty(),
        "{} of {} pairs failed:\n{}",
        failures.len(),
        pairs.len(),
        failures.join("\n")
    );
}
