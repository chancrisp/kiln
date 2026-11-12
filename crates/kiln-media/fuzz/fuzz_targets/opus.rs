#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../entry.rs"]
mod entry;

thread_local! {
    static SCRATCH: std::cell::RefCell<Option<entry::Scratch>> = const { std::cell::RefCell::new(None) };
}

fuzz_target!(|data: &[u8]| {
    // The pages are checksummed; mend the checksums so the mutation reaches the code behind them.
    let mut data = data.to_vec();
    entry::fix_ogg_crcs(&mut data);
    entry::fuzz_one(&SCRATCH, "opus", entry::Reader::Opus, &data);
});
