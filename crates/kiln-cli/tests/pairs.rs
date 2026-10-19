//! One `kiln convert` run for every pair in formats.toml, checking the
//! output's type and basic properties.

#[path = "../../../tests/support/mod.rs"]
mod support;

use std::sync::Mutex;
use std::thread;

use assert_cmd::Command;
use kiln_core::Registry;
use support::{assert_output, sample_for, stage};

#[test]
fn every_supported_pair_converts() {
    let pairs: Vec<(String, String)> = Registry::builtin()
        .pairs()
        .map(|(f, t, _)| (f.ext.clone(), t.ext.clone()))
        .collect();
    assert!(pairs.len() > 100, "only {} pairs", pairs.len());

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
                    let (dir, input) = stage(&sample_for(&from));
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
                    let expected = dir.path().join(format!("{stem}.{to}"));
                    assert_eq!(
                        String::from_utf8_lossy(&out.stdout).trim_end(),
                        expected.display().to_string()
                    );
                    assert_output(&expected, &to);
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

    let failures = failures.into_inner().unwrap();
    assert!(
        failures.is_empty(),
        "{} of {} pairs failed:\n{}",
        failures.len(),
        pairs.len(),
        failures.join("\n")
    );
}
