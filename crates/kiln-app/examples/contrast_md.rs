//! Prints a markdown table of every contrast pair `contrast_report`
//! measures, for each built-in palette, with the ratio it needs and whether
//! it passes. It reads no files.
//!
//! The markdown goes to stdout, so nothing is written unless asked:
//!
//! cargo run -p kiln-app --example contrast_md > contrast.md

use kiln_app::theme::fluent::{contrast_report, ALL_BUILT_IN};

fn main() {
    println!("# Contrast");
    println!();
    println!("Generated from the code by `cargo run -p kiln-app --example contrast_md`. It uses `contrast_report` in `crates/kiln-app/src/theme/fluent.rs`, the same function the unit tests check. Don't edit this file by hand.");
    println!();
    println!("Ratios are WCAG 2.x relative-luminance contrast. Washes are composited on what they sit on, as the renderer draws them.");
    println!();
    println!("- Text needs 4.5:1. A cue that isn't text needs 3:1.");
    println!("- A row with no threshold is exempt (disabled) or advisory. Its name says why, and something else carries the meaning.");
    println!();

    let mut failing = 0;
    let mut tables = String::new();
    let mut summary = vec![];
    for p in ALL_BUILT_IN {
        let report = contrast_report(&p);
        let (mut pass, mut fail, mut exempt) = (0, 0, 0);
        tables.push_str(&format!("## {} theme\n\n", cap(p.name)));
        tables.push_str("| Element | Foreground | Background | Ratio | Needs | Result |\n");
        tables.push_str("|---|---|---|---|---|---|\n");
        for (name, fg, bg, ratio, need) in &report {
            let (needs, result) = match need {
                Some(n) if ratio >= n => {
                    pass += 1;
                    (format!("{n}:1"), "pass")
                }
                Some(n) => {
                    fail += 1;
                    failing += 1;
                    (format!("{n}:1"), "**fail**")
                }
                None => {
                    exempt += 1;
                    ("none".to_string(), "n/a")
                }
            };
            tables.push_str(&format!(
                "| {name} | `{}` | `{}` | {ratio:.2}:1 | {needs} | {result} |\n",
                fg.to_hex(),
                bg.to_hex()
            ));
        }
        tables.push('\n');
        summary.push(format!(
            "- {}: {} pairs. {pass} pass, {fail} fail, {exempt} with no threshold.",
            cap(p.name),
            report.len()
        ));
    }
    println!("## Summary\n");
    for s in &summary {
        println!("{s}");
    }
    println!(
        "- {}",
        if failing == 0 {
            "No pair with a threshold fails."
        } else {
            "Some pairs with a threshold fail."
        }
    );
    println!();
    print!("{tables}");
}

/// A palette's name as a title: `solarized-dark` is `Solarized Dark`.
fn cap(s: &str) -> String {
    s.split('-')
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}
