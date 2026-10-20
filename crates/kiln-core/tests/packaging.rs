//! The installer's Open with list stays in step with the format registry.

use std::collections::BTreeSet;

use kiln_core::Registry;

const FRAGMENT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packaging/open-with-types.wxs"
);

/// The text between `start` and the next `"` after it, for each line
/// that holds `start`.
fn quoted_after<'a>(lines: impl Iterator<Item = &'a str>, start: &str) -> BTreeSet<String> {
    lines
        .filter_map(|line| {
            let rest = line.split_once(start)?.1;
            Some(rest.split_once('"')?.0.to_string())
        })
        .collect()
}

fn compare(what: &str, found: &BTreeSet<String>, want: &BTreeSet<String>) -> Option<String> {
    if found == want {
        return None;
    }
    let missing: Vec<&String> = want.difference(found).collect();
    let extra: Vec<&String> = found.difference(want).collect();
    Some(format!(
        "{what}: missing from the MSI: {missing:?}; not a Kiln input: {extra:?}"
    ))
}

#[test]
fn the_open_with_list_matches_the_registry() {
    let text =
        std::fs::read_to_string(FRAGMENT).unwrap_or_else(|e| panic!("cannot read {FRAGMENT}: {e}"));
    let want: BTreeSet<String> = Registry::builtin().input_extensions().into_iter().collect();

    // SupportedTypes: Name=".<ext>" on the lines that carry that key.
    let supported = quoted_after(
        text.lines()
            .filter(|l| l.contains(r"\kiln-app.exe\SupportedTypes")),
        "Name=\".",
    );

    // OpenWithProgids: Key="Software\Classes\.<ext>\OpenWithProgids".
    let progids: BTreeSet<String> = quoted_after(text.lines(), r#"Key="Software\Classes\."#)
        .iter()
        .filter_map(|rest| rest.strip_suffix(r"\OpenWithProgids"))
        .map(str::to_string)
        .collect();

    let problems: Vec<String> = [
        compare("SupportedTypes", &supported, &want),
        compare("OpenWithProgids", &progids, &want),
    ]
    .into_iter()
    .flatten()
    .collect();
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
