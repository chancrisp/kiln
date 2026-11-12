#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../entry.rs"]
mod entry;

thread_local! {
    static SCRATCH: std::cell::RefCell<Option<entry::Scratch>> = const { std::cell::RefCell::new(None) };
}

// The video facts of a Matroska or WebM file. The EBML signature goes in
// front of the input, so every input is read as Matroska.
fuzz_target!(|data: &[u8]| {
    let mut file = vec![0x1A, 0x45, 0xDF, 0xA3];
    file.extend_from_slice(data);
    entry::fuzz_one(
        &SCRATCH,
        "video_info_mkv",
        entry::Reader::VideoInfo,
        &file,
    );
});
