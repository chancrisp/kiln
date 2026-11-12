//! `lame_enc.dll`: LAME 3.100, built from the source bundled in `mp3lame-sys`.
//!
//! The library has no Rust API. `build.rs` makes the linker export the C
//! functions Kiln's MP3 sink needs (see `EXPORTS` there), and `kiln-media`
//! loads the DLL at run time. LAME is LGPL; keeping it in its own DLL is what
//! lets the rest of Kiln stay under its own licence.

// Pulls libmp3lame into the link.
extern crate mp3lame_sys;

/// The LAME version this DLL was built from, for example "3.100".
pub fn version() -> String {
    // SAFETY: `get_lame_version` returns a static, NUL-terminated string.
    unsafe {
        std::ffi::CStr::from_ptr(mp3lame_sys::get_lame_version())
            .to_string_lossy()
            .into_owned()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_is_lame_3_100() {
        assert_eq!(super::version(), "3.100");
    }
}
