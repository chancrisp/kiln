//! The window's thread never waits on the disk: folder walks, the look for
//! helper apps and where Settings says each is (and whether the chosen
//! folder is there), Media Foundation's first
//! probe, settings writes, the check for results removed by hand, a
//! Discard's removals and the start's look on every drive for results
//! crashed runs left all run on workers (`win/workers.rs`,
//! `model::walk_dropped`, the settings writer, the notes worker,
//! `held::look_for_stale`). Only a held result in the local holding folder
//! is looked at on the window's thread, on a person's action
//! (`present_in`), and so is the notices file beside `kiln-app.exe` when
//! About's Open is pressed (`notices_file`).
//! One call to another process is listed here too: a notification's
//! toast (`notify_about`), with its reason below.
//! The start's look is in `win/held.rs` and kiln-core, which this doesn't
//! read; on the window's thread the start opens only the local holding
//! folder (`held::open_holding`), as phase 1 did. This reads the source of
//! `win/mod.rs` and `win/surfaces/` and fails on any call that reads the
//! disk outside the allow-list below. Test code is skipped.

use std::path::{Path, PathBuf};

/// The calls that read or write the disk.
const CALLS: &[&str] = &[
    "read_dir(",
    "metadata(",
    "symlink_metadata(",
    "fs::write(",
    "availability(",
    "Availability::find(",
    ".exists()",
    ".is_file()",
    ".is_dir()",
    "read_to_string(",
    "create_dir",
    "fs::copy(",
    "rename(",
    "fs::read(",
    "File::open(",
    "canonicalize(",
    "try_exists(",
    "remove_file(",
    "engine_status(",
    "find_on_path(",
    "find_libreoffice(",
    "find_calibre(",
    "current_path(",
    // Not the disk: a call to another process that may be slow to answer.
    "toast::show(",
];

/// `(file, function, text, why)`: a line of that function containing that
/// text may make the call.
const ALLOWED_IN: &[(&str, &str, &str, &str)] = &[
    (
        "mod.rs",
        "present_in",
        "p.exists()",
        "a held result in the local holding folder, in %LOCALAPPDATA%, on a person's action: one on another drive goes by the gone thread's look",
    ),
    (
        "mod.rs",
        "on_runner_thread",
        "metadata(",
        "JobNews::on_runner_thread runs on the runner's thread, which reads a finished job's result sizes",
    ),
    (
        "mod.rs",
        "notices_file",
        "file.is_file()",
        "About's Open, on a person's press: one look at THIRD_PARTY_NOTICES.md in kiln-app.exe's own folder, so Notepad isn't asked to make a missing one. The open that follows (Effect::OpenText) hands the same path to the shell on this thread anyway, so a worker's look would keep no disk off it",
    ),
    (
        "mod.rs",
        "notify_about",
        "toast::show(",
        "one toast per notice (a batch ended, a result kept), never per frame. Windows' notification platform usually answers in milliseconds; when it refuses, the balloon is shown in its place at once, which needs the answer on this thread. On a worker the balloon would come later and from another thread, a change the installed MSI's toasts would have to be seen again for",
    ),
];

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The line without its comment, if it has code.
fn code(line: &str) -> &str {
    match line.find("//") {
        Some(i) => &line[..i],
        None => line,
    }
}

/// The function `line` starts, if it starts one: `fn name(` or
/// `fn name<`, after any `pub`, `unsafe`, `extern` or `const`.
fn starts_fn(line: &str) -> Option<&str> {
    let mut words = line.trim_start();
    loop {
        let (word, rest) = words.split_once(char::is_whitespace)?;
        if word == "fn" {
            let name = rest.trim_start();
            let end = name.find(|c: char| !(c.is_alphanumeric() || c == '_'))?;
            return Some(&name[..end]);
        }
        let known = word.starts_with("pub")
            || word == "unsafe"
            || word == "extern"
            || word == "const"
            || word.starts_with('"');
        if !known {
            return None;
        }
        words = rest.trim_start();
    }
}

/// The lines of `text` that aren't test code: an item after
/// `#[cfg(test)]` is left out, to the brace that closes it, or to its `;`.
fn non_test(text: &str) -> Vec<(usize, &str)> {
    let mut out = vec![];
    let mut lines = text.lines().enumerate();
    while let Some((n, line)) = lines.next() {
        if code(line).trim() != "#[cfg(test)]" {
            out.push((n, line));
            continue;
        }
        let mut depth = 0i32;
        let mut opened = false;
        for (_, line) in lines.by_ref() {
            let c = code(line);
            depth += c.matches('{').count() as i32;
            depth -= c.matches('}').count() as i32;
            opened |= c.contains('{');
            if (opened && depth <= 0) || (!opened && c.trim_end().ends_with(';')) {
                break;
            }
        }
    }
    out
}

/// The calls in `text`, the source of `name`, that the allow-list doesn't
/// cover, as `name:line: code`.
fn offending(name: &str, text: &str) -> Vec<String> {
    let mut found = vec![];
    let mut function = "";
    for (n, line) in non_test(text) {
        let code = code(line);
        if let Some(f) = starts_fn(code) {
            function = f;
        }
        for call in CALLS {
            if !code.contains(call) {
                continue;
            }
            if ALLOWED_IN
                .iter()
                .any(|(f, func, text, _)| *f == name && *func == function && code.contains(text))
            {
                continue;
            }
            found.push(format!("{name}:{}: {}", n + 1, line.trim()));
        }
    }
    found
}

#[test]
fn the_window_thread_reads_no_disk_outside_the_allow_list() {
    let win = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("win");
    let mut files = vec![win.join("mod.rs")];
    sources(&win.join("surfaces"), &mut files);
    assert!(files.len() > 5, "found the window's sources");
    let mut found = vec![];
    for file in &files {
        let name = file.file_name().unwrap().to_str().unwrap();
        found.extend(offending(name, &std::fs::read_to_string(file).unwrap()));
    }
    assert!(
        found.is_empty(),
        "read the disk on a worker instead (win/workers.rs):\n{}",
        found.join("\n")
    );
}

#[test]
fn every_allowed_call_is_still_there() {
    // An allow-list line whose call has gone would let a new one in
    // unseen: each must still match a line.
    let win = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("win");
    for (file, func, text, _) in ALLOWED_IN {
        let path = if *file == "mod.rs" {
            win.join(file)
        } else {
            win.join("surfaces").join(file)
        };
        let src = std::fs::read_to_string(&path).unwrap();
        let mut function = "";
        let mut seen = false;
        for (_, line) in non_test(&src) {
            let code = code(line);
            if let Some(f) = starts_fn(code) {
                function = f;
            }
            seen |= function == *func && code.contains(text);
        }
        assert!(seen, "{file}: {func} no longer has {text}");
    }
}

#[test]
fn the_check_sees_a_call_and_skips_comments_tests_and_the_allowed() {
    let handler = "fn add(&mut self) {\n    let m = std::fs::metadata(p);\n}\n";
    assert_eq!(offending("mod.rs", handler).len(), 1);
    let commented = "fn add() {\n    // std::fs::metadata(p) would wait on the drive\n}\n";
    assert!(offending("mod.rs", commented).is_empty());
    let test = "#[cfg(test)]\nmod tests {\n    fn t() {\n        p.exists();\n    }\n}\nfn after() {\n    p.exists();\n}\n";
    assert_eq!(offending("mod.rs", test), ["mod.rs:8: p.exists();"]);
    let one_line = "#[cfg(test)]\nfn t() { p.exists(); }\nfn after() {\n    p.is_file();\n}\n";
    assert_eq!(offending("mod.rs", one_line), ["mod.rs:4: p.is_file();"]);
    let decl = "#[cfg(test)]\nmod flow_tests;\nfn after() {\n    p.is_file();\n}\n";
    assert_eq!(offending("mod.rs", decl), ["mod.rs:4: p.is_file();"]);
    let allowed = "fn present_in<'a>() {\n    move |p: &Path| p.exists()\n}\n";
    assert!(offending("mod.rs", allowed).is_empty());
    let elsewhere = "fn other(&mut self) {\n    self.note_gone(|p| p.exists());\n}\n";
    assert_eq!(offending("mod.rs", elsewhere).len(), 1);
    // Allowed once, before its look went to the gone thread.
    let was = "fn check_gone(&mut self) {\n    self.note_gone(|p| p.exists());\n}\n";
    assert_eq!(offending("mod.rs", was).len(), 1);
    let engines = "fn wheel(&self) {\n    let a = self.kiln.availability(&opts);\n}\n";
    assert_eq!(offending("mod.rs", engines).len(), 1);
}
