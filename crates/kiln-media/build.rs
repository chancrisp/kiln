//! Builds the vendored libwavpack (see `vendor/README.md`).
//!
//! Only the library's own C files are compiled, with no assembly, no legacy
//! decoder and no DSD support. Warnings from the vendored C are switched off:
//! they are upstream's, and they must not fail Kiln's build.

const SOURCES: &[&str] = &[
    "common_utils",
    "decorr_utils",
    "entropy_utils",
    "extra1",
    "extra2",
    "open_legacy",
    "open_raw",
    "open_utils",
    "pack",
    "pack_dns",
    "pack_floats",
    "pack_utils",
    "read_words",
    "tag_utils",
    "tags",
    "unpack",
    "unpack_floats",
    "unpack_seek",
    "unpack_utils",
    "write_words",
];

fn main() {
    let dir = std::path::Path::new("vendor").join("wavpack");
    let mut build = cc::Build::new();
    build
        .include(dir.join("include"))
        .include(dir.join("src"))
        .warnings(false)
        .extra_warnings(false)
        .opt_level(2);
    for name in SOURCES {
        let file = dir.join("src").join(format!("{name}.c"));
        println!("cargo:rerun-if-changed={}", file.display());
        build.file(file);
    }
    println!("cargo:rerun-if-changed=build.rs");
    build.compile("wavpack");

    // kiln-app.exe and kiln.exe delay-load Media Foundation (DIST-04); the
    // tests here do too, so no_media_foundation.rs can see that a Windows
    // without it never has its DLLs touched.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        for dll in ["mfplat.dll", "mfreadwrite.dll", "mf.dll"] {
            println!("cargo:rustc-link-arg-tests=/DELAYLOAD:{dll}");
        }
        println!("cargo:rustc-link-arg-tests=delayimp.lib");
    }
}
