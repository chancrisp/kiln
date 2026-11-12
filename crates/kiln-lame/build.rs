//! Exports LAME's C functions from `lame_enc.dll`.
//!
//! libmp3lame is a static library that `mp3lame-sys` builds from the LAME
//! 3.100 source. A Rust wrapper could not carry the same names, because the
//! C objects already define them, so the linker is told to export them straight
//! from the library instead.

/// The LAME functions Kiln's MP3 sink calls.
const EXPORTS: &[&str] = &[
    "lame_init",
    "lame_set_in_samplerate",
    "lame_set_num_channels",
    "lame_set_mode",
    "lame_set_quality",
    "lame_set_VBR",
    "lame_set_VBR_quality",
    "lame_set_brate",
    "lame_set_VBR_mean_bitrate_kbps",
    "lame_set_write_id3tag_automatic",
    "lame_init_params",
    "lame_encode_buffer_interleaved_ieee_float",
    "lame_encode_buffer_ieee_float",
    "lame_encode_flush",
    "lame_get_lametag_frame",
    "lame_close",
    "get_lame_version",
];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_env == "msvc" {
        for name in EXPORTS {
            println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name}");
        }
        // lame_enc.dll's version resource: who built it, and LAME's
        // copyright and licence.
        println!("cargo:rerun-if-changed=assets/lame_enc.rc");
        let macros = ["MAJOR", "MINOR", "PATCH"].map(|part| {
            let name = format!("CARGO_PKG_VERSION_{part}");
            let value = std::env::var(&name).unwrap();
            format!("{name}={value}")
        });
        embed_resource::compile_for_cdylib("assets/lame_enc.rc", &macros)
            .manifest_optional()
            .unwrap();
    }
}
