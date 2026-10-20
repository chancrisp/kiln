//! Writes docs/ux/phase2/contrast.md: every contrast pair `contrast_report`
//! measures, for both themes, with the Phase 1 ratio beside the new one
//! where docs/ux/phase1/accessibility.md has the pair.
//!
//! The markdown goes to stdout, so nothing is written unless asked:
//!
//! cargo run -p kiln-app --example contrast_md > docs/ux/phase2/contrast.md
//!
//! Run it from the repository root. Phase 1 rows are matched by name; a
//! name that changed is matched without its trailing bracket.

use std::collections::HashMap;
use std::path::Path;

use kiln_app::theme::{contrast_report, DARK, LIGHT};

/// One Phase 1 row: its ratio as printed, and whether it failed.
struct Old {
    ratio: String,
    verdict: String,
}

/// The Phase 1 table, keyed by theme and element name.
fn phase1(root: &Path) -> HashMap<(String, String), Old> {
    let path = root.join("docs/ux/phase1/accessibility.md");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "can't read {}: {e}. Run from the repository root.",
            path.display()
        )
    });
    let mut out = HashMap::new();
    for line in text.lines() {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        // | theme | element | fg | bg | ratio | needs | result |
        if cells.len() < 8 || !(cells[1] == "dark" || cells[1] == "light") {
            continue;
        }
        let verdict = if cells[7].contains("fail") {
            "fail"
        } else if cells[7].contains("pass") {
            "pass"
        } else {
            "n/a"
        };
        out.insert(
            (cells[1].to_string(), cells[2].to_string()),
            Old {
                ratio: cells[5].to_string(),
                verdict: verdict.to_string(),
            },
        );
    }
    out
}

/// A name without its trailing bracket, for pairs whose note changed.
fn bare(name: &str) -> &str {
    match name.rfind(" (") {
        Some(i) if name.ends_with(')') => &name[..i],
        _ => name,
    }
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let old = phase1(&root);
    // The same table by bare name, for pairs whose bracketed note changed.
    let old_bare: HashMap<(String, String), &Old> = old
        .iter()
        .map(|((t, n), o)| ((t.clone(), bare(n).to_string()), o))
        .collect();

    println!("# Contrast, re-measured");
    println!();
    println!("Generated from the code by `cargo run -p kiln-app --example contrast_md`. It uses `contrast_report` in `crates/kiln-app/src/theme.rs`, the same function the unit test `every_pair_reaches_its_threshold` checks. Don't edit this file by hand.");
    println!();
    println!("Ratios are WCAG 2.x relative-luminance contrast. Washes are composited on what they sit on, as `win/render.rs` draws them. The 3% background grain is ignored. The method is the one in the [Phase 1 table](../phase1/accessibility.md#contrast).");
    println!();
    println!("- Text needs 4.5:1. A cue that isn't text needs 3:1.");
    println!("- Phase 1 is the ratio printed in the Phase 1 table, with its verdict. \"new\" means the pair wasn't in that table.");
    println!("- A row with no threshold is exempt (disabled) or advisory. Its name says why, and something else carries the meaning.");
    println!("- Some pairs Phase 1 marked as fail are now advisory with no threshold: the selected-row wash, the lit-segment wash, the progress track and the control border. Their ratios are unchanged. What carries the meaning now is a pair that passes: the row's accent edge, the segment arc, the progress fill, and the button label.");
    println!("- Phase 1 measured the old scroll thumb at 50% and the old light accent `#7FB5A0`. The scroll thumb is now 80%, and the light theme uses a darker accent for non-text cues.");
    println!();

    let mut failing = 0;
    let mut unmatched_new = 0;
    let mut matched = 0;
    let mut tables = String::new();
    let mut summary = vec![];
    for p in [DARK, LIGHT] {
        let report = contrast_report(&p);
        let mut pass = 0;
        let mut fail = 0;
        let mut exempt = 0;
        let mut fixed = 0;
        tables.push_str(&format!("## {} theme\n\n", cap(p.name)));
        tables.push_str(
            "| Element | Foreground | Background | Phase 1 | Phase 2 | Needs | Result |\n",
        );
        tables.push_str("|---|---|---|---|---|---|---|\n");
        for (name, fg, bg, ratio, need) in &report {
            let found = old
                .get(&(p.name.to_string(), name.to_string()))
                .or_else(|| {
                    old_bare
                        .get(&(p.name.to_string(), bare(name).to_string()))
                        .copied()
                });
            let before = match found {
                Some(o) => {
                    matched += 1;
                    format!("{} ({})", o.ratio, o.verdict)
                }
                None => {
                    unmatched_new += 1;
                    "new".to_string()
                }
            };
            let (needs, result) = match need {
                Some(n) if ratio >= n => {
                    pass += 1;
                    if found.is_some_and(|o| o.verdict == "fail") {
                        fixed += 1;
                    }
                    (format!("{n}:1"), "pass".to_string())
                }
                Some(n) => {
                    fail += 1;
                    failing += 1;
                    (format!("{n}:1"), "**fail**".to_string())
                }
                None => {
                    exempt += 1;
                    ("none".to_string(), "n/a".to_string())
                }
            };
            tables.push_str(&format!(
                "| {name} | `{}` | `{}` | {before} | {ratio:.2}:1 | {needs} | {result} |\n",
                fg.to_hex(),
                bg.to_hex()
            ));
        }
        tables.push('\n');
        summary.push(format!(
            "- {}: {} pairs. {pass} pass, {fail} fail, {exempt} with no threshold. {fixed} of the pairs that failed in Phase 1 now pass.",
            cap(p.name),
            report.len()
        ));
    }
    println!("## Summary\n");
    for s in &summary {
        println!("{s}");
    }
    println!(
        "- {matched} rows are matched to Phase 1 and {unmatched_new} are new. {}",
        if failing == 0 {
            "No pair with a threshold fails."
        } else {
            "Some pairs with a threshold fail."
        }
    );
    println!();
    print!("{tables}");
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}
