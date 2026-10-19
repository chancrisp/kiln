//! The README's format table has to keep up with formats.toml.

#[test]
fn readme_names_every_format_and_alias() {
    let readme = include_str!("../../../README.md");
    let registry: toml::Table = include_str!("../formats.toml").parse().unwrap();
    let formats = registry["formats"].as_table().unwrap();
    let mut missing = vec![];
    for (ext, f) in formats {
        let aliases = f
            .get("aliases")
            .and_then(|a| a.as_array())
            .into_iter()
            .flatten()
            .filter_map(|a| a.as_str());
        for name in std::iter::once(ext.as_str()).chain(aliases) {
            let code = name.to_ascii_uppercase();
            let named = readme
                .split(|c: char| !c.is_ascii_alphanumeric())
                .any(|word| word == code);
            if !named {
                missing.push(code);
            }
        }
    }
    assert!(
        missing.is_empty(),
        "README.md's Formats section doesn't mention {missing:?}"
    );
}
