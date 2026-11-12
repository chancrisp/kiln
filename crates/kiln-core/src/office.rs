//! Office files to PDF through Microsoft Office, when it is installed: Word
//! for documents, Excel for workbooks and PowerPoint for presentations
//! (spec 4.2). LibreOffice is the fallback (`formats.toml`).
//!
//! Office is driven through late-bound automation (`IDispatch`) in a fresh
//! single-threaded apartment per job. The person's file is copied into the
//! job's working folder first and Office opens the copy, read-only, so
//! Office never locks, changes or lists the person's file. Macros are
//! forced off before anything is opened, alerts are off, the document is
//! closed without saving, and only an instance Kiln started, with nothing
//! else open in it and no window showing, is asked to quit. Kiln never
//! kills an Office process: one that doesn't answer is left alone.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use crate::converter::{CancelToken, Context, Converter, Progress, Request};
use crate::error::{Error, Result};

/// How long Kiln waits for Office to make one PDF.
pub const TIMEOUT: Duration = Duration::from_secs(120);

/// The switch for Office to PDF through Microsoft Office. Off (owner
/// decision D14) until review 9/10/14's C1 and I3 land: Kiln then never
/// counts Office as a converter, so Office files go to LibreOffice as
/// before, and Settings shows no Office card. Not a setting: nothing a
/// person can turn on.
pub const OFFICE_TO_PDF: bool = false;

/// The Office apps Kiln may convert with: those installed while
/// [`OFFICE_TO_PDF`] is on, none while it is off.
pub fn usable() -> Installed {
    if OFFICE_TO_PDF {
        find_installed()
    } else {
        Installed::default()
    }
}

/// Set in tests to make Kiln behave as if Office weren't installed.
pub const NO_OFFICE: &str = "KILN_TEST_NO_OFFICE";

/// Whether [`NO_OFFICE`] is set. Read only by a debug or test-hooks build;
/// a shipped one reads it as unset.
fn no_office_asked() -> bool {
    #[cfg(any(debug_assertions, feature = "test-hooks"))]
    {
        std::env::var_os(NO_OFFICE).is_some()
    }
    #[cfg(not(any(debug_assertions, feature = "test-hooks")))]
    {
        false
    }
}

/// `msoAutomationSecurityForceDisable`: no macro in a file Kiln opens runs.
const MACROS_OFF: i32 = 3;

/// The password Kiln gives every file it opens. A file without one opens
/// as usual; a file with one fails at once instead of asking for it.
const NO_PASSWORD: &str = "kiln-has-no-password";

/// Word's "The password is incorrect." (error 5408).
const WORD_BAD_PASSWORD: i32 = 0x800A_1520_u32 as i32;

/// The three Office apps Kiln uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum App {
    Word,
    Excel,
    PowerPoint,
}

impl App {
    pub const ALL: [App; 3] = [App::Word, App::Excel, App::PowerPoint];

    /// The app's name, as messages say it.
    pub fn name(self) -> &'static str {
        match self {
            App::Word => "Word",
            App::Excel => "Excel",
            App::PowerPoint => "PowerPoint",
        }
    }

    /// The automation ProgID.
    pub fn prog_id(self) -> &'static str {
        match self {
            App::Word => "Word.Application",
            App::Excel => "Excel.Application",
            App::PowerPoint => "PowerPoint.Application",
        }
    }

    /// The program's file name.
    pub fn exe(self) -> &'static str {
        match self {
            App::Word => "WINWORD.EXE",
            App::Excel => "EXCEL.EXE",
            App::PowerPoint => "POWERPNT.EXE",
        }
    }

    /// The app that opens files of type `ext` for Kiln, if one does.
    pub fn for_ext(ext: &str) -> Option<App> {
        match ext {
            "doc" | "docx" | "odt" | "rtf" | "txt" => Some(App::Word),
            "xlsx" | "xls" | "ods" | "csv" => Some(App::Excel),
            "pptx" | "ppt" | "odp" => Some(App::PowerPoint),
            _ => None,
        }
    }

    /// The key [`crate::Availability`] keeps for this app.
    pub(crate) fn key(self) -> &'static str {
        match self {
            App::Word => "office-word",
            App::Excel => "office-excel",
            App::PowerPoint => "office-powerpoint",
        }
    }

    /// The collection of open files: `Documents`, `Workbooks`,
    /// `Presentations`.
    fn files(self) -> &'static str {
        match self {
            App::Word => "Documents",
            App::Excel => "Workbooks",
            App::PowerPoint => "Presentations",
        }
    }
}

/// Where each Office app's program is, for the apps that are installed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Installed {
    pub word: Option<PathBuf>,
    pub excel: Option<PathBuf>,
    pub powerpoint: Option<PathBuf>,
}

impl Installed {
    pub fn get(&self, app: App) -> Option<&Path> {
        match app {
            App::Word => self.word.as_deref(),
            App::Excel => self.excel.as_deref(),
            App::PowerPoint => self.powerpoint.as_deref(),
        }
    }

    pub fn has(&self, app: App) -> bool {
        self.get(app).is_some()
    }

    /// The apps found, in Word, Excel, PowerPoint order.
    pub fn apps(&self) -> Vec<App> {
        App::ALL.into_iter().filter(|a| self.has(*a)).collect()
    }

    pub fn any(&self) -> bool {
        App::ALL.into_iter().any(|a| self.has(a))
    }

    /// The folder of the first app found, as Settings shows it.
    pub fn folder(&self) -> Option<&Path> {
        App::ALL
            .into_iter()
            .find_map(|a| self.get(a))
            .and_then(Path::parent)
    }
}

/// Which Office apps are installed: each ProgID's class, its
/// `LocalServer32` program, and that program on disk. Reads the registry
/// and the disk, so the window's thread leaves it to a worker.
pub fn find_installed() -> Installed {
    if no_office_asked() {
        return Installed::default();
    }
    find_installed_with(&crate::process::class_value, &|p: &Path| p.is_file())
}

/// [`find_installed`] with `read` giving the default value of a key under
/// `HKEY_CLASSES_ROOT` and `exists` saying whether a program is there.
pub fn find_installed_with(
    read: &dyn Fn(&str) -> Option<String>,
    exists: &dyn Fn(&Path) -> bool,
) -> Installed {
    let find = |app: App| {
        let class = read(&format!(r"{}\CLSID", app.prog_id()))?;
        let server = read(&format!(r"CLSID\{}\LocalServer32", class.trim()))?;
        server_path(&server).filter(|p| exists(p))
    };
    Installed {
        word: find(App::Word),
        excel: find(App::Excel),
        powerpoint: find(App::PowerPoint),
    }
}

/// The program a `LocalServer32` value starts, without its arguments:
/// `"C:\…\WINWORD.EXE" /Automation` or the same unquoted.
fn server_path(value: &str) -> Option<PathBuf> {
    let value = value.trim();
    let program = match value.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or_default(),
        // Unquoted, the folder may still have spaces: the program ends
        // at ".exe".
        None => match value.to_ascii_lowercase().find(".exe") {
            Some(end) => &value[..end + 4],
            None => value.split_whitespace().next().unwrap_or_default(),
        },
    };
    let program = program.trim();
    (!program.is_empty()).then(|| PathBuf::from(program))
}

/// The first bytes of an OLE compound file.
const COMPOUND_FILE: &[u8] = b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1";

/// What a file holds, by its content rather than its name. This, not the
/// extension, says how the copy Office opens is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Content {
    /// An Office Open XML package: a zip with `[Content_Types].xml`.
    Ooxml,
    /// Any other zip Kiln can read, such as an OpenDocument file.
    OtherZip,
    /// Something that starts as a zip but that Kiln can't read.
    BadZip,
    /// An OLE compound file: a Word, Excel or PowerPoint 97 file, or an
    /// Office Open XML file with a password.
    Compound,
    Rtf,
    /// HTML, XML or a web archive, which Office would open as a page and
    /// fetch what it names.
    Markup,
    /// Anything else, taken as text.
    Text,
}

/// What [`look`] found in a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Look {
    content: Content,
    /// The file has a password: Office would ask for it.
    password: bool,
}

/// Look at what the file at `path` holds, and whether it has a password,
/// before Office is given it. A password shows as an `EncryptedPackage`
/// stream (Office Open XML) or an `EncryptedSummary` stream (PowerPoint
/// 97) in a compound file, or as encryption data in an OpenDocument
/// file's manifest. Word 97 and Excel 97 passwords are left to Office,
/// which refuses Kiln's made-up password at once.
fn look(path: &Path) -> Result<Look> {
    let mut file = fs::File::open(path)
        .map(std::io::BufReader::new)
        .map_err(|e| Error::io(path, e))?;
    let mut head = Vec::new();
    (&mut file)
        .take(512)
        .read_to_end(&mut head)
        .map_err(|e| Error::io(path, e))?;
    let found = |content| Look {
        content,
        password: false,
    };
    if head.starts_with(COMPOUND_FILE) {
        let password = compound_streams(&mut file).is_some_and(|streams| {
            streams
                .iter()
                .any(|s| s == "EncryptedPackage" || s == "EncryptedSummary")
        });
        return Ok(Look {
            content: Content::Compound,
            password,
        });
    }
    if head.starts_with(b"PK") {
        let manifest = |name: &str| name.eq_ignore_ascii_case("META-INF/manifest.xml");
        let Ok(parts) = package::read(&mut file, &manifest) else {
            return Ok(found(Content::BadZip));
        };
        let ooxml = parts
            .iter()
            .any(|p| p.name.eq_ignore_ascii_case("[Content_Types].xml"));
        let password = parts
            .iter()
            .filter_map(|p| p.text.as_deref())
            .any(|t| t.windows(15).any(|w| w == b"encryption-data"));
        return Ok(Look {
            content: if ooxml {
                Content::Ooxml
            } else {
                Content::OtherZip
            },
            password,
        });
    }
    Ok(found(if head.starts_with(br"{\rtf") {
        Content::Rtf
    } else if is_markup(&head) {
        Content::Markup
    } else {
        Content::Text
    }))
}

/// Whether `head`, the start of a text file, is markup or a web archive:
/// it starts with `<` or `MIME-Version:`, after any byte order mark and
/// blank space, in UTF-8 or UTF-16.
fn is_markup(head: &[u8]) -> bool {
    let text: Vec<u8> = if let Some(rest) = head.strip_prefix(b"\xFF\xFE") {
        rest.iter().step_by(2).copied().collect()
    } else if let Some(rest) = head.strip_prefix(b"\xFE\xFF") {
        rest.iter().skip(1).step_by(2).copied().collect()
    } else {
        head.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(head).to_vec()
    };
    let start = text
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(text.len());
    let text = &text[start..];
    const MIME: &[u8] = b"mime-version:";
    text.starts_with(b"<")
        || text
            .get(..MIME.len())
            .is_some_and(|t| t.eq_ignore_ascii_case(MIME))
}

/// Whether a file named `.ext` may hold `content` and still go to Office:
/// what Office would open as that kind of file, and nothing that would make
/// it open a page or a package under another name's rules. Office Open XML
/// under an older or OpenDocument name is a package, and its copy is made
/// as one.
fn accepts(ext: &str, content: Content) -> bool {
    use Content::*;
    match ext {
        // Kiln's routes don't send the macro-enabled kinds, but they are the
        // same packages.
        "docx" | "xlsx" | "pptx" | "docm" | "xlsm" | "pptm" => matches!(content, Ooxml | Compound),
        // Word 97 files are often RTF under this name.
        "doc" => matches!(content, Compound | Ooxml | Rtf),
        "xls" | "ppt" => matches!(content, Compound | Ooxml),
        "odt" | "ods" | "odp" => matches!(content, OtherZip | Ooxml),
        "rtf" => content == Rtf,
        "txt" | "csv" => content == Text,
        _ => false,
    }
}

/// The names of the streams in `file`, an OLE compound file, from its
/// directory. `None` when it isn't one Kiln can read. Reads the header,
/// then the directory's sectors one at a time through the FAT, and never
/// more of them than the file has.
fn compound_streams<R: Read + std::io::Seek>(file: &mut R) -> Option<Vec<String>> {
    use std::io::SeekFrom;
    /// Sector numbers from here up mean "none" (free, end of chain, FAT).
    const NO_SECTOR: u32 = 0xFFFF_FFFA;
    let u16_at =
        |b: &[u8], at: usize| Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?));
    let u32_at =
        |b: &[u8], at: usize| Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?));

    let len = file.seek(SeekFrom::End(0)).ok()?;
    let mut header = [0u8; 512];
    file.seek(SeekFrom::Start(0)).ok()?;
    file.read_exact(&mut header).ok()?;
    if !header.starts_with(COMPOUND_FILE) {
        return None;
    }
    let shift = u16_at(&header, 30)?;
    if shift != 9 && shift != 12 {
        return None;
    }
    let size = 1u64 << shift;
    let sectors = (len / size).saturating_sub(1);
    let mut read_sector = |id: u32| -> Option<Vec<u8>> {
        if u64::from(id) >= sectors {
            return None;
        }
        let mut sector = vec![0u8; size as usize];
        file.seek(SeekFrom::Start((u64::from(id) + 1) * size))
            .ok()?;
        file.read_exact(&mut sector).ok()?;
        Some(sector)
    };
    let per_sector = (size / 4) as u32;

    // Where the FAT's sectors are: the first 109 in the header, the rest in
    // a chain of DIFAT sectors.
    let fat_count = u32_at(&header, 44)?;
    if u64::from(fat_count) > sectors / u64::from(per_sector) + 1 {
        return None;
    }
    let mut fat = Vec::with_capacity(fat_count as usize);
    for i in 0..109 {
        let id = u32_at(&header, 76 + i * 4)?;
        if id < NO_SECTOR && fat.len() < fat_count as usize {
            fat.push(id);
        }
    }
    let mut difat = u32_at(&header, 68)?;
    let mut steps = 0u64;
    while difat < NO_SECTOR && fat.len() < fat_count as usize {
        steps += 1;
        if steps > sectors {
            return None;
        }
        let sector = read_sector(difat)?;
        for i in 0..per_sector as usize - 1 {
            let id = u32_at(&sector, i * 4)?;
            if id < NO_SECTOR && fat.len() < fat_count as usize {
                fat.push(id);
            }
        }
        difat = u32_at(&sector, size as usize - 4)?;
    }

    // The directory, 128 bytes an entry, through its chain in the FAT.
    let mut names = Vec::new();
    let mut at = u32_at(&header, 48)?;
    let mut steps = 0u64;
    while at < NO_SECTOR {
        steps += 1;
        if steps > sectors {
            return None;
        }
        let sector = read_sector(at)?;
        for entry in sector.chunks_exact(128) {
            let name_len = usize::from(u16_at(entry, 64)?);
            // A stream (2), with a name of 1 to 31 characters and its NUL.
            if entry[66] != 2 || !(4..=64).contains(&name_len) {
                continue;
            }
            let wide: Vec<u16> = entry[..name_len - 2]
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            names.push(String::from_utf16_lossy(&wide));
        }
        let fat_sector = *fat.get((at / per_sector) as usize)?;
        let fat_part = read_sector(fat_sector)?;
        at = u32_at(&fat_part, (at % per_sector) as usize * 4)?;
    }
    Some(names)
}

/// One argument to an automation call. `Missing` leaves an optional
/// argument at its default, so later ones can be given by position.
#[derive(Debug, Clone, PartialEq)]
pub enum Arg<'a> {
    Missing,
    Bool(bool),
    Int(i32),
    Str(&'a str),
}

/// What an automation call failed with: the HRESULT, or the app's own
/// error code, and its words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fault {
    pub code: i32,
    pub text: String,
}

/// The calls Kiln makes on an Office app, so the order of them can be
/// tested without Office.
pub(crate) trait Automation {
    type Obj;
    /// A property that is an object.
    fn object(&self, of: &Self::Obj, name: &str) -> std::result::Result<Self::Obj, Fault>;
    /// A property that is a number or a yes/no (`true` is nonzero).
    fn number(&self, of: &Self::Obj, name: &str) -> std::result::Result<i32, Fault>;
    fn put(&self, of: &Self::Obj, name: &str, value: Arg<'_>) -> std::result::Result<(), Fault>;
    fn call(
        &self,
        of: &Self::Obj,
        name: &str,
        args: &[Arg<'_>],
    ) -> std::result::Result<Option<Self::Obj>, Fault>;
}

/// Where in the work a fault came from.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Failure {
    /// Opening the file: a password shows here.
    Open(Fault),
    Other(Fault),
}

/// This instance's settings: macros off first, then no window and no
/// alerts. None of these is kept by Office after the instance quits.
fn prepare<A: Automation>(a: &A, app: App, root: &A::Obj) -> std::result::Result<(), Fault> {
    a.put(root, "AutomationSecurity", Arg::Int(MACROS_OFF))?;
    match app {
        App::Word => {
            a.put(root, "Visible", Arg::Bool(false))?;
            // wdAlertsNone.
            a.put(root, "DisplayAlerts", Arg::Int(0))
        }
        App::Excel => {
            a.put(root, "Visible", Arg::Bool(false))?;
            a.put(root, "DisplayAlerts", Arg::Bool(false))
        }
        // PowerPoint can't hide its own window; the file opens without
        // one. ppAlertsNone.
        App::PowerPoint => a.put(root, "DisplayAlerts", Arg::Int(1)),
    }
}

/// The arguments that open `input` read-only, with no recent-files
/// entry, no links updated, no repair and no password prompt.
fn open_args(app: App, input: &str) -> Vec<Arg<'_>> {
    use Arg::{Bool, Int, Missing, Str};
    match app {
        // Documents.Open: FileName, ConfirmConversions, ReadOnly,
        // AddToRecentFiles, PasswordDocument, PasswordTemplate, Revert,
        // WritePasswordDocument, WritePasswordTemplate, Format, Encoding,
        // Visible, OpenAndRepair, DocumentDirection, NoEncodingDialog.
        App::Word => vec![
            Str(input),
            Bool(false),
            Bool(true),
            Bool(false),
            Str(NO_PASSWORD),
            Str(NO_PASSWORD),
            Bool(false),
            Missing,
            Missing,
            Missing,
            Missing,
            Bool(false),
            Bool(false),
            Missing,
            Bool(true),
        ],
        // Workbooks.Open: Filename, UpdateLinks, ReadOnly, Format,
        // Password, WriteResPassword, IgnoreReadOnlyRecommended, Origin,
        // Delimiter, Editable, Notify, Converter, AddToMru, Local,
        // CorruptLoad (xlNormalLoad).
        App::Excel => vec![
            Str(input),
            Int(0),
            Bool(true),
            Missing,
            Str(NO_PASSWORD),
            Missing,
            Bool(true),
            Missing,
            Missing,
            Bool(false),
            Bool(false),
            Missing,
            Bool(false),
            Missing,
            Int(0),
        ],
        // Presentations.Open: FileName, ReadOnly (msoTrue), Untitled,
        // WithWindow (msoFalse).
        App::PowerPoint => vec![Str(input), Int(-1), Int(0), Int(0)],
    }
}

/// Open `input` in `root`, an instance of `app`, write its PDF to
/// `output`, and close it without saving, whether the PDF was written or
/// not.
fn export<A: Automation>(
    a: &A,
    app: App,
    root: &A::Obj,
    input: &str,
    output: &str,
) -> std::result::Result<(), Failure> {
    prepare(a, app, root).map_err(Failure::Other)?;
    let files = a.object(root, app.files()).map_err(Failure::Other)?;
    let file = a
        .call(&files, "Open", &open_args(app, input))
        .map_err(Failure::Open)?
        .ok_or_else(|| {
            Failure::Open(Fault {
                code: 0,
                text: "it opened nothing.".to_owned(),
            })
        })?;
    let saved = match app {
        // wdExportFormatPDF, not opened afterwards.
        App::Word => a.call(
            &file,
            "ExportAsFixedFormat",
            &[Arg::Str(output), Arg::Int(17), Arg::Bool(false)],
        ),
        // The workbook, so every sheet: xlTypePDF, Filename, Quality,
        // IncludeDocProperties, IgnorePrintAreas, From, To,
        // OpenAfterPublish.
        App::Excel => a.call(
            &file,
            "ExportAsFixedFormat",
            &[
                Arg::Int(0),
                Arg::Str(output),
                Arg::Missing,
                Arg::Bool(true),
                Arg::Bool(false),
                Arg::Missing,
                Arg::Missing,
                Arg::Bool(false),
            ],
        ),
        // ppSaveAsPDF: a new file, so the presentation itself isn't saved.
        App::PowerPoint => a.call(&file, "SaveAs", &[Arg::Str(output), Arg::Int(32)]),
    };
    let close: &[Arg<'_>] = match app {
        // wdDoNotSaveChanges.
        App::Word => &[Arg::Int(0)],
        // SaveChanges: no.
        App::Excel => &[Arg::Bool(false)],
        App::PowerPoint => &[],
    };
    // The PDF is written or not by now; a failed close changes neither.
    let _ = a.call(&file, "Close", close);
    saved.map(|_| ()).map_err(Failure::Other)
}

/// Whether `root` may be asked to quit: nothing is open in it and it
/// shows no window, so nobody else is using it. When it can't say, it
/// stays.
fn may_quit<A: Automation>(a: &A, app: App, root: &A::Obj) -> bool {
    let open = a
        .object(root, app.files())
        .and_then(|files| a.number(&files, "Count"));
    let visible = a.number(root, "Visible");
    matches!((open, visible), (Ok(0), Ok(0)))
}

/// Quit `root`, an instance Kiln started, when [`may_quit`] says so.
/// Whether it was asked to.
fn quit<A: Automation>(a: &A, app: App, root: &A::Obj) -> bool {
    if !may_quit(a, app, root) {
        return false;
    }
    let args: &[Arg<'_>] = match app {
        // wdDoNotSaveChanges.
        App::Word => &[Arg::Int(0)],
        _ => &[],
    };
    let _ = a.call(root, "Quit", args);
    true
}

/// The error a failure of `app` becomes.
fn error_of(app: App, failure: Failure) -> Error {
    match failure {
        Failure::Open(f)
            if f.code == WORD_BAD_PASSWORD || f.text.to_lowercase().contains("password") =>
        {
            Error::OfficePassword { app: app.name() }
        }
        Failure::Open(f) | Failure::Other(f) => {
            let summary = f.text.lines().next().unwrap_or_default().trim().to_owned();
            Error::EngineFailed {
                engine: app.name(),
                summary: if summary.is_empty() {
                    format!("it stopped with code {:#010X}.", f.code)
                } else {
                    summary
                },
                details: format!("{} ({:#010X})", f.text.trim(), f.code),
            }
        }
    }
}

/// One instance of each app at a time: PowerPoint has only one, and a job
/// must know the instance it quits is its own.
static TURNS: [Mutex<()>; 3] = [Mutex::new(()), Mutex::new(()), Mutex::new(())];

/// The process ids of the Office programs Kiln started that haven't been
/// seen to exit, for each app.
static OURS: [Mutex<Vec<u32>>; 3] = [Mutex::new(vec![]), Mutex::new(vec![]), Mutex::new(vec![])];

/// How long Kiln waits for an Office program it asked to quit to exit.
const EXIT_WAIT: Duration = Duration::from_secs(10);

/// Wait at most `wait` for the programs in `ours` to exit, looking with
/// `running`. `ours` keeps those still running.
fn settle(ours: &mut Vec<u32>, running: &dyn Fn() -> Vec<u32>, wait: Duration) {
    let deadline = Instant::now() + wait;
    loop {
        let now = running();
        ours.retain(|pid| now.contains(pid));
        if ours.is_empty() || Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Whether PowerPoint is in use, so Kiln mustn't start one (decision 4):
/// a PowerPoint program runs that Kiln didn't start, or one is listed for
/// others to use (`listed`). Kiln's own, from the job before, is first
/// given `wait` to exit; one that still runs then can't be told from
/// someone's using it, so it counts as in use too.
fn powerpoint_in_use(
    ours: &mut Vec<u32>,
    running: &dyn Fn() -> Vec<u32>,
    listed: &dyn Fn() -> bool,
    wait: Duration,
) -> bool {
    if running().iter().any(|pid| !ours.contains(pid)) {
        return true;
    }
    settle(ours, running, wait);
    !ours.is_empty() || running().iter().any(|pid| !ours.contains(pid)) || listed()
}

/// Run `work` on a thread of its own and wait for it at most `timeout`,
/// or until `cancel`. Past either, Kiln stops waiting, the flag `work` is
/// given is set, and the thread is left to finish by itself: Office is
/// never killed.
fn within(
    app: App,
    timeout: Duration,
    cancel: &CancelToken,
    work: impl FnOnce(&AtomicBool) -> Result<()> + Send + 'static,
) -> Result<()> {
    let gone = Arc::new(AtomicBool::new(false));
    let flag = gone.clone();
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name(format!("kiln-{}", app.name().to_lowercase()))
        .spawn(move || {
            let _ = tx.send(work(&flag));
        })
        .map_err(|e| Error::EngineFailed {
            engine: app.name(),
            summary: "Kiln couldn't start a thread for it.".to_owned(),
            details: e.to_string(),
        })?;
    let deadline = Instant::now() + timeout;
    let give_up = |e: Error| {
        gone.store(true, Ordering::SeqCst);
        Err(e)
    };
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(answer) => return answer,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(Error::EngineFailed {
                    engine: app.name(),
                    summary: "it stopped without saying why.".to_owned(),
                    details: String::new(),
                })
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if cancel.is_cancelled() {
                    return give_up(Error::Cancelled);
                }
                if Instant::now() >= deadline {
                    return give_up(Error::OfficeTimeout { app: app.name() });
                }
            }
        }
    }
}

/// One Office job: on a thread of its own, once `turn` (the app's) comes,
/// `start` makes the PDF at `private`, a name only Office writes. Kiln
/// moves it to `output` only when it was made while Kiln still waited. A
/// job cancelled or out of time while it waits for its turn never calls
/// `start`; `start` is given the flag that says Kiln stopped waiting, to
/// look at again before it starts Office.
fn office_job(
    app: App,
    turn: &'static Mutex<()>,
    private: PathBuf,
    output: &Path,
    timeout: Duration,
    cancel: &CancelToken,
    start: impl FnOnce(&Path, &AtomicBool) -> Result<()> + Send + 'static,
) -> Result<()> {
    let made = private.clone();
    within(app, timeout, cancel, move |gone| {
        // A poisoned turn only means an earlier job panicked.
        let _turn = turn.lock().unwrap_or_else(|e| e.into_inner());
        if gone.load(Ordering::SeqCst) {
            return Err(Error::Cancelled);
        }
        start(&made, gone)
    })?;
    fs::rename(&private, output).map_err(|e| Error::io(output, e))
}

/// Write the PDF of `input`, a file `app` opens, to `output`, through
/// Microsoft Office. The file is copied beside `output` first and Office
/// opens the copy. Office writes the PDF under a name of its own, which
/// becomes `output` only when it finished in time. Waits at most
/// `timeout`.
pub fn to_pdf(
    app: App,
    input: &Path,
    output: &Path,
    cancel: &CancelToken,
    timeout: Duration,
) -> Result<()> {
    let ext = input
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let seen = look(input)?;
    if seen.password {
        return Err(Error::OfficePassword { app: app.name() });
    }
    if !accepts(&ext, seen.content) {
        return Err(Error::WrongContent {
            path: input.to_path_buf(),
            code: ext.to_ascii_uppercase(),
        });
    }
    let work = output
        .parent()
        .expect("the output is inside the job's working folder");
    let copy = copy_for_office(app, input, work, seen.content)?;
    let out_dir = work.join("office-out");
    fs::create_dir_all(&out_dir).map_err(|e| Error::io(&out_dir, e))?;
    office_job(
        app,
        &TURNS[app as usize],
        out_dir.join("out.pdf"),
        output,
        timeout,
        cancel,
        move |private, gone| com::run(app, &copy, private, gone),
    )
}

/// The most characters of the person's file name the copy Office opens
/// keeps: the job folder is deep in the temp folder, and Word fails past
/// about 255 characters of path. A PDF title taken from the name stays
/// close to it.
const COPY_NAME_CHARS: usize = 80;

/// The copy of `input` Office opens, in `work\office-in`: the person's file
/// is never locked, changed or listed in Office's recent files. It keeps
/// the start of the file's name, and its extension follows `content`, what
/// the file holds ([`look`]), so Office takes it for what it is. An RTF's
/// picture fields would have Word fetch the files they name, from the disk
/// or an address, so the copy has them defused, as LibreOffice's does; a
/// package has its outside links defused ([`copy_package`]).
fn copy_for_office(app: App, input: &Path, work: &Path, content: Content) -> Result<PathBuf> {
    let dir = work.join("office-in");
    fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
    let named = input
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let ext = match (content, app) {
        (Content::Ooxml, App::Word) => "docx",
        (Content::Ooxml, App::Excel) => "xlsx",
        (Content::Ooxml, App::PowerPoint) => "pptx",
        (Content::Compound, App::Word) => "doc",
        (Content::Compound, App::Excel) => "xls",
        (Content::Compound, App::PowerPoint) => "ppt",
        (Content::OtherZip, App::Word) => "odt",
        (Content::OtherZip, App::Excel) => "ods",
        (Content::OtherZip, App::PowerPoint) => "odp",
        (Content::Rtf, _) => "rtf",
        _ if named == "csv" => "csv",
        _ => "txt",
    };
    let stem = input
        .file_stem()
        .and_then(|n| n.to_str())
        .map(|n| n.chars().take(COPY_NAME_CHARS).collect::<String>())
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "document".to_owned());
    let copy = dir.join(format!("{stem}.{ext}"));
    match content {
        Content::Rtf => {
            let bytes = fs::read(input).map_err(|e| Error::io(input, e))?;
            let bytes = crate::libreoffice::neutralise_rtf_links(&bytes).unwrap_or(bytes);
            fs::write(&copy, bytes).map_err(|e| Error::io(&copy, e))?;
        }
        // A package whose parts Kiln can't read isn't handed to Office:
        // its links would go with it.
        Content::Ooxml => copy_package(input, &copy).map_err(|e| {
            e.unwrap_or_else(|| Error::WrongContent {
                path: input.to_path_buf(),
                code: named.to_ascii_uppercase(),
            })
        })?,
        _ => {
            fs::copy(input, &copy).map_err(|e| Error::io(&copy, e))?;
        }
    }
    Ok(copy)
}

/// Copy the package `input` to `copy` with its relationships defused.
/// Only those parts are read into memory; the rest are copied from the
/// file as stored. `Err(None)` when it isn't a package Kiln can read.
fn copy_package(input: &Path, copy: &Path) -> std::result::Result<(), Option<Error>> {
    let mut source = fs::File::open(input)
        .map(std::io::BufReader::new)
        .map_err(|e| Some(Error::io(input, e)))?;
    let mut parts = package::read(&mut source, &is_rels).map_err(|()| None)?;
    if !defuse_parts(&mut parts) {
        fs::copy(input, copy).map_err(|e| Some(Error::io(copy, e)))?;
        return Ok(());
    }
    let written = fs::File::create(copy).and_then(|f| {
        let mut out = std::io::BufWriter::new(f);
        package::write(&mut source, &parts, &mut out)?;
        std::io::Write::flush(&mut out)
    });
    written.map_err(|e| Some(Error::io(copy, e)))
}

/// `rels`, an Office package's relationships part, with the address of
/// every part kept outside the file taken out, except links to click
/// (hyperlinks): Office would fetch a picture, a template, a linked object
/// or a workbook from it, from the disk, a share or the web. `None` when
/// there is no such address.
fn defuse_rels(rels: &str) -> Option<String> {
    let mut out = String::with_capacity(rels.len());
    let mut rest = rels;
    let mut changed = false;
    while let Some(at) = rest.find("Relationship") {
        let starts_tag = rest[..at].ends_with('<') || rest[..at].ends_with(':');
        let after = &rest[at + "Relationship".len()..];
        if !starts_tag || !after.starts_with(char::is_whitespace) {
            out.push_str(&rest[..at + "Relationship".len()]);
            rest = after;
            continue;
        }
        let end = after
            .find('>')
            .map_or(rest.len(), |e| at + "Relationship".len() + e + 1);
        let tag = &rest[at..end];
        let external = attr(tag, "TargetMode").is_some_and(|(_, v)| v == "External");
        let hyperlink = attr(tag, "Type").is_some_and(|(_, v)| v.ends_with("/hyperlink"));
        out.push_str(&rest[..at]);
        match attr(tag, "Target").filter(|_| external && !hyperlink) {
            Some((range, _)) => {
                out.push_str(&tag[..range.start]);
                out.push_str(&tag[range.end..]);
                changed = true;
            }
            None => out.push_str(tag),
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    changed.then_some(out)
}

/// The value of attribute `name` in `tag`, and where that value sits.
fn attr<'t>(tag: &'t str, name: &str) -> Option<(std::ops::Range<usize>, &'t str)> {
    let mut from = 0;
    while let Some(i) = tag[from..].find(name) {
        let at = from + i;
        from = at + name.len();
        let before = tag[..at].chars().next_back();
        if !before.is_some_and(char::is_whitespace) {
            continue;
        }
        let after = tag[from..].trim_start();
        let Some(after) = after.strip_prefix('=') else {
            continue;
        };
        let after = after.trim_start();
        let quote = after.chars().next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        let start = tag.len() - after.len() + 1;
        let len = tag[start..].find(quote)?;
        return Some((start..start + len, &tag[start..start + len]));
    }
    None
}

/// Whether `name`, a part of an Office package, is a relationships part:
/// the only parts Kiln reads and changes in a package's copy.
fn is_rels(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".rels")
}

/// `zip`, an Office package, with every relationships part defused
/// ([`defuse_rels`]). `Ok(None)` when nothing needed it; `Err` when it
/// isn't a zip Kiln can read. Only the relationships parts are inflated;
/// every other part is copied as it is stored. [`copy_package`] does the
/// same from a file to a file.
#[cfg(test)]
fn defuse_package(zip: &[u8]) -> std::result::Result<Option<Vec<u8>>, ()> {
    let mut source = std::io::Cursor::new(zip);
    let mut parts = package::read(&mut source, &is_rels)?;
    if !defuse_parts(&mut parts) {
        return Ok(None);
    }
    let mut out = Vec::new();
    package::write(&mut source, &parts, &mut out).map_err(|_| ())?;
    Ok(Some(out))
}

/// Defuse the relationships parts among `parts`, as read by
/// [`package::read`] with [`is_rels`]. Whether any changed.
fn defuse_parts(parts: &mut [package::Part]) -> bool {
    let mut changed = false;
    for part in parts.iter_mut() {
        let Some(bytes) = part.text.as_mut().filter(|_| is_rels(&part.name)) else {
            continue;
        };
        let text = String::from_utf8_lossy(bytes);
        if let Some(defused) = defuse_rels(&text) {
            *bytes = defused.into_bytes();
            part.changed = true;
            changed = true;
        }
    }
    changed
}

/// Just enough of the zip format for an Office package: its directory,
/// the parts Kiln reads inflated (stored or deflated, no encryption, no
/// zip64), and a package written again from them. A part's stated size is
/// never trusted for memory: each part Kiln inflates is capped, and so are
/// all of them together. Every other part is copied as it is stored,
/// without being inflated.
mod package {
    use std::io::{self, Read, Seek, SeekFrom, Write};

    /// The most one part Kiln reads may inflate to.
    pub(super) const PART_CAP: u64 = 16 << 20;
    /// The most all the parts Kiln reads in one package may inflate to.
    pub(super) const TOTAL_CAP: u64 = 64 << 20;
    /// The most the central directory may take: far more than 65,535
    /// entries with names of any usual length need.
    const DIRECTORY_CAP: u64 = 16 << 20;

    /// The caps a read keeps to.
    #[derive(Debug, Clone, Copy)]
    pub(super) struct Caps {
        pub part: u64,
        pub total: u64,
    }

    pub(super) const CAPS: Caps = Caps {
        part: PART_CAP,
        total: TOTAL_CAP,
    };

    /// One entry of a package, as its central directory gives it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) struct Part {
        pub name: String,
        /// The part inflated, for a part Kiln reads; `None` for one it
        /// copies as stored.
        pub text: Option<Vec<u8>>,
        /// Whether `text` was changed, so the part is written from it.
        pub changed: bool,
        pub method: u16,
        pub crc: u32,
        /// The stored (compressed) size and the inflated size the
        /// directory states.
        pub packed: u32,
        pub size: u32,
        /// Where the stored bytes start in the package.
        pub data_at: u64,
    }

    impl Part {
        /// A new part holding `bytes`, written deflated.
        #[cfg(test)]
        pub(super) fn new(name: &str, bytes: Vec<u8>) -> Part {
            Part {
                name: name.to_owned(),
                text: Some(bytes),
                changed: true,
                method: 8,
                crc: 0,
                packed: 0,
                size: 0,
                data_at: 0,
            }
        }
    }

    fn u16_at(b: &[u8], at: usize) -> Option<u16> {
        Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
    }

    fn u32_at(b: &[u8], at: usize) -> Option<u32> {
        Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
    }

    /// Whether `name` stays inside the package: not empty, no drive,
    /// root or `..` step, with `/` or `\` between steps, and no NUL.
    pub(super) fn safe_name(name: &str) -> bool {
        !name.is_empty()
            && !name.starts_with(['/', '\\'])
            && !name.contains([':', '\0'])
            && !name.split(['/', '\\']).any(|step| step == "..")
    }

    /// Every entry of the package `zip`, by its central directory, with
    /// the parts `inflate` names inflated. `Err` when it isn't a zip Kiln
    /// can read, when a name would lead outside the package or comes
    /// twice, or when the parts read would inflate past [`CAPS`].
    pub(super) fn read<R: Read + Seek>(
        zip: &mut R,
        inflate: &dyn Fn(&str) -> bool,
    ) -> Result<Vec<Part>, ()> {
        read_with(zip, inflate, CAPS)
    }

    /// [`read`] with caps of its own.
    pub(super) fn read_with<R: Read + Seek>(
        zip: &mut R,
        inflate: &dyn Fn(&str) -> bool,
        caps: Caps,
    ) -> Result<Vec<Part>, ()> {
        read_parts(zip, inflate, caps).ok_or(())
    }

    fn read_parts<R: Read + Seek>(
        zip: &mut R,
        inflate: &dyn Fn(&str) -> bool,
        caps: Caps,
    ) -> Option<Vec<Part>> {
        let len = zip.seek(SeekFrom::End(0)).ok()?;
        // The end record: 22 bytes and a comment of at most 64 KB.
        let tail_at = len.saturating_sub(22 + 0xFFFF);
        let mut tail = Vec::new();
        zip.seek(SeekFrom::Start(tail_at)).ok()?;
        zip.by_ref()
            .take(len - tail_at)
            .read_to_end(&mut tail)
            .ok()?;
        let end = (0..=tail.len().checked_sub(22)?)
            .rev()
            .find(|&i| u32_at(&tail, i) == Some(0x0605_4b50))?;
        let count = usize::from(u16_at(&tail, end + 10)?);
        let directory_len = u64::from(u32_at(&tail, end + 12)?);
        let directory_at = u64::from(u32_at(&tail, end + 16)?);
        if count == 0xFFFF || directory_at == 0xFFFF_FFFF || directory_len > DIRECTORY_CAP {
            return None;
        }
        if directory_at.checked_add(directory_len)? > len {
            return None;
        }
        let mut directory = Vec::new();
        zip.seek(SeekFrom::Start(directory_at)).ok()?;
        zip.by_ref()
            .take(directory_len)
            .read_to_end(&mut directory)
            .ok()?;

        let mut parts = Vec::with_capacity(count);
        let mut names = std::collections::HashSet::with_capacity(count);
        let mut inflated = 0u64;
        let mut at = 0usize;
        for _ in 0..count {
            if u32_at(&directory, at)? != 0x0201_4b50 {
                return None;
            }
            let flags = u16_at(&directory, at + 8)?;
            let method = u16_at(&directory, at + 10)?;
            let crc = u32_at(&directory, at + 16)?;
            let packed = u32_at(&directory, at + 20)?;
            let size = u32_at(&directory, at + 24)?;
            let name_len = usize::from(u16_at(&directory, at + 28)?);
            let extra = usize::from(u16_at(&directory, at + 30)?);
            let comment = usize::from(u16_at(&directory, at + 32)?);
            let local = u64::from(u32_at(&directory, at + 42)?);
            let name =
                String::from_utf8(directory.get(at + 46..at + 46 + name_len)?.to_vec()).ok()?;
            if flags & 1 != 0 || packed == 0xFFFF_FFFF || size == 0xFFFF_FFFF {
                return None;
            }
            if !matches!(method, 0 | 8) || !safe_name(&name) {
                return None;
            }
            // Part names are compared without case (OPC), and each comes once.
            if !names.insert(name.to_lowercase()) {
                return None;
            }
            let mut header = [0u8; 30];
            zip.seek(SeekFrom::Start(local)).ok()?;
            zip.read_exact(&mut header).ok()?;
            if u32_at(&header, 0)? != 0x0403_4b50 {
                return None;
            }
            let data_at =
                local + 30 + u64::from(u16_at(&header, 26)?) + u64::from(u16_at(&header, 28)?);
            if data_at.checked_add(u64::from(packed))? > len {
                return None;
            }
            let text = if inflate(&name) {
                let size64 = u64::from(size);
                inflated = inflated.checked_add(size64)?;
                if size64 > caps.part || inflated > caps.total {
                    return None;
                }
                zip.seek(SeekFrom::Start(data_at)).ok()?;
                let stored = zip.by_ref().take(u64::from(packed));
                // Never more than the cap, whatever the part says.
                let mut out = Vec::new();
                match method {
                    0 => stored.take(caps.part + 1).read_to_end(&mut out).ok()?,
                    _ => flate2::read::DeflateDecoder::new(stored)
                        .take(caps.part + 1)
                        .read_to_end(&mut out)
                        .ok()?,
                };
                let mut check = flate2::Crc::new();
                check.update(&out);
                if out.len() as u64 != size64 || check.sum() != crc {
                    return None;
                }
                Some(out)
            } else {
                None
            };
            parts.push(Part {
                name,
                text,
                changed: false,
                method,
                crc,
                packed,
                size,
                data_at,
            });
            at += 46 + name_len + extra + comment;
        }
        Some(parts)
    }

    /// Write a package of `parts` to `out`: each changed part deflated
    /// from its text, every other part copied as stored from `zip`, the
    /// package it was read from.
    pub(super) fn write<R: Read + Seek, W: Write>(
        zip: &mut R,
        parts: &[Part],
        out: &mut W,
    ) -> io::Result<()> {
        let too_big = || io::Error::new(io::ErrorKind::InvalidData, "the package is too big");
        let mut written = 0u64;
        let mut central = Vec::new();
        for part in parts {
            let (method, crc, packed, size, deflated) = match part.text.as_ref() {
                Some(bytes) if part.changed => {
                    let mut crc = flate2::Crc::new();
                    crc.update(bytes);
                    let mut deflate = flate2::write::DeflateEncoder::new(
                        Vec::new(),
                        flate2::Compression::default(),
                    );
                    deflate.write_all(bytes)?;
                    let packed = deflate.finish()?;
                    let len = u32::try_from(packed.len()).map_err(|_| too_big())?;
                    let size = u32::try_from(bytes.len()).map_err(|_| too_big())?;
                    (8u16, crc.sum(), len, size, Some(packed))
                }
                _ => (part.method, part.crc, part.packed, part.size, None),
            };
            let offset = u32::try_from(written).map_err(|_| too_big())?;
            // Shared by the local header and the central record: version
            // needed, flags (UTF-8 names), method, time, date, CRC, sizes,
            // name length.
            let mut common = Vec::new();
            common.extend_from_slice(&20u16.to_le_bytes());
            common.extend_from_slice(&0x0800u16.to_le_bytes());
            common.extend_from_slice(&method.to_le_bytes());
            common.extend_from_slice(&0u16.to_le_bytes());
            common.extend_from_slice(&0x0021u16.to_le_bytes());
            common.extend_from_slice(&crc.to_le_bytes());
            common.extend_from_slice(&packed.to_le_bytes());
            common.extend_from_slice(&size.to_le_bytes());
            let name_len = u16::try_from(part.name.len()).map_err(|_| too_big())?;
            common.extend_from_slice(&name_len.to_le_bytes());
            let mut header = Vec::with_capacity(32 + part.name.len());
            header.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            header.extend_from_slice(&common);
            header.extend_from_slice(&0u16.to_le_bytes());
            header.extend_from_slice(part.name.as_bytes());
            out.write_all(&header)?;
            match deflated {
                Some(bytes) => out.write_all(&bytes)?,
                None => {
                    zip.seek(SeekFrom::Start(part.data_at))?;
                    let copied = io::copy(&mut zip.by_ref().take(u64::from(packed)), out)?;
                    if copied != u64::from(packed) {
                        return Err(io::ErrorKind::UnexpectedEof.into());
                    }
                }
            }
            written += header.len() as u64 + u64::from(packed);
            central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&common);
            // Extra, comment, disk, internal and external attributes.
            central.extend_from_slice(&[0u8; 12]);
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(part.name.as_bytes());
        }
        let central_at = u32::try_from(written).map_err(|_| too_big())?;
        let count = u16::try_from(parts.len()).map_err(|_| too_big())?;
        let central_len = u32::try_from(central.len()).map_err(|_| too_big())?;
        let mut end = Vec::with_capacity(22);
        end.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        end.extend_from_slice(&[0u8; 4]);
        end.extend_from_slice(&count.to_le_bytes());
        end.extend_from_slice(&count.to_le_bytes());
        end.extend_from_slice(&central_len.to_le_bytes());
        end.extend_from_slice(&central_at.to_le_bytes());
        end.extend_from_slice(&0u16.to_le_bytes());
        out.write_all(&central)?;
        out.write_all(&end)
    }
}

/// The process ids of `app`'s running programs.
pub fn running(app: App) -> Vec<u32> {
    com::running(app)
}

pub struct OfficeConverter;

impl Converter for OfficeConverter {
    fn id(&self) -> &'static str {
        "office"
    }

    fn convert(&self, req: &Request<'_>, ctx: &mut Context<'_>) -> Result<()> {
        let app = App::for_ext(&req.from.ext).filter(|_| req.to.ext == "pdf");
        let Some(app) = app else {
            return Err(Error::Registry(format!(
                "{} to {} uses Microsoft Office, which only makes PDFs of the files it opens.",
                req.from.code(),
                req.to.code()
            )));
        };
        (ctx.progress)(Progress::Indeterminate);
        to_pdf(app, req.input, req.output, ctx.cancel, TIMEOUT)
    }
}

#[cfg(windows)]
pub use com::{Apartment, Object, Value};

/// Office's automation, through COM. Every call Kiln makes on Office goes
/// through [`Object`]; the `unsafe` stays in here.
#[cfg(windows)]
mod com {
    use std::mem::ManuallyDrop;
    use std::path::Path;
    use std::time::{Duration, Instant};

    use windows::core::{IUnknown, BSTR, GUID, PCWSTR};
    use windows::Win32::Foundation::{
        DISP_E_EXCEPTION, DISP_E_PARAMNOTFOUND, RPC_E_CALL_REJECTED, RPC_E_SERVERCALL_RETRYLATER,
        VARIANT_FALSE, VARIANT_TRUE,
    };
    use windows::Win32::System::Com::{
        CLSIDFromProgID, CoCreateInstance, CoInitializeEx, CoUninitialize, IDispatch,
        CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, DISPATCH_FLAGS,
        DISPATCH_METHOD, DISPATCH_PROPERTYGET, DISPATCH_PROPERTYPUT, DISPPARAMS, EXCEPINFO,
    };
    use windows::Win32::System::Ole::{GetActiveObject, DISPID_PROPERTYPUT};
    use windows::Win32::System::Variant::{
        VariantClear, VARIANT, VT_BOOL, VT_BSTR, VT_DISPATCH, VT_EMPTY, VT_ERROR, VT_I2, VT_I4,
        VT_INT, VT_UI1,
    };

    use std::sync::atomic::{AtomicBool, Ordering};

    use super::{
        error_of, export, powerpoint_in_use, quit, settle, App, Arg, Automation, Fault, EXIT_WAIT,
        OURS,
    };
    use crate::error::{Error, Result};

    const LOCALE_USER_DEFAULT: u32 = 0x0400;

    /// How long a call Office turns away while it is busy is tried again.
    const BUSY_FOR: Duration = Duration::from_secs(30);

    /// COM on this thread, as a single-threaded apartment, while it lives.
    pub struct Apartment(bool);

    impl Apartment {
        #[allow(clippy::new_without_default)]
        pub fn new() -> Self {
            // SAFETY: a plain call on this thread; a success is balanced by
            // CoUninitialize when this drops, on the same thread (the type
            // isn't Send).
            let ok =
                unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) }
                    .is_ok();
            Apartment(ok)
        }
    }

    impl Drop for Apartment {
        fn drop(&mut self) {
            if self.0 {
                // SAFETY: balances the CoInitializeEx above.
                unsafe { CoUninitialize() };
            }
        }
    }

    /// What an automation call gave back.
    #[derive(Debug, Clone)]
    pub enum Value {
        Empty,
        Bool(bool),
        Int(i32),
        Str(String),
        Object(Object),
        /// A type Kiln doesn't read.
        Other,
    }

    /// An Office object: an application, a collection, a document.
    #[derive(Debug, Clone)]
    pub struct Object(IDispatch);

    impl Object {
        /// A new instance of the app `prog_id` names, started for the
        /// caller. Kiln's jobs start theirs in [`super::to_pdf`]; this is
        /// for tests that need an Office of their own.
        pub fn create(prog_id: &str) -> windows::core::Result<Object> {
            let class = class_of(prog_id)?;
            // SAFETY: a registered class id, no outer object; COM is
            // initialised on this thread by the caller's Apartment.
            let d: IDispatch = unsafe { CoCreateInstance(&class, None, CLSCTX_LOCAL_SERVER) }?;
            Ok(Object(d))
        }

        pub fn get(&self, name: &str) -> std::result::Result<Value, Fault> {
            invoke(&self.0, name, DISPATCH_PROPERTYGET, &[])
        }

        pub fn put(&self, name: &str, value: Arg<'_>) -> std::result::Result<(), Fault> {
            invoke(&self.0, name, DISPATCH_PROPERTYPUT, &[value]).map(|_| ())
        }

        pub fn call(&self, name: &str, args: &[Arg<'_>]) -> std::result::Result<Value, Fault> {
            invoke(&self.0, name, DISPATCH_METHOD | DISPATCH_PROPERTYGET, args)
        }
    }

    /// [`Automation`] through [`Object`].
    struct Office;

    impl Automation for Office {
        type Obj = Object;

        fn object(&self, of: &Object, name: &str) -> std::result::Result<Object, Fault> {
            match of.get(name)? {
                Value::Object(o) => Ok(o),
                v => Err(Fault {
                    code: 0,
                    text: format!("{name} gave {v:?}, not an object."),
                }),
            }
        }

        fn number(&self, of: &Object, name: &str) -> std::result::Result<i32, Fault> {
            match of.get(name)? {
                Value::Int(n) => Ok(n),
                Value::Bool(b) => Ok(i32::from(b)),
                v => Err(Fault {
                    code: 0,
                    text: format!("{name} gave {v:?}, not a number."),
                }),
            }
        }

        fn put(&self, of: &Object, name: &str, value: Arg<'_>) -> std::result::Result<(), Fault> {
            of.put(name, value)
        }

        fn call(
            &self,
            of: &Object,
            name: &str,
            args: &[Arg<'_>],
        ) -> std::result::Result<Option<Object>, Fault> {
            Ok(match of.call(name, args)? {
                Value::Object(o) => Some(o),
                _ => None,
            })
        }
    }

    /// Make the PDF of `input` (Kiln's copy) at `output` in an instance of
    /// `app` that Kiln starts for it, then let that instance go.
    pub(super) fn run(app: App, input: &Path, output: &Path, gone: &AtomicBool) -> Result<()> {
        let _apartment = Apartment::new();
        let failed = |summary: &str, e: windows::core::Error| Error::EngineFailed {
            engine: app.name(),
            summary: summary.to_owned(),
            details: e.to_string(),
        };
        let class = class_of(app.prog_id()).map_err(|e| failed("it isn't installed.", e))?;
        let ours = &OURS[app as usize];
        let ours_now = || ours.lock().unwrap_or_else(|e| e.into_inner());
        // PowerPoint runs once per person: asking for a new one while
        // someone's is open would hand Kiln theirs (decision 4). Kiln's own
        // from the job before may still be on its way out.
        if app == App::PowerPoint {
            let mut mine = ours_now().clone();
            let in_use =
                powerpoint_in_use(&mut mine, &|| running(app), &|| active(&class), EXIT_WAIT);
            *ours_now() = mine;
            if in_use {
                return Err(Error::OfficeInUse { app: app.name() });
            }
        }
        // Kiln stopped waiting for this job: nothing is started.
        if gone.load(Ordering::SeqCst) {
            return Err(Error::Cancelled);
        }
        let before = running(app);
        // SAFETY: as in Object::create; the apartment lives until the end.
        let created: windows::core::Result<IDispatch> =
            unsafe { CoCreateInstance(&class, None, CLSCTX_LOCAL_SERVER) };
        let root = Object(created.map_err(|e| failed("it didn't start.", e))?);
        // Word and Excel start a new program for every caller. If none
        // started, Kiln was handed someone's own: it is let go untouched.
        let started: Vec<u32> = running(app)
            .into_iter()
            .filter(|pid| !before.contains(pid))
            .collect();
        if started.is_empty() {
            return Err(Error::OfficeInUse { app: app.name() });
        }
        ours_now().extend(&started);
        let office = Office;
        let done = export(
            &office,
            app,
            &root,
            &input.to_string_lossy(),
            &output.to_string_lossy(),
        );
        let asked = quit(&office, app, &root);
        // Office exits once Kiln lets its instance go. The next job of this
        // app waits for that, so it doesn't take Kiln's own leaving
        // program for someone else's.
        drop(root);
        if asked {
            let mut mine = ours_now().clone();
            settle(&mut mine, &|| running(app), EXIT_WAIT);
            *ours_now() = mine;
        }
        done.map_err(|f| error_of(app, f))
    }

    /// The process ids of `app`'s running programs.
    pub(super) fn running(app: App) -> Vec<u32> {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        };
        let mut pids = vec![];
        // SAFETY: a snapshot of every process; its handle is closed below.
        let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snap == INVALID_HANDLE_VALUE || snap.is_null() {
            return pids;
        }
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        // SAFETY: `entry` is ours, with its size set as the call asks.
        let mut more = unsafe { Process32FirstW(snap, &mut entry) } != 0;
        while more {
            let name = &entry.szExeFile;
            let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
            if String::from_utf16_lossy(&name[..len]).eq_ignore_ascii_case(app.exe()) {
                pids.push(entry.th32ProcessID);
            }
            // SAFETY: as above.
            more = unsafe { Process32NextW(snap, &mut entry) } != 0;
        }
        // SAFETY: the handle came from CreateToolhelp32Snapshot and is
        // closed once.
        unsafe { CloseHandle(snap) };
        pids
    }

    fn class_of(prog_id: &str) -> windows::core::Result<GUID> {
        let wide: Vec<u16> = prog_id.encode_utf16().chain(Some(0)).collect();
        // SAFETY: a NUL-ended name that outlives the call.
        unsafe { CLSIDFromProgID(PCWSTR(wide.as_ptr())) }
    }

    /// Whether an instance of `class` is running and listed for others to
    /// use. Nothing on it is touched.
    fn active(class: &GUID) -> bool {
        let mut found: Option<IUnknown> = None;
        // SAFETY: `found` takes the object, released when it drops.
        let r = unsafe { GetActiveObject(class, None, &mut found) };
        r.is_ok() && found.is_some()
    }

    /// Call `name` on `obj`, trying again while Office says it is busy.
    fn invoke(
        obj: &IDispatch,
        name: &str,
        flags: DISPATCH_FLAGS,
        args: &[Arg<'_>],
    ) -> std::result::Result<Value, Fault> {
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let names = [PCWSTR(wide.as_ptr())];
        let mut id = 0i32;
        // SAFETY: one NUL-ended name that outlives the call, and room for
        // one id.
        unsafe {
            obj.GetIDsOfNames(
                &GUID::zeroed(),
                names.as_ptr(),
                1,
                LOCALE_USER_DEFAULT,
                &mut id,
            )
        }
        .map_err(|e| Fault {
            code: e.code().0,
            text: format!("{name}: {}", e.message()),
        })?;
        // Arguments are passed last to first.
        let mut vars: Vec<VARIANT> = args.iter().rev().map(variant_of).collect();
        let put = flags == DISPATCH_PROPERTYPUT;
        let mut named = DISPID_PROPERTYPUT;
        let params = DISPPARAMS {
            rgvarg: if vars.is_empty() {
                std::ptr::null_mut()
            } else {
                vars.as_mut_ptr()
            },
            rgdispidNamedArgs: if put {
                &mut named
            } else {
                std::ptr::null_mut()
            },
            cArgs: vars.len() as u32,
            cNamedArgs: u32::from(put),
        };
        let deadline = Instant::now() + BUSY_FOR;
        let answer = loop {
            let mut result = VARIANT::default();
            let mut excep = EXCEPINFO::default();
            let mut arg_err = 0u32;
            // SAFETY: `params` points into `vars` and `named`, which
            // outlive the call; `result`, `excep` and `arg_err` are owned
            // here, and what Office puts in them is freed below.
            let r = unsafe {
                obj.Invoke(
                    id,
                    &GUID::zeroed(),
                    LOCALE_USER_DEFAULT,
                    flags,
                    &params,
                    Some(&mut result),
                    Some(&mut excep),
                    Some(&mut arg_err),
                )
            };
            match r {
                Ok(()) => {
                    let v = value_of(&result);
                    clear(&mut result);
                    break Ok(v);
                }
                Err(e) => {
                    let fault = fault_of(&e, &mut excep);
                    clear(&mut result);
                    let busy =
                        e.code() == RPC_E_CALL_REJECTED || e.code() == RPC_E_SERVERCALL_RETRYLATER;
                    if busy && Instant::now() < deadline {
                        std::thread::sleep(Duration::from_millis(100));
                        continue;
                    }
                    break Err(fault);
                }
            }
        };
        for v in &mut vars {
            clear(v);
        }
        answer
    }

    /// The fault an `Invoke` failed with, Office's own words when it gave
    /// them. Frees the strings in `excep`.
    fn fault_of(e: &windows::core::Error, excep: &mut EXCEPINFO) -> Fault {
        if e.code() == DISP_E_EXCEPTION {
            if let Some(fill) = excep.pfnDeferredFillIn {
                // SAFETY: Office asked for this to be called to fill
                // `excep` in.
                let _ = unsafe { fill(excep) };
            }
        }
        // SAFETY: each string is Office's, or null when it gave none, and
        // is taken (so freed) once.
        let (source, text, help) = unsafe {
            (
                ManuallyDrop::take(&mut excep.bstrSource),
                ManuallyDrop::take(&mut excep.bstrDescription),
                ManuallyDrop::take(&mut excep.bstrHelpFile),
            )
        };
        drop((source, help));
        if e.code() == DISP_E_EXCEPTION {
            let code = if excep.scode != 0 {
                excep.scode
            } else {
                i32::from(excep.wCode)
            };
            Fault {
                code,
                text: text.to_string(),
            }
        } else {
            Fault {
                code: e.code().0,
                text: e.message(),
            }
        }
    }

    fn variant_of(arg: &Arg<'_>) -> VARIANT {
        let mut v = VARIANT::default();
        // SAFETY: writes the type tag and the member it names into a zeroed
        // VARIANT; a string is freed by `clear`.
        unsafe {
            let inner = &mut *v.Anonymous.Anonymous;
            match arg {
                Arg::Missing => {
                    inner.vt = VT_ERROR;
                    inner.Anonymous.scode = DISP_E_PARAMNOTFOUND.0;
                }
                Arg::Bool(b) => {
                    inner.vt = VT_BOOL;
                    inner.Anonymous.boolVal = if *b { VARIANT_TRUE } else { VARIANT_FALSE };
                }
                Arg::Int(n) => {
                    inner.vt = VT_I4;
                    inner.Anonymous.lVal = *n;
                }
                Arg::Str(s) => {
                    inner.vt = VT_BSTR;
                    inner.Anonymous.bstrVal = ManuallyDrop::new(BSTR::from(*s));
                }
            }
        }
        v
    }

    fn value_of(v: &VARIANT) -> Value {
        // SAFETY: reads only the member the type tag names; an object is
        // cloned (AddRef) before `clear` releases the VARIANT's own.
        unsafe {
            let inner = &*v.Anonymous.Anonymous;
            match inner.vt {
                VT_EMPTY => Value::Empty,
                VT_BOOL => Value::Bool(inner.Anonymous.boolVal.0 != 0),
                VT_I4 | VT_INT => Value::Int(inner.Anonymous.lVal),
                VT_I2 => Value::Int(i32::from(inner.Anonymous.iVal)),
                VT_UI1 => Value::Int(i32::from(inner.Anonymous.bVal)),
                VT_BSTR => Value::Str(inner.Anonymous.bstrVal.to_string()),
                VT_DISPATCH => match &*inner.Anonymous.pdispVal {
                    Some(d) => Value::Object(Object(d.clone())),
                    None => Value::Empty,
                },
                _ => Value::Other,
            }
        }
    }

    fn clear(v: &mut VARIANT) {
        // SAFETY: `v` was zeroed or filled by Office or `variant_of`.
        let _ = unsafe { VariantClear(v) };
    }
}

/// Without Windows there is no Microsoft Office to drive.
#[cfg(not(windows))]
mod com {
    use std::path::Path;

    use super::App;
    use crate::error::{Error, Result};

    pub(super) fn run(_: App, _: &Path, _: &Path, _: &std::sync::atomic::AtomicBool) -> Result<()> {
        Err(Error::EngineMissing {
            engine: "Microsoft Office",
        })
    }

    pub(super) fn running(_: App) -> Vec<u32> {
        vec![]
    }
}

#[cfg(test)]
/// Tests: an OLE compound file (version 3, 512-byte sectors) holding empty
/// streams named `streams`: one FAT sector, then the directory.
pub(crate) fn compound_file(streams: &[&str]) -> Vec<u8> {
    let mut names = vec![("Root Entry", 5u8)];
    names.extend(streams.iter().map(|n| (*n, 2u8)));
    let dir_sectors = names.len().div_ceil(4);
    let mut out = vec![0u8; 512];
    out[..8].copy_from_slice(b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1");
    out[24..26].copy_from_slice(&0x3Eu16.to_le_bytes());
    out[26..28].copy_from_slice(&3u16.to_le_bytes());
    out[28..30].copy_from_slice(&0xFFFEu16.to_le_bytes());
    out[30..32].copy_from_slice(&9u16.to_le_bytes());
    out[32..34].copy_from_slice(&6u16.to_le_bytes());
    out[44..48].copy_from_slice(&1u32.to_le_bytes());
    out[48..52].copy_from_slice(&1u32.to_le_bytes());
    out[56..60].copy_from_slice(&4096u32.to_le_bytes());
    out[60..64].copy_from_slice(&0xFFFF_FFFEu32.to_le_bytes());
    out[68..72].copy_from_slice(&0xFFFF_FFFEu32.to_le_bytes());
    for i in 0..109 {
        let v: u32 = if i == 0 { 0 } else { 0xFFFF_FFFF };
        out[76 + i * 4..80 + i * 4].copy_from_slice(&v.to_le_bytes());
    }
    // The FAT: itself, then the directory's chain.
    let mut fat = vec![0xFFFF_FFFFu32; 128];
    fat[0] = 0xFFFF_FFFD;
    for (s, next) in fat.iter_mut().enumerate().take(dir_sectors + 1).skip(1) {
        *next = if s == dir_sectors {
            0xFFFF_FFFE
        } else {
            s as u32 + 1
        };
    }
    for v in fat {
        out.extend_from_slice(&v.to_le_bytes());
    }
    let mut dir = vec![0u8; dir_sectors * 512];
    for (i, (name, kind)) in names.iter().enumerate() {
        let e = &mut dir[i * 128..(i + 1) * 128];
        let wide: Vec<u16> = name.encode_utf16().collect();
        for (j, c) in wide.iter().enumerate() {
            e[j * 2..j * 2 + 2].copy_from_slice(&c.to_le_bytes());
        }
        e[64..66].copy_from_slice(&(((wide.len() + 1) * 2) as u16).to_le_bytes());
        e[66] = *kind;
    }
    out.extend_from_slice(&dir);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    #[test]
    fn each_office_file_goes_to_its_own_app() {
        for ext in ["doc", "docx", "odt", "rtf", "txt"] {
            assert_eq!(App::for_ext(ext), Some(App::Word), "{ext}");
        }
        for ext in ["xlsx", "xls", "ods", "csv"] {
            assert_eq!(App::for_ext(ext), Some(App::Excel), "{ext}");
        }
        for ext in ["pptx", "ppt", "odp"] {
            assert_eq!(App::for_ext(ext), Some(App::PowerPoint), "{ext}");
        }
        for ext in ["pdf", "pages", "key", "numbers", "md", "docm"] {
            assert_eq!(App::for_ext(ext), None, "{ext}");
        }
    }

    #[test]
    fn a_server_value_gives_its_program() {
        let word = r"C:\Program Files\Microsoft Office\Root\Office16\WINWORD.EXE";
        assert_eq!(
            server_path(&format!("{word} /Automation")),
            Some(PathBuf::from(word))
        );
        assert_eq!(
            server_path(&format!("\"{word}\" /Automation")),
            Some(PathBuf::from(word))
        );
        assert_eq!(
            server_path(&format!("  {word}  ")),
            Some(PathBuf::from(word))
        );
        let lower = r"C:\Office\excel.exe /automation";
        assert_eq!(
            server_path(lower),
            Some(PathBuf::from(r"C:\Office\excel.exe"))
        );
        assert_eq!(server_path(""), None);
        assert_eq!(server_path("\"\""), None);
    }

    #[test]
    fn office_is_found_through_its_progids() {
        let folder = r"C:\Program Files\Microsoft Office\root\Office16";
        let keys: HashMap<String, String> = [
            (
                r"Word.Application\CLSID",
                "{000209FF-0000-0000-C000-000000000046}",
            ),
            (
                r"CLSID\{000209FF-0000-0000-C000-000000000046}\LocalServer32",
                r"C:\Program Files\Microsoft Office\root\Office16\WINWORD.EXE /Automation",
            ),
            (
                r"Excel.Application\CLSID",
                "{00024500-0000-0000-C000-000000000046}",
            ),
            (
                r"CLSID\{00024500-0000-0000-C000-000000000046}\LocalServer32",
                r#""C:\Program Files\Microsoft Office\root\Office16\EXCEL.EXE" /automation"#,
            ),
            // PowerPoint's class is registered, but its program is gone.
            (
                r"PowerPoint.Application\CLSID",
                "{91493441-5A91-11CF-8700-00AA0060263B}",
            ),
            (
                r"CLSID\{91493441-5A91-11CF-8700-00AA0060263B}\LocalServer32",
                r"C:\Gone\POWERPNT.EXE /AUTOMATION",
            ),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
        let read = |k: &str| keys.get(k).cloned();
        let exists = |p: &Path| p.starts_with(folder);
        let found = find_installed_with(&read, &exists);
        assert_eq!(
            found.word.as_deref(),
            Some(Path::new(folder).join("WINWORD.EXE").as_path())
        );
        assert_eq!(
            found.excel.as_deref(),
            Some(Path::new(folder).join("EXCEL.EXE").as_path())
        );
        assert_eq!(found.powerpoint, None);
        assert_eq!(found.apps(), [App::Word, App::Excel]);
        assert_eq!(found.folder(), Some(Path::new(folder)));
        assert!(found.any());

        // Nothing registered: nothing found.
        let none = find_installed_with(&|_| None, &|_| true);
        assert_eq!(none, Installed::default());
        assert!(!none.any());
        assert_eq!(none.folder(), None);
    }

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures")
            .join(name)
    }

    #[test]
    fn a_password_is_found_in_the_file_not_guessed() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, bytes: &[u8]| {
            let path = dir.path().join(name);
            fs::write(&path, bytes).unwrap();
            look(&path).unwrap()
        };
        // An Office Open XML file with a password, under any name.
        let encrypted = compound_file(&["EncryptedInfo", "EncryptedPackage"]);
        for name in ["locked.docx", "locked.xlsx", "locked.pptx", "locked.doc"] {
            let seen = write(name, &encrypted);
            assert_eq!(
                (seen.content, seen.password),
                (Content::Compound, true),
                "{name}"
            );
        }
        // A PowerPoint 97 file with a password.
        let ppt = write(
            "locked.ppt",
            &compound_file(&["PowerPoint Document", "EncryptedSummary"]),
        );
        assert!(ppt.password);
        // A Word 97 file is a compound file, with no password.
        let doc = write("old.docx", &compound_file(&["WordDocument", "1Table"]));
        assert_eq!((doc.content, doc.password), (Content::Compound, false));
        let bare = write("bare.docx", b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1rest");
        assert_eq!((bare.content, bare.password), (Content::Compound, false));
        // An OpenDocument file with a password says so in its manifest.
        let manifest = |extra: &str| {
            let text = format!(
                r#"<manifest:manifest><manifest:file-entry manifest:full-path="content.xml">{extra}</manifest:file-entry></manifest:manifest>"#
            );
            zip_of(&[
                (
                    "mimetype",
                    "application/vnd.oasis.opendocument.presentation",
                ),
                ("META-INF/manifest.xml", &text),
            ])
        };
        let odp = write(
            "locked.odp",
            &manifest(r#"<manifest:encryption-data manifest:checksum-type="SHA1"/>"#),
        );
        assert_eq!((odp.content, odp.password), (Content::OtherZip, true));
        assert!(!write("open.odp", &manifest("")).password);
        // The real samples: none has a password, each is what its name says.
        for name in [
            "sample.doc",
            "sample.xls",
            "sample.ppt",
            "sample.docx",
            "sample.xlsx",
            "sample.pptx",
            "sample.odt",
            "sample.ods",
            "sample.odp",
            "sample.rtf",
            "sample.txt",
            "sample.csv",
        ] {
            let path = fixture(name);
            let seen = look(&path).unwrap();
            let ext = path.extension().unwrap().to_str().unwrap();
            assert!(!seen.password, "{name}");
            assert!(accepts(ext, seen.content), "{name}: {:?}", seen.content);
        }
        let mut doc = fs::File::open(fixture("sample.doc")).unwrap();
        let streams = compound_streams(&mut doc).unwrap();
        assert!(streams.iter().any(|s| s == "WordDocument"), "{streams:?}");
    }

    /// Office as a list of what was asked of it. Objects are named by
    /// how they were reached ("app.Documents"); an opened file is "file".
    #[derive(Default)]
    struct Fake {
        log: RefCell<Vec<String>>,
        fail: HashMap<&'static str, Fault>,
        open: i32,
        visible: i32,
    }

    impl Fake {
        fn log(&self) -> Vec<String> {
            self.log.borrow().clone()
        }

        fn failing(&self, name: &str) -> Option<Fault> {
            self.fail.get(name).cloned()
        }
    }

    impl Automation for Fake {
        type Obj = String;

        fn object(&self, of: &String, name: &str) -> std::result::Result<String, Fault> {
            self.log.borrow_mut().push(format!("get {of}.{name}"));
            Ok(format!("{of}.{name}"))
        }

        fn number(&self, of: &String, name: &str) -> std::result::Result<i32, Fault> {
            self.log.borrow_mut().push(format!("get {of}.{name}"));
            Ok(if name == "Count" {
                self.open
            } else {
                self.visible
            })
        }

        fn put(&self, of: &String, name: &str, value: Arg<'_>) -> std::result::Result<(), Fault> {
            self.log
                .borrow_mut()
                .push(format!("put {of}.{name} = {value:?}"));
            Ok(())
        }

        fn call(
            &self,
            of: &String,
            name: &str,
            args: &[Arg<'_>],
        ) -> std::result::Result<Option<String>, Fault> {
            self.log
                .borrow_mut()
                .push(format!("call {of}.{name}{args:?}"));
            if let Some(f) = self.failing(name) {
                return Err(f);
            }
            Ok((name == "Open").then(|| "file".to_owned()))
        }
    }

    fn app() -> String {
        "app".to_owned()
    }

    #[test]
    fn macros_are_off_before_anything_is_opened() {
        for app_ in App::ALL {
            let fake = Fake::default();
            export(&fake, app_, &app(), r"C:\job\in.x", r"C:\job\out.pdf").unwrap();
            let log = fake.log();
            assert_eq!(
                log[0], "put app.AutomationSecurity = Int(3)",
                "{app_:?}: {log:#?}"
            );
            let open = log.iter().position(|l| l.contains(".Open[")).unwrap();
            assert!(open > 0, "{log:#?}");
            // Alerts are off before the file opens.
            let alerts = log
                .iter()
                .position(|l| l.contains("DisplayAlerts"))
                .unwrap();
            assert!(alerts < open, "{log:#?}");
        }
    }

    #[test]
    fn word_opens_a_copy_read_only_with_no_recent_entry_or_prompt() {
        let fake = Fake::default();
        export(
            &fake,
            App::Word,
            &app(),
            r"C:\job\in.docx",
            r"C:\job\out.pdf",
        )
        .unwrap();
        let log = fake.log();
        assert!(
            log.contains(&"put app.Visible = Bool(false)".to_owned()),
            "{log:#?}"
        );
        assert!(
            log.contains(&"put app.DisplayAlerts = Int(0)".to_owned()),
            "{log:#?}"
        );
        let args = open_args(App::Word, r"C:\job\in.docx");
        assert_eq!(args[0], Arg::Str(r"C:\job\in.docx"));
        assert_eq!(args[1], Arg::Bool(false), "ConfirmConversions");
        assert_eq!(args[2], Arg::Bool(true), "ReadOnly");
        assert_eq!(args[3], Arg::Bool(false), "AddToRecentFiles");
        assert_eq!(args[4], Arg::Str(NO_PASSWORD), "PasswordDocument");
        assert_eq!(args[11], Arg::Bool(false), "Visible");
        assert_eq!(args[12], Arg::Bool(false), "OpenAndRepair");
        assert_eq!(args[14], Arg::Bool(true), "NoEncodingDialog");
        let want = [
            format!("call app.Documents.Open{args:?}"),
            format!(
                "call file.ExportAsFixedFormat{:?}",
                [Arg::Str(r"C:\job\out.pdf"), Arg::Int(17), Arg::Bool(false)]
            ),
            // wdDoNotSaveChanges.
            format!("call file.Close{:?}", [Arg::Int(0)]),
        ];
        let calls: Vec<&String> = log.iter().filter(|l| l.starts_with("call")).collect();
        assert_eq!(calls, want.iter().collect::<Vec<_>>());
    }

    #[test]
    fn excel_opens_without_updating_links_and_exports_the_whole_workbook() {
        let fake = Fake::default();
        export(
            &fake,
            App::Excel,
            &app(),
            r"C:\job\in.xlsx",
            r"C:\job\out.pdf",
        )
        .unwrap();
        let log = fake.log();
        assert!(
            log.contains(&"put app.Visible = Bool(false)".to_owned()),
            "{log:#?}"
        );
        assert!(
            log.contains(&"put app.DisplayAlerts = Bool(false)".to_owned()),
            "{log:#?}"
        );
        let args = open_args(App::Excel, r"C:\job\in.xlsx");
        assert_eq!(args[1], Arg::Int(0), "UpdateLinks");
        assert_eq!(args[2], Arg::Bool(true), "ReadOnly");
        assert_eq!(args[4], Arg::Str(NO_PASSWORD), "Password");
        assert_eq!(args[6], Arg::Bool(true), "IgnoreReadOnlyRecommended");
        assert_eq!(args[9], Arg::Bool(false), "Editable");
        assert_eq!(args[10], Arg::Bool(false), "Notify");
        assert_eq!(args[12], Arg::Bool(false), "AddToMru");
        assert_eq!(args[14], Arg::Int(0), "CorruptLoad: no repair");
        // The workbook, so every sheet, as xlTypePDF; not opened after.
        let export = log
            .iter()
            .find(|l| l.contains("ExportAsFixedFormat"))
            .unwrap();
        assert!(
            export.starts_with("call file.ExportAsFixedFormat"),
            "{export}"
        );
        assert!(
            export.contains("[Int(0), Str(\"C:\\\\job\\\\out.pdf\")"),
            "{export}"
        );
        assert!(export.ends_with("Bool(false)]"), "{export}");
        assert_eq!(
            log.last().unwrap(),
            &format!("call file.Close{:?}", [Arg::Bool(false)])
        );
    }

    #[test]
    fn powerpoint_opens_without_a_window_and_saves_a_pdf() {
        let fake = Fake::default();
        export(
            &fake,
            App::PowerPoint,
            &app(),
            r"C:\job\in.pptx",
            r"C:\job\out.pdf",
        )
        .unwrap();
        let log = fake.log();
        // PowerPoint can't be hidden; its file opens without a window.
        assert!(!log.iter().any(|l| l.contains(".Visible")), "{log:#?}");
        assert!(
            log.contains(&"put app.DisplayAlerts = Int(1)".to_owned()),
            "{log:#?}"
        );
        let args = open_args(App::PowerPoint, r"C:\job\in.pptx");
        assert_eq!(args[1], Arg::Int(-1), "ReadOnly");
        assert_eq!(args[2], Arg::Int(0), "Untitled");
        assert_eq!(args[3], Arg::Int(0), "WithWindow");
        assert!(log.contains(&format!(
            "call file.SaveAs{:?}",
            [Arg::Str(r"C:\job\out.pdf"), Arg::Int(32)]
        )));
        assert_eq!(log.last().unwrap(), "call file.Close[]");
    }

    #[test]
    fn the_file_is_closed_unsaved_when_its_pdf_fails() {
        for (app_, save) in [
            (App::Word, "ExportAsFixedFormat"),
            (App::Excel, "ExportAsFixedFormat"),
            (App::PowerPoint, "SaveAs"),
        ] {
            let mut fake = Fake::default();
            let fault = Fault {
                code: 1,
                text: "The printer is out of paper.".to_owned(),
            };
            fake.fail.insert(save, fault.clone());
            let r = export(&fake, app_, &app(), "in", "out");
            assert_eq!(r, Err(Failure::Other(fault)), "{app_:?}");
            let log = fake.log();
            assert!(
                log.last().unwrap().starts_with("call file.Close"),
                "{log:#?}"
            );
            // Nothing is ever saved over the file.
            assert!(!log.iter().any(|l| l.contains(".Save[")), "{log:#?}");
        }
    }

    #[test]
    fn a_file_that_doesnt_open_is_an_open_failure() {
        let mut fake = Fake::default();
        let fault = Fault {
            code: WORD_BAD_PASSWORD,
            text: "The password is incorrect.".to_owned(),
        };
        fake.fail.insert("Open", fault.clone());
        let r = export(&fake, App::Word, &app(), "in", "out");
        assert_eq!(r, Err(Failure::Open(fault)));
        assert!(!fake.log().iter().any(|l| l.contains("Close")));
    }

    #[test]
    fn only_an_idle_hidden_instance_is_asked_to_quit() {
        for (open, visible, quits) in [(0, 0, true), (1, 0, false), (0, -1, false), (0, 1, false)] {
            for app_ in App::ALL {
                let fake = Fake {
                    open,
                    visible,
                    ..Fake::default()
                };
                quit(&fake, app_, &app());
                let asked = fake.log().iter().any(|l| l.starts_with("call app.Quit"));
                assert_eq!(asked, quits, "{app_:?} open {open} visible {visible}");
            }
        }
        // Word is told not to save anything as it goes.
        let fake = Fake::default();
        quit(&fake, App::Word, &app());
        assert!(fake
            .log()
            .contains(&format!("call app.Quit{:?}", [Arg::Int(0)])));
    }

    #[test]
    fn office_failures_read_plainly() {
        let password = Fault {
            code: WORD_BAD_PASSWORD,
            text: "The password is incorrect.".to_owned(),
        };
        assert_eq!(
            error_of(App::Word, Failure::Open(password)).to_string(),
            "This file has a password. Open it in Word and save it without one, then try again."
        );
        // Excel and PowerPoint say "password" in their own words.
        let excel = Fault {
            code: 0x800A_03EC_u32 as i32,
            text: "The password you supplied is not correct.".to_owned(),
        };
        assert_eq!(
            error_of(App::Excel, Failure::Open(excel)).to_string(),
            "This file has a password. Open it in Excel and save it without one, then try again."
        );
        // Anything else is Office's failure, its words kept for details.
        let other = Fault {
            code: 1,
            text: "Word experienced an error trying to open the file.".to_owned(),
        };
        match error_of(App::Word, Failure::Open(other)) {
            Error::EngineFailed {
                engine, details, ..
            } => {
                assert_eq!(engine, "Word");
                assert!(details.contains("experienced an error"), "{details}");
            }
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn a_job_that_takes_too_long_is_left_to_finish_alone() {
        let started = Instant::now();
        let r = within(
            App::Word,
            Duration::from_millis(200),
            &CancelToken::new(),
            |_| {
                std::thread::sleep(Duration::from_secs(3));
                Ok(())
            },
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        match r {
            Err(e) => assert_eq!(
                e.to_string(),
                "Word didn't finish. Try again, or close Word if it's showing a message."
            ),
            Ok(()) => panic!("it waited"),
        }
        // A job that answers in time gives its answer.
        let r = within(
            App::Excel,
            Duration::from_secs(5),
            &CancelToken::new(),
            |_| Err(Error::Cancelled),
        );
        assert!(matches!(r, Err(Error::Cancelled)));
        assert!(within(
            App::Excel,
            Duration::from_secs(5),
            &CancelToken::new(),
            |_| Ok(())
        )
        .is_ok());
        // Cancel stops the wait too.
        let cancel = CancelToken::new();
        cancel.cancel();
        let started = Instant::now();
        let r = within(App::PowerPoint, TIMEOUT, &cancel, |_| {
            std::thread::sleep(Duration::from_secs(3));
            Ok(())
        });
        assert!(matches!(r, Err(Error::Cancelled)));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_job_that_stops_waiting_for_its_turn_never_starts_office() {
        static TURN: Mutex<()> = Mutex::new(());
        let dir = tempfile::tempdir().unwrap();
        let started = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let start = |started: std::sync::Arc<std::sync::atomic::AtomicUsize>| {
            move |private: &Path, _: &std::sync::atomic::AtomicBool| {
                started.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                fs::write(private, b"%PDF").map_err(|e| Error::io(private, e))
            }
        };
        // Another job has the turn.
        let held = TURN.lock().unwrap();
        // This one is cancelled while it waits...
        let cancel = CancelToken::new();
        cancel.cancel();
        let output = dir.path().join("cancelled.pdf");
        let r = office_job(
            App::Word,
            &TURN,
            dir.path().join("cancelled-private.pdf"),
            &output,
            TIMEOUT,
            &cancel,
            start(started.clone()),
        );
        assert!(matches!(r, Err(Error::Cancelled)), "{r:?}");
        // ...and this one runs out of time while it waits.
        let late = dir.path().join("late.pdf");
        let r = office_job(
            App::Word,
            &TURN,
            dir.path().join("late-private.pdf"),
            &late,
            Duration::from_millis(200),
            &CancelToken::new(),
            start(started.clone()),
        );
        assert!(matches!(r, Err(Error::OfficeTimeout { .. })), "{r:?}");
        // The turn comes: neither starts Office or writes anything.
        drop(held);
        std::thread::sleep(Duration::from_millis(500));
        drop(TURN.lock().unwrap());
        assert_eq!(started.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(!output.exists() && !late.exists());
        assert!(!dir.path().join("late-private.pdf").exists());
    }

    #[test]
    fn office_writes_a_name_of_its_own_and_kiln_places_it_when_done() {
        static TURN: Mutex<()> = Mutex::new(());
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("output.pdf");
        let private = dir.path().join("office-out.pdf");
        // Done in time: the PDF is moved to the output.
        let r = office_job(
            App::Word,
            &TURN,
            private.clone(),
            &output,
            TIMEOUT,
            &CancelToken::new(),
            |p: &Path, _: &std::sync::atomic::AtomicBool| {
                assert_ne!(p, Path::new("output.pdf"));
                fs::write(p, b"%PDF").map_err(|e| Error::io(p, e))
            },
        );
        assert!(r.is_ok(), "{r:?}");
        assert_eq!(fs::read(&output).unwrap(), b"%PDF");
        assert!(!private.exists());
        fs::remove_file(&output).unwrap();
        // Failed after writing something: nothing is placed.
        let r = office_job(
            App::Word,
            &TURN,
            private.clone(),
            &output,
            TIMEOUT,
            &CancelToken::new(),
            |p: &Path, _: &std::sync::atomic::AtomicBool| {
                fs::write(p, b"half").unwrap();
                Err(Error::OfficePassword { app: "Word" })
            },
        );
        assert!(matches!(r, Err(Error::OfficePassword { .. })), "{r:?}");
        assert!(!output.exists());
        // Too late: Office writes its own name after Kiln stopped waiting,
        // and the output is never touched.
        let r = office_job(
            App::Word,
            &TURN,
            dir.path().join("slow.pdf"),
            &output,
            Duration::from_millis(100),
            &CancelToken::new(),
            |p: &Path, gone: &std::sync::atomic::AtomicBool| {
                std::thread::sleep(Duration::from_millis(400));
                assert!(gone.load(std::sync::atomic::Ordering::SeqCst));
                fs::write(p, b"%PDF").map_err(|e| Error::io(p, e))
            },
        );
        assert!(matches!(r, Err(Error::OfficeTimeout { .. })), "{r:?}");
        std::thread::sleep(Duration::from_millis(800));
        assert!(!output.exists());
        assert!(dir.path().join("slow.pdf").exists());
    }

    #[test]
    fn powerpoint_is_in_use_only_when_someone_else_started_it() {
        use std::cell::Cell;
        let wait = Duration::from_secs(5);
        // Kiln's own PowerPoint (10) is still on its way out when the next
        // job looks; it is gone a moment later. Nobody else's is open.
        let looks = Cell::new(0);
        let exiting = || {
            looks.set(looks.get() + 1);
            if looks.get() < 3 {
                vec![10]
            } else {
                vec![]
            }
        };
        let mut ours = vec![10];
        assert!(!powerpoint_in_use(&mut ours, &exiting, &|| false, wait));
        assert!(ours.is_empty(), "{ours:?}");
        // Someone's own PowerPoint (20) beside Kiln's: in use.
        let mut ours = vec![10];
        let both = || vec![10, 20];
        let started = Instant::now();
        assert!(powerpoint_in_use(
            &mut ours,
            &both,
            &|| false,
            Duration::from_millis(300)
        ));
        assert!(started.elapsed() < Duration::from_secs(2));
        // Only someone else's: in use, at once.
        let mut ours = vec![];
        let started = Instant::now();
        assert!(powerpoint_in_use(&mut ours, &|| vec![20], &|| false, wait));
        assert!(started.elapsed() < Duration::from_secs(1));
        // Kiln's own that never exits can't be told from someone's using it.
        let mut ours = vec![10];
        assert!(powerpoint_in_use(
            &mut ours,
            &|| vec![10],
            &|| false,
            Duration::from_millis(300)
        ));
        assert_eq!(ours, [10]);
        // Nothing running, but one listed for others to use: in use.
        assert!(powerpoint_in_use(&mut vec![], &Vec::new, &|| true, wait));
        // Nothing at all: free.
        assert!(!powerpoint_in_use(&mut vec![], &Vec::new, &|| false, wait));
    }

    #[test]
    fn office_opens_a_copy_with_picture_fields_defused() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        fs::create_dir(&work).unwrap();
        let rtf = br"{\rtf1 {\field{\*\fldinst INCLUDEPICTURE http://x/p.png}}}";
        // An RTF, under its own name and under a Word 97 one.
        for name in ["linked.rtf", "linked.doc"] {
            let input = dir.path().join(name);
            fs::write(&input, rtf).unwrap();
            let content = look(&input).unwrap().content;
            assert_eq!(content, Content::Rtf);
            let copy = copy_for_office(App::Word, &input, &work, content).unwrap();
            assert_eq!(copy, work.join("office-in").join("linked.rtf"));
            let copied = fs::read(&copy).unwrap();
            assert!(
                !copied.windows(14).any(|w| w == b"INCLUDEPICTURE"),
                "{name}"
            );
            // The person's file is as it was.
            assert_eq!(fs::read(&input).unwrap(), rtf);
        }
        // Anything else is copied as it is.
        let odt = dir.path().join("a.odt");
        let zip = zip_of(&[("content.xml", "INCLUDEPICTURE")]);
        fs::write(&odt, &zip).unwrap();
        let copy = copy_for_office(App::Word, &odt, &work, Content::OtherZip).unwrap();
        assert_eq!(copy.extension().unwrap(), "odt");
        assert_eq!(fs::read(copy).unwrap(), zip);
        // A zip Kiln can't read is no file Office is given.
        fs::write(&odt, b"PK\x03\x04INCLUDEPICTURE").unwrap();
        assert_eq!(look(&odt).unwrap().content, Content::BadZip);
        assert!(!accepts("odt", Content::BadZip));
    }

    #[test]
    fn external_parts_other_than_hyperlinks_lose_their_address() {
        let rels = concat!(
            r#"<?xml version="1.0"?><Relationships xmlns="x">"#,
            r#"<Relationship Id="rId1" Type="http://s/relationships/styles" Target="styles.xml"/>"#,
            r#"<Relationship Id="rId2" Type="http://s/relationships/image" Target="http://127.0.0.1:47391/x.png" TargetMode="External"/>"#,
            r#"<Relationship TargetMode='External' Target='\\server\share\t.dotm' Id='rId3' Type='http://s/relationships/attachedTemplate'/>"#,
            r#"<Relationship Id="rId4" Type="http://s/relationships/hyperlink" Target="https://example.com/" TargetMode="External"/>"#,
            "</Relationships>"
        );
        let out = defuse_rels(rels).unwrap();
        assert!(!out.contains("47391"), "{out}");
        assert!(!out.contains("server"), "{out}");
        // A link to click and a part inside the file are kept.
        assert!(out.contains(r#"Target="https://example.com/""#), "{out}");
        assert!(out.contains(r#"Target="styles.xml""#), "{out}");
        assert!(
            out.contains(r#"Id="rId2""#) && out.contains("Id='rId3'"),
            "{out}"
        );
        // Nothing to defuse: no copy.
        assert_eq!(
            defuse_rels(r#"<Relationship Id="a" Target="b.xml"/>"#),
            None
        );
    }

    /// A zip of `parts`, deflated, as an Office file is.
    fn zip_of(parts: &[(&str, &str)]) -> Vec<u8> {
        let parts: Vec<package::Part> = parts
            .iter()
            .map(|(n, t)| package::Part::new(n, t.as_bytes().to_vec()))
            .collect();
        let mut out = Vec::new();
        package::write(&mut std::io::Cursor::new(Vec::new()), &parts, &mut out).unwrap();
        out
    }

    /// Every part of `zip`, inflated.
    fn parts_of(zip: &[u8]) -> Vec<(String, Vec<u8>)> {
        package::read(&mut std::io::Cursor::new(zip), &|_| true)
            .unwrap()
            .into_iter()
            .map(|p| (p.name, p.text.unwrap()))
            .collect()
    }

    #[test]
    fn a_package_is_rewritten_with_its_links_defused() {
        let rels = r#"<Relationships><Relationship Id="r" Type="t/image" Target="http://h/x.png" TargetMode="External"/></Relationships>"#;
        let doc = "<w:document>hello</w:document>";
        let zip = zip_of(&[
            ("word/document.xml", doc),
            ("word/_rels/document.xml.rels", rels),
        ]);
        let parts = parts_of(&zip);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].1, doc.as_bytes());

        let defused = defuse_package(&zip).unwrap().unwrap();
        let parts = parts_of(&defused);
        assert_eq!(
            parts[0],
            ("word/document.xml".to_owned(), doc.as_bytes().to_vec())
        );
        let rels_now = String::from_utf8(parts[1].1.clone()).unwrap();
        assert!(!rels_now.contains("http://h"), "{rels_now}");
        // A package with nothing to defuse is left as it is.
        let plain = zip_of(&[("word/document.xml", doc)]);
        assert_eq!(defuse_package(&plain).unwrap(), None);
        // Something that isn't a zip Kiln can read is said so.
        assert!(defuse_package(b"PK\x03\x04 and nothing").is_err());
        // The real fixture: its picture's address goes.
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/hostile/remote-picture.docx");
        let defused = defuse_package(&fs::read(fixture).unwrap())
            .unwrap()
            .unwrap();
        let all: Vec<u8> = parts_of(&defused)
            .into_iter()
            .flat_map(|(_, b)| b)
            .collect();
        assert!(!String::from_utf8_lossy(&all).contains("47391"));
    }

    /// A zip whose entries are written as given: name, method, the bytes
    /// stored, and the CRC and inflated size its directory claims.
    fn raw_zip(entries: &[(&str, u16, &[u8], u32, u32)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, method, stored, crc, size) in entries {
            let offset = out.len() as u32;
            let mut common = Vec::new();
            for v in [20u16, 0, *method, 0, 0x21] {
                common.extend_from_slice(&v.to_le_bytes());
            }
            for v in [*crc, stored.len() as u32, *size] {
                common.extend_from_slice(&v.to_le_bytes());
            }
            common.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            out.extend_from_slice(&common);
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(stored);
            central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&common);
            central.extend_from_slice(&[0u8; 12]);
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let central_at = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&[0u8; 4]);
        let n = entries.len() as u16;
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&central_at.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    fn deflated(bytes: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut d = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        d.write_all(bytes).unwrap();
        d.finish().unwrap()
    }

    fn crc_of(bytes: &[u8]) -> u32 {
        let mut c = flate2::Crc::new();
        c.update(bytes);
        c.sum()
    }

    const EXTERNAL_RELS: &str = r#"<Relationships><Relationship Id="r" Type="t/image" Target="http://h/x.png" TargetMode="External"/></Relationships>"#;

    #[test]
    fn a_part_that_inflates_past_its_cap_is_refused() {
        let rels = EXTERNAL_RELS.as_bytes();
        // A relationships part that claims a size past the cap.
        let claims = raw_zip(&[("_rels/.rels", 8, &deflated(rels), crc_of(rels), 0xFFFF_FFF0)]);
        assert!(defuse_package(&claims).is_err());
        // One that is as big as it says, a little past the cap.
        let big = vec![b' '; (16 << 20) + 1];
        let honest = raw_zip(&[(
            "_rels/.rels",
            8,
            &deflated(&big),
            crc_of(&big),
            big.len() as u32,
        )]);
        assert!(defuse_package(&honest).is_err());
        // Parts that each fit, but not all together.
        let part = vec![b' '; 600];
        let entry = |name| (name, 8, deflated(&part), crc_of(&part), 600u32);
        let entries = [entry("a.rels"), entry("b.rels")];
        let zip = raw_zip(
            &entries
                .iter()
                .map(|(n, m, d, c, s)| (*n, *m, d.as_slice(), *c, *s))
                .collect::<Vec<_>>(),
        );
        let caps = |total| package::Caps { part: 1000, total };
        let read =
            |total| package::read_with(&mut std::io::Cursor::new(&zip), &is_rels, caps(total));
        assert!(read(1200).is_ok());
        assert!(read(1100).is_err());
        assert_eq!(
            (package::PART_CAP, package::TOTAL_CAP),
            (16 << 20, 64 << 20)
        );
    }

    #[test]
    fn a_part_kiln_doesnt_read_is_copied_as_stored() {
        let rels = EXTERNAL_RELS.as_bytes();
        // A "video" whose stored bytes aren't even deflate, claiming 4 GB:
        // inflating it would fail, so it must go across untouched.
        let video: &[u8] = b"\xFF\xFFnot deflate at all\xFF";
        let zip = raw_zip(&[
            ("ppt/media/video.mp4", 8, video, 0x1234_5678, 0xFFFF_FFF0),
            (
                "ppt/_rels/presentation.xml.rels",
                8,
                &deflated(rels),
                crc_of(rels),
                rels.len() as u32,
            ),
        ]);
        let out = defuse_package(&zip).unwrap().unwrap();
        // The stored bytes, and the CRC and size claimed, are as they were.
        let at = out
            .windows(video.len())
            .position(|w| w == video)
            .expect("the video's bytes are copied");
        let header = at - 30 - "ppt/media/video.mp4".len();
        assert_eq!(&out[header..header + 4], b"PK\x03\x04");
        assert_eq!(
            &out[header + 14..header + 18],
            &0x1234_5678u32.to_le_bytes()
        );
        assert_eq!(
            &out[header + 22..header + 26],
            &0xFFFF_FFF0u32.to_le_bytes()
        );
        assert!(!String::from_utf8_lossy(&out).contains("http://h"));
    }

    #[test]
    fn a_part_named_outside_the_package_is_refused() {
        let body = b"<x/>";
        for name in [
            "../evil.xml",
            "word/../../evil.xml",
            "/evil.xml",
            r"\evil.xml",
            r"word\..\evil.xml",
            "C:/evil.xml",
            "word/a\0.xml",
        ] {
            let zip = raw_zip(&[(name, 0, body, crc_of(body), body.len() as u32)]);
            assert!(defuse_package(&zip).is_err(), "{name:?}");
        }
        // A name that comes twice, in any case.
        let twice = raw_zip(&[
            ("word/a.xml", 0, body, crc_of(body), 4),
            ("Word/A.xml", 0, body, crc_of(body), 4),
        ]);
        assert!(defuse_package(&twice).is_err());
        // Ordinary names are fine.
        let fine = raw_zip(&[
            ("[Content_Types].xml", 0, body, crc_of(body), 4),
            ("word/a..b.xml", 0, body, crc_of(body), 4),
        ]);
        assert_eq!(defuse_package(&fine), Ok(None));
    }

    #[test]
    fn office_opens_a_package_copy_with_its_links_defused() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        fs::create_dir(&work).unwrap();
        let rels = r#"<Relationships><Relationship Id="r" Type="t/image" Target="http://h/x.png" TargetMode="External"/></Relationships>"#;
        let input = dir.path().join("a.docx");
        let zip = zip_of(&[("word/_rels/document.xml.rels", rels)]);
        fs::write(&input, &zip).unwrap();
        let copy = copy_for_office(App::Word, &input, &work, Content::Ooxml).unwrap();
        let parts = parts_of(&fs::read(copy).unwrap());
        assert!(!String::from_utf8_lossy(&parts[0].1).contains("http://h"));
        assert_eq!(fs::read(&input).unwrap(), zip);
    }

    #[test]
    fn a_file_with_a_password_is_refused_before_office_starts() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        fs::create_dir(&work).unwrap();
        let odp = zip_of(&[(
            "META-INF/manifest.xml",
            r#"<manifest:manifest><manifest:file-entry><manifest:encryption-data/></manifest:file-entry></manifest:manifest>"#,
        )]);
        for (app_, name, bytes) in [
            (
                App::Word,
                "locked.docx",
                compound_file(&["EncryptedPackage"]),
            ),
            (
                App::PowerPoint,
                "locked.pptx",
                compound_file(&["EncryptedPackage"]),
            ),
            (
                App::PowerPoint,
                "locked.ppt",
                compound_file(&["EncryptedSummary"]),
            ),
            (App::PowerPoint, "locked.odp", odp),
        ] {
            let input = dir.path().join(name);
            fs::write(&input, bytes).unwrap();
            let r = to_pdf(
                app_,
                &input,
                &work.join("output.pdf"),
                &CancelToken::new(),
                TIMEOUT,
            );
            assert_eq!(
                r.unwrap_err().to_string(),
                format!(
                    "This file has a password. Open it in {} and save it without one, then try again.",
                    app_.name()
                ),
                "{name}"
            );
            assert!(!work.join("output.pdf").exists());
        }
    }

    #[test]
    fn the_content_not_the_name_says_how_office_gets_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        fs::create_dir(&work).unwrap();
        let docx = zip_of(&[
            ("[Content_Types].xml", "<Types/>"),
            ("word/_rels/document.xml.rels", EXTERNAL_RELS),
        ]);
        // A DOCX named .doc is a package: its copy is a DOCX, links defused.
        let input = dir.path().join("report.doc");
        fs::write(&input, &docx).unwrap();
        let seen = look(&input).unwrap();
        assert_eq!(seen.content, Content::Ooxml);
        assert!(accepts("doc", seen.content));
        let copy = copy_for_office(App::Word, &input, &work, seen.content).unwrap();
        assert_eq!(copy.extension().unwrap(), "docx");
        let parts = parts_of(&fs::read(&copy).unwrap());
        assert!(!parts
            .iter()
            .any(|(_, b)| String::from_utf8_lossy(b).contains("http://h")));
        // Plain text named .doc isn't a Word file Office should be given.
        let text = dir.path().join("notes.doc");
        fs::write(&text, "Just some notes.\r\n").unwrap();
        assert_eq!(look(&text).unwrap().content, Content::Text);
        let r = to_pdf(
            App::Word,
            &text,
            &work.join("output.pdf"),
            &CancelToken::new(),
            TIMEOUT,
        );
        match r {
            Err(Error::WrongContent { path, code }) => {
                assert_eq!((path, code.as_str()), (text.clone(), "DOC"));
            }
            other => panic!("{other:?}"),
        }
        // What each name may hold.
        use Content::*;
        for (ext, ok, refused) in [
            (
                "doc",
                &[Compound, Ooxml, Rtf][..],
                &[Text, Markup, OtherZip][..],
            ),
            ("docx", &[Ooxml, Compound], &[Text, Markup, Rtf, OtherZip]),
            ("xls", &[Compound, Ooxml], &[Text, Markup, Rtf, OtherZip]),
            ("ppt", &[Compound, Ooxml], &[Text, Markup, Rtf, OtherZip]),
            ("odt", &[OtherZip, Ooxml], &[Text, Markup, Rtf, Compound]),
            ("rtf", &[Rtf], &[Text, Markup, Ooxml, Compound, OtherZip]),
            ("txt", &[Text], &[Markup, Rtf, Ooxml, Compound, OtherZip]),
            ("csv", &[Text], &[Markup, Rtf, Ooxml, Compound, OtherZip]),
        ] {
            for c in ok {
                assert!(accepts(ext, *c), "{ext} {c:?}");
            }
            for c in refused {
                assert!(!accepts(ext, *c), "{ext} {c:?}");
            }
        }
        // Markup and web archives are told apart from text, in UTF-8 or
        // UTF-16.
        for (bytes, want) in [
            (&b"  <html><img src='http://h/x.png'></html>"[..], Markup),
            (b"\xEF\xBB\xBF<?xml version='1.0'?>", Markup),
            (b"\xFF\xFE<\0h\0t\0m\0l\0>\0", Markup),
            (
                b"MIME-Version: 1.0\r\nContent-Type: multipart/related",
                Markup,
            ),
            (br"{\rtf1 hello}", Rtf),
            (b"a,b\r\n1,2\r\n", Text),
        ] {
            let path = dir.path().join("x.txt");
            fs::write(&path, bytes).unwrap();
            assert_eq!(
                look(&path).unwrap().content,
                want,
                "{:?}",
                String::from_utf8_lossy(bytes)
            );
        }
    }

    #[test]
    fn the_copy_office_opens_has_a_short_name() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        fs::create_dir(&work).unwrap();
        let long = format!("{}.txt", "é".repeat(200));
        let input = dir.path().join(&long);
        fs::write(&input, "text").unwrap();
        let copy = copy_for_office(App::Word, &input, &work, Content::Text).unwrap();
        let stem = copy.file_stem().unwrap().to_str().unwrap();
        assert_eq!(stem, "é".repeat(80));
        assert_eq!(copy.extension().unwrap(), "txt");
        assert_eq!(fs::read(&copy).unwrap(), b"text");
    }
}
