//! Office documents, spreadsheets and slides through LibreOffice, run
//! headless as an external process. LibreOffice is optional: without it,
//! these pairs say it isn't installed.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::converter::{Context, Converter, Progress, Request};
use crate::error::{Error, Result};
use crate::job::stem_of;
use crate::process;

pub struct LibreOfficeConverter;

const ENGINE: &str = "LibreOffice";

/// Written into each job's fresh LibreOffice profile before it starts: no
/// macros, no active content (OLE objects, DDE and other self-updating
/// links), no update check, and a proxy that goes nowhere, so a document
/// that points at a web address gets no answer.
const PROFILE_SETTINGS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<oor:items xmlns:oor="http://openoffice.org/2001/registry" xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
<item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="DisableMacrosExecution" oor:op="fuse"><value>true</value></prop></item>
<item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="MacroSecurityLevel" oor:op="fuse"><value>3</value></prop></item>
<item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="DisableActiveContent" oor:op="fuse"><value>true</value></prop></item>
<item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="BlockUntrustedRefererLinks" oor:op="fuse"><value>true</value></prop></item>
<item oor:path="/org.openoffice.Office.Jobs/Jobs/org.openoffice.Office.Jobs:Job['UpdateCheck']/Arguments"><prop oor:name="AutoCheckEnabled" oor:op="fuse"><value>false</value></prop></item>
<item oor:path="/org.openoffice.Inet/Settings"><prop oor:name="ooInetProxyType" oor:op="fuse"><value>1</value></prop></item>
<item oor:path="/org.openoffice.Inet/Settings"><prop oor:name="ooInetHTTPProxyName" oor:op="fuse"><value>127.0.0.1</value></prop></item>
<item oor:path="/org.openoffice.Inet/Settings"><prop oor:name="ooInetHTTPProxyPort" oor:op="fuse"><value>9</value></prop></item>
<item oor:path="/org.openoffice.Inet/Settings"><prop oor:name="ooInetHTTPSProxyName" oor:op="fuse"><value>127.0.0.1</value></prop></item>
<item oor:path="/org.openoffice.Inet/Settings"><prop oor:name="ooInetHTTPSProxyPort" oor:op="fuse"><value>9</value></prop></item>
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
        // Every path LibreOffice is given goes by the folder's short name
        // (D39).
        let work = &short_path(work);
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        let profile = Profile::make(work, local.as_deref())?;
        // LibreOffice names the result after the input, so it gets a
        // folder of its own and is renamed afterwards.
        let out_dir = work.join("libreoffice-out");
        fs::create_dir_all(&out_dir).map_err(|e| Error::io(&out_dir, e))?;
        let input = input_for(req, work)?;

        let mut cmd = Command::new(program(req.options.engines.libreoffice.as_deref()));
        cmd.arg(format!("-env:UserInstallation={}", file_url(profile.dir())))
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
            .arg(&input);

        let mut said = Vec::new();
        let stderr = match process::run(ENGINE, cmd, ctx.cancel, |line| said.push(line.to_owned()))
        {
            Ok(stderr) => stderr,
            Err(Error::EngineFailed { details, .. }) => {
                let summary = complaint(said.iter().map(String::as_str).chain(details.lines()))
                    .unwrap_or_else(silent);
                return Err(Error::EngineFailed {
                    engine: ENGINE,
                    summary,
                    details: [said.join("\n"), details].join("\n"),
                });
            }
            Err(e) => return Err(e),
        };

        let written = results_in(&out_dir)?;
        // The CSV filter writes a file for each sheet of a workbook. No other
        // target's files are results of their own (an HTML page's pictures
        // are not), so they keep going by the first file.
        if req.to.ext == "csv" && written.len() > 1 {
            return keep_sheets(written, &stem_of(&input), req.output);
        }
        match written.into_iter().next() {
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

/// Hands over the CSV files LibreOffice wrote for a workbook's sheets, the
/// way a backend with several results does: as `output-1.csv`,
/// `output-2.csv` and so on, in the order of the sheets' names, with the
/// names in the sidecar [`Request::names`] says. `stem` is the stem of the
/// file LibreOffice was given, which it puts in front of each sheet's name.
fn keep_sheets(files: Vec<PathBuf>, stem: &str, output: &Path) -> Result<()> {
    let sheets = sheet_names(files, stem);
    for (n, (path, _)) in sheets.iter().enumerate() {
        let to = Request::numbered(output, n + 1);
        fs::rename(path, &to).map_err(|e| Error::io(path, e))?;
    }
    // Without every name, the results are told apart by their numbers.
    if let Some(names) = sheets
        .iter()
        .map(|(_, name)| name.as_deref())
        .collect::<Option<Vec<_>>>()
    {
        let sidecar = Request::names(output);
        fs::write(&sidecar, names.join("\n")).map_err(|e| Error::io(sidecar, e))?;
    }
    Ok(())
}

/// Each of `files`, which LibreOffice named `{stem}-{sheet}.csv`, with its
/// sheet's name, in the order of those names (capitals and small letters
/// alike). A file not named that way has no name. A name can't hold a line
/// break, as the sidecar has a name a line.
fn sheet_names(files: Vec<PathBuf>, stem: &str) -> Vec<(PathBuf, Option<String>)> {
    let prefix = format!("{stem}-");
    let mut sheets: Vec<(PathBuf, Option<String>)> = files
        .into_iter()
        .map(|path| {
            let name = path
                .file_stem()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_prefix(&prefix))
                .map(|name| name.replace(['\r', '\n'], "_"));
            (path, name)
        })
        .collect();
    sheets.sort_by_cached_key(|(path, _)| {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        (name.to_lowercase(), name.into_owned())
    });
    sheets
}

/// The first thing LibreOffice said that is an error, without its
/// "Error: " prefix.
fn complaint<'a>(lines: impl Iterator<Item = &'a str>) -> Option<String> {
    lines
        .map(str::trim)
        .filter(|l| !l.is_empty() && !NOISE.iter().any(|n| l.starts_with(n)))
        .find_map(|l| l.strip_prefix("Error:").map(|rest| rest.trim().to_owned()))
}

/// What a LibreOffice that stopped without a word is said to have done,
/// with a next step.
fn silent() -> String {
    "it stopped without saying why. Check that LibreOffice opens on its own, then try again."
        .to_owned()
}

/// Makes the job's LibreOffice profile in `work`, its locked-down settings
/// written, and gives its folder by the shortest name Windows has for it:
/// LibreOffice's own paths run deep under it and stop it, without a word,
/// once they pass MAX_PATH, as they did under a TEMP over about 100
/// characters (D39). It stays in the job's folder, so it goes with it.
fn make_profile(work: &Path) -> Result<PathBuf> {
    let profile = work.join("lo");
    write_profile(&profile)?;
    Ok(short_path(&profile))
}

/// A LibreOffice profile at `profile`: its folder and locked-down settings.
fn write_profile(profile: &Path) -> Result<()> {
    let user = profile.join("user");
    fs::create_dir_all(&user).map_err(|e| Error::io(&user, e))?;
    let settings = user.join("registrymodifications.xcu");
    fs::write(&settings, PROFILE_SETTINGS).map_err(|e| Error::io(&settings, e))
}

/// The longest a profile folder's path may be. LibreOffice's own files run
/// about 135 characters deeper, and it crashed under a TEMP of about 205
/// characters, whose job folder was 117 even by its short name (D48).
const PROFILE_ROOT_MAX: usize = 80;

/// The job's LibreOffice profile. It goes in the job's folder, and with it,
/// unless that folder's path is too long even by its short name; then in
/// `%LOCALAPPDATA%\Kiln\lo`, in a folder named as the job's, removed when
/// this is dropped. Any left there by a Kiln that has since closed are
/// cleared first, as old job folders are.
struct Profile {
    /// The folder, by its short name, given to LibreOffice.
    dir: PathBuf,
    /// The folder made outside the job's, to remove when done.
    own: Option<PathBuf>,
}

impl Profile {
    fn make(work: &Path, local: Option<&Path>) -> Result<Self> {
        let in_job = short_path(work).join("lo");
        let name = work
            .file_name()
            .filter(|_| in_job.as_os_str().len() > PROFILE_ROOT_MAX);
        let (Some(local), Some(name)) = (local, name) else {
            return Ok(Self {
                dir: make_profile(work)?,
                own: None,
            });
        };
        let root = local.join("Kiln").join("lo");
        fs::create_dir_all(&root).map_err(|e| Error::io(&root, e))?;
        crate::job::sweep_stale_jobs(&root);
        let dir = root.join(name);
        write_profile(&dir)?;
        Ok(Self {
            dir: short_path(&dir),
            own: Some(dir),
        })
    }

    fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for Profile {
    fn drop(&mut self) {
        if let Some(dir) = &self.own {
            // Only the folder made for this job, which LibreOffice has
            // closed by now.
            let _ = fs::remove_dir_all(dir);
        }
    }
}

/// `path` by its 8.3 short name where its drive has them; as it is where
/// it doesn't, or the name can't be had.
#[cfg(windows)]
fn short_path(path: &Path) -> PathBuf {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: `wide` is NUL-terminated; a null buffer of 0 asks the size.
    let need = unsafe { GetShortPathNameW(wide.as_ptr(), std::ptr::null_mut(), 0) };
    if need == 0 {
        return path.to_path_buf();
    }
    let mut buf = vec![0u16; need as usize];
    // SAFETY: `buf` holds `need` characters, as the call asked for.
    let got = unsafe { GetShortPathNameW(wide.as_ptr(), buf.as_mut_ptr(), need) };
    if got == 0 || got >= need {
        return path.to_path_buf();
    }
    buf.truncate(got as usize);
    PathBuf::from(std::ffi::OsString::from_wide(&buf))
}

#[cfg(not(windows))]
fn short_path(path: &Path) -> PathBuf {
    path.to_path_buf()
}

/// The files LibreOffice wrote in `out_dir`, in the order the folder
/// lists them. A lock file it left there is not one: under a very long
/// TEMP it can fail to remove it (D48).
fn results_in(out_dir: &Path) -> Result<Vec<PathBuf>> {
    Ok(fs::read_dir(out_dir)
        .map_err(|e| Error::io(out_dir, e))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .filter(|p| {
            !p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(".~lock."))
        })
        .collect())
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
    let on_path =
        std::env::split_paths(&crate::process::current_path()).find_map(|dir| launcher_in(&dir));
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

/// How an RTF file starts.
const RTF_START: &[u8] = br"{\rtf";

/// The file LibreOffice opens for `req`: the input itself, or, for an RTF
/// whose fields name picture files, a copy with those fields defused.
/// LibreOffice would put each file they name, a local one or from an
/// address, into the result, and no setting stops it. LibreOffice goes by
/// what a file holds, not by its extension (an RTF named `.doc` is read as
/// RTF), so the file's start decides, whatever it is called. The copy has
/// the input's name (LibreOffice names its result after it, and may look at
/// its extension) and a folder of its own in the job folder, away from the
/// files there.
fn input_for(req: &Request<'_>, work: &Path) -> Result<PathBuf> {
    let mut file = fs::File::open(req.input).map_err(|e| Error::io(req.input, e))?;
    let mut rtf = Vec::new();
    (&mut file)
        .take(RTF_START.len() as u64)
        .read_to_end(&mut rtf)
        .map_err(|e| Error::io(req.input, e))?;
    if rtf != RTF_START {
        return Ok(req.input.to_path_buf());
    }
    file.read_to_end(&mut rtf)
        .map_err(|e| Error::io(req.input, e))?;
    let Some(defused) = neutralise_rtf_links(&rtf) else {
        return Ok(req.input.to_path_buf());
    };
    let dir = work.join("libreoffice-in");
    fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
    let copy = dir.join(
        req.input
            .file_name()
            .expect("the input is a file, so it has a name"),
    );
    fs::write(&copy, defused).map_err(|e| Error::io(&copy, e))?;
    Ok(copy)
}

/// The Word field that puts a picture file into a document.
const INCLUDE_PICTURE: &[u8] = b"INCLUDEPICTURE";

/// What it becomes: the same length, and a word nothing knows.
const DEFUSED: &[u8] = b"INCLUDEPICTURX";

const _: () = assert!(INCLUDE_PICTURE.len() == DEFUSED.len());

/// A copy of `rtf` in which every `INCLUDEPICTURE` in a field's
/// instruction (a `\fldinst` group and the groups inside it) is changed to
/// `INCLUDEPICTURX`, whatever its case. `None` if the file has no such word
/// anywhere. The word elsewhere is the document's own text and is kept.
///
/// LibreOffice itself reads the word in capitals only; the other cases are
/// changed too, so that no reading of the file finds a picture. What ends a
/// group is read as LibreOffice reads it: a brace does, unless a backslash
/// or the data after `\binN` hides it.
pub(crate) fn neutralise_rtf_links(rtf: &[u8]) -> Option<Vec<u8>> {
    let n = INCLUDE_PICTURE.len();
    let is_word = |at: usize| {
        rtf.get(at..at + n)
            .is_some_and(|w| w.eq_ignore_ascii_case(INCLUDE_PICTURE))
    };
    if !(0..rtf.len()).any(is_word) {
        return None;
    }
    let mut out = rtf.to_vec();
    let mut depth = 0usize;
    // How deep the group is that holds the `\fldinst` the scan is inside.
    let mut field: Option<usize> = None;
    let mut at = 0;
    while at < rtf.len() {
        match rtf[at] {
            b'{' => {
                depth += 1;
                at += 1;
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                if field.is_some_and(|d| depth < d) {
                    field = None;
                }
                at += 1;
            }
            b'\\' => at = skip_control(rtf, at, depth, &mut field),
            _ if field.is_some() && is_word(at) => {
                out[at..at + n].copy_from_slice(DEFUSED);
                at += n;
            }
            _ => at += 1,
        }
    }
    Some(out)
}

/// The index after the control word or symbol that starts at `rtf[at]`, a
/// backslash. A `\fldinst` opens a field at `depth`, unless one is open.
fn skip_control(rtf: &[u8], at: usize, depth: usize, field: &mut Option<usize>) -> usize {
    let mut i = at + 1;
    let Some(&first) = rtf.get(i) else {
        return i;
    };
    if !first.is_ascii_alphabetic() {
        // A symbol: `\{`, `\}`, `\\`, `\*` and the like. `\'` takes two hex
        // digits, which are not text.
        i += 1;
        if first == b'\'' {
            for _ in 0..2 {
                if rtf.get(i).is_some_and(u8::is_ascii_hexdigit) {
                    i += 1;
                }
            }
        }
        return i;
    }
    let word_start = i;
    while rtf.get(i).is_some_and(u8::is_ascii_alphabetic) {
        i += 1;
    }
    let word = &rtf[word_start..i];
    // An optional number, then one space that ends the word and is not text.
    let negative = rtf.get(i) == Some(&b'-');
    let digits_start = i + usize::from(negative);
    let mut digits_end = digits_start;
    while rtf.get(digits_end).is_some_and(u8::is_ascii_digit) {
        digits_end += 1;
    }
    let number = (digits_end > digits_start).then(|| {
        rtf[digits_start..digits_end].iter().fold(0usize, |n, d| {
            n.saturating_mul(10).saturating_add(usize::from(d - b'0'))
        })
    });
    if number.is_some() {
        i = digits_end;
    }
    if rtf.get(i) == Some(&b' ') {
        i += 1;
    }
    if word.eq_ignore_ascii_case(b"fldinst") {
        field.get_or_insert(depth);
    } else if let Some(len) = number.filter(|_| word == b"bin" && !negative) {
        // `\binN` is followed by N bytes of data, which can be anything,
        // braces included.
        i = i.saturating_add(len).min(rtf.len());
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_profile_folder_is_kept_short_under_a_long_temp() {
        // D39: under a TEMP past about 100 characters LibreOffice's deep
        // profile paths passed MAX_PATH and it stopped without a word.
        let dir = tempfile::tempdir().unwrap();
        let long = dir
            .path()
            .join("a folder with a long name to make the path long")
            .join("another folder with a long name for the same reason")
            .join("job");
        fs::create_dir_all(&long).unwrap();
        assert!(long.as_os_str().len() > 110);
        let profile = make_profile(&long).unwrap();
        assert!(profile
            .join("user")
            .join("registrymodifications.xcu")
            .is_file());
        // The same folder, by its short name where the drive has them.
        assert!(profile.as_os_str().len() < long.as_os_str().len());
        let short = short_path(&long);
        if short != long {
            assert!(profile.as_os_str().len() <= short.as_os_str().len() + 3);
        }
    }

    #[test]
    fn under_a_very_long_temp_the_profile_goes_to_a_short_folder_and_is_removed() {
        // D48.3: under a TEMP of about 205 characters even the job folder's
        // short name is too long for LibreOffice's profile, and it crashed.
        // The profile then goes in Kiln's own short folder, named as the
        // job's, and is gone when the job is done with it.
        let temp = tempfile::tempdir().unwrap();
        let mut long = temp.path().to_path_buf();
        while long.as_os_str().len() < 205 {
            long.push("a-folder-with-a-long-name-for-the-check");
        }
        let work = long.join("Kiln").join("job-1-0-ab");
        fs::create_dir_all(&work).unwrap();
        let local = tempfile::tempdir().unwrap();
        let home = {
            let profile = Profile::make(&work, Some(local.path())).unwrap();
            assert!(
                profile.dir().as_os_str().len() <= PROFILE_ROOT_MAX,
                "{}",
                profile.dir().display()
            );
            assert!(profile
                .dir()
                .join("user")
                .join("registrymodifications.xcu")
                .is_file());
            assert!(fs::canonicalize(profile.dir())
                .unwrap()
                .starts_with(fs::canonicalize(local.path()).unwrap()));
            profile.dir().to_path_buf()
        };
        assert!(!home.exists(), "{}", home.display());
        // Under a usual TEMP it stays in the job folder, which goes with
        // the job.
        let short = temp.path().join("job-1-1-ab");
        fs::create_dir_all(&short).unwrap();
        let profile = Profile::make(&short, Some(local.path())).unwrap();
        assert!(fs::canonicalize(profile.dir())
            .unwrap()
            .starts_with(fs::canonicalize(&short).unwrap()));
    }

    #[test]
    fn a_lock_file_left_beside_the_result_is_not_taken_for_it() {
        // D48.3: under a very long TEMP LibreOffice can leave its lock file
        // in the output folder; the result is the file it wrote.
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".~lock.in.pdf#"), "lock").unwrap();
        fs::write(dir.path().join("in.pdf"), "%PDF").unwrap();
        assert_eq!(
            results_in(dir.path()).unwrap(),
            vec![dir.path().join("in.pdf")]
        );
    }

    #[test]
    fn a_silent_failure_says_what_to_check() {
        assert_eq!(
            complaint(["", "Could not find platform independent libraries"].into_iter())
                .unwrap_or_else(silent),
            "it stopped without saying why. Check that LibreOffice opens on its own, then try again."
        );
    }

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

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(|n| Path::new("out").join(n)).collect()
    }

    #[test]
    fn sheets_are_named_by_what_follows_the_input_stem() {
        // A sheet's name can hold a dash or a dot, and the sheets go in the
        // order of their names, capitals or not.
        let files = paths(&[
            "book-Sales.csv",
            "book-notes.csv",
            "book-v1.2.csv",
            "book-Archive.csv",
            "book-Q1-Q2.csv",
        ]);
        let sheets = sheet_names(files, "book");
        let names: Vec<Option<&str>> = sheets.iter().map(|(_, n)| n.as_deref()).collect();
        assert_eq!(
            names,
            [
                Some("Archive"),
                Some("notes"),
                Some("Q1-Q2"),
                Some("Sales"),
                Some("v1.2")
            ]
        );
        assert_eq!(sheets[2].0, Path::new("out").join("book-Q1-Q2.csv"));
    }

    #[test]
    fn a_file_not_named_after_the_input_has_no_sheet_name() {
        // The stem of a book with dots in its name is kept whole.
        let sheets = sheet_names(paths(&["my.book-Sales.csv", "other-Notes.csv"]), "my.book");
        assert_eq!(sheets[0].1.as_deref(), Some("Sales"));
        assert_eq!(sheets[1].1, None);
    }

    #[test]
    fn a_sheet_name_has_no_line_break() {
        let sheets = sheet_names(paths(&["book-a\nb.csv", "book-c\r\nd.csv"]), "book");
        assert_eq!(sheets[0].1.as_deref(), Some("a_b"));
        assert_eq!(sheets[1].1.as_deref(), Some("c__d"));
    }

    /// A folder holding a CSV file with each of `names`.
    fn csv_folder(names: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for name in names {
            fs::write(dir.path().join(name), name).unwrap();
        }
        dir
    }

    #[test]
    fn the_sheets_are_handed_over_numbered_with_their_names() {
        let dir = csv_folder(&["book-Sales.csv", "book-Notes.csv", "book-Archive.csv"]);
        let files: Vec<PathBuf> = ["book-Sales.csv", "book-Notes.csv", "book-Archive.csv"]
            .iter()
            .map(|n| dir.path().join(n))
            .collect();
        let output = dir.path().join("output.csv");
        keep_sheets(files, "book", &output).unwrap();
        // In the order of the names, which the names file follows.
        for (n, sheet) in ["Archive", "Notes", "Sales"].iter().enumerate() {
            let text = fs::read_to_string(Request::numbered(&output, n + 1)).unwrap();
            assert_eq!(text, format!("book-{sheet}.csv"));
        }
        assert_eq!(
            fs::read_to_string(Request::names(&output)).unwrap(),
            "Archive\nNotes\nSales"
        );
        assert!(!dir.path().join("book-Sales.csv").exists());
        assert!(!output.exists());
    }

    #[test]
    fn sheets_whose_names_are_not_all_known_come_without_a_names_file() {
        let dir = csv_folder(&["book-Sales.csv", "strange.csv"]);
        let files = vec![
            dir.path().join("book-Sales.csv"),
            dir.path().join("strange.csv"),
        ];
        let output = dir.path().join("output.csv");
        keep_sheets(files, "book", &output).unwrap();
        assert!(Request::numbered(&output, 1).is_file());
        assert!(Request::numbered(&output, 2).is_file());
        assert!(!Request::names(&output).exists());
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

    /// The text of `bytes`, for assertions that print.
    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    /// `neutralise_rtf_links`, with the original handed back when there was
    /// nothing to change.
    fn neutralised(rtf: &[u8]) -> Vec<u8> {
        neutralise_rtf_links(rtf).unwrap_or_else(|| rtf.to_vec())
    }

    #[test]
    fn rtf_include_picture_is_neutralised() {
        let rtf = br#"{\field{\*\fldinst INCLUDEPICTURE "C:\\a.jpg"}{\fldrslt x}}"#;
        let out = neutralise_rtf_links(rtf).expect("the file names a picture");
        assert_eq!(
            text(&out),
            r#"{\field{\*\fldinst INCLUDEPICTURX "C:\\a.jpg"}{\fldrslt x}}"#
        );
        assert_eq!(out.len(), rtf.len());
    }

    #[test]
    fn the_word_in_a_group_inside_the_field_is_neutralised() {
        // The form Word writes: the instruction is in a group of its own.
        let rtf = br#"{\field{\*\fldinst { INCLUDEPICTURE "C:\\a.jpg" \d }}{\fldrslt {}}}"#;
        assert_eq!(
            text(&neutralised(rtf)),
            r#"{\field{\*\fldinst { INCLUDEPICTURX "C:\\a.jpg" \d }}{\fldrslt {}}}"#
        );
        let tight = br#"{\field{\*\fldinst{INCLUDEPICTURE "a"}}}"#;
        assert_eq!(
            text(&neutralised(tight)),
            r#"{\field{\*\fldinst{INCLUDEPICTURX "a"}}}"#
        );
    }

    #[test]
    fn lowercase_and_mixed_case_are_neutralised_too() {
        for word in ["includepicture", "IncludePicture", "iNcLuDePiCtUrE"] {
            let rtf = format!(r#"{{\field{{\*\fldinst {word} "C:\\a.jpg"}}}}"#);
            let out = neutralise_rtf_links(rtf.as_bytes()).expect(word);
            assert_eq!(
                text(&out),
                r#"{\field{\*\fldinst INCLUDEPICTURX "C:\\a.jpg"}}"#,
                "{word}"
            );
            assert_eq!(out.len(), rtf.len());
        }
    }

    #[test]
    fn every_picture_field_is_neutralised() {
        let rtf =
            br#"{\field{\*\fldinst INCLUDEPICTURE "a"}}{\field{\*\fldinst INCLUDEPICTURE "b"}}"#;
        assert_eq!(
            text(&neutralised(rtf)),
            r#"{\field{\*\fldinst INCLUDEPICTURX "a"}}{\field{\*\fldinst INCLUDEPICTURX "b"}}"#
        );
    }

    #[test]
    fn the_word_in_body_text_is_left_alone() {
        let rtf = br#"INCLUDEPICTURE in the text {\field{\*\fldinst INCLUDEPICTURE "a"}} and INCLUDEPICTURE after"#;
        assert_eq!(
            text(&neutralised(rtf)),
            r#"INCLUDEPICTURE in the text {\field{\*\fldinst INCLUDEPICTURX "a"}} and INCLUDEPICTURE after"#
        );
        let only_body = br"{\rtf1 Type INCLUDEPICTURE to add a picture.}";
        assert_eq!(neutralised(only_body), only_body);
        // Other fields keep their words too.
        let other = br#"{\field{\*\fldinst HYPERLINK "http://example.com"}} INCLUDEPICTURE"#;
        assert_eq!(neutralised(other), other);
    }

    #[test]
    fn a_file_without_the_word_gives_none() {
        assert_eq!(neutralise_rtf_links(br"{\rtf1\ansi Hello.\par}"), None);
        assert_eq!(neutralise_rtf_links(b""), None);
    }

    #[test]
    fn an_escaped_brace_does_not_end_the_field() {
        let rtf = br#"{\field{\*\fldinst \} INCLUDEPICTURE "a"}} INCLUDEPICTURE"#;
        assert_eq!(
            text(&neutralised(rtf)),
            r#"{\field{\*\fldinst \} INCLUDEPICTURX "a"}} INCLUDEPICTURE"#
        );
        // Nor does a hex escape, or an escaped backslash in front of one.
        let rtf = br#"{\fldinst \\\'7d\} INCLUDEPICTURE "a"}"#;
        assert_eq!(
            text(&neutralised(rtf)),
            r#"{\fldinst \\\'7d\} INCLUDEPICTURX "a"}"#
        );
    }

    #[test]
    fn binary_data_does_not_end_the_field() {
        // \bin3 is followed by three raw bytes, which here look like the
        // ends of groups.
        let rtf = b"{\\field{\\*\\fldinst \\bin3 }}} INCLUDEPICTURE \"a\"}} INCLUDEPICTURE";
        assert_eq!(
            text(&neutralised(rtf)),
            "{\\field{\\*\\fldinst \\bin3 }}} INCLUDEPICTURX \"a\"}} INCLUDEPICTURE"
        );
    }

    #[test]
    fn the_fixtures_that_link_a_picture_are_neutralised() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/hostile");
        for name in ["linked.rtf", "remote.rtf"] {
            let rtf = fs::read(dir.join(name)).unwrap();
            let out = neutralise_rtf_links(&rtf).expect(name);
            assert_eq!(out.len(), rtf.len(), "{name}");
            assert!(!text(&out).contains("INCLUDEPICTURE"), "{name}");
            assert!(text(&out).contains("INCLUDEPICTURX"), "{name}");
        }
    }

    #[test]
    fn profile_has_a_dead_proxy() {
        // Nothing listens on port 9 of this PC, so whatever LibreOffice
        // tries to fetch gets nowhere.
        for (name, value) in [
            ("ooInetProxyType", "1"),
            ("ooInetHTTPProxyName", "127.0.0.1"),
            ("ooInetHTTPProxyPort", "9"),
            ("ooInetHTTPSProxyName", "127.0.0.1"),
            ("ooInetHTTPSProxyPort", "9"),
        ] {
            let item = format!(
                "<item oor:path=\"/org.openoffice.Inet/Settings\"><prop oor:name=\"{name}\" oor:op=\"fuse\"><value>{value}</value></prop></item>"
            );
            assert!(PROFILE_SETTINGS.contains(&item), "{item}");
        }
    }
}
