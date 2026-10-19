//! Office documents, spreadsheets and slides through LibreOffice, run
//! headless as an external process. LibreOffice is optional: without it,
//! these pairs say it isn't installed.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::converter::{Context, Converter, Progress, Request};
use crate::error::{Error, Result};
use crate::process;

pub struct LibreOfficeConverter;

const ENGINE: &str = "LibreOffice";

/// Written into each job's fresh LibreOffice profile before it starts: no
/// macros, no active content (OLE objects, DDE and other self-updating
/// links), and no update check.
const PROFILE_SETTINGS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<oor:items xmlns:oor="http://openoffice.org/2001/registry" xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
<item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="DisableMacrosExecution" oor:op="fuse"><value>true</value></prop></item>
<item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="MacroSecurityLevel" oor:op="fuse"><value>3</value></prop></item>
<item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="DisableActiveContent" oor:op="fuse"><value>true</value></prop></item>
<item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="BlockUntrustedRefererLinks" oor:op="fuse"><value>true</value></prop></item>
<item oor:path="/org.openoffice.Office.Jobs/Jobs/org.openoffice.Office.Jobs:Job['UpdateCheck']/Arguments"><prop oor:name="AutoCheckEnabled" oor:op="fuse"><value>false</value></prop></item>
</oor:items>
"#;

/// stderr lines LibreOffice prints on every run, error or not.
const NOISE: &[&str] = &["Could not find platform independent libraries"];

impl Converter for LibreOfficeConverter {
    fn id(&self) -> &'static str {
        "libreoffice"
    }

    fn convert(&self, req: &Request<'_>, ctx: &mut Context<'_>) -> Result<()> {
        let Some(filter) = &req.to.libreoffice else {
            return Err(Error::Registry(format!(
                "{} to {} uses LibreOffice, but {} has no libreoffice filter.",
                req.from.code(),
                req.to.code(),
                req.to.code()
            )));
        };
        (ctx.progress)(Progress::Indeterminate);

        // A profile of its own keeps parallel jobs apart, starts from the
        // locked-down settings every time, and goes with the job folder.
        let work = req
            .output
            .parent()
            .expect("the output is inside the job's working folder");
        let profile = work.join("libreoffice-profile");
        let user = profile.join("user");
        fs::create_dir_all(&user).map_err(|e| Error::io(&user, e))?;
        let settings = user.join("registrymodifications.xcu");
        fs::write(&settings, PROFILE_SETTINGS).map_err(|e| Error::io(&settings, e))?;
        // LibreOffice names the result after the input, so it gets a
        // folder of its own and is renamed afterwards.
        let out_dir = work.join("libreoffice-out");
        fs::create_dir_all(&out_dir).map_err(|e| Error::io(&out_dir, e))?;

        let mut cmd = Command::new(program(req.options.engines.libreoffice.as_deref()));
        cmd.arg(format!("-env:UserInstallation={}", file_url(&profile)))
            .args([
                "--headless",
                "--nologo",
                "--nodefault",
                "--norestore",
                "--nolockcheck",
            ])
            // Without these, LibreOffice asks the default printer for page
            // metrics, which can stall for most of a minute on a network
            // printer that's asleep. Output no longer depends on the
            // printer either.
            .env("SAL_DISABLE_DEFAULTPRINTER", "1")
            .env("SAL_DISABLE_PRINTERLIST", "1");
        if let Some(infilter) = &req.from.libreoffice_infilter {
            cmd.arg(format!("--infilter={infilter}"));
        }
        cmd.arg("--convert-to")
            .arg(filter)
            .arg("--outdir")
            .arg(&out_dir)
            // Paths are absolute, so they can't be mistaken for options.
            .arg(req.input);

        let mut said = Vec::new();
        let stderr = match process::run(ENGINE, cmd, ctx.cancel, |line| said.push(line.to_owned()))
        {
            Ok(stderr) => stderr,
            Err(Error::EngineFailed { details, .. }) => {
                let summary = complaint(said.iter().map(String::as_str).chain(details.lines()))
                    .unwrap_or_else(|| "it stopped without saying why.".to_owned());
                return Err(Error::EngineFailed {
                    engine: ENGINE,
                    summary,
                    details: [said.join("\n"), details].join("\n"),
                });
            }
            Err(e) => return Err(e),
        };

        let written = fs::read_dir(&out_dir)
            .map_err(|e| Error::io(&out_dir, e))?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .find(|p| p.is_file());
        match written {
            Some(path) => fs::rename(&path, req.output).map_err(|e| Error::io(path, e)),
            // LibreOffice exits cleanly when it has no way to write the
            // type, and says so on stdout.
            None => Err(Error::EngineFailed {
                engine: ENGINE,
                summary: complaint(said.iter().map(String::as_str).chain(stderr.lines()))
                    .unwrap_or_else(|| "it finished without writing a file.".to_owned()),
                details: [said.join("\n"), stderr].join("\n"),
            }),
        }
    }
}

/// The first thing LibreOffice said that is an error, without its
/// "Error: " prefix.
fn complaint<'a>(lines: impl Iterator<Item = &'a str>) -> Option<String> {
    lines
        .map(str::trim)
        .filter(|l| !l.is_empty() && !NOISE.iter().any(|n| l.starts_with(n)))
        .find_map(|l| l.strip_prefix("Error:").map(|rest| rest.trim().to_owned()))
}

/// The LibreOffice launcher to run. soffice.com is preferred over
/// soffice.exe: it is the console build, so its messages reach Kiln.
fn program(configured: Option<&Path>) -> PathBuf {
    if let Some(path) = configured {
        let console = path.with_file_name("soffice.com");
        let is_exe = path
            .file_name()
            .is_some_and(|n| n.eq_ignore_ascii_case("soffice.exe"));
        return if is_exe && console.is_file() {
            console
        } else {
            path.to_path_buf()
        };
    }
    find_installed().unwrap_or_else(|| PathBuf::from("soffice"))
}

/// Where LibreOffice is when no path is set: on PATH, or in its default
/// install folder. `None` if it isn't in either.
pub fn find_installed() -> Option<PathBuf> {
    let on_path = std::env::var_os("PATH")
        .and_then(|path| std::env::split_paths(&path).find_map(|dir| launcher_in(&dir)));
    on_path.or_else(|| {
        ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"]
            .into_iter()
            .filter_map(std::env::var_os)
            .find_map(|dir| launcher_in(&Path::new(&dir).join("LibreOffice").join("program")))
    })
}

fn launcher_in(dir: &Path) -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) {
        &["soffice.com", "soffice.exe"]
    } else {
        &["soffice"]
    };
    names.iter().map(|n| dir.join(n)).find(|p| p.is_file())
}

/// A file: URL for `-env:UserInstallation`. Anything outside the
/// unreserved characters is percent-encoded, so spaces and non-ASCII
/// folder names survive.
fn file_url(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let text = match text.strip_prefix("//?/") {
        Some(rest) => match rest.strip_prefix("UNC/") {
            Some(share) => format!("//{share}"),
            None => rest.to_owned(),
        },
        None => text,
    };
    // C:/x becomes file:///C:/x; //server/share becomes file://server/share.
    let mut url = String::from(if text.starts_with("//") {
        "file:"
    } else {
        "file:///"
    });
    for b in text.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                url.push(b as char)
            }
            _ => url.push_str(&format!("%{b:02X}")),
        }
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_urls_are_encoded() {
        assert_eq!(
            file_url(Path::new(r"C:\Users\Ana María\Temp\job 1")),
            "file:///C:/Users/Ana%20Mar%C3%ADa/Temp/job%201"
        );
        assert_eq!(
            file_url(Path::new(r"\\?\C:\Temp\Kiln")),
            "file:///C:/Temp/Kiln"
        );
        assert_eq!(
            file_url(Path::new(r"\\server\share\Kiln")),
            "file://server/share/Kiln"
        );
        assert_eq!(
            file_url(Path::new(r"\\?\UNC\server\share\Kiln")),
            "file://server/share/Kiln"
        );
        assert_eq!(file_url(Path::new(r"C:\a#b%c")), "file:///C:/a%23b%25c");
    }

    #[test]
    fn the_complaint_skips_noise_and_the_prefix() {
        let lines = [
            "Could not find platform independent libraries <prefix>",
            "convert x.docx as a Writer document",
            "Error: source file could not be loaded",
        ];
        assert_eq!(
            complaint(lines.into_iter()).as_deref(),
            Some("source file could not be loaded")
        );
        assert_eq!(complaint(["all fine"].into_iter()), None);
    }

    #[test]
    fn profile_settings_turn_macros_and_active_content_off() {
        for key in [
            "\"DisableMacrosExecution\" oor:op=\"fuse\"><value>true",
            "\"MacroSecurityLevel\" oor:op=\"fuse\"><value>3",
            "\"DisableActiveContent\" oor:op=\"fuse\"><value>true",
            "\"AutoCheckEnabled\" oor:op=\"fuse\"><value>false",
        ] {
            assert!(PROFILE_SETTINGS.contains(key), "{key}");
        }
    }

    #[test]
    fn a_configured_soffice_exe_is_used_as_given_without_a_console_twin() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("soffice.exe");
        fs::write(&exe, b"").unwrap();
        assert_eq!(program(Some(&exe)), exe);
        let com = dir.path().join("soffice.com");
        fs::write(&com, b"").unwrap();
        assert_eq!(program(Some(&exe)), com);
        let other = dir.path().join("lo.exe");
        assert_eq!(program(Some(&other)), other);
    }
}
