#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../entry.rs"]
mod entry;

thread_local! {
    static SCRATCH: std::cell::RefCell<Option<entry::Scratch>> = const { std::cell::RefCell::new(None) };
}

// The video facts of an MP4 or MOV. The file starts with an empty `ftyp` box
// the input follows, so every input is read as an MP4 and none is lost to the
// Matroska check.
fuzz_target!(|data: &[u8]| {
    let mut file = vec![0, 0, 0, 8, b'f', b't', b'y', b'p'];
    file.extend_from_slice(data);
    entry::fuzz_one(
        &SCRATCH,
        "video_info_mp4",
        entry::Reader::VideoInfo,
        &file,
    );
});
