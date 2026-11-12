#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../entry.rs"]
mod entry;

thread_local! {
    static SCRATCH: std::cell::RefCell<Option<entry::Scratch>> = const { std::cell::RefCell::new(None) };
}

// The first byte picks the extension hint, the rest is the file.
fuzz_target!(|data: &[u8]| {
    let Some((&pick, rest)) = data.split_first() else {
        return;
    };
    let ext = entry::SYM_EXTS[pick as usize % entry::SYM_EXTS.len()];
    let mut rest = rest.to_vec();
    if ext == "ogg" {
        entry::fix_ogg_crcs(&mut rest);
    }
    entry::fuzz_one(&SCRATCH, "sym", entry::Reader::Sym(ext), &rest);
});
