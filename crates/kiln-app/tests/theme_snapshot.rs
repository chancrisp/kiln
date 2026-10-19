//! Pins every design token. An accidental palette change fails here.
//! After a deliberate change, run with KILN_UPDATE_SNAPSHOTS=1 and commit
//! the new snapshot alongside a DESIGN.md update.

use std::path::Path;

#[test]
fn theme_tokens_match_the_snapshot() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/theme_tokens.snap");
    let actual = kiln_app::theme::describe();
    if std::env::var_os("KILN_UPDATE_SNAPSHOTS").is_some() {
        std::fs::write(&path, &actual).unwrap();
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
            "theme tokens changed:\n{}\n(line counts {} vs {})",
            diff.join("\n"),
            expected.lines().count(),
            actual.lines().count()
        );
    }
}
