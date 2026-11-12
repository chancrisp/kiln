fn main() {
    println!("cargo:rerun-if-changed=assets/kiln.rc");
    println!("cargo:rerun-if-changed=assets/kiln.ico");
    println!("cargo:rerun-if-changed=assets/kiln.exe.manifest");
    commit();
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
    delay_load_media_foundation();
    pdb_by_name();
}

/// Windows N and KN editions have no Media Foundation until the Media
/// Feature Pack is added. Loading its DLLs when the program starts would
/// stop Kiln with a system error there, so they are loaded on first use
/// instead, and kiln-media checks they exist before that (DIST-04).
/// kiln-cli's build.rs does the same for kiln.exe.
fn delay_load_media_foundation() {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    for dll in ["mfplat.dll", "mfreadwrite.dll", "mf.dll"] {
        println!("cargo:rustc-link-arg-bins=/DELAYLOAD:{dll}");
    }
    println!("cargo:rustc-link-arg-bins=delayimp.lib");
}

/// `KILN_COMMIT`, the commit being built, for crash files: with it, a
/// backtrace's bare addresses can be read later against that build's
/// symbols. "unknown" outside a git checkout.
fn commit() {
    let commit = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=KILN_COMMIT={commit}");
    // Run again when HEAD moves: HEAD itself, the branch it names, the
    // packed refs a branch can live in, and HEAD's reflog. Only files that
    // exist are named, since a missing one would run this on every build.
    // So a branch packed away by `git gc` is no longer watched, and the
    // commit after it makes a loose file this run didn't name; the reflog
    // gains a line for that commit, and for every other move of HEAD.
    let mut watch = vec![git(&["rev-parse", "--git-path", "HEAD"])];
    if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
        watch.push(git(&["rev-parse", "--git-path", &branch]));
    }
    watch.push(git(&["rev-parse", "--git-path", "packed-refs"]));
    watch.push(git(&["rev-parse", "--git-path", "logs/HEAD"]));
    for path in watch.into_iter().flatten() {
        if std::path::Path::new(&path).is_file() {
            println!("cargo:rerun-if-changed={path}");
        }
    }
}

/// What `git args` prints, trimmed, when it succeeds.
fn git(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git").args(args).output().ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let text = text.trim();
    (out.status.success() && !text.is_empty()).then(|| text.to_owned())
}

/// The program names its PDB by file name only, not by the full path of the
/// folder it was built in.
fn pdb_by_name() {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/PDBALTPATH:%_PDB%");
    }
}
