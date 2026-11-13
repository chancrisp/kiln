//! The installer's Open with list stays in step with the format registry,
//! the MSI and the ZIP ship the MP3 encoder and its notices, and the licence
//! list names every crate built into the programs.

use std::collections::BTreeSet;

use kiln_core::Registry;

const FRAGMENT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packaging/open-with-types.wxs"
);

/// Inputs Kiln reads but the Open with list leaves out. TypeScript files
/// share the `.ts` extension with MPEG-TS video.
const NOT_OFFERED: &[&str] = &["ts"];

/// The text between `start` and the next `"` after it, for each line
/// that holds `start`.
fn quoted_after<'a>(lines: impl Iterator<Item = &'a str>, start: &str) -> BTreeSet<String> {
    lines
        .filter_map(|line| {
            let rest = line.split_once(start)?.1;
            Some(rest.split_once('"')?.0.to_string())
        })
        .collect()
}

fn compare(what: &str, found: &BTreeSet<String>, want: &BTreeSet<String>) -> Option<String> {
    if found == want {
        return None;
    }
    let missing: Vec<&String> = want.difference(found).collect();
    let extra: Vec<&String> = found.difference(want).collect();
    Some(format!(
        "{what}: missing from the MSI: {missing:?}; not a Kiln input: {extra:?}"
    ))
}

#[test]
fn the_open_with_list_matches_the_registry() {
    let text =
        std::fs::read_to_string(FRAGMENT).unwrap_or_else(|e| panic!("cannot read {FRAGMENT}: {e}"));
    let inputs: BTreeSet<String> = Registry::builtin().input_extensions().into_iter().collect();
    let stale: Vec<&&str> = NOT_OFFERED
        .iter()
        .filter(|ext| !inputs.contains(**ext))
        .collect();
    assert!(
        stale.is_empty(),
        "NOT_OFFERED lists extensions that are no longer Kiln inputs: {stale:?}"
    );
    let want: BTreeSet<String> = inputs
        .into_iter()
        .filter(|ext| !NOT_OFFERED.contains(&ext.as_str()))
        .collect();

    // SupportedTypes: Name=".<ext>" on the lines that carry that key.
    let supported = quoted_after(
        text.lines()
            .filter(|l| l.contains(r"\kiln-app.exe\SupportedTypes")),
        "Name=\".",
    );

    // OpenWithProgids: Key="Software\Classes\.<ext>\OpenWithProgids".
    let progids: BTreeSet<String> = quoted_after(text.lines(), r#"Key="Software\Classes\."#)
        .iter()
        .filter_map(|rest| rest.strip_suffix(r"\OpenWithProgids"))
        .map(str::to_string)
        .collect();

    let mut problems: Vec<String> = [
        compare("SupportedTypes", &supported, &want),
        compare("OpenWithProgids", &progids, &want),
    ]
    .into_iter()
    .flatten()
    .collect();

    // Each type lists only Kiln.Convert, and every value is per machine.
    for (i, line) in text.lines().enumerate() {
        let n = i + 1;
        if line.contains(r"\OpenWithProgids") && !line.contains(r#"Name="Kiln.Convert""#) {
            problems.push(format!(
                "line {n}: an OpenWithProgids value not named Kiln.Convert"
            ));
        }
        if line.contains("<RegistryValue") && !line.contains(r#"Root="HKLM""#) {
            problems.push(format!("line {n}: a RegistryValue outside HKLM"));
        }
        // Under an extension's own key, only OpenWithProgids: never its
        // default value, which would make Kiln the default app.
        if let Some((_, rest)) = line.split_once(r#"Key="Software\Classes\."#) {
            let key = rest.split_once('"').map_or(rest, |(k, _)| k);
            if !key.ends_with(r"\OpenWithProgids") {
                problems.push(format!(
                    "line {n}: a value under an extension's key outside OpenWithProgids"
                ));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

const MSI: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../packaging/kiln.wxs");
const PACKAGE_SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../packaging/package.ps1");

/// The `Directory` of the component group each `File` is in, keyed by the
/// file's name (the end of its `Source`).
fn install_dirs(wxs: &str) -> Vec<(String, String)> {
    let mut dir = String::new();
    let mut files = Vec::new();
    for line in wxs.lines() {
        if line.contains("<ComponentGroup ") {
            dir = quoted_after(std::iter::once(line), "Directory=\"")
                .into_iter()
                .next()
                .unwrap_or_default();
        }
        if line.contains("</ComponentGroup>") {
            dir.clear();
        }
        if line.contains("<File ") {
            let source = quoted_after(std::iter::once(line), "Source=\"")
                .into_iter()
                .next()
                .unwrap_or_default();
            let name = source.rsplit(['\\', '/']).next().unwrap_or_default();
            files.push((name.to_string(), dir.clone()));
        }
    }
    files
}

#[test]
fn the_msi_installs_the_mp3_encoder() {
    let text = std::fs::read_to_string(MSI).unwrap_or_else(|e| panic!("cannot read {MSI}: {e}"));
    let files = install_dirs(&text);
    let dir_of = |name: &str| {
        files
            .iter()
            .find(|(file, _)| file == name)
            .map(|(_, dir)| dir.as_str())
    };
    let app = dir_of("kiln-app.exe").expect("the MSI has no kiln-app.exe");
    assert!(!app.is_empty(), "kiln-app.exe is not in a component group");
    // The encoder is loaded from the program's own folder and nowhere else,
    // so it has to land beside both programs.
    for name in ["lame_enc.dll", "kiln.exe"] {
        assert_eq!(
            dir_of(name),
            Some(app),
            "{name} must be installed in the same folder as kiln-app.exe"
        );
    }
    // LAME is LGPL: its licence and the notices ship with it.
    for name in ["LAME-COPYING.txt", "THIRD_PARTY_NOTICES.md"] {
        assert_eq!(dir_of(name), Some(app), "{name} is missing from the MSI");
    }
}

const TOAST_SOURCE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../crates/kiln-app/src/win/toast.rs"
);

/// The installed Kiln's notifications (R9): Windows shows a toast only for
/// an AppUserModelID that a Start menu shortcut carries, and its buttons
/// open a `kiln:` link, which starts kiln-app.exe with the link. Both are
/// the MSI's; the portable Kiln has neither and keeps the balloon.
#[test]
fn the_msi_gives_the_shortcut_kilns_app_id_and_registers_the_link() {
    let text = std::fs::read_to_string(MSI).unwrap_or_else(|e| panic!("cannot read {MSI}: {e}"));
    let source = std::fs::read_to_string(TOAST_SOURCE)
        .unwrap_or_else(|e| panic!("cannot read {TOAST_SOURCE}: {e}"));
    let aumid = source
        .lines()
        .find_map(|l| l.trim().strip_prefix("pub const AUMID: &str = \""))
        .and_then(|rest| rest.split('"').next())
        .expect("toast.rs names no AUMID");
    // The Start menu shortcut carries it.
    let shortcut = text
        .split("<Shortcut ")
        .find(|s| s.contains(r#"Directory="ProgramMenuFolder""#))
        .and_then(|s| s.split("</Shortcut>").next())
        .expect("the MSI has no Start menu shortcut with properties");
    let property = format!(r#"<ShortcutProperty Key="System.AppUserModel.ID" Value="{aumid}" />"#);
    assert!(
        shortcut.contains(&property),
        "the Start menu shortcut lacks {property}"
    );
    // The link: a URL protocol whose command starts this kiln-app.exe with
    // the link as its one argument.
    let link = text
        .split("<Component Id=\"KilnLink\"")
        .nth(1)
        .and_then(|c| c.split("</Component>").next())
        .expect("the MSI has no KilnLink component");
    for needle in [
        r#"Root="HKLM" Key="Software\Classes\kiln" Type="string" Value="URL:Kiln""#,
        r#"Root="HKLM" Key="Software\Classes\kiln" Name="URL Protocol" Type="string" Value="""#,
        r#"Root="HKLM" Key="Software\Classes\kiln\shell\open\command" Type="string" Value='"[INSTALLFOLDER]kiln-app.exe" "%1"'"#,
    ] {
        assert!(link.contains(needle), "the link lacks {needle}");
    }
    // It is installed with the program, in its component group.
    let files = text
        .split(r#"<ComponentGroup Id="KilnFiles""#)
        .nth(1)
        .and_then(|g| g.split("</ComponentGroup>").next())
        .expect("no KilnFiles group");
    assert!(
        files.contains(r#"<Component Id="KilnLink""#),
        "the link isn't installed with the program"
    );
}

/// The uninstall removes Kiln's start-up entry by asking Kiln to
/// (`kiln-app.exe --forget-startup`), which removes it only when it is its
/// own. It must never stop the uninstall, never run for the old version a
/// major upgrade replaces, and run as the person, before the files go.
#[test]
fn the_uninstall_asks_kiln_to_remove_its_start_up_entry() {
    let text = std::fs::read_to_string(MSI).unwrap_or_else(|e| panic!("cannot read {MSI}: {e}"));
    // The package itself never writes the Run value.
    assert!(
        !text.contains(r"CurrentVersion\Run\") && !text.contains(r#"CurrentVersion\Run""#),
        "the MSI writes the Run key itself"
    );
    let action = text
        .split("<CustomAction ")
        .nth(1)
        .and_then(|a| a.split("/>").next())
        .expect("the MSI has no custom action");
    for needle in [
        r#"Id="ForgetStartup""#,
        r#"ExeCommand='"[INSTALLFOLDER]kiln-app.exe" --forget-startup'"#,
        r#"Execute="deferred""#,
        r#"Impersonate="yes""#,
        r#"Return="ignore""#,
    ] {
        assert!(action.contains(needle), "ForgetStartup lacks {needle}");
    }
    let custom = text
        .split("<Custom ")
        .nth(1)
        .and_then(|c| c.split("/>").next())
        .expect("ForgetStartup is never scheduled");
    for needle in [
        r#"Action="ForgetStartup""#,
        r#"Before="RemoveFiles""#,
        r#"Condition='REMOVE~="ALL" AND NOT UPGRADINGPRODUCTCODE'"#,
    ] {
        assert!(custom.contains(needle), "its schedule lacks {needle}");
    }
}

#[test]
fn the_package_script_builds_and_stages_the_mp3_encoder() {
    let text = std::fs::read_to_string(PACKAGE_SCRIPT)
        .unwrap_or_else(|e| panic!("cannot read {PACKAGE_SCRIPT}: {e}"));
    for needle in [
        "-p kiln-lame",
        "lame_enc.dll",
        "LAME-COPYING.txt",
        "THIRD_PARTY_NOTICES.md",
    ] {
        assert!(text.contains(needle), "package.ps1 never mentions {needle}");
    }
    // It is signed like the executables, by the release workflow's first
    // signing round.
    let files = read(SIGNPATH_FILES);
    for name in ["kiln-app.exe", "kiln.exe", "lame_enc.dll"] {
        assert!(
            files.contains(&format!(r#"<pe-file path="{name}""#)),
            "kiln-files.xml doesn't sign {name}"
        );
    }
}

const LICENSES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../THIRD_PARTY_LICENSES");
const WORKSPACE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml");

/// "name version" for each crate the two programs are built from, as the
/// command in THIRD_PARTY_LICENSES lists them, Kiln's own crates left out.
fn crates_in_the_programs() -> BTreeSet<String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = std::process::Command::new(cargo)
        .args([
            "tree",
            "--locked",
            "--offline",
            "--manifest-path",
            WORKSPACE,
        ])
        .args(["--target", "x86_64-pc-windows-msvc", "-e", "normal,build"])
        .args(["-p", "kiln-app", "-p", "kiln-cli", "--prefix", "none"])
        .args(["-f", "{p}"])
        .output()
        .expect("cargo tree runs");
    assert!(
        out.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            // "name v1.2.3", then " (path)" for a path crate, " (*)" when
            // already listed.
            let mut words = line.split_whitespace();
            let name = words.next()?;
            let version = words.next()?.strip_prefix('v')?;
            (!name.starts_with("kiln-")).then(|| format!("{name} {version}"))
        })
        .collect()
}

#[test]
fn the_licence_list_names_every_crate_in_the_programs() {
    let text =
        std::fs::read_to_string(LICENSES).unwrap_or_else(|e| panic!("cannot read {LICENSES}: {e}"));
    // The table's rows: a name, a version and a licence, in columns.
    let listed: BTreeSet<String> = text
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let (name, version) = (words.next()?, words.next()?);
            let looks_like_a_row = version.starts_with(|c: char| c.is_ascii_digit())
                && version.contains('.')
                && words.next().is_some();
            looks_like_a_row.then(|| format!("{name} {version}"))
        })
        .collect();
    let built = crates_in_the_programs();
    assert!(!built.is_empty(), "cargo tree listed no crates");
    let missing: Vec<&String> = built.difference(&listed).collect();
    assert!(
        missing.is_empty(),
        "THIRD_PARTY_LICENSES doesn't list {missing:?}; regenerate its table with the command it gives"
    );
}

// ---------------------------------------------------------------------------
// FFmpeg, shipped as its own LGPL program (spec 4.1, review focus 6)

const FFMPEG_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../packaging/ffmpeg");

/// What `package.ps1` takes FFmpeg from: the build workflow's artifacts,
/// unzipped. Not in git.
const STAGED_FFMPEG: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../packaging/ffmpeg/staged");

/// The encoders the spec's 4.1 asks of the build. Each inner list is
/// satisfied by any one of its names.
const SHIPPED_ENCODERS: &[&[&str]] = &[
    &["h264_mf"],
    &["hevc_mf"],
    &["libvpx-vp9"],
    &["libaom-av1", "libsvtav1"],
    &["libopus"],
    &["libvorbis"],
    &["libmp3lame"],
    &["libwebp_anim"],
    &["libjxl"],
    &["libtheora"],
    &["gif"],
    &["apng"],
    &["mpeg4"],
    &["mpeg2video"],
    &["wmv2"],
];

/// The configure options in `buildconf` (FFmpeg's `-buildconf` output)
/// that Kiln can't ship: GPL and nonfree parts.
fn forbidden_options(buildconf: &str) -> Vec<String> {
    buildconf
        .split_whitespace()
        .filter(|w| *w == "--enable-gpl" || *w == "--enable-nonfree")
        .map(str::to_owned)
        .collect()
}

/// The encoder names in FFmpeg's `-encoders` output.
fn listed_encoders(text: &str) -> BTreeSet<String> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let caps = words.next()?;
            let name = words.next()?;
            let is_caps = caps.len() == 6
                && caps.starts_with(['V', 'A', 'S'])
                && caps.chars().all(|c| c == '.' || c.is_ascii_uppercase());
            (is_caps && name != "=").then(|| name.to_owned())
        })
        .collect()
}

/// The encoders `formats.toml` names, less Kiln's stand-ins (`@h264`) and
/// `copy`.
fn encoders_formats_name() -> BTreeSet<String> {
    let reg = Registry::builtin();
    reg.extensions()
        .filter_map(|e| reg.format(e))
        .flat_map(|f| {
            f.ffmpeg
                .windows(2)
                .filter(|w| w[0] == "-c:v" || w[0] == "-c:a")
                .map(|w| w[1].clone())
                .collect::<Vec<_>>()
        })
        .filter(|c| !c.starts_with('@') && c != "copy")
        .collect()
}

#[test]
fn the_build_checks_catch_gpl_parts_and_missing_encoders() {
    let full = "  configuration:\n    --enable-gpl\n    --enable-version3\n    --enable-libx264\n    --enable-nonfree\n";
    assert_eq!(
        forbidden_options(full),
        ["--enable-gpl", "--enable-nonfree"]
    );
    let lgpl = "  configuration:\n    --enable-version3\n    --enable-gplv3-free-thing\n";
    assert!(forbidden_options(lgpl).is_empty());
    let listed = listed_encoders(
        " V..... = Video\n ------\n V....D h264_mf  H264 via MediaFoundation\n A....D aac  AAC\n",
    );
    assert_eq!(
        listed.into_iter().collect::<Vec<_>>(),
        ["aac".to_owned(), "h264_mf".to_owned()]
    );
    // formats.toml's encoders are all ones an LGPL build can have.
    let named = encoders_formats_name();
    assert!(
        named.contains("libvpx-vp9") && named.contains("aac"),
        "{named:?}"
    );
    for gpl in ["libx264", "libx265", "libxvid", "libfdk_aac"] {
        assert!(!named.contains(gpl), "formats.toml names {gpl}");
    }
}

#[test]
fn the_msi_installs_ffmpeg_in_its_own_folder() {
    let text = std::fs::read_to_string(MSI).unwrap_or_else(|e| panic!("cannot read {MSI}: {e}"));
    // ffmpeg\bin inside Kiln's folder.
    let nested = text
        .split("<Directory Id=\"INSTALLFOLDER\"")
        .nth(1)
        .and_then(|rest| rest.split("</StandardDirectory>").next())
        .expect("the MSI has no INSTALLFOLDER");
    for needle in [
        r#"<Directory Id="FFMPEGFOLDER" Name="ffmpeg">"#,
        r#"<Directory Id="FFMPEGBINFOLDER" Name="bin" />"#,
    ] {
        assert!(nested.contains(needle), "the MSI lacks {needle}");
    }
    // Its licence, source offer and build notes beside bin.
    let files = install_dirs(&text);
    for name in ["LICENSE.txt", "SOURCE.txt", "BUILD.txt"] {
        assert!(
            files
                .iter()
                .any(|(file, dir)| file == name && dir == "FFMPEGFOLDER"),
            "the MSI doesn't install ffmpeg\\{name}"
        );
    }
    // Every file of the build's bin folder, in a group of its own.
    let group = text
        .split("<ComponentGroup Id=\"FfmpegProgram\"")
        .nth(1)
        .and_then(|g| g.split("</ComponentGroup>").next())
        .expect("the MSI has no FfmpegProgram group");
    assert!(group.contains(r#"Directory="FFMPEGBINFOLDER""#), "{group}");
    assert!(
        group.contains(r#"<Files Include="$(StageDir)\ffmpeg\bin\**" />"#),
        "{group}"
    );
    for group in ["FfmpegFiles", "FfmpegProgram"] {
        assert!(
            text.contains(&format!(r#"<ComponentGroupRef Id="{group}" />"#)),
            "{group} isn't in a feature"
        );
    }
}

#[test]
fn the_package_script_stages_ffmpeg_with_its_notices() {
    let text = std::fs::read_to_string(PACKAGE_SCRIPT)
        .unwrap_or_else(|e| panic!("cannot read {PACKAGE_SCRIPT}: {e}"));
    for needle in [
        r"ffmpeg\staged",
        r"ffmpeg\bin",
        "LICENSE.txt",
        "SOURCE.txt",
        "BUILD.txt",
        "-buildconf",
        "--enable-(gpl|nonfree)",
        "ffmpeg-source.zip",
    ] {
        assert!(text.contains(needle), "package.ps1 never mentions {needle}");
    }
    // The templates it fills are in the repository.
    let source = std::fs::read_to_string(format!("{FFMPEG_DIR}/SOURCE.txt"))
        .expect("packaging/ffmpeg/SOURCE.txt");
    for needle in [
        "LGPL",
        "https://github.com/chancrisp/kiln/releases/download/v{{KILN_VERSION}}/Kiln-{{KILN_VERSION}}-ffmpeg-source.zip",
    ] {
        assert!(source.contains(needle), "SOURCE.txt lacks {needle}");
    }
    assert!(
        !source.to_ascii_lowercase().contains("ffmpeg.org"),
        "SOURCE.txt sends people to ffmpeg.org"
    );
    let build = std::fs::read_to_string(format!("{FFMPEG_DIR}/BUILD.txt"))
        .expect("packaging/ffmpeg/BUILD.txt");
    for needle in ["{{BUILDS_COMMIT}}", "{{CONFIGURATION}}"] {
        assert!(build.contains(needle), "BUILD.txt lacks {needle}");
    }
    // Staged builds never reach git.
    let ignore = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../.gitignore"))
        .expect(".gitignore");
    assert!(
        ignore
            .lines()
            .any(|l| l.trim() == "/packaging/ffmpeg/staged/"),
        ".gitignore doesn't keep the staged FFmpeg out"
    );
}

/// Review focus 6: the FFmpeg staged for packaging has no GPL or nonfree
/// parts and has the encoders Kiln asks of it. Skipped when none is staged.
#[test]
fn a_staged_ffmpeg_is_lgpl_with_the_encoders_kiln_uses() {
    let staged = std::path::Path::new(STAGED_FFMPEG);
    let exe = staged.join("bin").join("ffmpeg.exe");
    if !exe.is_file() {
        eprintln!(
            "a_staged_ffmpeg_is_lgpl_with_the_encoders_kiln_uses: skipped, no FFmpeg staged at {}",
            exe.display()
        );
        return;
    }
    let run = |args: &[&str]| {
        let out = std::process::Command::new(&exe)
            .args(args)
            .output()
            .unwrap_or_else(|e| panic!("{} doesn't run: {e}", exe.display()));
        assert!(out.status.success(), "ffmpeg {args:?} failed");
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr)
    };
    let conf = run(&["-hide_banner", "-buildconf"]);
    assert!(
        conf.contains("configuration:"),
        "no build configuration: {conf}"
    );
    let bad = forbidden_options(&conf);
    assert!(bad.is_empty(), "the staged FFmpeg was built with {bad:?}");

    let listed = listed_encoders(&run(&["-hide_banner", "-encoders"]));
    let missing: Vec<String> = SHIPPED_ENCODERS
        .iter()
        .filter(|any| !any.iter().any(|e| listed.contains(*e)))
        .map(|any| any.join(" or "))
        .chain(
            encoders_formats_name()
                .into_iter()
                .filter(|e| !listed.contains(e)),
        )
        .collect();
    assert!(missing.is_empty(), "the staged FFmpeg lacks {missing:?}");
    for gpl in GPL_ENCODERS {
        assert!(!listed.contains(*gpl), "the staged FFmpeg has {gpl}");
    }
    // Its libraries are its own build's, and it is under the LGPL.
    let bad = mixed_or_gpl(&run(&["-version"]), &run(&["-hide_banner", "-L"]));
    assert!(bad.is_empty(), "the staged FFmpeg says {bad:?}");
    let refused: Vec<String> = std::fs::read_dir(staged.join("bin"))
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|name| bin_file(name) == BinFile::Refused)
        .collect();
    assert!(refused.is_empty(), "the staged bin has {refused:?}");

    for name in ["bin/ffprobe.exe", "LICENSE.txt", "BUILD.txt"] {
        assert!(staged.join(name).is_file(), "nothing staged at {name}");
    }
    let notices = std::fs::read_dir(staged.join("licenses"))
        .map(|dir| {
            dir.filter_map(Result::ok)
                .filter(|e| e.path().extension().is_some_and(|x| x == "txt"))
                .count()
        })
        .unwrap_or(0);
    assert!(notices > 0, "no library notices staged in licenses");
    let build = std::fs::read_to_string(staged.join("BUILD.txt")).unwrap();
    assert!(
        !build.contains("{{") && forbidden_options(&build).is_empty(),
        "BUILD.txt is unfilled or names GPL parts"
    );
    assert!(
        staged.join("ffmpeg-source.zip").is_file(),
        "the source zip isn't staged beside the build"
    );
}

const FFMPEG_WORKFLOW: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../.github/workflows/ffmpeg-lgpl.yml"
);

/// The pattern `package.ps1` copies the staged `bin` folder by: FFmpeg's
/// two programs and its own libraries, nothing else.
const SHIPPED_BIN: &str = r"^(ffmpeg|ffprobe)\.exe$|^(avcodec|avdevice|avfilter|avformat|avutil|swresample|swscale)-\d+\.dll$";

/// What `package.ps1` does with a file in the staged `bin` folder.
#[derive(Debug, PartialEq, Eq)]
enum BinFile {
    Shipped,
    /// Left out (ffplay.exe, anything else).
    Left,
    /// Stops the packaging: libpostproc is GPL only, so an LGPL build
    /// never has it.
    Refused,
}

/// [`SHIPPED_BIN`], and the postproc rule, by hand.
fn bin_file(name: &str) -> BinFile {
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("postproc") && lower.ends_with(".dll") {
        return BinFile::Refused;
    }
    if lower == "ffmpeg.exe" || lower == "ffprobe.exe" {
        return BinFile::Shipped;
    }
    let library = [
        "avcodec",
        "avdevice",
        "avfilter",
        "avformat",
        "avutil",
        "swresample",
        "swscale",
    ]
    .iter()
    .any(|lib| {
        lower
            .strip_prefix(lib)
            .and_then(|rest| rest.strip_prefix('-'))
            .and_then(|rest| rest.strip_suffix(".dll"))
            .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
    });
    if library {
        BinFile::Shipped
    } else {
        BinFile::Left
    }
}

/// Why the output of `ffmpeg -version` and `ffmpeg -L` would stop the
/// packaging: libraries from another build, or a GPL licence.
fn mixed_or_gpl(version: &str, licence: &str) -> Vec<&'static str> {
    let mut found = vec![];
    if version
        .to_ascii_lowercase()
        .contains("configuration mismatch")
    {
        found.push("configuration mismatch");
    }
    if licence.contains("GNU General Public License") {
        found.push("GNU General Public License");
    }
    found
}

/// The encoders that mean a GPL or nonfree build.
const GPL_ENCODERS: &[&str] = &["libx264", "libx265", "libxvid", "libfdk_aac"];

#[test]
fn only_ffmpegs_own_files_are_taken_from_the_staged_bin() {
    for name in [
        "ffmpeg.exe",
        "ffprobe.exe",
        "avcodec-61.dll",
        "avutil-59.dll",
        "swscale-8.dll",
        "AVFORMAT-61.DLL",
    ] {
        assert_eq!(bin_file(name), BinFile::Shipped, "{name}");
    }
    for name in [
        "ffplay.exe",
        "avcodec.dll",
        "avcodec-61.dll.bak",
        "libx264-164.dll",
        "notes.txt",
    ] {
        assert_eq!(bin_file(name), BinFile::Left, "{name}");
    }
    assert_eq!(bin_file("postproc-58.dll"), BinFile::Refused);
    assert_eq!(
        mixed_or_gpl(
            "ffmpeg version 7.1\nWARNING: library configuration mismatch\n",
            "under the terms of the GNU Lesser General Public License"
        ),
        ["configuration mismatch"]
    );
    assert_eq!(
        mixed_or_gpl(
            "ffmpeg version 7.1",
            "under the terms of the GNU General Public License"
        ),
        ["GNU General Public License"]
    );
    assert!(mixed_or_gpl(
        "ffmpeg version 7.1",
        "the GNU Lesser General Public License, version 2.1"
    )
    .is_empty());

    // package.ps1 does the same: it copies by the pattern, never the whole
    // folder, and refuses what these refuse.
    let script = std::fs::read_to_string(PACKAGE_SCRIPT)
        .unwrap_or_else(|e| panic!("cannot read {PACKAGE_SCRIPT}: {e}"));
    for needle in [
        SHIPPED_BIN,
        "postproc*.dll",
        "configuration mismatch",
        "GNU General Public License",
        "-encoders",
    ] {
        assert!(
            script.contains(needle),
            "package.ps1 never mentions {needle}"
        );
    }
    for encoder in GPL_ENCODERS {
        assert!(
            script.contains(encoder),
            "package.ps1 doesn't refuse {encoder}"
        );
    }
    assert!(
        !script.contains(r"'bin\*'"),
        "package.ps1 copies the whole staged bin folder"
    );
}

#[test]
fn the_libraries_built_into_ffmpeg_ship_their_notices() {
    // The build gathers each library's licence files...
    let workflow = std::fs::read_to_string(FFMPEG_WORKFLOW)
        .unwrap_or_else(|e| panic!("cannot read {FFMPEG_WORKFLOW}: {e}"));
    for needle in ["out/ffmpeg/licenses", "COPYING", "LICENSE", "PATENTS"] {
        assert!(
            workflow.contains(needle),
            "ffmpeg-lgpl.yml never mentions {needle}"
        );
    }
    // ...package.ps1 requires and stages them...
    let script = std::fs::read_to_string(PACKAGE_SCRIPT).unwrap();
    assert!(
        script.contains(r"ffmpeg\licenses") || script.contains("'licenses'"),
        "package.ps1 doesn't stage ffmpeg\\licenses"
    );
    // ...and the MSI installs them in ffmpeg\licenses.
    let wxs = std::fs::read_to_string(MSI).unwrap();
    assert!(
        wxs.contains(r#"<Directory Id="FFMPEGLICENSESFOLDER" Name="licenses" />"#),
        "the MSI has no ffmpeg\\licenses folder"
    );
    let group = wxs
        .split("<ComponentGroup Id=\"FfmpegLicenses\"")
        .nth(1)
        .and_then(|g| g.split("</ComponentGroup>").next())
        .expect("the MSI has no FfmpegLicenses group");
    assert!(
        group.contains(r#"Directory="FFMPEGLICENSESFOLDER""#),
        "{group}"
    );
    assert!(
        group.contains(r#"<Files Include="$(StageDir)\ffmpeg\licenses\**" />"#),
        "{group}"
    );
    assert!(
        wxs.contains(r#"<ComponentGroupRef Id="FfmpegLicenses" />"#),
        "FfmpegLicenses isn't in a feature"
    );
    // The notices say where they are.
    let notices = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../THIRD_PARTY_NOTICES.md"
    ))
    .unwrap();
    assert!(
        notices.contains(r"`ffmpeg\licenses`"),
        "THIRD_PARTY_NOTICES.md doesn't point at ffmpeg\\licenses"
    );
}

#[test]
fn the_shipped_licence_list_names_ffmpeg_as_bundled() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../THIRD_PARTY_LICENSES"
    ))
    .unwrap();
    assert!(
        !text.to_ascii_lowercase().contains("ffmpeg.org"),
        "THIRD_PARTY_LICENSES names ffmpeg.org"
    );
    let not_bundled = text
        .replace("\r\n", "\n")
        .split("not bundled")
        .nth(1)
        // The section ends where a blank line comes before the next rule.
        .and_then(|rest| rest.split("\n\n---").next())
        .unwrap_or_default()
        .to_owned();
    assert!(
        !not_bundled.contains("FFmpeg"),
        "THIRD_PARTY_LICENSES lists FFmpeg as not bundled: {not_bundled}"
    );
    assert!(text.contains(r"ffmpeg\LICENSE.txt"), "{text}");
}

const FONTS_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../crates/kiln-app/assets/fonts"
);

/// R16: the old look's bundled serif is gone, and its licence with it. The
/// MSI, the ZIP and the licence list name neither, and no font is bundled.
#[test]
fn nothing_ships_the_old_serif_or_its_licence() {
    for path in [MSI, PACKAGE_SCRIPT, LICENSES] {
        let text =
            std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"));
        assert!(!text.contains("OFL.txt"), "{path} still names OFL.txt");
        assert!(
            !text.to_lowercase().contains("fraunces"),
            "{path} still names Fraunces"
        );
    }
    let fonts: Vec<String> = std::fs::read_dir(FONTS_DIR)
        .map(|dir| {
            dir.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    assert!(fonts.is_empty(), "fonts are still bundled: {fonts:?}");
}

// ---------------------------------------------------------------------------
// Releases: the LGPL FFmpeg is built once and kept on a private release of
// this repository, which later release builds reuse; the first build
// pins FFmpeg-Builds' latest commit and records it there; and nothing
// ships while the source offer has no address. These read the files,
// and run one script on made-up files: no test here touches the network.

const RELEASE_WORKFLOW: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../.github/workflows/release.yml"
);
const UNPACK_STORED: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packaging/ffmpeg/unpack-stored.ps1"
);
/// The release the stored FFmpeg is kept on.
const STORE_TAG: &str = "ffmpeg-lgpl";
/// The SignPath artifact configurations the release workflow names.
const SIGNPATH_FILES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packaging/signpath/kiln-files.xml"
);
const SIGNPATH_MSI: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packaging/signpath/kiln-msi.xml"
);
const CHECK_SIGNATURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packaging/check-signatures.ps1"
);

/// The file at `path`, with LF line ends: a Windows checkout with
/// `core.autocrlf` gives the workflows CRLF ones, and `job` looks for
/// a job by the LF line ends around its name.
fn read(path: &str) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {path}: {e}"))
        .replace("\r\n", "\n")
}

/// The value of `key` in a workflow's `env:` blocks, without quotes.
fn workflow_env(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let value = line.trim().strip_prefix(key)?.strip_prefix(':')?;
        let value = value.split(" #").next().unwrap_or_default().trim();
        Some(value.trim_matches('"').to_owned())
    })
}

/// The text of job `name` in a workflow: from its `  name:` line to the
/// next job.
fn job<'a>(text: &'a str, name: &str) -> &'a str {
    let head = format!("\n  {name}:\n");
    let start = text
        .find(&head)
        .unwrap_or_else(|| panic!("the workflow has no job {name}"))
        + 1;
    let rest = &text[start..];
    let end = rest
        .match_indices("\n  ")
        .find(|(i, _)| {
            let after = &rest[i + 3..];
            after.starts_with(|c: char| c.is_ascii_alphanumeric())
                && after.split('\n').next().is_some_and(|l| l.ends_with(':'))
        })
        .map_or(rest.len(), |(i, _)| i);
    &rest[..end]
}

/// The secrets a workflow reads.
fn secrets(text: &str) -> BTreeSet<String> {
    text.match_indices("secrets.")
        .map(|(i, _)| {
            text[i + "secrets.".len()..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect()
        })
        .collect()
}

#[test]
fn the_ffmpeg_build_is_stored_once_and_reused_by_release_builds() {
    let build = read(FFMPEG_WORKFLOW);
    assert!(
        build.contains("workflow_dispatch:"),
        "it can't be run by hand"
    );
    assert!(
        build.contains("workflow_call:"),
        "the release workflow can't call it"
    );
    assert_eq!(
        workflow_env(&build, "STORE_TAG").as_deref(),
        Some(STORE_TAG)
    );
    // It builds only when nothing is stored yet.
    let look = job(&build, "look");
    assert!(look.contains("releases/tags/$STORE_TAG"), "{look}");
    assert!(
        job(&build, "build").contains("if: needs.look.outputs.stored != 'true'"),
        "the build runs even when a build is stored"
    );
    // It stores the zips, their SHA-256 and the pin on a release of this
    // repository, with the token every run has.
    let store = job(&build, "store");
    for needle in [
        "contents: write",
        "GH_TOKEN: ${{ github.token }}",
        "gh release create \"$STORE_TAG\"",
        "--latest=false",
        "sha256sum ffmpeg-lgpl-win64.zip ffmpeg-source.zip > SHA256SUMS",
        "builds-commit.txt",
    ] {
        assert!(store.contains(needle), "the store job lacks {needle}");
    }
    assert!(
        secrets(&build).is_empty(),
        "ffmpeg-lgpl.yml reads secrets: {:?}",
        secrets(&build)
    );

    // Each release build calls it, then takes the stored zips, checked
    // against their SHA-256, and packages FFmpeg with them.
    let release = read(RELEASE_WORKFLOW);
    let ffmpeg = job(&release, "ffmpeg");
    assert!(
        ffmpeg.contains("uses: ./.github/workflows/ffmpeg-lgpl.yml"),
        "{ffmpeg}"
    );
    assert!(ffmpeg.contains("contents: write"), "{ffmpeg}");
    let package = job(&release, "build");
    assert!(package.contains("needs: [ci, ffmpeg]"), "{package}");
    let download = package
        .find(&format!("gh release download {STORE_TAG} "))
        .expect("the release build doesn't fetch the stored FFmpeg");
    let unpack = package
        .find("packaging/ffmpeg/unpack-stored.ps1")
        .expect("the release build doesn't check and unpack the stored FFmpeg");
    let pack = package
        .find("./packaging/package.ps1 -Stage")
        .expect("the release build doesn't package");
    assert!(download < unpack && unpack < pack, "{package}");
    assert!(
        !release.contains("-WithoutFfmpeg"),
        "a release build leaves FFmpeg out"
    );
    // No new secrets: the stored FFmpeg comes with the run's own token, and
    // the only others are the signing services'.
    let known: BTreeSet<String> = [
        "SIGNPATH_API_TOKEN",
        "AZURE_CLIENT_ID",
        "AZURE_TENANT_ID",
        "AZURE_SUBSCRIPTION_ID",
    ]
    .map(String::from)
    .into();
    assert_eq!(secrets(&release), known);

    // The script checks the SHA-256 recorded with the zips before anything
    // is unpacked.
    let script = read(UNPACK_STORED);
    for needle in ["SHA256SUMS", "Get-FileHash", "builds-commit.txt"] {
        assert!(script.contains(needle), "unpack-stored.ps1 lacks {needle}");
    }
}

#[test]
fn the_first_build_pins_the_latest_builds_commit_and_later_ones_keep_it() {
    let build = read(FFMPEG_WORKFLOW);
    let pin = workflow_env(&build, "BUILDS_COMMIT").expect("BUILDS_COMMIT");
    assert!(
        pin.is_empty() || (pin.len() == 40 && pin.bytes().all(|b| b.is_ascii_hexdigit())),
        "BUILDS_COMMIT is {pin:?}: leave it empty to keep the recorded pin, or set a full commit to change it on purpose"
    );
    assert!(!build.contains("FILL-IN"), "a fill-in placeholder is left");
    // The pin recorded on the stored release wins; only a run with nothing
    // stored and nothing set takes FFmpeg-Builds' latest commit.
    let look = job(&build, "look");
    let recorded = look
        .find("builds-commit.txt")
        .expect("the look never reads the recorded pin");
    let latest = look
        .find("refs/heads/master")
        .expect("the first run never takes the latest commit");
    assert!(recorded < latest, "{look}");
    // A different pin set on purpose stops the run: a run never replaces
    // the stored build.
    let differs = &look[recorded..latest];
    assert!(differs.contains("::error::"), "{differs}");
    assert!(differs.contains("exit 1"), "{differs}");
    // The build, its notes and the record all use the look's pin.
    for name in ["build", "check", "store"] {
        assert!(
            job(&build, name).contains("BUILDS_COMMIT: ${{ needs.look.outputs.builds_commit }}"),
            "job {name} doesn't use the look's pin"
        );
    }
    assert!(
        job(&build, "store").contains(r#"printf '%s\n' "$BUILDS_COMMIT" > builds-commit.txt"#),
        "the store job doesn't record the pin"
    );
}

/// FFmpeg's source goes beside each download on the same release page:
/// SOURCE.txt says where, with no written offer and no address, and nothing
/// waits for one.
#[test]
fn ffmpegs_source_is_on_the_release_page_beside_it() {
    let note = read(&format!("{FFMPEG_DIR}/SOURCE.txt"));
    for gone in [
        "SOURCE_OFFER_ADDRESS_NOT_SET",
        "on request",
        "three years",
        "@",
    ] {
        assert!(!note.contains(gone), "SOURCE.txt still has {gone:?}");
    }
    let script = read(PACKAGE_SCRIPT);
    assert!(!script.contains("SOURCE_OFFER_ADDRESS_NOT_SET"));
    // The zip SOURCE.txt names is made beside the packages.
    assert!(script.contains(r#"Join-Path $dist "Kiln-$version-ffmpeg-source.zip""#));
    let release = read(RELEASE_WORKFLOW);
    assert!(
        !release.contains(
            "
  offer:
"
        ),
        "the release still has an offer job"
    );
    assert!(job(&release, "ffmpeg").contains("needs: version"));
}

/// Runs `unpack-stored.ps1` in pwsh. Whether it succeeded, and what it
/// printed.
fn unpack_stored(from: &std::path::Path, to: &std::path::Path) -> (bool, String) {
    let out = std::process::Command::new("pwsh")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-File",
            UNPACK_STORED,
            "-From",
        ])
        .arg(from)
        .arg("-To")
        .arg(to)
        .output()
        .expect("pwsh runs");
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    (out.status.success(), text)
}

/// Writes made-up copies of what the store job puts on the release into
/// `dir\from`: the two zips, `SHA256SUMS`, and `builds-commit.txt` saying
/// `pin`, with a `BUILD.txt` in the win64 zip naming `built_at`. `None`
/// when pwsh isn't installed.
fn stored_ffmpeg(dir: &std::path::Path, pin: &str, built_at: &str) -> Option<()> {
    let make = r#"
param([string]$Dir, [string]$Pin, [string]$BuiltAt)
$ErrorActionPreference = 'Stop'
$w = Join-Path $Dir 'w'
New-Item -ItemType Directory -Force (Join-Path $w 'bin') | Out-Null
Set-Content (Join-Path $w 'bin' 'ffmpeg.exe') 'not really ffmpeg'
Set-Content (Join-Path $w 'BUILD.txt') "FFmpeg-Builds at commit $BuiltAt"
$from = Join-Path $Dir 'from'
New-Item -ItemType Directory -Force $from | Out-Null
Compress-Archive -Path (Join-Path $w '*') -DestinationPath (Join-Path $from 'ffmpeg-lgpl-win64.zip')
Compress-Archive -Path (Join-Path $w 'BUILD.txt') -DestinationPath (Join-Path $from 'ffmpeg-source.zip')
$sums = foreach ($n in 'ffmpeg-lgpl-win64.zip', 'ffmpeg-source.zip') {
  "$((Get-FileHash (Join-Path $from $n) -Algorithm SHA256).Hash.ToLower())  $n"
}
Set-Content (Join-Path $from 'SHA256SUMS') $sums
Set-Content (Join-Path $from 'builds-commit.txt') $Pin
"#;
    let script = dir.join("make.ps1");
    std::fs::write(&script, make).unwrap();
    let status = std::process::Command::new("pwsh")
        .args(["-NoProfile", "-NonInteractive", "-File"])
        .arg(&script)
        .arg("-Dir")
        .arg(dir)
        .args(["-Pin", pin, "-BuiltAt", built_at])
        .status()
        .ok()?;
    assert!(status.success(), "couldn't make the stored files");
    Some(())
}

#[test]
fn the_stored_ffmpeg_is_unpacked_only_when_it_matches_its_record() {
    const PIN: &str = "0123456789abcdef0123456789abcdef01234567";
    const OTHER: &str = "89abcdef0123456789abcdef0123456789abcdef";
    let entries = |p: &std::path::Path| -> Vec<String> {
        std::fs::read_dir(p)
            .map(|d| {
                d.filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    };

    // As stored: unpacked into the staged layout package.ps1 reads.
    let dir = tempfile::tempdir().unwrap();
    if stored_ffmpeg(dir.path(), PIN, PIN).is_none() {
        eprintln!(
            "the_stored_ffmpeg_is_unpacked_only_when_it_matches_its_record: skipped, no pwsh"
        );
        return;
    }
    let from = dir.path().join("from");
    let to = dir.path().join("staged");
    let (ok, text) = unpack_stored(&from, &to);
    assert!(ok, "{text}");
    assert!(to.join("bin").join("ffmpeg.exe").is_file(), "{text}");
    assert!(to.join("BUILD.txt").is_file(), "{text}");
    assert!(to.join("ffmpeg-source.zip").is_file(), "{text}");

    // A zip that changed after its SHA-256 was recorded: nothing unpacked.
    let dir = tempfile::tempdir().unwrap();
    stored_ffmpeg(dir.path(), PIN, PIN).unwrap();
    let from = dir.path().join("from");
    let zip = from.join("ffmpeg-lgpl-win64.zip");
    let mut bytes = std::fs::read(&zip).unwrap();
    bytes.push(0);
    std::fs::write(&zip, bytes).unwrap();
    let to = dir.path().join("staged");
    let (ok, text) = unpack_stored(&from, &to);
    assert!(!ok, "a changed zip was unpacked");
    assert!(text.contains("SHA-256"), "{text}");
    assert!(entries(&to).is_empty(), "{:?}", entries(&to));

    // A build whose BUILD.txt names another FFmpeg-Builds commit than the
    // record: nothing unpacked.
    let dir = tempfile::tempdir().unwrap();
    stored_ffmpeg(dir.path(), PIN, OTHER).unwrap();
    let from = dir.path().join("from");
    let to = dir.path().join("staged");
    let (ok, text) = unpack_stored(&from, &to);
    assert!(!ok, "a build from another pin was unpacked");
    assert!(text.contains("BUILD.txt"), "{text}");
    assert!(entries(&to).is_empty(), "{:?}", entries(&to));

    // A staged folder that already has files is left as it is.
    let dir = tempfile::tempdir().unwrap();
    stored_ffmpeg(dir.path(), PIN, PIN).unwrap();
    let from = dir.path().join("from");
    let to = dir.path().join("staged");
    std::fs::create_dir(&to).unwrap();
    std::fs::write(to.join("keep.txt"), "mine").unwrap();
    let (ok, text) = unpack_stored(&from, &to);
    assert!(!ok, "files already staged were unpacked over");
    assert!(text.contains("already has files"), "{text}");
    assert_eq!(entries(&to), ["keep.txt"]);
    assert_eq!(
        std::fs::read_to_string(to.join("keep.txt")).unwrap(),
        "mine"
    );
}

const CI_WORKFLOW: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../.github/workflows/ci.yml"
);

#[test]
fn every_workflow_action_is_pinned_by_its_full_commit() {
    // A tag can be moved to other code; a commit can't (M6). The release
    // step sees KILN_RELEASES_TOKEN. The tag stays in a comment.
    let mut seen = 0;
    for path in [CI_WORKFLOW, FFMPEG_WORKFLOW, RELEASE_WORKFLOW] {
        let text =
            std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"));
        for line in text.lines() {
            let Some(uses) = line
                .trim_start()
                .trim_start_matches("- ")
                .strip_prefix("uses:")
            else {
                continue;
            };
            let uses = uses.trim();
            // The workflow's own reusable workflows are in this repository.
            if uses.starts_with("./") {
                continue;
            }
            seen += 1;
            let (action, rest) = uses
                .split_once('@')
                .unwrap_or_else(|| panic!("{path}: {line}"));
            let (sha, comment) = rest.split_once(" # ").unwrap_or((rest, ""));
            assert!(
                action.contains('/')
                    && sha.len() == 40
                    && sha
                        .chars()
                        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                "{path}: not pinned by a full commit: {line}"
            );
            assert!(
                !comment.trim().is_empty(),
                "{path}: no tag named beside the commit: {line}"
            );
        }
    }
    assert!(seen >= 10, "{seen} actions");
}

/// Kiln is GPL-3.0-or-later (phase 4): LICENSE is the GPL's own text, every
/// Kiln crate says so, and nothing Kiln ships or shows still calls it
/// proprietary.
#[test]
fn kiln_is_gpl_3_or_later() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let read = |p: &str| {
        std::fs::read_to_string(format!("{root}/{p}")).unwrap_or_else(|e| panic!("{p}: {e}"))
    };
    let licence = read("LICENSE").replace("\r\n", "\n");
    assert!(licence.starts_with("                    GNU GENERAL PUBLIC LICENSE\n                       Version 3, 29 June 2007\n"));
    assert!(licence
        .trim_end()
        .ends_with("<https://www.gnu.org/licenses/why-not-lgpl.html>."));
    assert_eq!(
        licence.len(),
        35_149,
        "LICENSE isn't the GPL-3.0 text unchanged"
    );

    assert!(read("Cargo.toml").contains("\nlicense = \"GPL-3.0-or-later\""));
    for c in ["kiln-app", "kiln-core", "kiln-media", "kiln-cli"] {
        assert!(
            read(&format!("crates/{c}/Cargo.toml")).contains("license.workspace = true"),
            "{c}"
        );
    }
    for p in [
        "README.md",
        "THIRD_PARTY_NOTICES.md",
        "crates/kiln-app/src/settings_page.rs",
    ] {
        let s = read(p).to_lowercase();
        assert!(!s.contains("proprietary"), "{p} calls Kiln proprietary");
        assert!(!s.contains("licenseref-kiln"), "{p}");
    }
    assert!(read("THIRD_PARTY_NOTICES.md").contains("Copyright (C) 2026 Chandler Crisp"));
}

/// One publisher name everywhere: the MSI's Manufacturer and
/// every shipped binary's CompanyName, with a copyright line in each.
#[test]
fn one_publisher_name_everywhere() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let read = |p: &str| {
        std::fs::read_to_string(format!("{root}/{p}")).unwrap_or_else(|e| panic!("{p}: {e}"))
    };
    assert!(read("packaging/kiln.wxs").contains(r#"Manufacturer="Chandler Crisp""#));
    for rc in [
        "crates/kiln-app/assets/kiln.rc",
        "crates/kiln-cli/assets/kiln-cli.rc",
        "crates/kiln-lame/assets/lame_enc.rc",
    ] {
        let s = read(rc);
        assert!(
            s.contains(r#"VALUE "CompanyName", "Chandler Crisp""#),
            "{rc}"
        );
        assert!(s.contains(r#"VALUE "LegalCopyright", ""#), "{rc}");
        assert!(s.contains("1 VERSIONINFO"), "{rc}");
    }
    assert!(read("crates/kiln-cli/build.rs").contains("assets/kiln-cli.rc"));
    assert!(read("crates/kiln-lame/build.rs").contains("compile_for_cdylib(\"assets/lame_enc.rc\""));
}

/// Both programs delay-load Media Foundation, so Windows N editions start
///; kiln-media checks the DLLs are there before calling in.
#[test]
fn both_programs_delay_load_media_foundation() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    for build in ["crates/kiln-app/build.rs", "crates/kiln-cli/build.rs"] {
        let s = std::fs::read_to_string(format!("{root}/{build}")).unwrap();
        assert!(s.contains("delay_load_media_foundation();"), "{build}");
        assert!(
            s.contains(r#"for dll in ["mfplat.dll", "mfreadwrite.dll", "mf.dll"]"#)
                && s.contains("cargo:rustc-link-arg-bins=/DELAYLOAD:{dll}")
                && s.contains("cargo:rustc-link-arg-bins=delayimp.lib"),
            "{build}"
        );
    }
}

/// The MSI asks Windows Error Reporting to keep small crash dumps of
/// kiln-app.exe in the user's own Kiln folder, five at most, so a
/// crash that kills the process before the panic hook runs leaves something
/// to read. They stay on the PC.
#[test]
fn the_msi_keeps_minidumps_of_kiln_on_the_pc() {
    let wxs = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../packaging/kiln.wxs"
    ))
    .unwrap()
    .replace("\r\n", "\n");
    let key = r"Software\Microsoft\Windows\Windows Error Reporting\LocalDumps\kiln-app.exe";
    let start = wxs
        .find(r#"<Component Id="CrashDumps">"#)
        .expect("no CrashDumps component");
    let c = &wxs[start..start + wxs[start..].find("</Component>").unwrap()];
    for want in [
        format!(
            r#"Root="HKLM" Key="{key}" Name="DumpFolder" Type="expandable" Value="%LOCALAPPDATA%\Kiln\CrashDumps""#
        ),
        format!(r#"Root="HKLM" Key="{key}" Name="DumpCount" Type="integer" Value="5""#),
        format!(r#"Root="HKLM" Key="{key}" Name="DumpType" Type="integer" Value="1""#),
    ] {
        assert!(c.contains(&want), "missing {want}");
    }
    // It is in KilnFiles, which the Main feature installs.
    let group = wxs.find(r#"<ComponentGroup Id="KilnFiles""#).unwrap();
    let group_end = group + wxs[group..].find("</ComponentGroup>").unwrap();
    assert!(
        (group..group_end).contains(&start),
        "CrashDumps is not in KilnFiles"
    );
}

/// A release is signed in CI only, through SignPath or Azure Artifact
/// Signing, and every file is checked before anything is published
///. Nothing reads a certificate file any more.
#[test]
fn a_release_is_signed_in_ci_and_every_file_is_checked() {
    let script = read(PACKAGE_SCRIPT);
    let release = read(RELEASE_WORKFLOW);
    for text in [&script, &release] {
        for gone in [".pfx", "signtool", "KILN_SIGN_CERT", "KILN_SIGN_PASSWORD"] {
            assert!(!text.contains(gone), "{gone} is still used");
        }
    }
    // A tag is built only from code CI passes, only when it is the version
    // Cargo.toml says, and only signed.
    assert!(job(&release, "ci").contains("uses: ./.github/workflows/ci.yml"));
    assert!(read(CI_WORKFLOW).contains("\n  workflow_call:\n"));
    let version = job(&release, "version");
    for needle in [
        r#"if [ "$GITHUB_REF_NAME" != "v$version" ]; then"#,
        r#"if [ "$SIGNING" != signpath ] && [ "$SIGNING" != azure ]; then"#,
        "exit 1",
    ] {
        assert!(version.contains(needle), "the version job lacks {needle}");
    }
    // The build job, which runs every build script, can write nothing and
    // holds no signing credentials; the package job runs no build script.
    let build = job(&release, "build");
    assert!(
        build.contains(
            "permissions:
      contents: read
    steps:"
        ),
        "{build}"
    );
    assert!(!build.contains("secrets."), "the build job reads a secret");
    assert!(build.contains("./packaging/package.ps1 -Stage"));
    for name in ["build", "package", "version"] {
        assert!(
            job(&release, name).contains("persist-credentials: false"),
            "job {name} keeps the token in .git"
        );
    }
    // Two rounds each way: the staged files, then the MSI that holds them.
    let package = job(&release, "package");
    assert!(package.contains("needs: [version, build]"));
    assert!(!package.contains("cargo build") && !package.contains("-Stage"));
    assert!(package.contains("environment: release"));
    let at = |needle: &str| {
        package
            .find(needle)
            .unwrap_or_else(|| panic!("the package job lacks {needle}"))
    };
    let stage = at("name: staged");
    let files = at("artifact-configuration-slug: kiln-files");
    let azure_files = at("files-folder-filter: exe,dll");
    let check_files = at("./packaging/check-signatures.ps1 -Path dist/stage @o");
    let pack = at("./packaging/package.ps1 -Pack");
    let msi = at("artifact-configuration-slug: kiln-msi");
    let azure_msi = at("files-folder-filter: msi");
    let check_all = at("./packaging/check-signatures.ps1 -Path dist -Packages @o");
    let sums = at("./packaging/package.ps1 -Sums");
    let draft = at("gh release create");
    assert!(stage < files && files < azure_files && azure_files < check_files);
    assert!(check_files < pack && pack < msi && msi < azure_msi && azure_msi < check_all);
    assert!(check_all < sums && sums < draft);
    // The checks are skipped only when nothing is signed, which a tag
    // refuses; a test certificate is allowed only off a tag.
    assert_eq!(package.matches("if: env.SIGNING != ''").count(), 2);
    assert!(package.contains("TEST_CERT: ${{ !startsWith(github.ref, 'refs/tags/') }}"));
    // A tag is signed with the release policy, anything else with the test
    // one, and both rounds wait long enough for a person to approve.
    assert!(package.contains(
        "POLICY: ${{ startsWith(github.ref, 'refs/tags/') && 'release-signing' || 'test-signing' }}"
    ));
    assert_eq!(
        package
            .matches("signing-policy-slug: ${{ env.POLICY }}")
            .count(),
        2
    );
    assert_eq!(
        package
            .matches("wait-for-completion-timeout-in-seconds: 7200")
            .count(),
        2
    );
    let step = at("- name: Draft the release");
    assert!(
        package[step..draft]
            .contains("if: github.event_name == 'push' && startsWith(github.ref, 'refs/tags/')"),
        "a release is drafted off a tag"
    );
    assert!(package.contains("--draft"));
    // The SignPath configurations the slugs name are in the repository.
    // FFmpeg is an upstream project's, which SignPath Foundation's terms
    // don't let Kiln sign, under either service; every signed file
    // says it is Kiln at this build's version.
    let files = read(SIGNPATH_FILES);
    assert!(
        !files.contains(r#"path="ffmpeg"#) && !files.contains("<directory"),
        "kiln-files.xml signs FFmpeg"
    );
    assert_eq!(
        files
            .matches(r#"product-name="Kiln" product-version="${version}""#)
            .count(),
        files.matches("<pe-file ").count()
    );
    // SignPath checks the numbers the version resources carry, which have no
    // pre-release part.
    assert!(package.contains(r#"version: "${{ needs.version.outputs.numeric }}""#));
    assert!(version.contains(r#"echo "numeric=${version%%-*}" >> "$GITHUB_OUTPUT""#));
    assert!(package.contains("files-folder-recurse: false"));
    assert!(read(SIGNPATH_MSI).contains("<msi-file"));
    // One tag may be released unsigned, by name, before SignPath takes Kiln.
    assert!(version.contains(r#"[ "$UNSIGNED_RELEASE" = "$GITHUB_REF_NAME" ]"#));
}

/// Runs `check-signatures.ps1` on `path`. Whether it passed, and what it
/// printed.
fn check_signatures(path: &std::path::Path, packages: bool) -> (bool, String) {
    let mut cmd = std::process::Command::new("pwsh");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-File",
        CHECK_SIGNATURES,
        "-Path",
    ])
    .arg(path);
    if packages {
        cmd.arg("-Packages");
    }
    let out = cmd.output().expect("pwsh runs");
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    (out.status.success(), text)
}

/// The check passes a folder of validly signed programs and stops at an
/// unsigned one, in the folder or inside a ZIP.
#[cfg(windows)]
#[test]
fn the_signature_check_passes_only_validly_signed_files() {
    if std::process::Command::new("pwsh")
        .arg("-Version")
        .output()
        .is_err()
    {
        eprintln!("skipped: no pwsh");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let signed = dir.path().join("signed");
    std::fs::create_dir_all(signed.join("bin")).unwrap();
    let windir = std::env::var("WINDIR").unwrap();
    std::fs::copy(
        format!(r"{windir}\System32\cmd.exe"),
        signed.join("bin/cmd.exe"),
    )
    .unwrap();
    let (ok, text) = check_signatures(&signed, false);
    assert!(ok, "{text}");
    assert!(text.contains(r"ok       bin\cmd.exe"), "{text}");

    // An unsigned DLL beside it fails, and is named.
    std::fs::write(signed.join("bin/unsigned.dll"), b"MZ\0\0").unwrap();
    let (ok, text) = check_signatures(&signed, false);
    assert!(!ok, "{text}");
    assert!(text.contains(r"FAIL     bin\unsigned.dll"), "{text}");

    // An unsigned FFmpeg is another project's, listed apart, never a failure.
    std::fs::remove_file(signed.join("bin/unsigned.dll")).unwrap();
    std::fs::create_dir_all(signed.join(r"ffmpeg\bin")).unwrap();
    std::fs::write(signed.join(r"ffmpeg\bin\avcodec-61.dll"), b"MZ\0\0").unwrap();
    let (ok, text) = check_signatures(&signed, false);
    assert!(ok, "{text}");
    assert!(
        text.contains(r"upstream ffmpeg\bin\avcodec-61.dll"),
        "{text}"
    );
    assert!(
        text.contains("1 upstream files were left as they are."),
        "{text}"
    );
    // Under ffmpeg\bin, only FFmpeg's own names count as upstream.
    std::fs::write(signed.join(r"ffmpeg\bin\helper.exe"), b"MZ\0\0").unwrap();
    let (ok, text) = check_signatures(&signed, false);
    assert!(!ok, "{text}");
    assert!(text.contains(r"FAIL     ffmpeg\bin\helper.exe"), "{text}");
    std::fs::remove_file(signed.join(r"ffmpeg\bin\helper.exe")).unwrap();
    std::fs::write(signed.join("bin/unsigned.dll"), b"MZ\0\0").unwrap();

    // Inside a portable ZIP, with -Packages.
    let packages = dir.path().join("dist");
    std::fs::create_dir_all(&packages).unwrap();
    std::fs::remove_file(signed.join("bin/unsigned.dll")).unwrap();
    std::fs::write(signed.join("kiln.exe"), b"MZ\0\0").unwrap();
    let zip = packages.join("Kiln-0.0.0-x64-portable.zip");
    let status = std::process::Command::new("pwsh")
        .args(["-NoProfile", "-NonInteractive", "-Command"])
        .arg(format!(
            r"Compress-Archive -Path '{}\*' -DestinationPath '{}'",
            signed.display(),
            zip.display()
        ))
        .status()
        .unwrap();
    assert!(status.success());
    let (ok, text) = check_signatures(&packages, true);
    assert!(!ok, "{text}");
    assert!(
        text.contains(r"FAIL     Kiln-0.0.0-x64-portable.zip!kiln.exe"),
        "{text}"
    );

    // A folder with nothing to check is a mistake, not a pass.
    let empty = dir.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    assert!(!check_signatures(&empty, false).0);
}

/// THIRD_PARTY_LICENSES has the full text of every crate's licence, not
/// just its name: each crate in its table is named under a text.
#[test]
fn every_crate_listed_has_its_licence_text() {
    let text = read(LICENSES);
    let (table, texts) = text
        .split_once("\nLicense texts\n")
        .expect("THIRD_PARTY_LICENSES has no License texts");
    let used: BTreeSet<String> = texts
        .lines()
        .filter_map(|l| l.strip_prefix("Used by: "))
        .flat_map(|l| l.split(", ").map(str::to_owned))
        .collect();
    let table = table
        .split_once("Notices for crates under BSD licenses")
        .map_or(table, |(t, _)| t);
    let listed: Vec<String> = table
        .lines()
        .filter_map(|line| {
            let mut w = line.split_whitespace();
            let (name, version) = (w.next()?, w.next()?);
            (version.starts_with(|c: char| c.is_ascii_digit()) && w.next().is_some())
                .then(|| format!("{name} {version}"))
        })
        .collect();
    assert!(listed.len() > 150, "{} crates listed", listed.len());
    let without: Vec<&String> = listed.iter().filter(|c| !used.contains(*c)).collect();
    assert!(without.is_empty(), "no licence text for {without:?}");
    assert!(texts.contains("Permission is hereby granted"));
    assert!(texts.contains("Mozilla Public License Version 2.0"));
    assert!(
        !text.contains("&quot;") && !text.contains("&#"),
        "HTML escapes in the text"
    );
    assert!(text.contains("Kiln is Copyright (C) 2026 Chandler Crisp"));
}

/// The privacy notice ships beside the programs, in the MSI and the ZIP,
/// with the licences (phase 4, legal kit).
#[test]
fn the_privacy_notice_ships_with_kiln() {
    let wxs = read(MSI);
    let files = install_dirs(&wxs);
    let dir_of = |name: &str| {
        files
            .iter()
            .find(|(file, _)| file == name)
            .map(|(_, dir)| dir.clone())
    };
    assert!(dir_of("PRIVACY.md").is_some(), "the MSI has no PRIVACY.md");
    assert_eq!(dir_of("PRIVACY.md"), dir_of("kiln-app.exe"));
    let script = read(PACKAGE_SCRIPT);
    assert!(
        script.contains(
            "Copy-Item LICENSE, THIRD_PARTY_LICENSES, THIRD_PARTY_NOTICES.md, PRIVACY.md,"
        ),
        "package.ps1 doesn't stage PRIVACY.md"
    );
    let notice = read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../PRIVACY.md"));
    for needle in [
        "Check for updates",
        "api.github.com",
        r"%LOCALAPPDATA%\Kiln\CrashDumps",
    ] {
        assert!(notice.contains(needle), "PRIVACY.md doesn't say {needle}");
    }
}

/// A release candidate and its release carry the same MSI version (0.1.0
/// for 0.1.0-rc.1), so a same-version MSI replaces the installed one rather
/// than adding a second entry; WiX gets the numbers only.
#[test]
fn a_same_version_msi_replaces_the_installed_one() {
    let wxs = read(MSI);
    assert!(wxs.contains(r#"<MajorUpgrade AllowSameVersionUpgrades="yes""#));
    let script = read(PACKAGE_SCRIPT);
    assert!(script.contains("$numeric = $version -replace '-.*$', ''"));
    assert!(script.contains(r#"-d "Version=$numeric""#));
    assert!(!script.contains(r#"-d "Version=$version""#));
}

/// The source ZIP's note on building offline names its crate folder from the
/// source's top folder, never by the build machine's path.
#[test]
fn the_source_zip_note_names_no_build_path() {
    let script = read(PACKAGE_SCRIPT);
    assert!(script.contains(
        r#"if ($_ -match '^directory = ') { 'directory = "third-party/crates"' } else { $_ }"#
    ));
    assert!(!script.contains("--quiet $crates"));
}

/// Every build uses one Rust, named in rust-toolchain.toml: CI and
/// the release install that one, never whatever "stable" is that day.
#[test]
fn the_rust_toolchain_is_pinned() {
    let file = read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../rust-toolchain.toml"
    ));
    let channel = file
        .lines()
        .find_map(|l| l.trim().strip_prefix("channel = "))
        .expect("rust-toolchain.toml names no channel")
        .trim_matches('"');
    let parts: Vec<&str> = channel.split('.').collect();
    assert!(
        parts.len() == 3 && parts.iter().all(|p| p.parse::<u32>().is_ok()),
        "the channel is {channel:?}, not an exact version"
    );
    for c in ["rustfmt", "clippy"] {
        assert!(file.contains(&format!("\"{c}\"")), "no {c} component");
    }
    for wf in [CI_WORKFLOW, RELEASE_WORKFLOW] {
        let text = read(wf);
        assert!(
            !text.contains("dtolnay/rust-toolchain"),
            "{wf} installs stable"
        );
        assert!(text.contains("run: rustup toolchain install"), "{wf}");
    }
}

/// The MSI has pages: WixUI_Minimal, whose licence page shows the
/// GPL made from LICENSE, a finish page that can open Kiln as the user, and
/// links in Apps > Installed apps to the public pages.
#[test]
fn the_msi_shows_the_licence_and_can_open_kiln() {
    let wxs = read(MSI);
    for needle in [
        r#"xmlns:ui="http://wixtoolset.org/schemas/v4/wxs/ui""#,
        r#"<ui:WixUI Id="WixUI_Minimal" />"#,
        r#"<WixVariable Id="WixUILicenseRtf" Value="$(StageDir)\License.rtf" />"#,
        r#"<Property Id="WIXUI_EXITDIALOGOPTIONALCHECKBOXTEXT" Value="Open Kiln" />"#,
        r#"<Property Id="WixShellExecTarget" Value="[#KilnAppExe]" />"#,
        r#"DllEntry="WixShellExec" Impersonate="yes""#,
        r#"Condition="WIXUI_EXITDIALOGOPTIONALCHECKBOX = 1 and NOT Installed""#,
        r#"<Property Id="ARPHELPLINK" Value="https://github.com/chancrisp/kiln/blob/main/SUPPORT.md" />"#,
        r#"<Property Id="ARPURLINFOABOUT" Value="https://github.com/chancrisp/kiln" />"#,
        r#"<Property Id="ARPURLUPDATEINFO" Value="https://github.com/chancrisp/kiln/releases" />"#,
    ] {
        assert!(wxs.contains(needle), "kiln.wxs lacks {needle}");
    }
    assert!(wxs.contains(r#"<File Id="KilnAppExe" Source="$(StageDir)\kiln-app.exe""#));
    let script = read(PACKAGE_SCRIPT);
    for needle in [
        "License.rtf",
        "-ext WixToolset.UI.wixext -ext WixToolset.Util.wixext",
        // The RTF is the MSI's; the portable ZIP has LICENSE itself.
        "Get-ChildItem $stageDir -Exclude kiln.ico, License.rtf | Compress-Archive",
    ] {
        assert!(script.contains(needle), "package.ps1 lacks {needle}");
    }
    let release = read(RELEASE_WORKFLOW);
    assert!(release
        .contains("wix extension add -g WixToolset.UI.wixext/5.0.2 WixToolset.Util.wixext/5.0.2"));
}

/// Both programs carry a manifest that says they run as the user who
/// starts them and take long paths, with the same Windows 10 and
/// 11 support and UTF-8 code page.
#[test]
fn both_programs_run_as_invoker_and_take_long_paths() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    for m in [
        "crates/kiln-app/assets/kiln.exe.manifest",
        "crates/kiln-cli/assets/kiln.exe.manifest",
    ] {
        let s =
            std::fs::read_to_string(format!("{root}/{m}")).unwrap_or_else(|e| panic!("{m}: {e}"));
        for needle in [
            r#"<requestedExecutionLevel level="asInvoker" uiAccess="false"/>"#,
            r#"<longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>"#,
            r#"<activeCodePage xmlns="http://schemas.microsoft.com/SMI/2019/WindowsSettings">UTF-8</activeCodePage>"#,
            r#"<supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>"#,
        ] {
            assert!(s.contains(needle), "{m} lacks {needle}");
        }
    }
    let rc = std::fs::read_to_string(format!("{root}/crates/kiln-cli/assets/kiln-cli.rc")).unwrap();
    assert!(rc.contains(r#"1 RT_MANIFEST "kiln.exe.manifest""#));
    let build = std::fs::read_to_string(format!("{root}/crates/kiln-cli/build.rs")).unwrap();
    assert!(build.contains(".manifest_required()"));
}

/// The winget manifests name the MSI the release publishes, read
/// its SHA-256 from the release's sums, take the publisher and licence the
/// MSI uses, and the script submits nothing.
#[test]
fn the_winget_manifests_match_the_msi() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let read = |p: &str| read(&format!("{root}/{p}"));
    let script = read("packaging/winget.ps1");
    for needle in [
        "PackageIdentifier: $id",
        "$id = 'ChandlerCrisp.Kiln'",
        "$msi = \"Kiln-$Version-x64.msi\"",
        "SHA256SUMS.txt",
        "InstallerType: wix",
        "Scope: machine",
        "Publisher: Chandler Crisp",
        "License: GPL-3.0-or-later",
        "/releases/download/v$Version/$msi",
        "UpgradeCode",
    ] {
        assert!(script.contains(needle), "winget.ps1 lacks {needle}");
    }
    assert!(
        !script
            .lines()
            .any(|l| l.trim_start().starts_with("wingetcreate") || l.contains("winget submit")),
        "winget.ps1 must only write files"
    );
    let wxs = read("packaging/kiln.wxs");
    assert!(wxs.contains(r#"Scope="perMachine""#));
    assert!(wxs.contains(r#"Manufacturer="Chandler Crisp""#));
    let package = read("packaging/package.ps1");
    assert!(package.contains("x64.msi"));
}

/// The packages carry a short plain-text `Read me.txt` in place of the
/// developer README, which says where Kiln keeps its data and
/// that uninstalling leaves it.
#[test]
fn the_packages_ship_read_me_txt_not_the_readme() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let read = |p: &str| read(&format!("{root}/{p}"));
    let wxs = read("packaging/kiln.wxs");
    assert!(wxs.contains(r#"Source="$(StageDir)\Read me.txt""#));
    assert!(!wxs.contains(r#"$(StageDir)\README.md"#));
    let package = read("packaging/package.ps1");
    assert!(package.contains(r"'packaging\Read me.txt' $stageDir"));
    assert!(!package.contains("PRIVACY.md, README.md"));
    let text = read("packaging/Read me.txt");
    for needle in [
        r"%APPDATA%\Kiln\settings.toml",
        r"%LOCALAPPDATA%\Kiln\logs",
        "Uninstalling Kiln leaves these",
        "GNU\nGeneral Public License, version 3 or later",
    ] {
        assert!(text.contains(needle), "Read me.txt lacks {needle}");
    }
    assert!(
        !text.contains("]("),
        "Read me.txt is plain text, not Markdown"
    );
}

/// The README's pictures are files in `docs/readme`, which the public
/// tree keeps, and every picture there is one the README shows.
#[test]
fn every_readme_picture_is_in_docs_readme() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let readme = read(&format!("{root}/README.md"));
    let mut shown = std::collections::BTreeSet::new();
    for part in readme.split(['"', ' ']) {
        let Some(path) = part.strip_prefix("docs/") else {
            continue;
        };
        if !path.ends_with(".png") {
            continue;
        }
        assert!(
            path.starts_with("readme/"),
            "docs/{path} is outside docs/readme"
        );
        assert!(
            std::path::Path::new(&format!("{root}/docs/{path}")).is_file(),
            "docs/{path} is missing"
        );
        shown.insert(path.trim_start_matches("readme/").to_owned());
    }
    assert!(shown.len() >= 7, "{} pictures", shown.len());
    assert!(!readme.contains("docs/screenshots/"));
    for entry in std::fs::read_dir(format!("{root}/docs/readme")).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        assert!(
            shown.contains(&name),
            "docs/readme/{name} isn't shown in the README"
        );
    }
}

/// The donation links are filled in (D22), and no placeholder for them
/// can ship again: DONATE.md and FUNDING.yml name the Ko-fi page, and no
/// text file in the repository holds a Ko-fi placeholder. The placeholders
/// are spelled in pieces here so this file isn't one of them.
#[test]
fn the_donation_links_are_filled_in_and_no_placeholder_ships() {
    let root = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
    let read =
        |p: &str| std::fs::read_to_string(root.join(p)).unwrap_or_else(|e| panic!("{p}: {e}"));
    let donate = read("DONATE.md");
    assert!(
        donate.contains("(https://ko-fi.com/chancrisp)"),
        "DONATE.md"
    );
    assert!(
        donate.contains("(https://github.com/sponsors/chancrisp)"),
        "DONATE.md"
    );
    let funding = read(".github/FUNDING.yml");
    let live: Vec<&str> = funding
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    assert_eq!(
        live,
        ["github: [chancrisp]", "ko_fi: chancrisp"],
        "FUNDING.yml"
    );

    let placeholders = [
        ["KO-FI", "-LINK"].concat(),
        ["KO-FI", "-PAGE-NAME"].concat(),
        ["KO-FI", "-NAME"].concat(),
    ];
    // Build output, vendored code, and tool folders; .github is read.
    let skip = ["target", "vendor", "node_modules"];
    let text = [
        "md", "txt", "yml", "yaml", "toml", "rs", "ps1", "wxs", "hbs", "json", "rc", "html",
    ];
    let mut found = Vec::new();
    let mut dirs = vec![root.clone()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("{dir:?}: {e}")) {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if path.is_dir() {
                let hidden = name.starts_with('.') && name != ".github";
                if !hidden && !skip.contains(&name.as_str()) {
                    dirs.push(path);
                }
                continue;
            }
            let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase());
            if !ext.is_some_and(|e| text.contains(&e.as_str())) {
                continue;
            }
            let Ok(s) = std::fs::read_to_string(&path) else {
                continue;
            };
            let upper = s.to_uppercase();
            for p in &placeholders {
                if upper.contains(p.as_str()) {
                    found.push(format!("{}: {p}", path.display()));
                }
            }
        }
    }
    assert!(found.is_empty(), "a Ko-fi placeholder is left: {found:#?}");
}
