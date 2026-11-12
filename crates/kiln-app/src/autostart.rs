//! Start with Windows while the drag-wheel is on: Kiln's value `Kiln`
//! under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, which reads
//! `"<kiln-app.exe>" --background`. Windows lists it in Task Manager's
//! Startup apps, where the person can turn it off.
//!
//! Kiln writes only that value, and removes it only when it names this
//! `kiln-app.exe` ([`is_ours`]): another copy of Kiln, or anything else
//! that took the name, is left alone. [`RunKey`] is the only code that
//! touches the registry for it, and [`Autostart`] does so off the window's
//! thread.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The command-line flag the Run value starts Kiln with: hidden, in the
/// notification area, with the drag-wheel watching.
pub const BACKGROUND: &str = "--background";

/// The command-line flag the uninstall runs Kiln with: Kiln removes its
/// Run value, if it is its own, and ends, with no window.
pub const FORGET_STARTUP: &str = "--forget-startup";

/// The value's name under the Run key.
pub const VALUE: &str = "Kiln";

/// The Run key, under HKEY_CURRENT_USER.
pub const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// The Run value's data for `exe`: its path in quotes, then
/// [`BACKGROUND`].
pub fn run_value(exe: &Path) -> String {
    format!("\"{}\" {BACKGROUND}", exe.display())
}

/// Whether the Run value's `data` starts `exe`, this Kiln: the program it
/// names is the same path, whatever its case, quoted or not. Arguments
/// after it don't matter. Another program, or a Kiln in another folder,
/// is not ours.
pub fn is_ours(data: &str, exe: &Path) -> bool {
    let program = program_in(data);
    !program.is_empty() && same_path(program, &exe.to_string_lossy())
}

/// The program a Run value's `data` starts: what is inside the first
/// quotes, or, unquoted, everything before [`BACKGROUND`] (a path with
/// spaces may be unquoted) or the whole line.
fn program_in(data: &str) -> &str {
    let data = data.trim();
    if let Some(rest) = data.strip_prefix('"') {
        return rest.split('"').next().unwrap_or_default().trim();
    }
    let flag = format!(" {BACKGROUND}");
    let at = data.char_indices().map(|(i, _)| i).find(|&i| {
        data.get(i..i + flag.len())
            .is_some_and(|s| s.eq_ignore_ascii_case(&flag))
    });
    match at {
        Some(i) => data[..i].trim(),
        None => data,
    }
}

/// Paths compared as Windows does: case doesn't matter, and `/` is `\`.
fn same_path(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.replace('/', "\\").to_lowercase();
    norm(a) == norm(b)
}

/// How Kiln was started, from its command line (without the program).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Launch {
    /// Files to open, in the order given.
    pub files: Vec<PathBuf>,
    /// [`BACKGROUND`] was given.
    pub background: bool,
    /// [`FORGET_STARTUP`] was given.
    pub forget_startup: bool,
    /// The first `kiln:` link given ([`is_link`]), as a notification's
    /// button starts Kiln with. A link is never a file, and a launch with
    /// one is a link's only: every other argument is ignored.
    pub link: Option<String>,
}

/// Whether a command-line argument is a `kiln:` link rather than a file:
/// it starts with the scheme, in any case, even when the rest isn't valid
/// Unicode.
pub fn is_link(arg: &std::ffi::OsStr) -> bool {
    const SCHEME: &str = "kiln:";
    // Lossy keeps a valid start as it is, whatever comes after it.
    arg.to_string_lossy()
        .get(..SCHEME.len())
        .is_some_and(|s| s.eq_ignore_ascii_case(SCHEME))
}

impl Launch {
    /// Read the arguments after the program's name. Anything starting
    /// `--` is a flag; ones Kiln doesn't know are ignored. Everything else
    /// is a file. With a `kiln:` link among them, only the link counts: a
    /// link's quotes broken by whatever started Kiln can't add files or
    /// flags to it.
    pub fn parse(args: impl IntoIterator<Item = OsString>) -> Launch {
        let mut launch = Launch::default();
        for a in args {
            match a.to_str() {
                Some(BACKGROUND) => launch.background = true,
                Some(FORGET_STARTUP) => launch.forget_startup = true,
                Some(s) if s.starts_with("--") => {}
                _ if is_link(&a) => {
                    if launch.link.is_none() {
                        launch.link = Some(a.to_string_lossy().into_owned());
                    }
                }
                _ => launch.files.push(PathBuf::from(a)),
            }
        }
        match launch.link {
            Some(link) => Launch {
                link: Some(link),
                ..Launch::default()
            },
            None => launch,
        }
    }

    /// Whether Kiln starts with no window shown: started in the background
    /// with no files. Files win: they open the window.
    pub fn hidden(&self) -> bool {
        self.background && self.files.is_empty()
    }
}

/// Where the Run value is kept: the registry in Kiln, memory in tests.
pub trait RunStore: Send + Sync {
    /// The value's data, or `None` when there is no value.
    fn read(&self) -> io::Result<Option<String>>;
    /// Write the value, making the key if it isn't there.
    fn write(&self, data: &str) -> io::Result<()>;
    /// Remove the value. No value there is not an error.
    fn delete(&self) -> io::Result<()>;
}

/// Point the Run value at `exe`, unless it already does just so. True
/// when it was written.
pub fn set(store: &dyn RunStore, exe: &Path) -> io::Result<bool> {
    let want = run_value(exe);
    if store.read()?.as_deref() == Some(want.as_str()) {
        return Ok(false);
    }
    store.write(&want)?;
    Ok(true)
}

/// Remove the Run value if it names `exe`. True when it was removed; a
/// value that names anything else is left as it is.
pub fn remove_if_ours(store: &dyn RunStore, exe: &Path) -> io::Result<bool> {
    match store.read()? {
        Some(data) if is_ours(&data, exe) => {
            store.delete()?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// The Run value in memory, for tests. Clones share the value.
#[derive(Debug, Clone, Default)]
pub struct MemoryRun(Arc<Mutex<Option<String>>>);

impl MemoryRun {
    pub fn holding(data: &str) -> MemoryRun {
        MemoryRun(Arc::new(Mutex::new(Some(data.to_owned()))))
    }

    pub fn value(&self) -> Option<String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl RunStore for MemoryRun {
    fn read(&self) -> io::Result<Option<String>> {
        Ok(self.value())
    }

    fn write(&self, data: &str) -> io::Result<()> {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(data.to_owned());
        Ok(())
    }

    fn delete(&self) -> io::Result<()> {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = None;
        Ok(())
    }
}

/// How long the end of the run waits for a change to the Run value still
/// being made.
pub const FINISH_WAIT: Duration = Duration::from_secs(2);

/// Keeps the Run value in step with the drag-wheel, off the calling
/// thread: each change runs on a short-lived thread of its own, one at a
/// time, and does what the latest change asked, so a quick on, off, on
/// ends on. Dropping it waits up to [`FINISH_WAIT`] for one under way.
pub struct Autostart {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

struct Shared {
    store: Box<dyn RunStore>,
    /// This `kiln-app.exe`. Without it nothing is written or removed.
    exe: Option<PathBuf>,
    /// The latest change asked for: on or off.
    on: AtomicBool,
    /// One change at a time.
    serial: Mutex<()>,
}

impl std::fmt::Debug for Autostart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Autostart")
            .field("exe", &self.shared.exe)
            .field("threads", &self.threads.len())
            .finish_non_exhaustive()
    }
}

impl Autostart {
    pub fn new(store: impl RunStore + 'static, exe: Option<PathBuf>) -> Autostart {
        Autostart {
            shared: Arc::new(Shared {
                store: Box::new(store),
                exe,
                on: AtomicBool::new(false),
                serial: Mutex::new(()),
            }),
            threads: Vec::new(),
        }
    }

    /// Make the Run value follow the drag-wheel: `on` writes it, or writes
    /// it again where it names another path (a portable folder that
    /// moved); off removes it if it is this Kiln's. Returns at once.
    pub fn follow(&mut self, on: bool) {
        self.shared.on.store(on, Ordering::SeqCst);
        self.threads.retain(|t| !t.is_finished());
        let shared = self.shared.clone();
        match std::thread::Builder::new()
            .name("kiln-autostart".into())
            .spawn(move || shared.apply())
        {
            Ok(t) => self.threads.push(t),
            Err(e) => crate::diag::log(&format!("the start-up entry: no thread: {e}")),
        }
    }

    /// Wait until every change asked for so far is made, or `limit` has
    /// passed. True when all are made.
    pub fn finish(&mut self, limit: Duration) -> bool {
        let until = Instant::now() + limit;
        while self.threads.iter().any(|t| !t.is_finished()) {
            if Instant::now() >= until {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
        true
    }
}

impl Drop for Autostart {
    fn drop(&mut self) {
        if !self.finish(FINISH_WAIT) {
            crate::diag::log("the start-up entry was still being changed as Kiln ended");
        }
    }
}

impl Shared {
    fn apply(&self) {
        let _one = self.serial.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(exe) = &self.exe else {
            return;
        };
        let on = self.on.load(Ordering::SeqCst);
        let done = if on {
            set(&*self.store, exe).map(|_| ())
        } else {
            remove_if_ours(&*self.store, exe).map(|_| ())
        };
        if let Err(e) = done {
            let what = if on { "write" } else { "remove" };
            crate::diag::log(&format!("couldn't {what} the start-up entry: {e}"));
        }
    }
}

#[cfg(windows)]
pub use registry::RunKey;

/// The key under HKEY_CURRENT_USER that Kiln keeps its Run value under,
/// given `KILN_TEST_RUN_KEY` (`test_key`), in a `debug` build or not;
/// `None` for no key at all. The variable can only narrow what Kiln
/// touches: set and empty, no key, in every build; set to a key, that
/// key in a debug build and none in a release one.
fn run_key_for(test_key: Option<&str>, debug: bool) -> Option<String> {
    match test_key {
        None => Some(RUN.to_owned()),
        Some(k) if debug && !k.is_empty() => Some(k.to_owned()),
        Some(_) => None,
    }
}

/// The Run value in the registry.
#[cfg(windows)]
mod registry {
    use std::io;

    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, WIN32_ERROR};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW,
        HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_NOEXPAND,
        RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ,
    };

    use super::{RunStore, VALUE};

    /// Kiln's value under a key in HKEY_CURRENT_USER: [`super::RUN`], or for
    /// tests a scratch key of their own.
    #[derive(Debug, Clone)]
    pub struct RunKey {
        key: HSTRING,
    }

    impl RunKey {
        /// The Run key Windows starts programs from, unless
        /// `KILN_TEST_RUN_KEY` is set for tests that start Kiln, so they
        /// never touch the real one ([`super::run_key_for`]): set and
        /// empty, nothing is read or written; set to a key under
        /// HKEY_CURRENT_USER, that key is used. Only a debug or
        /// test-hooks build reads it; a shipped build reads it as unset.
        pub fn user() -> Option<RunKey> {
            // Read only by a debug or test-hooks build; a shipped one
            // reads it as unset.
            #[cfg(any(debug_assertions, feature = "test-hooks"))]
            let test_key =
                std::env::var_os("KILN_TEST_RUN_KEY").map(|k| k.to_string_lossy().into_owned());
            #[cfg(not(any(debug_assertions, feature = "test-hooks")))]
            let test_key: Option<String> = None;
            super::run_key_for(test_key.as_deref(), true).map(|k| RunKey::at(&k))
        }

        /// Kiln's value under `key`, in HKEY_CURRENT_USER.
        pub fn at(key: &str) -> RunKey {
            RunKey {
                key: HSTRING::from(key),
            }
        }
    }

    fn io_err(e: WIN32_ERROR) -> io::Error {
        io::Error::from_raw_os_error(e.0 as i32)
    }

    /// An open key, closed when dropped.
    struct Open(HKEY);

    impl Drop for Open {
        fn drop(&mut self) {
            // SAFETY: the key was opened by this module and is closed once.
            let _ = unsafe { RegCloseKey(self.0) };
        }
    }

    impl RunStore for RunKey {
        fn read(&self) -> io::Result<Option<String>> {
            let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND;
            let name = HSTRING::from(VALUE);
            loop {
                let mut size = 0u32;
                // SAFETY: the key and value names are NUL-terminated
                // HSTRINGs that outlive the call; with no buffer it only
                // writes the size, a local.
                let e = unsafe {
                    RegGetValueW(
                        HKEY_CURRENT_USER,
                        PCWSTR(self.key.as_ptr()),
                        PCWSTR(name.as_ptr()),
                        flags,
                        None,
                        None,
                        Some(&mut size),
                    )
                };
                if e == ERROR_FILE_NOT_FOUND {
                    return Ok(None);
                }
                if e.is_err() {
                    return Err(io_err(e));
                }
                let mut buf = vec![0u16; (size as usize).div_ceil(2).max(1)];
                let mut got = (buf.len() * 2) as u32;
                // SAFETY: as above, and `buf` holds `got` bytes.
                let e = unsafe {
                    RegGetValueW(
                        HKEY_CURRENT_USER,
                        PCWSTR(self.key.as_ptr()),
                        PCWSTR(name.as_ptr()),
                        flags,
                        None,
                        Some(buf.as_mut_ptr().cast()),
                        Some(&mut got),
                    )
                };
                if e == ERROR_FILE_NOT_FOUND {
                    return Ok(None);
                }
                // Grown between the two calls: ask again.
                if e == windows::Win32::Foundation::ERROR_MORE_DATA {
                    continue;
                }
                if e.is_err() {
                    return Err(io_err(e));
                }
                let units = (got as usize / 2).min(buf.len());
                let text = &buf[..units];
                let end = text.iter().position(|&c| c == 0).unwrap_or(text.len());
                return Ok(Some(String::from_utf16_lossy(&text[..end])));
            }
        }

        fn write(&self, data: &str) -> io::Result<()> {
            let mut key = HKEY::default();
            // SAFETY: the key name outlives the call; `key` is a local the
            // call fills, closed by `Open`.
            let e = unsafe {
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    PCWSTR(self.key.as_ptr()),
                    None,
                    PCWSTR::null(),
                    REG_OPTION_NON_VOLATILE,
                    KEY_SET_VALUE,
                    None,
                    &mut key,
                    None,
                )
            };
            if e.is_err() {
                return Err(io_err(e));
            }
            let key = Open(key);
            let units: Vec<u16> = data.encode_utf16().chain(Some(0)).collect();
            let bytes: Vec<u8> = units.iter().flat_map(|u| u.to_le_bytes()).collect();
            let name = HSTRING::from(VALUE);
            // SAFETY: an open key, a NUL-terminated name, and the data's
            // bytes with their NUL, which outlive the call.
            let e =
                unsafe { RegSetValueExW(key.0, PCWSTR(name.as_ptr()), None, REG_SZ, Some(&bytes)) };
            if e.is_err() {
                return Err(io_err(e));
            }
            Ok(())
        }

        fn delete(&self) -> io::Result<()> {
            let mut key = HKEY::default();
            // SAFETY: as in `write`.
            let e = unsafe {
                RegOpenKeyExW(
                    HKEY_CURRENT_USER,
                    PCWSTR(self.key.as_ptr()),
                    None,
                    KEY_SET_VALUE,
                    &mut key,
                )
            };
            if e == ERROR_FILE_NOT_FOUND {
                return Ok(());
            }
            if e.is_err() {
                return Err(io_err(e));
            }
            let key = Open(key);
            let name = HSTRING::from(VALUE);
            // SAFETY: an open key and a NUL-terminated name.
            let e = unsafe { RegDeleteValueW(key.0, PCWSTR(name.as_ptr())) };
            if e.is_err() && e != ERROR_FILE_NOT_FOUND {
                return Err(io_err(e));
            }
            Ok(())
        }
    }

    #[cfg(test)]
    mod tests {
        use std::path::Path;

        use windows::Win32::System::Registry::RegDeleteKeyW;

        use super::*;
        use crate::autostart::{remove_if_ours, run_value, set};

        /// A scratch key `HKCU\Software\KilnTests\<random>\Run` that the
        /// test makes, and removes again when dropped: the Run key, the
        /// random one, and `KilnTests` if nothing else is in it. Each is
        /// removed only empty (`RegDeleteKeyW` refuses a key with keys
        /// under it), and Kiln's value is the only one written.
        struct Scratch {
            parent: String,
            run: String,
        }

        impl Scratch {
            fn new() -> Scratch {
                let nanos = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos();
                let parent = format!(r"Software\KilnTests\{}-{nanos:x}", std::process::id());
                let run = format!(r"{parent}\Run");
                Scratch { parent, run }
            }
        }

        impl Drop for Scratch {
            fn drop(&mut self) {
                let _ = RunKey::at(&self.run).delete();
                for k in [
                    self.run.as_str(),
                    self.parent.as_str(),
                    r"Software\KilnTests",
                ] {
                    let k = HSTRING::from(k);
                    // SAFETY: a NUL-terminated key name that outlives the
                    // call. Only an empty key is removed.
                    let _ = unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(k.as_ptr())) };
                }
            }
        }

        fn key_exists(key: &str) -> bool {
            let mut h = HKEY::default();
            let k = HSTRING::from(key);
            // SAFETY: as in `delete`.
            let e = unsafe {
                RegOpenKeyExW(
                    HKEY_CURRENT_USER,
                    PCWSTR(k.as_ptr()),
                    None,
                    windows::Win32::System::Registry::KEY_READ,
                    &mut h,
                )
            };
            if e.is_ok() {
                drop(Open(h));
            }
            e.is_ok()
        }

        #[test]
        fn the_value_is_set_read_and_removed_only_when_ours() {
            let scratch = Scratch::new();
            let key = RunKey::at(&scratch.run);
            let exe = Path::new(r"C:\Program Files\Kiln\kiln-app.exe");
            // Nothing there yet: nothing to read, and nothing to remove.
            assert_eq!(key.read().unwrap(), None);
            assert!(!remove_if_ours(&key, exe).unwrap());

            // Set: written once, read back as written.
            assert!(set(&key, exe).unwrap());
            assert_eq!(key.read().unwrap(), Some(run_value(exe)));
            assert!(
                !set(&key, exe).unwrap(),
                "the same value isn't written again"
            );

            // Someone else's value under the name is left alone.
            let theirs = r#""C:\Tools\other.exe" --start"#;
            key.write(theirs).unwrap();
            assert!(!remove_if_ours(&key, exe).unwrap());
            assert_eq!(key.read().unwrap().as_deref(), Some(theirs));
            // A Kiln in another folder is not this one.
            let moved = Path::new(r"D:\Portable\Kiln\kiln-app.exe");
            assert!(!remove_if_ours(&key, moved).unwrap());
            assert_eq!(key.read().unwrap().as_deref(), Some(theirs));

            // Ours, with odd case and no quotes, is removed.
            key.write(r"c:\program files\kiln\KILN-APP.EXE --background")
                .unwrap();
            assert!(remove_if_ours(&key, exe).unwrap());
            assert_eq!(key.read().unwrap(), None);
            // Removing what isn't there is fine.
            key.delete().unwrap();

            // A path with characters outside ASCII survives the trip.
            let odd = Path::new(r"C:\Users\Zoë\Programme 写真\kiln-app.exe");
            assert!(set(&key, odd).unwrap());
            assert_eq!(key.read().unwrap(), Some(run_value(odd)));
            assert!(remove_if_ours(&key, odd).unwrap());

            let run = scratch.run.clone();
            let parent = scratch.parent.clone();
            drop(scratch);
            assert!(!key_exists(&run), "the scratch Run key is gone");
            assert!(!key_exists(&parent), "the scratch key is gone");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exe() -> PathBuf {
        PathBuf::from(r"C:\Program Files\Kiln\kiln-app.exe")
    }

    #[test]
    fn the_run_value_quotes_the_path_and_starts_in_the_background() {
        assert_eq!(
            run_value(&exe()),
            r#""C:\Program Files\Kiln\kiln-app.exe" --background"#
        );
        assert_eq!(BACKGROUND, "--background");
        assert_eq!(VALUE, "Kiln");
        assert_eq!(RUN, r"Software\Microsoft\Windows\CurrentVersion\Run");
    }

    #[test]
    fn only_this_kiln_is_ours() {
        let exe = exe();
        for data in [
            r#""C:\Program Files\Kiln\kiln-app.exe" --background"#,
            r#""c:\program files\kiln\KILN-APP.EXE" --background"#,
            r#""C:\Program Files\Kiln\kiln-app.exe""#,
            r"C:\Program Files\Kiln\kiln-app.exe --background",
            r"C:\Program Files\Kiln\kiln-app.exe",
            r"C:/Program Files/Kiln/kiln-app.exe --BACKGROUND",
            r#"  "C:\Program Files\Kiln\kiln-app.exe"   --background  "#,
        ] {
            assert!(is_ours(data, &exe), "{data}");
        }
        for data in [
            r#""C:\Tools\other.exe" --background"#,
            r#""D:\Portable\Kiln\kiln-app.exe" --background"#,
            r#""C:\Program Files\Kiln\kiln-app.exe.old" --background"#,
            r"C:\Program Files\Kiln\kiln.exe",
            "",
            "\"\"",
        ] {
            assert!(!is_ours(data, &exe), "{data:?}");
        }
        // The value Kiln writes is its own.
        assert!(is_ours(&run_value(&exe), &exe));
    }

    fn args(a: &[&str]) -> Vec<OsString> {
        a.iter().map(OsString::from).collect()
    }

    #[test]
    fn background_alone_starts_hidden() {
        let l = Launch::parse(args(&["--background"]));
        assert!(l.background);
        assert!(l.files.is_empty());
        assert!(l.hidden());
        // Nothing at all opens the window.
        let plain = Launch::parse(args(&[]));
        assert_eq!(plain, Launch::default());
        assert!(!plain.hidden());
    }

    #[test]
    fn files_win_over_background() {
        let l = Launch::parse(args(&[r"C:\a.png", "--background", r"D:\b c.mp4"]));
        assert!(l.background);
        assert_eq!(
            l.files,
            [PathBuf::from(r"C:\a.png"), PathBuf::from(r"D:\b c.mp4")]
        );
        assert!(!l.hidden(), "files open the window");
    }

    #[test]
    fn a_link_is_never_a_file() {
        let l = Launch::parse(args(&["kiln:show?n=42"]));
        assert_eq!(l.link.as_deref(), Some("kiln:show?n=42"));
        assert!(l.files.is_empty());
        // Whatever follows the scheme, in any case: still not a file.
        let l = Launch::parse(args(&["KILN:../../a.png", "kiln:x"]));
        assert_eq!(l.link.as_deref(), Some("KILN:../../a.png"));
        assert!(l.files.is_empty());
    }

    #[test]
    fn a_link_launch_ignores_every_other_argument() {
        let link_only = |link: &str| Launch {
            link: Some(link.to_owned()),
            ..Launch::default()
        };
        for a in [
            &[
                "kiln:show?n=1",
                "--forget-startup",
                r"C:\a.png",
                "--background",
            ][..],
            &[r"C:\a.png", "--forget-startup", "kiln:show?n=1"],
            &["--background", "kiln:show?n=1", "kiln:show?n=2"],
        ] {
            let l = Launch::parse(args(a));
            assert_eq!(l, link_only("kiln:show?n=1"), "{a:?}");
            assert!(!l.hidden());
        }
    }

    /// `line` split into arguments as Windows splits a command line, less
    /// the program.
    #[cfg(windows)]
    fn split(line: &str) -> Vec<OsString> {
        use std::os::windows::ffi::OsStringExt;
        use windows::core::HSTRING;
        use windows::Win32::Foundation::{LocalFree, HLOCAL};
        use windows::Win32::UI::Shell::CommandLineToArgvW;
        let mut n = 0;
        // SAFETY: a valid string; the array is read within its count and
        // freed once.
        unsafe {
            let argv = CommandLineToArgvW(&HSTRING::from(line), &mut n);
            assert!(!argv.is_null());
            let out = (1..n as usize)
                .map(|i| OsString::from_wide((*argv.add(i)).as_wide()))
                .collect();
            LocalFree(Some(HLOCAL(argv.cast())));
            out
        }
    }

    #[cfg(windows)]
    #[test]
    fn quotes_injected_into_a_link_reach_nothing_but_the_link() {
        // The link's registered command: "kiln-app.exe" "%1".
        let command = |uri: &str| format!(r#""C:\Program Files\Kiln\kiln-app.exe" "{uri}""#);
        for uri in [
            r#"kiln:x" --forget-startup ""#,
            r#"kiln:x" "C:\Users\me\secret.docx"#,
            r#"kiln:show?n=1" --background "C:\a.png" ""#,
            r#"kiln:show?n=1"" "--forget-startup"#,
        ] {
            let l = Launch::parse(split(&command(uri)));
            assert!(l.link.is_some(), "{uri}");
            assert!(l.files.is_empty(), "{uri}: {:?}", l.files);
            assert!(!l.forget_startup && !l.background, "{uri}");
        }
        // The quote does break out: the flag is an argument of its own.
        let argv = split(&command(r#"kiln:x" --forget-startup ""#));
        assert_eq!(argv[1], "--forget-startup");
    }

    #[test]
    fn a_link_is_told_from_a_file_even_when_it_isnt_unicode() {
        use std::ffi::OsStr;
        assert!(is_link(OsStr::new("kiln:show?n=1")));
        assert!(is_link(OsStr::new("KILN:anything")));
        assert!(is_link(OsStr::new("kiln:")));
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt;
            let odd = OsString::from_wide(&[0x6B, 0x69, 0x6C, 0x6E, 0x3A, 0xD800]);
            assert!(is_link(&odd));
            let l = Launch::parse(vec![odd]);
            assert!(l.files.is_empty() && l.link.is_some());
        }
        assert!(!is_link(OsStr::new(r"C:\photos\kiln:a.jpg")));
        assert!(!is_link(OsStr::new("kiln.jpg")));
        assert!(!is_link(OsStr::new("kil")));
        assert!(!is_link(OsStr::new("")));
    }

    #[test]
    fn unknown_flags_are_ignored() {
        let l = Launch::parse(args(&["--frobnicate", r"C:\a.png", "--x=1"]));
        assert_eq!(l.files, [PathBuf::from(r"C:\a.png")]);
        assert!(!l.background && !l.forget_startup);
        let only = Launch::parse(args(&["--nope"]));
        assert_eq!(only, Launch::default());
        assert!(!only.hidden());
        let forget = Launch::parse(args(&["--forget-startup"]));
        assert!(forget.forget_startup);
        assert!(forget.files.is_empty());
    }

    #[test]
    fn a_test_key_only_ever_narrows_what_kiln_touches() {
        // Set and empty: no Run key at all, in every build, so a test
        // Kiln built for release never writes the real one.
        for debug in [true, false] {
            assert_eq!(run_key_for(Some(""), debug), None, "debug {debug}");
            assert_eq!(run_key_for(None, debug).as_deref(), Some(RUN));
        }
        let scratch = r"Software\KilnTests\abc\Run";
        assert_eq!(run_key_for(Some(scratch), true).as_deref(), Some(scratch));
        // A release build takes no key from a test: it touches none.
        assert_eq!(run_key_for(Some(scratch), false), None);
    }

    #[test]
    fn set_and_remove_keep_to_this_kiln() {
        let exe = exe();
        let store = MemoryRun::default();
        assert!(set(&store, &exe).unwrap());
        assert_eq!(store.value(), Some(run_value(&exe)));
        assert!(!set(&store, &exe).unwrap());
        assert!(remove_if_ours(&store, &exe).unwrap());
        assert_eq!(store.value(), None);

        let theirs = MemoryRun::holding(r#""C:\Tools\other.exe""#);
        assert!(!remove_if_ours(&theirs, &exe).unwrap());
        assert_eq!(theirs.value().as_deref(), Some(r#""C:\Tools\other.exe""#));
    }

    #[test]
    fn autostart_follows_the_last_change() {
        let exe = exe();
        let store = MemoryRun::default();
        let mut a = Autostart::new(store.clone(), Some(exe.clone()));
        a.follow(true);
        assert!(a.finish(Duration::from_secs(10)));
        assert_eq!(store.value(), Some(run_value(&exe)));

        // On, off, on in a row ends on; on, off ends off.
        a.follow(false);
        a.follow(true);
        assert!(a.finish(Duration::from_secs(10)));
        assert_eq!(store.value(), Some(run_value(&exe)));
        a.follow(true);
        a.follow(false);
        assert!(a.finish(Duration::from_secs(10)));
        assert_eq!(store.value(), None);
    }

    #[test]
    fn a_moved_kiln_points_the_value_at_itself() {
        // A portable folder that moved: the value names the old place, and
        // the start with the drag-wheel on writes the new one.
        let old = run_value(Path::new(r"D:\Old\Kiln\kiln-app.exe"));
        let store = MemoryRun::holding(&old);
        let mut a = Autostart::new(store.clone(), Some(exe()));
        a.follow(true);
        assert!(a.finish(Duration::from_secs(10)));
        assert_eq!(store.value(), Some(run_value(&exe())));
    }

    #[test]
    fn off_leaves_another_kiln_alone() {
        let other = run_value(Path::new(r"D:\Portable\Kiln\kiln-app.exe"));
        let store = MemoryRun::holding(&other);
        let mut a = Autostart::new(store.clone(), Some(exe()));
        a.follow(false);
        assert!(a.finish(Duration::from_secs(10)));
        assert_eq!(store.value(), Some(other));
    }

    #[test]
    fn without_its_own_path_nothing_changes() {
        let store = MemoryRun::holding("anything");
        let mut a = Autostart::new(store.clone(), None);
        a.follow(true);
        a.follow(false);
        assert!(a.finish(Duration::from_secs(10)));
        assert_eq!(store.value().as_deref(), Some("anything"));
    }
}
