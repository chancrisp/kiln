fn main() {
    println!("cargo:rerun-if-changed=assets/kiln.rc");
    println!("cargo:rerun-if-changed=assets/kiln.ico");
    println!("cargo:rerun-if-changed=assets/kiln.exe.manifest");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // The version resource in kiln.rc takes its numbers from these.
        let macros = ["MAJOR", "MINOR", "PATCH"].map(|part| {
            let name = format!("CARGO_PKG_VERSION_{part}");
            let value = std::env::var(&name).unwrap();
            format!("{name}={value}")
        });
        embed_resource::compile("assets/kiln.rc", &macros)
            .manifest_required()
            .unwrap();
    }
}
