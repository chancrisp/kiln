//! Pins every design token. An accidental palette change fails here.
//! After a deliberate change, run with KILN_UPDATE_SNAPSHOTS=1 and commit
//! the new snapshot alongside a DESIGN.md update.
//!
//! `fluent_tokens.snap` pins the phase 3 look spec's tables, and was
//! written by hand from them, so it checks the code against the spec.

use std::path::Path;

fn check(file: &str, actual: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(file);
    if std::env::var_os("KILN_UPDATE_SNAPSHOTS").is_some() {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .expect("snapshot missing; run with KILN_UPDATE_SNAPSHOTS=1 to create it")
        .replace("\r\n", "\n");
    if actual != expected {
        let diff: Vec<String> = expected
            .lines()
            .zip(actual.lines())
            .filter(|(e, a)| e != a)
            .map(|(e, a)| format!("- {e}\n+ {a}"))
            .collect();
        panic!(
            "{file} changed:\n{}\n(line counts {} vs {})",
            diff.join("\n"),
            expected.lines().count(),
            actual.lines().count()
        );
    }
}

/// Every `.rs` file under `dir`, in a stable order.
fn rust_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(d) = dirs.pop() {
        for entry in std::fs::read_dir(&d).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// Whether `line` names the old `Palette`: the word alone, not
/// `FluentPalette`.
fn names_old_palette(line: &str) -> bool {
    line.match_indices("Palette").any(|(i, _)| {
        let before = &line[..i];
        let after = &line[i + "Palette".len()..];
        !before.ends_with("Fluent")
            && !before.ends_with(|c: char| c.is_alphanumeric() || c == '_')
            && !after.starts_with(|c: char| c.is_alphanumeric() || c == '_')
    })
}

/// Phase 3 retired the old look (R16): its palette, glazes, grain, heat,
/// shadows and bundled serif. No source file of the app draws with them
/// or names them again; everything draws with `theme::fluent`.
#[test]
fn nothing_draws_with_the_old_look() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let words = ["glaze", "grain", "heat", "fraunces"];
    let names = [
        "theme::DARK",
        "theme::LIGHT",
        "theme_from_system",
        "shadow_layers",
        "depth::",
        "effects::GRAIN",
        "DISPLAY_TTF",
        "raised_panel",
        "press_shadow",
        "focus_colour",
        // The old spacing and motion tokens: `fluent::space` and
        // `fluent::motion` hold the ones still used.
        "theme::shape",
        "theme::motion",
        "theme::{shape",
        "shape::BORDER",
        "shape::GRID",
        "shape::PANEL_PADDING",
        "shape::ICON_",
    ];
    let mut found = Vec::new();
    for file in rust_files(&src) {
        let text = std::fs::read_to_string(&file).unwrap();
        let name = file.strip_prefix(&src).unwrap().display().to_string();
        for (n, line) in text.lines().enumerate() {
            let lower = line.to_lowercase();
            let hit = words
                .iter()
                .find(|w| lower.contains(*w))
                .copied()
                .or_else(|| names.iter().find(|w| line.contains(*w)).copied())
                .or_else(|| names_old_palette(line).then_some("Palette"));
            if let Some(w) = hit {
                found.push(format!("{name}:{}: {w}: {}", n + 1, line.trim()));
            }
        }
    }
    assert!(
        found.is_empty(),
        "the old look is still named:\n{}",
        found.join("\n")
    );
}

#[test]
fn fluent_tokens_match_the_look_spec() {
    check("fluent_tokens.snap", &kiln_app::theme::fluent::describe());
}
