#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../entry.rs"]
mod entry;

thread_local! {
    static SCRATCH: std::cell::RefCell<Option<entry::Scratch>> = const { std::cell::RefCell::new(None) };
}

fuzz_target!(|data: &[u8]| {
    entry::fuzz_one(&SCRATCH, "id3", entry::Reader::Id3, data);
});
