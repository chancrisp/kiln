fn main() {
    println!("cargo:rerun-if-changed=assets/kiln.rc");
    println!("cargo:rerun-if-changed=assets/kiln.ico");
    println!("cargo:rerun-if-changed=assets/kiln.exe.manifest");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/kiln.rc", embed_resource::NONE)
            .manifest_required()
            .unwrap();
    }
}
