//! The window's calls that run a loop of their own, or send straight back
//! to the window, are made only by the effects runner (`win/effects.rs`),
//! with `App` let go. Anywhere else in `win/`, a handler would make them
//! with `App` borrowed. This reads the source and fails on any such call
//! outside the allow-list below.

use std::path::{Path, PathBuf};

/// The calls that only the effects runner makes: the Win32 calls, and
/// Kiln's own wrappers around them.
const CALLS: &[&str] = &[
    "system::ask(",
    "pick_folder(",
    "pick_folder_in(",
    "pick_files(",
    "pick_program(",
    "save_as(",
    "popup_menu(",
    "show_menu(",
    "TrackPopupMenuEx(",
    "drag_files(",
    "SHDoDragDrop(",
    "open_file(",
    "open_text(",
    "set_clipboard(",
    "show_in_folder(",
    "bring_forward(",
    "place_now(",
    "refresh_frame(",
    ".sync(",
    "DestroyWindow(",
    "SetWindowPos(",
    "SetFocus(",
    "ShowWindow(",
    "SetForegroundWindow(",
    "UpdateWindow(",
    "ReleaseCapture(",
];

/// Files whose every call is allowed, and why.
const ALLOWED_FILES: &[(&str, &str)] = &[
    (
        "effects.rs",
        "the effects runner: it makes each call with App let go",
    ),
    (
        "float_wheel.rs",
        "the floating wheel's own window, which holds no App",
    ),
    ("crash.rs", "after a panic App is never touched again"),
    (
        "handoff.rs",
        "a second launch shows the first Kiln's window, from another process",
    ),
    ("flow_tests.rs", "the in-process tests' own hidden window"),
    (
        "tray_menu.rs",
        "the tray menu's own window, which holds no App: the runner opens it, \
         and App only ever asks it to close with a posted message",
    ),
];

/// `(file, function, text)`: a line of that function containing that text
/// may make the call, and why.
const ALLOWED_IN: &[(&str, &str, &str, &str)] = &[
    (
        "system.rs",
        "bring_forward",
        "ShowWindow(",
        "bring_forward itself, which only the runner calls",
    ),
    (
        "system.rs",
        "bring_forward",
        "SetForegroundWindow(",
        "bring_forward itself, which only the runner calls",
    ),
    (
        "tray.rs",
        "show_menu",
        "SetForegroundWindow(",
        "the tray menu itself, which only the runner shows",
    ),
    (
        "tray.rs",
        "show_menu",
        "TrackPopupMenuEx(",
        "the tray menu itself, which only the runner shows",
    ),
    (
        "drag_out.rs",
        "drag_files",
        "SHDoDragDrop(",
        "the drag itself, which only the runner starts",
    ),
    (
        "channel.rs",
        "drop",
        "DestroyWindow(",
        "its tests' own window, which holds no App",
    ),
    (
        "titlebar.rs",
        "drop_test_window",
        "DestroyWindow(",
        "its test's own window, which holds no App",
    ),
    (
        "list.rs",
        "drag_out",
        "self.files.drag_files(",
        "FileList's own method: which results a drag would carry",
    ),
    (
        "mod.rs",
        "show_first_frame",
        "ShowWindow(",
        "set-up, before the message loop, with App let go (run_window's first frame)",
    ),
    (
        "mod.rs",
        "show_first_frame",
        "UpdateWindow(",
        "set-up, before the message loop, with App let go (run_window's first frame)",
    ),
    (
        "mod.rs",
        "run_window",
        "place_now(",
        "set-up, before the message loop, with App let go",
    ),
    (
        "mod.rs",
        "run_window",
        "refresh_frame(",
        "set-up, before the message loop, with App let go",
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
/// `fn name<`, after any `pub`, `unsafe` or `extern`.
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

/// The calls in `text`, the source of `win/{name}`, that the allow-list
/// doesn't cover, as `name:line: code`.
fn offending(name: &str, text: &str) -> Vec<String> {
    let mut found = vec![];
    if ALLOWED_FILES.iter().any(|(f, _)| *f == name) {
        return found;
    }
    let mut function = "";
    for (n, line) in text.lines().enumerate() {
        let code = code(line);
        if let Some(f) = starts_fn(code) {
            function = f;
        }
        for call in CALLS {
            // The call by its own name, not the end of a longer one
            // (`draw_popup_menu(` is not `popup_menu(`).
            let named = |&(at, _): &(usize, &str)| {
                call.starts_with('.')
                    || !code[..at]
                        .chars()
                        .next_back()
                        .is_some_and(|c| c.is_alphanumeric() || c == '_')
            };
            let Some((at, _)) = code.match_indices(call).find(named) else {
                continue;
            };
            // The definition itself.
            if code[..at].trim_end().ends_with("fn") {
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
fn only_the_effects_runner_makes_calls_that_reenter_the_window() {
    let win = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("win");
    let mut files = vec![];
    sources(&win, &mut files);
    assert!(files.len() > 10, "found the window's sources");
    let mut found = vec![];
    for file in &files {
        let name = file.file_name().unwrap().to_str().unwrap();
        found.extend(offending(name, &std::fs::read_to_string(file).unwrap()));
    }
    assert!(
        found.is_empty(),
        "queue an Effect instead:\n{}",
        found.join("\n")
    );
}

#[test]
fn the_check_sees_a_call_and_skips_comments_definitions_and_set_up() {
    let handler = "fn key(app: &mut App) {\n    unsafe { let _ = SetFocus(Some(hwnd)); }\n}\n";
    assert_eq!(offending("tips.rs", handler).len(), 1);
    assert_eq!(offending("mod.rs", handler).len(), 1);
    assert!(offending("effects.rs", handler).is_empty());
    let commented = "    // SetFocus(Some(hwnd)) would send WM_SETFOCUS\n";
    assert!(offending("mod.rs", commented).is_empty());
    let def = "pub fn pick_folder(hwnd: HWND) -> Option<PathBuf> {\n";
    assert!(offending("system.rs", def).is_empty());
    let set_up = "fn show_first_frame() {\n    let _ = ShowWindow(hwnd, SW_SHOW);\n}\n";
    assert!(offending("mod.rs", set_up).is_empty());
    let after =
        "fn show_first_frame() {\n}\nfn other() {\n    let _ = ShowWindow(hwnd, SW_SHOW);\n}\n";
    assert_eq!(offending("mod.rs", after).len(), 1);
}

#[test]
fn an_allowed_call_is_allowed_only_in_its_own_function() {
    let inside =
        "pub fn bring_forward(hwnd: HWND) {\n    let _ = ShowWindow(hwnd, SW_RESTORE);\n}\n";
    assert!(offending("system.rs", inside).is_empty());
    let elsewhere = "pub fn other(hwnd: HWND) {\n    let _ = ShowWindow(hwnd, SW_RESTORE);\n}\n";
    assert_eq!(offending("system.rs", elsewhere).len(), 1);
    let menu = "fn handler() {\n    system::popup_menu(hwnd, &items, 0, 0);\n}\n";
    assert_eq!(offending("mod.rs", menu).len(), 1);
    let place = "fn grow(&mut self) {\n    effects::place_now(hwnd, p);\n}\n";
    assert_eq!(offending("mod.rs", place).len(), 1);
    let frame = "fn handle(app: &mut App) {\n    refresh_frame(app.hwnd);\n}\n";
    assert_eq!(offending("mod.rs", frame).len(), 1);
    let set_up = "fn run_window() {\n    effects::refresh_frame(hwnd);\n}\n";
    assert!(offending("mod.rs", set_up).is_empty());
    let sync = "fn paint(&mut self) {\n    t.sync(&tools, dpi);\n}\n";
    assert_eq!(offending("mod.rs", sync).len(), 1);
    assert_eq!(
        starts_fn("    pub(super) unsafe fn lparam_as<T: Copy>("),
        Some("lparam_as")
    );
    assert_eq!(
        starts_fn("extern \"system\" fn wndproc(hwnd: HWND)"),
        Some("wndproc")
    );
    assert_eq!(starts_fn("    let f = fn_name(1);"), None);
}
