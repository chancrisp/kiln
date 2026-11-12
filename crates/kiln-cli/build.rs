fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/kiln-cli.rc");
    println!("cargo:rerun-if-changed=assets/kiln.exe.manifest");
    println!("cargo:rerun-if-changed=../kiln-app/assets/kiln.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // The version resource takes its numbers from these.
        let macros = ["MAJOR", "MINOR", "PATCH"].map(|part| {
            let name = format!("CARGO_PKG_VERSION_{part}");
            let value = std::env::var(&name).unwrap();
            format!("{name}={value}")
        });
        embed_resource::compile("assets/kiln-cli.rc", &macros)
            .manifest_required()
            .unwrap();
    }
    delay_load_media_foundation();
    pdb_by_name();
}

/// Windows N and KN editions have no Media Foundation until the Media
/// Feature Pack is added. Loading its DLLs when the program starts would
/// stop kiln.exe with a system error there, so they are loaded on first use
/// instead, and kiln-media checks they exist before that (DIST-04).
/// kiln-app's build.rs does the same for kiln-app.exe.
fn delay_load_media_foundation() {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    for dll in ["mfplat.dll", "mfreadwrite.dll", "mf.dll"] {
        println!("cargo:rustc-link-arg-bins=/DELAYLOAD:{dll}");
    }
    println!("cargo:rustc-link-arg-bins=delayimp.lib");
}

/// The program names its PDB by file name only, not by the full path of the
/// folder it was built in.
fn pdb_by_name() {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/PDBALTPATH:%_PDB%");
    }
}
