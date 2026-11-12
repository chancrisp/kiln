//! The list of dropped files. Platform-independent so it can be tested
//! anywhere.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use kiln_core::{
    Availability, CancelToken, Facts, FileCheck, Kiln, Kind, Look, Registry, Suggestion, Tool,
};

use crate::inspector::InspectorControl;
use crate::wheel::Action;

/// Where a file's latest job stands.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum JobState {
    #[default]
    Idle,
    Running {
        action: Action,
        /// `None` while the engine can't say how far along it is.
        fraction: Option<f32>,
        /// When the job started, for the time left. `None` where it is
        /// not known.
        started: Option<Instant>,
    },
    Done {
        action: Action,
        outputs: Vec<PathBuf>,
        /// Where the results went. `None` means they are still held in
        /// Kiln's private folder.
        saved: Option<Saved>,
    },
    Failed {
        /// One plain sentence, whole. The row cuts it short if it must.
        message: String,
        /// The engine's output, for Show details.
        details: String,
        /// Who wrote `details`: the engine, or `None` for Kiln itself.
        engine: Option<&'static str>,
        /// The Microsoft Store extension the job needed and Windows lacks,
        /// when that is why it failed: the row's link gets it.
        store_product: Option<&'static str>,
    },
    Cancelled,
}

/// Where a finished row's results went once they left the holding folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Saved {
    /// Saved in this folder.
    Folder(PathBuf),
}

/// The details dialog's title (mock screen 8), unless a helper app's own
/// words are shown: then "What FFmpeg said".
pub const DETAILS_TITLE: &str = "What happened";

/// Whether the log line `line` says one of `sentences` again, as far as
/// case, spacing and a closing full stop go.
fn repeats(line: &str, sentences: &[&str]) -> bool {
    let plain = |s: &str| {
        s.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .trim_end_matches('.')
            .to_lowercase()
    };
    let line = plain(line);
    !line.is_empty() && sentences.iter().any(|s| plain(s) == line)
}

/// The details dialog's Copy button. For a moment after a copy it says
/// how that went: "Copied", or "Couldn't copy". `last` is whether the last
/// copy worked, and how long ago it was.
pub fn copy_label(last: Option<(bool, std::time::Duration)>) -> &'static str {
    match last {
        Some((true, d)) if d < COPIED_FOR => "Copied",
        Some((false, d)) if d < COPIED_FOR => "Couldn't copy",
        _ => "Copy",
    }
}

/// How long the Copy button says how a copy went.
pub const COPIED_FOR: std::time::Duration = std::time::Duration::from_millis(1200);

/// `text` on one line, its runs of spaces and line breaks folded to one
/// space.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Text for the clipboard, with Windows line ends.
pub fn clipboard_lines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\n', "\r\n")
}

/// How a row's second line is coloured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Muted,
    /// The output name after a job: primary text.
    Result,
    /// Errors, in brick.
    Crack,
}

/// The first seconds of a job say nothing about how long it will take.
const LEFT_AFTER: Duration = Duration::from_secs(3);

/// How much longer a job that has run for `elapsed` and is `fraction`
/// done will take, going by its pace so far. `None` too early to say.
pub fn time_left(elapsed: Duration, fraction: f32) -> Option<Duration> {
    if elapsed < LEFT_AFTER || !(0.02..1.0).contains(&fraction) {
        return None;
    }
    Some(elapsed.mul_f32((1.0 - fraction) / fraction))
}

/// A time left, roughly, as a person would say it: "a few seconds",
/// "about 40 sec", "about 2 min", "about 1 h 20 min".
pub fn left_text(left: Duration) -> String {
    let secs = left.as_secs();
    match secs {
        0..=7 => "a few seconds".to_owned(),
        8..=44 => format!("about {} sec", (secs + 2) / 5 * 5),
        45..=89 => "about 1 min".to_owned(),
        90..=3599 => format!("about {} min", (secs + 30) / 60),
        _ => {
            let mins = (secs + 30) / 60;
            match mins % 60 {
                0 => format!("about {} h", mins / 60),
                m => format!("about {} h {m} min", mins / 60),
            }
        }
    }
}

/// A running row's second line: what it is doing, how far along, and how
/// long is left, or "Working…" where the engine can't say how far:
/// "Converting to WEBM · 38% · about 2 min".
pub fn progress_line(doing: &str, fraction: Option<f32>, left: Option<Duration>) -> String {
    match fraction {
        None => format!("{doing}{DETAIL_SEPARATOR}Working…"),
        Some(f) => {
            let mut s = format!(
                "{doing}{DETAIL_SEPARATOR}{}%",
                (f.clamp(0.0, 1.0) * 100.0) as u32
            );
            if let Some(l) = left {
                s.push_str(&format!("{DETAIL_SEPARATOR}{}", left_text(l)));
            }
            s
        }
    }
}

/// How one job of a batch that goes on ended, for the footer and for
/// assistive tools to hear once: "photo.jpg is ready.".
pub fn job_outcome(name: &str, ok: bool, cancelled: bool) -> String {
    match (ok, cancelled) {
        (true, _) => format!("{name} is ready."),
        (false, true) => format!("{name} was cancelled."),
        (false, false) => format!("{name} couldn't be converted."),
    }
}

/// How a batch of jobs is getting on, for the footer: "12 of 50 done.",
/// or what is under way while none is done yet: "0 of 1 done." beside a
/// row an earlier batch saved read as if that row were still to come.
pub fn batch_progress(done: usize, total: usize) -> String {
    match (done, total) {
        (0, 1) => "Working on 1 file.".to_owned(),
        (0, n) if n > 1 => format!("Working on {n} files."),
        _ => format!("{done} of {total} done."),
    }
}

/// What the taskbar button shows for the work under way.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TaskbarProgress {
    /// Nothing is running.
    None,
    /// Running, with no way to say how far.
    Indeterminate,
    /// This far along, of 1.0.
    Fraction(f32),
}

/// The taskbar's progress for `total` jobs of which `done` have finished
/// and `running` are under way with the fractions they have reported.
pub fn taskbar_progress(total: usize, done: usize, running: &[Option<f32>]) -> TaskbarProgress {
    if total == 0 || (running.is_empty() && done >= total) {
        return TaskbarProgress::None;
    }
    if running.iter().all(Option::is_none) && done == 0 {
        return TaskbarProgress::Indeterminate;
    }
    let part: f32 = running
        .iter()
        .map(|f| f.unwrap_or(0.0).clamp(0.0, 1.0))
        .sum();
    TaskbarProgress::Fraction(((done as f32 + part) / total as f32).clamp(0.0, 1.0))
}

/// What the finish of work says when Kiln is not in front: the title and
/// the sentence, naming the result when there is one.
pub fn finish_notice(
    ok: usize,
    failed: usize,
    cancelled: usize,
    first: Option<&str>,
) -> Option<(String, String)> {
    if ok + failed == 0 {
        // Cancelled by the person themselves: nothing to tell them.
        let _ = cancelled;
        return None;
    }
    let text = match (ok, failed, first) {
        (1, 0, Some(name)) => format!("{name} is ready."),
        (n, 0, _) => format!("{n} files are ready."),
        (0, 1, _) => "A file couldn't be converted. Open Kiln to see why.".to_owned(),
        (0, n, _) => format!("{n} files couldn't be converted. Open Kiln to see why."),
        (n, f, _) => format!("{n} ready, {f} couldn't be converted."),
    };
    Some(("Kiln".to_owned(), text))
}

/// The finish notice once the results it counts that went next to their
/// originals are in (`saves`): with nothing failed, it says where they
/// went; failures read as `finish_notice` has them. The flag says every
/// result was saved there, so a click shows them in their folder.
pub fn finish_notice_saved(
    ok: usize,
    failed: usize,
    cancelled: usize,
    first: Option<&str>,
    saves: &Saves,
) -> Option<(String, String, bool)> {
    let (title, text) = finish_notice(ok, failed, cancelled, first)?;
    Some(match saves.notice() {
        Some(went) if failed == 0 => (title, went, saves.all_saved()),
        _ => (title, text, false),
    })
}

/// A job's note for its row: what the conversion leaves out (`drops`, as
/// `JobEvent::Notes` sent them), then the job's own (`own`: "Encoded on the
/// processor."), joined with [`DETAIL_SEPARATOR`]. `None` when there is
/// neither.
pub fn job_note(drops: &[String], own: Option<&str>) -> Option<String> {
    let parts: Vec<&str> = drops.iter().map(String::as_str).chain(own).collect();
    (!parts.is_empty()).then(|| parts.join(DETAIL_SEPARATOR))
}

/// The sentences every one of `lists` holds, in the first's order: what is
/// true of every file at once. Another file's sound tracks are no news
/// about this one's, so a sentence only some files carry is left out, and
/// a missing note is better than a false one.
pub fn shared_notes<T: AsRef<[String]>>(lists: &[T]) -> Vec<String> {
    let Some((first, rest)) = lists.split_first() else {
        return vec![];
    };
    first
        .as_ref()
        .iter()
        .filter(|n| rest.iter().all(|l| l.as_ref().contains(n)))
        .cloned()
        .collect()
}

/// What converting `paths` to `to` leaves out, as the options sheet says it
/// before the job does: the notes [`Kiln::drops`] gives each file (so the
/// job's `JobEvent::Notes` say the same), shared by all of them. `infos` are
/// the files' facts, in the order of `paths`, once they are read off the
/// window's thread; without them, or for a batch that wasn't read whole,
/// only what needs no reading is said (a PDF's text). This reads nothing.
pub fn sheet_notes(
    kiln: &Kiln,
    paths: &[PathBuf],
    to: &str,
    infos: Option<&[kiln_core::MediaInfo]>,
) -> Vec<String> {
    let infos = infos.filter(|i| i.len() == paths.len());
    let lists: Vec<Vec<String>> = paths
        .iter()
        .enumerate()
        .map(|(k, path)| kiln.drops(path, to, infos.map(|i| &i[k])))
        .collect();
    shared_notes(&lists)
}

/// Whether converting every one of `paths` to `to` makes its text only
/// (`Registry::text_only`: a PDF laid out as another kind of document), so
/// that the target reads "DOCX (text only)". A file that isn't a PDF becomes
/// the target as itself, so one such file in the batch is enough to leave
/// the label off; so is a file or target Kiln doesn't know, and no files.
pub fn text_only_for(kiln: &Kiln, paths: &[PathBuf], to: &str) -> bool {
    let reg = kiln.registry();
    let Some(to) = reg.format(to) else {
        return false;
    };
    !paths.is_empty()
        && paths
            .iter()
            .all(|p| reg.format_of(p).is_some_and(|from| reg.text_only(from, to)))
}

/// [`sheet_notes`] for a sheet that has just opened, from the facts the
/// rows already hold (`Entry::facts`, which the inspector reads off the
/// window's thread), so the lines are there when the sheet is. A file whose
/// facts haven't arrived, or the list doesn't hold, has no tracks yet, which
/// leaves out what only the others are told: a missing note, not a false
/// one. The sheet's own look at the files replaces these (`sheet_notes`).
pub fn notes_from_rows(kiln: &Kiln, files: &FileList, paths: &[PathBuf], to: &str) -> Vec<String> {
    let media: Vec<Option<&kiln_core::MediaInfo>> = paths
        .iter()
        .map(|p| {
            let e = &files.entries()[files.index_of(p)?];
            e.facts.as_ref()?.media.as_ref()
        })
        .collect();
    if media.iter().all(Option::is_none) {
        return sheet_notes(kiln, paths, to, None);
    }
    let infos: Vec<kiln_core::MediaInfo> = media
        .into_iter()
        .map(|m| m.cloned().unwrap_or_default())
        .collect();
    sheet_notes(kiln, paths, to, Some(&infos))
}

/// `text`, the drag-wheel's sentence, followed by the `drops` of the files
/// it tells of: "Converted clip.mkv to MP4. Keeps the first of 2 sound
/// tracks." The sentence is untouched.
pub fn with_drops(text: &str, drops: &[String]) -> String {
    std::iter::once(text)
        .chain(drops.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

/// What the drag-wheel says once every job from one drop has finished: one
/// notification for the drop, and which job's row a click on it opens.
///
/// `jobs` holds each finished job's file name and, if it failed, the row's
/// reason; cancelled jobs are left out. One job names its file:
/// "Converted photo.jpg to WEBP." or "Couldn't convert photo.jpg: {reason}".
/// Several count: "Converted 3 files to WEBP.", "Converted 2 of 3 files to
/// WEBP." or "Couldn't convert 3 files to WEBP.". A tool says its own
/// words: "Removed borders from 3 files.", "Couldn't make photo.png
/// smaller: {reason}".
/// The row opened is the one named, or the first that failed. `None` with
/// nothing to tell.
pub fn drag_notice(
    action: &Action,
    jobs: &[(String, Option<String>)],
) -> Option<(String, String, usize)> {
    let n = jobs.len();
    let failed: Vec<usize> = (0..n).filter(|&i| jobs[i].1.is_some()).collect();
    let ok = n - failed.len();
    let (name, reason) = match jobs {
        [] => return None,
        [(name, reason)] => (name.as_str(), reason.as_deref().unwrap_or("")),
        _ => ("", ""),
    };
    let text = match action {
        Action::Convert(ext) => {
            let to = ext.to_ascii_uppercase();
            match (n, ok) {
                (1, 1) => format!("Converted {name} to {to}."),
                (1, _) => format!("Couldn't convert {name}: {reason}"),
                (n, ok) if ok == n => format!("Converted {n} files to {to}."),
                (n, 0) => format!("Couldn't convert {n} files to {to}."),
                (n, ok) => format!("Converted {ok} of {n} files to {to}."),
            }
        }
        Action::Tool(t, kind) => {
            let did = |what: &str| inflect(&t.phrase(*kind, what), Form::Did);
            let verb = |what: &str| t.phrase(*kind, what);
            match (n, ok) {
                (1, 1) => format!("{}.", did(name)),
                (1, _) => format!("Couldn't {}: {reason}", verb(name)),
                (n, ok) if ok == n => format!("{}.", did(&format!("{n} files"))),
                (n, 0) => format!("Couldn't {}.", verb(&format!("{n} files"))),
                (n, ok) => format!("{}.", did(&format!("{ok} of {n} files"))),
            }
        }
    };
    Some((
        "Kiln".to_owned(),
        text,
        failed.first().copied().unwrap_or(0),
    ))
}

/// "Converting to JPG", "Removing borders".
pub fn doing(action: &Action) -> String {
    match action {
        Action::Convert(ext) => format!("Converting to {}", ext.to_ascii_uppercase()),
        Action::Tool(t, kind) => inflect(t.label(*kind), Form::Doing),
    }
}

/// "Converted to JPG", "Removed borders".
pub fn done(action: &Action) -> String {
    match action {
        Action::Convert(ext) => format!("Converted to {}", ext.to_ascii_uppercase()),
        Action::Tool(t, kind) => inflect(t.label(*kind), Form::Did),
    }
}

/// The forms of a tool's verb a row or a notice needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    /// "Removing borders"
    Doing,
    /// "Removed borders"
    Did,
}

/// `words`, a tool's name or sentence ("Remove borders", "make photo.png
/// smaller"), with its first word, the verb, in `form` and capitalised.
fn inflect(words: &str, form: Form) -> String {
    let (verb, rest) = words.split_once(' ').unwrap_or((words, ""));
    let verb = verb.to_ascii_lowercase();
    let changed = match (verb.as_str(), form) {
        ("make", Form::Did) => "Made".to_owned(),
        ("cut", Form::Did) => "Cut".to_owned(),
        ("cut", Form::Doing) => "Cutting".to_owned(),
        (v, Form::Did) => match v.strip_suffix('e') {
            Some(stem) => format!("{stem}ed"),
            None => format!("{v}ed"),
        },
        (v, Form::Doing) => match v.strip_suffix('e') {
            Some(stem) => format!("{stem}ing"),
            None => format!("{v}ing"),
        },
    };
    let mut out = String::with_capacity(words.len() + 4);
    let mut chars = changed.chars();
    if let Some(c) = chars.next() {
        out.extend(c.to_uppercase());
        out.push_str(chars.as_str());
    }
    if !rest.is_empty() {
        out.push(' ');
        out.push_str(rest);
    }
    out
}

/// The status line once a batch of jobs has finished.
pub fn batch_summary(
    action: &Action,
    files: usize,
    ok: usize,
    failed: usize,
    cancelled: usize,
) -> String {
    let n = |k: usize| format!("{k} file{}", if k == 1 { "" } else { "s" });
    let merge = matches!(action, Action::Tool(Tool::Merge, _));
    if ok == 0 && !merge {
        if failed == 0 {
            return "Cancelled.".to_owned();
        }
        let what = if files == 1 {
            "the file".to_owned()
        } else {
            n(failed)
        };
        let mut s = match action {
            Action::Convert(ext) => {
                format!("Couldn't convert {what} to {}.", ext.to_ascii_uppercase())
            }
            Action::Tool(t, kind) => format!("Couldn't {}.", t.phrase(*kind, &what)),
        };
        s.push_str(&says_why(failed));
        if cancelled > 0 {
            s.push_str(&format!(" {cancelled} cancelled."));
        }
        return s;
    }
    let mut s = match action {
        Action::Tool(Tool::Merge, _) if ok > 0 => format!("Merged {} into one.", n(files)),
        Action::Tool(Tool::Merge, _) => {
            format!("Couldn't merge the files.{}", says_why(failed))
        }
        Action::Convert(ext) if ok == files => {
            format!("Converted {} to {}.", n(ok), ext.to_ascii_uppercase())
        }
        Action::Convert(ext) => format!(
            "Converted {ok} of {} to {}.",
            n(files),
            ext.to_ascii_uppercase()
        ),
        Action::Tool(t, kind) if ok == files => {
            format!("{}.", inflect(&t.phrase(*kind, &n(ok)), Form::Did))
        }
        Action::Tool(t, kind) => {
            let what = format!("{ok} of {}", n(files));
            format!("{}.", inflect(&t.phrase(*kind, &what), Form::Did))
        }
    };
    if !merge {
        if failed > 0 {
            s.push_str(&format!(" {failed} didn't work.{}", says_why(failed)));
        }
        if cancelled > 0 {
            s.push_str(&format!(" {cancelled} cancelled."));
        }
    }
    s
}

/// What a footer line adds after "Couldn't ..." so it never stands alone:
/// the cause is on the row. `failed` rows failed.
fn says_why(failed: usize) -> String {
    if failed == 1 {
        " The row says why.".to_owned()
    } else {
        " Each row says why.".to_owned()
    }
}

/// What stands between the parts of a row's second line: a middle dot with
/// a space either side (mock screen 2).
pub const DETAIL_SEPARATOR: &str = " · ";

/// The footer's button that empties the list (spec 2.4).
pub const CLEAR_LIST: &str = "Clear list";

/// A saved result's button, on its row and in the side pane.
pub const SHOW_IN_FOLDER: &str = "Show in folder";

/// Whether a row shows its `link` while the side pane is out (`panel`):
/// Show in folder always stays on the row (mock screen 5); every other link
/// moves to the pane, which carries the actions, and comes back to the row
/// when the pane folds away.
pub fn link_on_row(link: &str, links: bool) -> bool {
    links || link == SHOW_IN_FOLDER
}

/// What a row shows at its right end, past its text (mock screens 2, 5).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RowMark {
    /// Nothing: a file waiting, or a job that was cancelled.
    None,
    /// A running job: its percentage once the engine says it (`None`
    /// draws the bar indeterminate), and Cancel.
    Progress(Option<f32>),
    /// A finished job: the checkmark.
    Done,
    /// A failed job: the error glyph.
    Failed,
}

/// A running row's percentage at its right end: "38%".
pub fn percent_text(fraction: f32) -> String {
    format!("{}%", crate::a11y::percent(fraction))
}

/// What a file is called by its type where its facts can't say more:
/// "Word document" for DOCX, else its code, "PNG".
pub fn type_name(code: &str) -> String {
    match code.to_ascii_lowercase().as_str() {
        "docx" | "doc" => "Word document".to_owned(),
        "xlsx" | "xls" => "Excel workbook".to_owned(),
        "pptx" | "ppt" => "PowerPoint presentation".to_owned(),
        _ => code.to_ascii_uppercase(),
    }
}

/// A row's facts (mock screen 2): what the file is, then its size.
/// "3000 × 2000 photo · 3.2 MB", "1920 × 800 video · 0:20 · 31 MB",
/// "Sound · 1:00 · 1.4 MB", "PDF · 3 pages · 57 KB", and before the facts
/// are in, "PNG · 445 KB".
pub fn row_facts(facts: Option<&Facts>, code: &str, size: u64) -> String {
    let mut parts: Vec<String> = vec![];
    let length = || {
        facts
            .and_then(|f| f.media.as_ref())
            .and_then(|m| m.duration)
            .filter(|d| *d > 0.0)
            .map(crate::quality::clock)
    };
    match facts {
        Some(f) if f.kind == Kind::Image && f.picture.is_some() => {
            parts.extend(describe(f));
        }
        Some(f) if f.kind == Kind::Video => {
            let frame = f.picture.as_ref().map(|p| (p.width, p.height)).or_else(|| {
                let m = f.media.as_ref()?;
                Some((m.width?, m.height?))
            });
            parts.push(match frame {
                Some((w, h)) => format!("{w} × {h} video"),
                None => "Video".to_owned(),
            });
            parts.extend(length());
        }
        Some(f) if f.kind == Kind::Audio => {
            parts.push("Sound".to_owned());
            parts.extend(length());
        }
        Some(f) => {
            parts.push(type_name(code));
            if let Some(pdf) = &f.pdf {
                parts.push(match pdf.pages {
                    1 => "1 page".to_owned(),
                    n => format!("{n} pages"),
                });
            }
        }
        None if !code.is_empty() => parts.push(type_name(code)),
        None => {}
    }
    parts.push(human_size(size));
    parts.join(DETAIL_SEPARATOR)
}

/// `paths` split for a conversion to `ext`: the files to convert, and
/// those already in that format, which are left alone (UX-06). A file's
/// format goes by its type, so `photo.jpeg` is already JPG.
pub fn already_in(kiln: &Kiln, paths: &[PathBuf], ext: &str) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let to = kiln.registry().format(ext).map(|f| f.ext.as_str());
    paths.iter().cloned().partition(|p| {
        let from = kiln.registry().format_of(p).map(|f| f.ext.as_str());
        from.is_none() || from != to
    })
}

/// The line of a row left alone because its file is in the format picked
/// already: "Already JPG".
pub fn already_line(ext: &str) -> String {
    format!(
        "Already {}",
        ext.trim_start_matches('.').to_ascii_uppercase()
    )
}

/// A held result that someone removed by hand.
pub const GONE: &str = "The result is no longer there. Convert it again.";

/// Lines that stand in for a row's own second line, by the row's file: an
/// export that failed, or a held result that's gone. Each change moves
/// [`Notes::revision`], which the inspector is built from, so the panel
/// says what the row says and never goes stale.
#[derive(Debug, Default)]
pub struct Notes {
    by_file: HashMap<PathBuf, (String, Tone)>,
    revision: u64,
}

impl Notes {
    /// The note standing in for `file`'s row's line, and its tone.
    pub fn get(&self, file: &Path) -> Option<&(String, Tone)> {
        self.by_file.get(file)
    }

    /// Whether `file`'s note says its held result is gone ([`GONE`]).
    pub fn gone(&self, file: &Path) -> bool {
        self.get(file).is_some_and(|(line, _)| line == GONE)
    }

    /// Put `note` in place of `file`'s row's line.
    pub fn insert(&mut self, file: PathBuf, note: (String, Tone)) {
        if self.by_file.get(&file) != Some(&note) {
            self.by_file.insert(file, note);
            self.changed();
        }
    }

    /// `file`'s row says its own line again.
    pub fn remove(&mut self, file: &Path) {
        if self.by_file.remove(file).is_some() {
            self.changed();
        }
    }

    /// Every row says its own line again.
    pub fn clear(&mut self) {
        if !self.by_file.is_empty() {
            self.by_file.clear();
            self.changed();
        }
    }

    /// Moves with every change to the notes, and only then.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Every note, by its row's file, as the rows draw them.
    pub fn by_file(&self) -> &HashMap<PathBuf, (String, Tone)> {
        &self.by_file
    }

    fn changed(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

/// Where results go that can't go where they were meant to, as the words
/// name it.
pub const KILN_RESULTS_PLACE: &str = "Documents\\Kiln results";

/// A row's line when its result couldn't be saved in the folder named
/// `folder`, so it went to Documents\Kiln results.
pub fn fallback_line(folder: &str) -> String {
    format!("Saved in {KILN_RESULTS_PLACE}. Kiln can't save in {folder}.")
}

/// A row's line when its result couldn't be saved where it was meant to,
/// nor in Documents\Kiln results: Kiln keeps it and tries again.
pub const KEPT_LINE: &str = "Couldn't save this. Kiln tries again the next time it starts.";

/// The InfoBar at start, once the results earlier runs left have been
/// tried: how many were saved, and where. `None` when there were none.
/// `fallback` is the folder the ones that couldn't go where they were
/// meant to went to. Where the others went is said from the folders they
/// went to (`Report::folders`), never from today's Settings, which may
/// not be what last time's were: the one folder by its name, or how many
/// folders.
pub fn delivered_info(r: &kiln_core::Report, fallback: Option<&Path>) -> Option<String> {
    let files = |n: usize| match n {
        1 => "1 converted file".to_owned(),
        n => format!("{n} converted files"),
    };
    let meant: Vec<&PathBuf> = r
        .folders
        .iter()
        .filter(|f| Some(f.as_path()) != fallback)
        .collect();
    let went = match meant.as_slice() {
        [folder] => format!("in {}", crate::actions::folder_name(folder)),
        // Delivered into the fallback folder itself.
        [] => format!("in {KILN_RESULTS_PLACE}"),
        folders => format!("in {} folders", folders.len()),
    };
    let saved = match (r.beside, r.fallback) {
        (0, 0) => None,
        (b, 0) => Some(format!("Saved {} from last time {went}.", files(b))),
        (0, f) => Some(format!(
            "Saved {} from last time in {KILN_RESULTS_PLACE}.",
            files(f)
        )),
        (b, f) => Some(format!(
            "Saved {} from last time: {b} {went}, {f} in {KILN_RESULTS_PLACE}.",
            files(b + f)
        )),
    };
    let again = "Kiln tries again the next time it starts.";
    match (saved, r.kept) {
        (None, 0) => None,
        (Some(s), 0) => Some(s),
        (Some(s), k) => Some(format!("{s} {k} couldn't be saved. {again}")),
        (None, k) => Some(format!(
            "{} from last time couldn't be saved. {again}",
            files(k)
        )),
    }
}

/// A notice's text once `names`, the results' names where they went, were
/// saved next to their originals.
pub fn saved_notice(names: &[String], into: Option<&str>) -> String {
    match (names, into) {
        ([name], None) => format!("Saved {name} next to the original."),
        (_, None) => format!("Saved {} files next to the originals.", names.len()),
        ([name], Some(folder)) => format!("Saved {name} in {folder}."),
        (_, Some(folder)) => format!("Saved {} files in {folder}.", names.len()),
    }
}

/// A notice's text when `name` couldn't be saved where it was meant to,
/// nor in Documents\Kiln results, so Kiln keeps it for the next start.
pub fn kept_notice(name: &str) -> String {
    format!("Couldn't save {name}. Kiln tries again the next time it starts.")
}

/// What to say when the results `names` couldn't be saved where they were
/// meant to, nor in Documents\Kiln results, so Kiln keeps them for the
/// next start: [`kept_notice`] for one. `None` for none.
pub fn kept_text(names: &[String]) -> Option<String> {
    match names {
        [] => None,
        [name] => Some(kept_notice(name)),
        _ => Some(format!(
            "Couldn't save {} files. Kiln tries again the next time it starts.",
            names.len()
        )),
    }
}

/// Where the results a notice is about went, for a notice that waits for
/// them to be saved next to their originals.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Saves {
    /// Each result saved next to its original, by its name there.
    pub saved: Vec<String>,
    /// The name of the folder each of `saved` went to, in its order.
    pub folders: Vec<String>,
    /// Each of `saved` where it went, in its order.
    pub paths: Vec<PathBuf>,
    /// The name of the folder chosen in Settings, when they went there
    /// rather than next to their originals.
    pub into: Option<String>,
    /// Each that couldn't be: its folder's name and the result's name.
    pub kept: Vec<(String, String)>,
    /// The row holding the first that couldn't be.
    pub first_kept: Option<PathBuf>,
    /// How many results stayed held without being sent there.
    pub held: usize,
}

impl Saves {
    /// What the notice says of where its results went, once every save is
    /// in: one that couldn't be saved is the news; otherwise where they
    /// were saved. `None` when some weren't sent next to the original, or
    /// none were: the notice reads as it did.
    pub fn notice(&self) -> Option<String> {
        if self.held > 0 {
            return None;
        }
        match self.kept.as_slice() {
            [] if self.saved.is_empty() => None,
            [] => Some(saved_notice(&self.saved, self.into.as_deref())),
            kept => {
                let names: Vec<String> = kept.iter().map(|(_, n)| n.clone()).collect();
                kept_text(&names)
            }
        }
    }

    /// The folder every saved result went to, and the results, when they
    /// all went to one: Show in folder then shows them all there. `None`
    /// when none was saved or they went to different folders.
    pub fn together(&self) -> Option<(PathBuf, Vec<PathBuf>)> {
        let folder = self.paths.first()?.parent()?;
        self.paths
            .iter()
            .all(|p| p.parent() == Some(folder))
            .then(|| (folder.to_path_buf(), self.paths.clone()))
    }

    /// Whether every result was saved next to its original: a click on the
    /// notice shows them there.
    pub fn all_saved(&self) -> bool {
        self.held == 0 && self.kept.is_empty() && !self.saved.is_empty()
    }

    /// The row a click on the notice opens when it says a result couldn't
    /// be saved: the row holding it, not the one the notice would name.
    /// `None` for any other notice.
    pub fn opens(&self) -> Option<&Path> {
        match self.notice() {
            Some(_) if !self.kept.is_empty() => self.first_kept.as_deref(),
            _ => None,
        }
    }
}

/// A row's line while its results are being saved.
pub const SAVING: &str = "Saving…";

/// A row's line while its results are being saved: "Saving…", and once a
/// copy to another drive says how far it is, "Saving… · 42%".
pub fn saving_line(fraction: Option<f32>) -> String {
    match fraction {
        None => SAVING.to_owned(),
        Some(f) => format!(
            "{SAVING}{DETAIL_SEPARATOR}{}%",
            (f.clamp(0.0, 1.0) * 100.0) as u32
        ),
    }
}

/// The footer, for as long as it has nothing more pressing to say, when
/// this run has no holding folder.
pub const NO_HOLDING: &str =
    "Kiln couldn't make its holding folder, so results are saved as soon as they're ready.";

/// What the footer shows: `status`, unless it says nothing more than the
/// plain count of listed files (`plain`), in which case a `sticky` warning
/// takes its place.
pub fn footer_status<'a>(status: &'a str, plain: &str, sticky: Option<&'a str>) -> &'a str {
    match sticky {
        Some(warning) if status.is_empty() || status == plain => warning,
        _ => status,
    }
}

/// A failed job's row line when it ran out of room writing `failed` and
/// that is on the same drive as the holding folder `holding`: "There isn't
/// room on C: to hold the result." `None` for anything else.
pub fn no_room_line(failed: &Path, holding: &Path) -> Option<String> {
    let drive = drive_of(holding)?;
    let same = drive_of(failed).is_some_and(|d| d.eq_ignore_ascii_case(&drive));
    same.then(|| format!("There isn't room on {drive} to hold the result."))
}

/// "C:" for a path on drive C, or the share for a UNC path.
fn drive_of(path: &Path) -> Option<String> {
    use std::path::{Component, Prefix};
    match path.components().next()? {
        Component::Prefix(p) => Some(match p.kind() {
            Prefix::Disk(d) | Prefix::VerbatimDisk(d) => {
                format!("{}:", (d as char).to_ascii_uppercase())
            }
            _ => p.as_os_str().to_string_lossy().into_owned(),
        }),
        _ => None,
    }
}

/// What the user is leaving by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Leave {
    Close,
    Clear,
    Remove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptButton {
    /// Stop the running jobs and go ahead.
    Stop(Leave),
    /// Add the files of a big folder after all.
    AddAll,
    /// Run the winget install of a helper app.
    Install,
    Cancel,
}

impl PromptButton {
    pub fn label(self) -> &'static str {
        match self {
            // Under "Stop converting and close?" and the like.
            PromptButton::Stop(Leave::Close) => "Close",
            PromptButton::Stop(Leave::Clear) => "Clear list",
            PromptButton::Stop(Leave::Remove) => "Remove from list",
            PromptButton::AddAll => "Add them all",
            PromptButton::Install => "Install",

            PromptButton::Cancel => "Cancel",
        }
    }
}

/// What becomes of a result that is ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoSave {
    /// It stays where it is: held until exported, or already saved.
    Hold,
    /// It is saved next to the original now.
    Beside,
}

impl AutoSave {
    /// How many of a job's `outputs` results this leaves held, for the
    /// footer's export hint: none of those on their way next to the
    /// original.
    pub fn held(self, outputs: usize) -> usize {
        match self {
            AutoSave::Hold => outputs,
            AutoSave::Beside => 0,
        }
    }
}

/// What to do with a ready result: `holding` says this run holds results
/// at all. Every result held is saved next to its original, or in the
/// folder chosen in Settings, as soon as it is ready; without a holding
/// folder it was written there already.
pub fn autosave_for(holding: bool) -> AutoSave {
    if holding {
        AutoSave::Beside
    } else {
        AutoSave::Hold
    }
}

/// The question asked before adding a folder's many files.
pub fn big_folder_prompt(files: usize) -> Prompt {
    Prompt {
        heading: None,
        title: format!("That's {files} files. Add them all?"),
        buttons: vec![PromptButton::AddAll, PromptButton::Cancel],
    }
}

/// The question asked before closing, clearing or removing a row, and
/// Kiln's other prompts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    /// The dialog's title, over `title` as its text; `None` when `title`
    /// says it all and is the dialog's title.
    pub heading: Option<String>,
    /// What is at stake, in sentences. It is also what Windows is told
    /// holds up the end of a session.
    pub title: String,
    pub buttons: Vec<PromptButton>,
}

impl Prompt {
    /// The dialog's title.
    pub fn dialog_title(&self) -> &str {
        self.heading.as_deref().unwrap_or(&self.title)
    }

    /// The dialog's text under its title, if it has any.
    pub fn dialog_text(&self) -> Option<&str> {
        self.heading.as_ref().map(|_| self.title.as_str())
    }

    /// The button that is the accent, which the keyboard starts on and
    /// Enter presses there: Cancel. A Stop never is.
    pub fn accent(&self) -> Option<usize> {
        self.buttons.iter().position(|&b| b == PromptButton::Cancel)
    }

    /// What button `b` says in this prompt: Cancel, beside Stop, says
    /// "Keep converting".
    pub fn label(&self, b: PromptButton) -> &'static str {
        let stops = self
            .buttons
            .iter()
            .any(|b| matches!(b, PromptButton::Stop(_)));
        match b {
            PromptButton::Cancel if stops => "Keep converting",
            b => b.label(),
        }
    }
}

/// What to ask before `verb` stops `running` jobs. `None` when nothing is
/// at stake: a result never waits to be saved.
pub fn leave_prompt(running: usize, verb: Leave) -> Option<Prompt> {
    if running == 0 {
        return None;
    }
    let mut sentences = vec![match running {
        1 => "1 file is still converting.".to_owned(),
        n => format!("{n} files are still converting."),
    }];
    // Closing says what stopping costs, as the mock does.
    if verb == Leave::Close {
        sentences.push(match running {
            1 => "If you close Kiln now, it stops and no new file is made.".to_owned(),
            _ => "If you close Kiln now, they stop and no new file is made.".to_owned(),
        });
    }
    let heading = match verb {
        Leave::Close => "Stop converting and close?",
        Leave::Clear => "Stop converting and clear the list?",
        Leave::Remove => "Stop converting and remove from the list?",
    };
    Some(Prompt {
        heading: Some(heading.to_owned()),
        title: sentences.join(" "),
        buttons: vec![PromptButton::Cancel, PromptButton::Stop(verb)],
    })
}
/// What a leave held up by exports may do now that a message is handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Due {
    Nothing,
    /// Its exports are all done: go ahead with it.
    Resume,
    /// Close was asked for once every export is saved, and they are.
    Close,
}

/// `ready` says a leave's exports are all done, `close` that Kiln is to
/// close once saved, `exporting` that an export, or the saving of what
/// last time left, still runs. While a dialog is open, nothing goes ahead:
/// a prompt, a folder picker or a closed window under it would pull the
/// ground from under that dialog. It goes ahead once the dialog is closed.
pub fn leave_due(modal: bool, ready: bool, close: bool, exporting: bool) -> Due {
    if modal {
        Due::Nothing
    } else if ready {
        Due::Resume
    } else if close && !exporting {
        Due::Close
    } else {
        Due::Nothing
    }
}

/// What the footer's accent button says for a selection of `selected`
/// rows: "Convert", or for several, "Convert 5 files" (UX-05); with the
/// pane showing, the format it converts to as well (`to`, an extension):
/// "Convert to JPG", "Convert 5 files to JPG".
pub fn convert_label(selected: usize, to: Option<&str>) -> String {
    let code = to.map(|ext| ext.trim_start_matches('.').to_ascii_uppercase());
    match (selected, code) {
        (0 | 1, None) => "Convert".to_owned(),
        (n, None) => format!("Convert {n} files"),
        (0 | 1, Some(code)) => format!("Convert to {code}"),
        (n, Some(code)) => format!("Convert {n} files to {code}"),
    }
}

/// Why the footer's Convert is disabled for a selection of `selected`
/// rows, `running` of which are converting and `copying` of which have
/// their result being copied to another drive; `None` when it can be used.
pub fn convert_block(selected: usize, running: usize, copying: usize) -> Option<&'static str> {
    if selected == 0 {
        Some("Select a file to convert.")
    } else if running > 0 {
        Some("A selected file is still converting. Wait for it, or cancel it.")
    } else if copying > 0 {
        Some(COPYING_BLOCK)
    } else {
        None
    }
}

/// Why Convert waits while a selected row's result is being copied to
/// another drive.
pub const COPYING_BLOCK: &str =
    "A selected result is still being copied to another drive. Wait for it, or cancel it.";

/// A header or footer button the keyboard can be on, besides the list, or
/// a control in the inspector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FocusButton {
    Convert,
    AddFiles,
    Clear,
    Help,
    Settings,
    Inspector(InspectorControl),
    /// The empty window's first-run card's Dismiss.
    Dismiss,
    /// The Dismiss of the InfoBar at the top of the list.
    InfoDismiss,
    /// The credit's link, at the window's bottom left.
    Credit,
}

/// Where Tab goes from `at`, or Shift+Tab with `back`; `None` is the list.
/// The order is the reading order: the header's Help and Settings, the
/// list, the inspector's controls (`inspector`, as it lists them), then
/// the footer. With files `listed` the footer runs from the list through
/// the inspector, then the footer as it is drawn, left to right: Add
/// files, Clear and Convert (while
/// it is `convert`, enabled, as a disabled control isn't a stop), then the
/// header's Help and Settings, and round to the list. The empty window has no list and no
/// inspector: Add files, Help, Settings and round, with `None` before Add
/// files.
pub fn tab_stop(
    at: Option<FocusButton>,
    back: bool,
    listed: bool,
    convert: bool,
    inspector: &[InspectorControl],
) -> Option<FocusButton> {
    tab_stop_with(at, back, listed, convert, false, inspector)
}

/// As [`tab_stop`], with the InfoBar at the top of the list showing
/// (`info`): its Dismiss sits between the header and the list, where it
/// is read.
pub fn tab_stop_with(
    at: Option<FocusButton>,
    back: bool,
    listed: bool,
    convert: bool,
    info: bool,
    inspector: &[InspectorControl],
) -> Option<FocusButton> {
    tab_stop_all(at, back, listed, convert, info, false, inspector)
}

/// As [`tab_stop_with`], with the credit's link shown (`credit`): at the
/// footer's left, under the status, it comes before Add files.
pub fn tab_stop_all(
    at: Option<FocusButton>,
    back: bool,
    listed: bool,
    convert: bool,
    info: bool,
    credit: bool,
    inspector: &[InspectorControl],
) -> Option<FocusButton> {
    use FocusButton::*;
    let order: Vec<Option<FocusButton>> = if listed {
        [Some(Help), Some(Settings)]
            .into_iter()
            .chain(info.then_some(Some(InfoDismiss)))
            .chain([None])
            .chain(inspector.iter().map(|&c| Some(Inspector(c))))
            .chain(credit.then_some(Some(Credit)))
            .chain([Some(AddFiles), Some(Clear)])
            .chain(convert.then_some(Some(Convert)))
            .collect()
    } else {
        vec![Some(Help), Some(Settings), Some(AddFiles)]
    };
    let n = order.len();
    match order.iter().position(|&x| x == at) {
        Some(i) if back => order[(i + n - 1) % n],
        Some(i) => order[(i + 1) % n],
        // From somewhere not in the order: a button that has just gone
        // hands the focus back to the list; the empty window, which has no
        // list, starts at its first stop or, going back, its last.
        None if listed => None,
        None if back => order[n - 1],
        None => order[0],
    }
}

/// Where Tab goes in the empty window, from `at` or back with `back`:
/// Help, Settings, Add files, then the first-run card's Dismiss while
/// `first_run` shows it, and round. From somewhere not on the path it
/// starts at the first stop, or going back at the last.
pub fn empty_tab_stop(
    at: Option<FocusButton>,
    back: bool,
    first_run: bool,
    info: bool,
) -> Option<FocusButton> {
    empty_tab_stop_all(at, back, first_run, info, false)
}

/// As [`empty_tab_stop`], with the credit's link in the bottom-left
/// corner (`credit`): the last stop, as it is the last thing read.
pub fn empty_tab_stop_all(
    at: Option<FocusButton>,
    back: bool,
    first_run: bool,
    info: bool,
    credit: bool,
) -> Option<FocusButton> {
    use FocusButton::*;
    let order: Vec<FocusButton> = [Help, Settings]
        .into_iter()
        .chain(info.then_some(InfoDismiss))
        .chain([AddFiles])
        .chain(first_run.then_some(Dismiss))
        .chain(credit.then_some(Credit))
        .collect();
    let n = order.len();
    Some(match order.iter().position(|&x| Some(x) == at) {
        Some(i) if back => order[(i + n - 1) % n],
        Some(i) => order[(i + 1) % n],
        None if back => order[n - 1],
        None => order[0],
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub folder: String,
    /// Format code shown in monospace: "PNG". Empty if there's no extension.
    pub code: String,
    pub size: u64,
    /// When the file was last changed, as read when it was listed, or
    /// newer once a preview found it changed since. `None` if Windows
    /// couldn't say. Previews are keyed by it, so the window never has to
    /// ask the disk.
    pub modified: Option<SystemTime>,
    /// Codes Kiln can convert this file to now, from formats.toml, with the
    /// helper apps installed.
    pub targets: Vec<String>,
    /// What a look at the file's first bytes found when it was added.
    pub check: FileCheck,
    /// A helper app some formats need and Kiln can't find.
    pub missing: Option<Missing>,
    /// Whether FFmpeg was found when the formats were worked out: with it,
    /// a video Windows can't decode converts through it.
    pub ffmpeg_found: bool,
    /// Saving this row's held result next to its original didn't work, so
    /// the row offers to choose another folder.
    pub save_failed: bool,
    pub job: JobState,
    /// What `inspect` found, once it has.
    pub facts: Option<Facts>,
    /// What Kiln puts forward for this file alone, from its facts.
    pub suggestion: Option<Suggestion>,
    /// When the facts arrived, for the second line's crossfade.
    pub facts_at: Option<Instant>,
    /// What was last asked of this file alone, for Retry. Set when a
    /// conversion or a one-file tool starts; a merge, which takes several
    /// files, leaves it unset.
    pub last: Option<Action>,
    /// The row's job was started by a drop on the drag-wheel, or redone
    /// from its Options: a finished conversion offers Options.
    pub from_drag: bool,
    /// What the job sent for its details, shown after its line while it
    /// runs and once it is done: what the conversion leaves out ("Keeps the
    /// first of 2 sound tracks."), then "Encoded on the processor." when
    /// Automatic fell back from the graphics card, joined with
    /// [`DETAIL_SEPARATOR`] (`job_note`).
    pub job_note: Option<String>,
    /// How big its finished job's results are, all told, once they are
    /// read: the done row's "31 MB → 9.6 MB, 69% smaller".
    pub result_bytes: Option<u64>,
    /// The helper app ("ffmpeg") a conversion asked of this row waited on
    /// and didn't find: the row's note names it ("Needs FFmpeg to become
    /// MOV") and its link gets it. Cleared when the row's next job
    /// starts or the helper is found.
    pub waits_on: Option<String>,
}

impl Entry {
    /// Whether the file's facts found a subtitle track in it. Until they
    /// are in, it is taken to have none.
    pub fn has_subtitles(&self) -> bool {
        self.facts
            .as_ref()
            .and_then(|f| f.media.as_ref())
            .is_some_and(|m| m.subtitle_tracks > 0)
    }

    /// The row as it comes back after Remove is undone: a result that was
    /// saved is still so, but anything else (a job that was
    /// stopped, a result that was discarded) is a file waiting to be
    /// converted again. Its note went with it, so it waits on no helper
    /// a conversion found missing.
    pub fn restored(mut self) -> Entry {
        let kept = matches!(&self.job, JobState::Done { saved: Some(_), .. });
        if !kept {
            self.job = JobState::Idle;
        }
        self.waits_on = None;
        self
    }

    /// The format a finished drag-wheel conversion's Options redoes it to:
    /// the row's "Options" link opens the options sheet for this pair, and
    /// choosing there converts again. Any finished conversion a drop on
    /// the drag-wheel started has it, whether its result is held, saved
    /// or gone. A failure, a tool, and a conversion chosen over Kiln's
    /// window have none.
    pub fn options_for(&self) -> Option<&str> {
        match &self.job {
            JobState::Done {
                action: Action::Convert(ext),
                ..
            } if self.from_drag => Some(ext),
            _ => None,
        }
    }

    /// Whether a new job running `action` on this row is still a drop's: a
    /// Retry of a drag-wheel conversion that was cancelled or failed, so
    /// that once done it offers Options as the drop's would have.
    pub fn retries_drop(&self, action: &Action) -> bool {
        self.from_drag && self.retryable() && self.last.as_ref() == Some(action)
    }

    /// A redo from Options has ended, its job's end (`Done`, `Failed` or
    /// `Cancelled`) now the row's job; `prior` is what the row was before
    /// the redo started. The old result was kept all the while.
    /// - Done: the new result replaces the old one. What goes is the old
    ///   result the row held, as a replace's Discard takes it, so the
    ///   holding folder keeps nothing no row names. A result already saved
    ///   next to the original or exported is the person's own file, and is
    ///   never touched.
    /// - Failed or cancelled: the row is back to its old result, untouched,
    ///   held (and so exportable, and counted when Kiln is closed) as
    ///   before. Nothing goes.
    ///
    /// Returns the files to take away.
    pub fn end_redo(&mut self, prior: JobState) -> Vec<PathBuf> {
        if !matches!(self.job, JobState::Done { .. }) {
            self.job = prior;
            return vec![];
        }
        let new = self.held().unwrap_or_default();
        held_in(&prior)
            .unwrap_or_default()
            .iter()
            // Never a file the new result is.
            .filter(|p| !new.contains(p))
            .cloned()
            .collect()
    }

    /// Whether the row offers Retry: what it was doing is known, and it
    /// was cancelled or failed in a way that trying again can fix (a
    /// damaged file would fail the same way, so it gets no Retry).
    pub fn retryable(&self) -> bool {
        self.last.is_some()
            && match &self.job {
                JobState::Cancelled => true,
                JobState::Failed { message, .. } => kiln_core::retry_may_help(message),
                _ => false,
            }
    }

    /// The second line of the row and its tone, following the job. The
    /// job's note follows its line while it runs and once it is done.
    pub fn line(&self) -> (String, Tone) {
        let (line, tone) = self.job_line();
        match (&self.job, &self.job_note) {
            (JobState::Running { .. } | JobState::Done { .. }, Some(note)) => {
                (format!("{line}{DETAIL_SEPARATOR}{note}"), tone)
            }
            _ => (line, tone),
        }
    }

    /// The row's second line in two parts, for a row that says a problem in
    /// one colour and the file's size after it in another: the line without
    /// the size and the size with its separator. A line with no size
    /// to split off is all of it in the first part.
    pub fn line_parts(&self) -> (String, Option<String>) {
        let (line, tone) = self.line();
        if tone == Tone::Crack && matches!(self.job, JobState::Idle) {
            let tail = format!("{DETAIL_SEPARATOR}{}", human_size(self.size));
            if let Some(head) = line.strip_suffix(&tail) {
                return (head.to_owned(), Some(tail));
            }
        }
        (line, None)
    }

    /// A saved row's result for a narrow list, which has the name on the
    /// line above: "Saved · 29 MB, about the same size", the size it came
    /// to and how it changed, without the name or the size it came from
    /// . `None` for a row that isn't saved to a folder.
    pub fn short_line(&self) -> Option<String> {
        let JobState::Done {
            saved: Some(Saved::Folder(_)),
            ..
        } = &self.job
        else {
            return None;
        };
        let mut line = "Saved".to_owned();
        if let Some(after) = self.result_bytes {
            line.push_str(DETAIL_SEPARATOR);
            line.push_str(&human_size(after));
            if let Some(words) = crate::inspector::change_words(self.size, after) {
                line.push_str(", ");
                line.push_str(&words);
            }
        }
        if let Some(note) = &self.job_note {
            line.push_str(DETAIL_SEPARATOR);
            line.push_str(note);
        }
        Some(line)
    }

    /// [`Entry::line`] without the job's note.
    fn job_line(&self) -> (String, Tone) {
        match &self.job {
            JobState::Idle => (
                self.detail(),
                match self.check {
                    FileCheck::Ok | FileCheck::Unknown => Tone::Muted,
                    _ => Tone::Crack,
                },
            ),
            // How far along is at the row's right end, and how long is
            // left in the side pane (mock screen 5).
            JobState::Running { action, .. } => (doing(action), Tone::Muted),
            JobState::Done {
                saved: Some(Saved::Folder(_)),
                ..
            } => (
                format!(
                    "Saved as {}{}",
                    self.results_named().unwrap_or_default(),
                    self.change()
                        .map(|c| format!("{DETAIL_SEPARATOR}{c}"))
                        .unwrap_or_default()
                ),
                Tone::Result,
            ),
            // On its way to its folder; one kept for the next start has
            // its note in its place.
            JobState::Done { .. } => (SAVING.to_owned(), Tone::Muted),
            JobState::Failed { .. } => (self.failure_sentence(), Tone::Crack),
            JobState::Cancelled => ("Cancelled.".to_owned(), Tone::Muted),
        }
    }

    /// A finished row's results against its file: "31 MB → 9.6 MB, 69%
    /// smaller", once their size is read.
    fn change(&self) -> Option<String> {
        let after = self.result_bytes?;
        Some(crate::inspector::change_line(self.size, after))
    }

    /// What the row's kind of file is called in a failure's words: "video",
    /// "sound file", or its type code ("PDF"). `None` where Kiln can't
    /// tell.
    fn failure_noun(&self) -> Option<String> {
        match self.facts.as_ref().map(|f| f.kind) {
            Some(Kind::Video) => Some("video".to_owned()),
            Some(Kind::Audio) => Some("sound file".to_owned()),
            _ if !self.code.is_empty() => Some(self.code.to_ascii_uppercase()),
            _ => None,
        }
    }

    /// A failed row's sentence, whole and on one line, in UX-13's words:
    /// what happened and what to do next. A damaged video or sound file
    /// says to try playing it in another app; a damaged PDF names itself.
    /// Every other sentence is the engine's plain one, as it came.
    pub fn failure_sentence(&self) -> String {
        let JobState::Failed { message, .. } = &self.job else {
            return String::new();
        };
        let message = one_line(message);
        let media_damaged =
            message.starts_with("This ") && message.ends_with(" is damaged and can't be read.");
        if media_damaged {
            return match self.facts.as_ref().map(|f| f.kind) {
                Some(Kind::Video) => {
                    "This video looks damaged. Try playing it in another app.".to_owned()
                }
                Some(Kind::Audio) => {
                    "This sound file looks damaged. Try playing it in another app.".to_owned()
                }
                _ => "This file looks damaged. Try opening it in another app.".to_owned(),
            };
        }
        let code = self.code.to_ascii_uppercase();
        let named = format!("This file looks damaged, or isn't really a {code}.");
        if !code.is_empty() && code == "PDF" && message.starts_with(&named) {
            return message.replacen("This file", "This PDF", 1);
        }
        message
    }

    /// A failure's title, over its sentence in the side pane's InfoBar
    /// (mock screen 8b): "Couldn't read this PDF" for a damaged file or a
    /// JPG cut short, else
    /// "Couldn't convert this file", or for a tool, "Couldn't finish".
    pub fn failure_title(&self) -> String {
        let sentence = self.failure_sentence();
        if sentence.contains("looks damaged") || sentence.contains("looks cut short") {
            return match self.failure_noun() {
                Some(noun) => format!("Couldn't read this {noun}"),
                None => "Couldn't read this file".to_owned(),
            };
        }
        match &self.last {
            Some(Action::Tool(..)) => "Couldn't finish".to_owned(),
            _ => "Couldn't convert this file".to_owned(),
        }
    }

    /// What the row shows at its right end (mock screens 2 and 5).
    pub fn mark(&self) -> RowMark {
        match &self.job {
            JobState::Running { fraction, .. } => RowMark::Progress(*fraction),
            JobState::Done { .. } => RowMark::Done,
            JobState::Failed { .. } => RowMark::Failed,
            JobState::Idle | JobState::Cancelled => RowMark::None,
        }
    }

    /// What a finished row's results are called: "photo.jpg", or "photo.jpg
    /// and 2 more". `None` where there is no result to name.
    pub fn results_named(&self) -> Option<String> {
        let JobState::Done { outputs, .. } = &self.job else {
            return None;
        };
        let first = outputs.first()?.file_name()?.to_string_lossy().into_owned();
        Some(match outputs.len() {
            1 => first,
            k => format!("{first} and {} more", k - 1),
        })
    }

    /// The results this row holds, if it holds any: a finished job that
    /// hasn't been exported.
    pub fn held(&self) -> Option<&[PathBuf]> {
        held_in(&self.job)
    }

    /// True when the row holds results and any of them is no longer
    /// there. `exists` says whether a path is still on disk.
    pub fn is_gone(&self, exists: impl Fn(&Path) -> bool) -> bool {
        self.held()
            .is_some_and(|outputs| outputs.iter().any(|p| !exists(p)))
    }

    /// The link at the right of the row: "Show details" on a failed row
    /// with more to show, Show in folder on a saved one. `cut` says
    /// the row's line doesn't fit, so its whole sentence is more to show.
    pub fn link(&self, cut: bool) -> Option<&'static str> {
        match &self.job {
            // The extension the job needed: its link opens its Store page.
            JobState::Failed {
                store_product: Some(product),
                ..
            } => Some(store_label(product)),
            // Show details is on every failure (UX-13).
            JobState::Failed { .. } if self.details(cut).is_some() => Some("Show details"),
            _ if self.held().is_some() => None,
            _ if self.saved().is_some() => Some(SHOW_IN_FOLDER),
            // A file nothing has been done to says how to start: Convert,
            // or Get the helper it can't be converted without. A file
            // ruled out on adding has nothing to offer.
            JobState::Idle if self.check != FileCheck::Ok => None,
            // A conversion left it out for a missing helper: its note names
            // the helper, and Get it gets it.
            JobState::Idle if self.waits_on.is_some() => {
                self.waits_on.as_deref().and_then(waits_on_link)
            }
            // A file waiting on a Store extension still converts: copies
            // need no decoder, and its line names the formats that do.
            JobState::Idle => match self.needs_helper() {
                Some(b) => get_link(b),
                None if !self.targets.is_empty() => Some("Convert"),
                None => self
                    .needs_extension()
                    .map(|(_, product)| store_label(product)),
            },
            _ => None,
        }
    }

    /// The Microsoft Store extension (codec, Store product) a video waits
    /// on before any conversion: Windows can't decode its picture
    /// ("HEVC" without Microsoft's HEVC Video Extensions). Only while FFmpeg
    /// isn't found either, since with FFmpeg the file converts through it, and only for a file nothing has been done to.
    pub fn needs_extension(&self) -> Option<(&'static str, &'static str)> {
        if !(matches!(self.job, JobState::Idle)
            && self.check == FileCheck::Ok
            && !self.ffmpeg_found)
        {
            return None;
        }
        self.facts.as_ref()?.media.as_ref()?.needs_extension
    }

    /// The row's sentence for [`Entry::needs_extension`], naming the
    /// formats that need the extension, those the picture is encoded for: "Needs Microsoft's AV1 Video Extensions to convert AV1 video
    /// to MP4, MOV or M4V." Where every format copies the picture, only a
    /// changed picture needs it: "Needs Microsoft's HEVC Video Extensions
    /// to re-encode HEVC video." The row keeps Convert; the inspector has
    /// Get it.
    pub fn extension_line(&self) -> Option<String> {
        let (codec, _) = self.needs_extension()?;
        let media = self.facts.as_ref()?.media.as_ref()?;
        let encoded: Vec<String> = self
            .targets
            .iter()
            .filter(|t| kiln_core::quality::decodes_picture(media, t))
            .map(|t| t.to_ascii_uppercase())
            .collect();
        let needs = format!("Needs Microsoft's {codec} Video Extensions");
        Some(match encoded.split_last() {
            None => format!("{needs} to re-encode {codec} video."),
            Some((last, [])) => format!("{needs} to convert {codec} video to {last}."),
            Some((last, rest)) => format!(
                "{needs} to convert {codec} video to {} or {last}.",
                rest.join(", ")
            ),
        })
    }

    /// The sound Kiln can't read in a video, said on the row before any
    /// conversion: "This file's audio is AC3, which Kiln can't read
    /// yet." for AC-3 or E-AC-3 in Matroska, as for DTS. Only while FFmpeg
    /// isn't found, since with FFmpeg such a file converts through it, and only for a file nothing has been done to. The row keeps
    /// Convert: the picture, and sound a container holds as it is, may
    /// still be copied.
    pub fn sound_line(&self) -> Option<String> {
        if !(matches!(self.job, JobState::Idle)
            && self.check == FileCheck::Ok
            && !self.ffmpeg_found)
        {
            return None;
        }
        let f = self.facts.as_ref()?;
        let codec = f.media.as_ref()?.codec.clone()?;
        let unread = kiln_media::MediaError::Unsupported { codec }.to_string();
        (f.kind == Kind::Video && f.audio_problem.as_deref() == Some(unread.as_str()))
            .then_some(unread)
    }

    /// The Store product the row's Get it opens: the extension the file
    /// waits on, or the one its failed job needed.
    pub fn store_product(&self) -> Option<&'static str> {
        match &self.job {
            JobState::Failed { store_product, .. } => *store_product,
            _ => self.needs_extension().map(|(_, product)| product),
        }
    }

    /// The helper app ("ffmpeg") a file nothing has been done to can't be
    /// converted without: it has no formats at all until that is
    /// installed. Its link, and the inspector's button, get it.
    /// A file whose sound Kiln found something wrong with (a damaged MP3)
    /// says that instead, with no link to a sound helper (Windows' Media
    /// Feature Pack): installing it wouldn't mend the file. A silent
    /// video still gets FFmpeg, which its picture needs.
    pub fn needs_helper(&self) -> Option<&str> {
        match (&self.job, &self.missing) {
            (JobState::Idle, Some(m))
                if self.check == FileCheck::Ok
                    && self.targets.is_empty()
                    && !(self.sound_problem() && sound_helper(&m.backend)) =>
            {
                Some(&m.backend)
            }
            _ => None,
        }
    }

    /// The helper app some of the formats of a file nothing has been done
    /// to wait on, while others are offered: the row's "14 formats need
    /// FFmpeg". `None` when every format waits on it
    /// ([`Entry::needs_helper`]) or none does.
    pub fn partly_needs(&self) -> Option<&str> {
        match (&self.job, &self.missing) {
            (JobState::Idle, Some(m))
                if self.check == FileCheck::Ok && !self.targets.is_empty() =>
            {
                Some(&m.backend)
            }
            _ => None,
        }
    }

    /// Whether the row's sentence is what Kiln found wrong with the
    /// file's sound, as it is when nothing is left it can become.
    fn sound_problem(&self) -> bool {
        self.targets.is_empty()
            && self
                .facts
                .as_ref()
                .is_some_and(|f| f.silent || f.audio_problem.is_some())
    }

    /// What Show details opens on a failed row (mock screen 8): the
    /// dialog's title, naming the helper app whose words it shows, its
    /// plain line, and the engine's own words, empty
    /// when it had none. The plain line is the row's whole sentence; a
    /// file Kiln couldn't read is named in it, as "Kiln couldn't read
    /// scan.pdf. It looks damaged, ...". Every failure has Show details
    /// (UX-13), cut short on its row or not (`cut` is kept for the callers
    /// that know it).
    pub fn details(&self, _cut: bool) -> Option<(String, String, String)> {
        let JobState::Failed {
            details, engine, ..
        } = &self.job
        else {
            return None;
        };
        let sentence = self.failure_sentence();
        let title = self.failure_title();
        let lead = match crate::inspector::under_title(&title, sentence.clone()) {
            it if it != sentence => format!("Kiln couldn't read {}. {it}", self.name),
            _ => sentence.clone(),
        };
        // The log's first line is dropped when it only says the sentence
        // again, in lower case: the other lines stay.
        let said = details.trim();
        let said = match said.split_once('\n') {
            Some((first, rest)) if repeats(first, &[&lead, &sentence]) => rest.trim(),
            None if repeats(said, &[&lead, &sentence]) => "",
            _ => said,
        };
        // A helper app's own words say whose they are; Kiln's own, or none,
        // are what happened.
        let heading = match engine {
            Some(e) if *e != "Kiln" && !said.is_empty() => format!("What {e} said"),
            _ => DETAILS_TITLE.to_owned(),
        };
        Some((heading, lead, said.to_owned()))
    }

    /// Where an exported row's results went: the folder and the files. A
    /// row whose results were dragged out has no folder to show.
    pub fn saved(&self) -> Option<(&Path, &[PathBuf])> {
        match &self.job {
            JobState::Done {
                outputs,
                saved: Some(Saved::Folder(folder)),
                ..
            } => Some((folder, outputs)),
            _ => None,
        }
    }

    /// [`Entry::line`], except a held result that has gone says so.
    pub fn line_checked(&self, exists: impl Fn(&Path) -> bool) -> (String, Tone) {
        if self.is_gone(exists) {
            (GONE.to_owned(), Tone::Crack)
        } else {
            self.line()
        }
    }

    /// The secondary line under the file name, before any job (mock
    /// screen 2): what the file is and its size, as [`row_facts`] says it.
    /// A file with nothing to offer says why instead, and one waiting on a
    /// Store extension, or whose sound Kiln can't read, says that. Neither
    /// the folder (the side pane and the tooltip have it) nor a suggestion
    /// (the side pane puts it first) is on the row.
    pub fn detail(&self) -> String {
        if let Some(line) = self.extension_line().or_else(|| self.sound_line()) {
            return line;
        }
        if self.offers_nothing() {
            return self.plain_detail();
        }
        self.with_missing(row_facts(self.facts.as_ref(), &self.code, self.size))
    }

    /// `line`, then, when some of the file's formats wait on a helper app
    /// that isn't installed, how many and which: "· 14 formats need
    /// FFmpeg" (UX-04: the row says what is missing).
    fn with_missing(&self, line: String) -> String {
        match self.missing_part() {
            Some(part) => format!("{line}{DETAIL_SEPARATOR}{part}"),
            None => line,
        }
    }

    /// How many of the file's formats wait on a helper app that isn't
    /// installed, and which: "14 formats need FFmpeg". `None` when none
    /// do, or when the file has no other formats.
    fn missing_part(&self) -> Option<String> {
        match &self.missing {
            Some(m) if self.check == FileCheck::Ok && !self.targets.is_empty() => Some(format!(
                "{} {} {}",
                m.count,
                if m.count == 1 {
                    "format needs"
                } else {
                    "formats need"
                },
                helper_name(&m.backend)
            )),
            _ => None,
        }
    }

    /// Whether the row's line says why the file has nothing to offer, in
    /// the place of what it is.
    fn offers_nothing(&self) -> bool {
        self.check != FileCheck::Ok || self.targets.is_empty()
    }

    /// The row's second line as labelled parts, for a screen reader to
    /// read apart: "1600 by 1200 photo. File size 3.9 MB. Folder files."
    pub fn spoken_detail(&self) -> String {
        let mut parts: Vec<String> = vec![];
        if let Some(line) = self.extension_line().or_else(|| self.sound_line()) {
            parts.push(line.trim_end_matches('.').to_owned());
        } else if self.offers_nothing() {
            // What is wrong with the file, without the size.
            let plain = self.plain_detail();
            if let Some(first) = plain.split(DETAIL_SEPARATOR).next() {
                // Joined with full stops below: one of its own would read
                // twice.
                parts.push(first.trim_end_matches('.').to_owned());
            }
        } else {
            let line = row_facts(self.facts.as_ref(), &self.code, self.size);
            let mut said: Vec<&str> = line.split(DETAIL_SEPARATOR).collect();
            // The size is said with its label below.
            said.pop();
            parts.extend(
                said.iter()
                    .map(|p| crate::tune_sheet::spoken(&p.replace(" × ", " by "))),
            );
        }
        if self.check == FileCheck::Ok {
            parts.push(format!("File size {}", human_size(self.size)));
        }
        // As the row shows it after the size.
        if self
            .extension_line()
            .or_else(|| self.sound_line())
            .is_none()
        {
            parts.extend(self.missing_part());
        }
        parts.push(format!("Folder {}", self.folder_name()));
        parts.join(". ")
    }

    /// The secondary line before facts arrive: its type and size, or for a
    /// file with nothing to offer, why, then its size.
    pub fn plain_detail(&self) -> String {
        if !self.offers_nothing() {
            return self.with_missing(row_facts(None, &self.code, self.size));
        }
        match self.check {
            // Nothing in it: the size would only say the same.
            FileCheck::Empty => self.reach(),
            _ => format!(
                "{}{DETAIL_SEPARATOR}{}",
                self.reach(),
                human_size(self.size)
            ),
        }
    }

    /// The first part of [`Entry::plain_detail`]: what the file converts
    /// to, or why it converts to nothing ("Converts to 5 formats", "Isn't
    /// really a DOCX file", "Needs FFmpeg to convert").
    pub fn reach(&self) -> String {
        match self.check {
            FileCheck::Empty => "Empty file (0 bytes)".to_owned(),
            FileCheck::Mismatch => format!("Isn't really a {} file", self.code),
            FileCheck::CutShort => {
                "Looks cut short. Try copying or downloading it again.".to_owned()
            }
            FileCheck::Unreadable => {
                "Can't read this file. Check it isn't open in another app.".to_owned()
            }
            FileCheck::Unknown if self.code.is_empty() => "Unknown type".to_owned(),
            FileCheck::Unknown => format!("Unknown type (.{})", self.code),
            FileCheck::Ok => match (self.targets.len(), &self.missing) {
                // A file left with nothing it can become, whose sound Kiln
                // found something wrong with (a silent video, a damaged
                // MP3), says that rather than which helper is missing.
                (0, _) if self.sound_problem() => self
                    .facts
                    .as_ref()
                    .and_then(|f| f.audio_problem.clone())
                    .unwrap_or_default(),
                // Part of Windows, not a helper app: the spec's own line,
                // beside Get Media Feature Pack.
                (0, Some(m)) if m.backend == "audio-mf" => {
                    "Needs Windows' Media Feature Pack, which isn't installed.".to_owned()
                }
                (0, Some(m)) if m.makes_pdf => {
                    format!("Needs {} to make a PDF", helper_name(&m.backend))
                }
                (0, Some(m)) => format!("Needs {} to convert", helper_name(&m.backend)),
                (0, None) => "No formats for this type".to_owned(),
                (n, Some(m)) => format!(
                    "Converts to {n} {}, {} more {} {}",
                    if n == 1 { "format" } else { "formats" },
                    m.count,
                    if m.count == 1 { "needs" } else { "need" },
                    helper_name(&m.backend)
                ),
                (1, None) => format!("Converts to {}", self.targets[0]),
                (n, None) => format!("Converts to {n} formats"),
            },
        }
    }

    /// The last part of the file's folder: "samples" for
    /// `C:\kiln\target\samples`.
    pub fn folder_name(&self) -> String {
        crate::actions::folder_name(Path::new(&self.folder))
    }

    /// The row's tooltip: the line whole when the row had to cut it
    /// (`cut`), a failed row's sentence just as it was written, then the
    /// folder's full path on a line of its own. `shown` is the line the
    /// row draws.
    pub fn tooltip(&self, shown: &str, cut: bool) -> String {
        let line = match &self.job {
            JobState::Failed { message, .. } if cut => Some(message.trim().to_owned()),
            _ if cut => Some(shown.to_owned()),
            _ => None,
        };
        // Where a saved result went, in full.
        let more = match &self.job {
            JobState::Done {
                saved: Some(Saved::Folder(folder)),
                ..
            } => Some(format!("Saved to {}", folder.display())),
            _ => None,
        };
        let mut lines: Vec<String> = line.into_iter().collect();
        lines.push(self.folder.clone());
        lines.extend(more);
        lines.join("\n")
    }
}

/// What a file is, in a few words: "4032 × 3024 photo", "1920 × 1080
/// video · HEVC · HDR · 0:42", "sound, 3:42", "12 pages, no text". A
/// video's codec and HDR are said where Kiln's own engine read its picture.
/// `None` when the facts say nothing worth showing.
pub fn describe(f: &Facts) -> Option<String> {
    let duration = || {
        f.media
            .as_ref()
            .and_then(|m| m.duration)
            .filter(|d| *d > 0.0)
            .map(crate::quality::clock)
    };
    match f.kind {
        Kind::Image => {
            let p = f.picture.as_ref()?;
            let look = match p.look {
                Look::Photo => "photo",
                Look::Graphic => "graphic",
            };
            Some(format!("{} × {} {look}", p.width, p.height))
        }
        Kind::Video => {
            let frame = f.picture.as_ref().map(|p| (p.width, p.height)).or_else(|| {
                let m = f.media.as_ref()?;
                Some((m.width?, m.height?))
            });
            let media = f.media.as_ref();
            let codec = media
                .and_then(|m| m.video_codec.as_ref())
                .map(video_codec_name);
            let hdr = media
                .and_then(|m| m.hdr)
                .filter(|h| *h != kiln_media::video::Hdr::None)
                .map(|_| "HDR".to_owned());
            let parts: Vec<String> = frame
                .map(|(w, h)| format!("{w} × {h} video"))
                .into_iter()
                .chain(codec)
                .chain(hdr)
                .chain(duration())
                .collect();
            match (frame, parts.is_empty()) {
                (_, true) => None,
                (Some(_), false) => Some(parts.join(" · ")),
                (None, false) => Some(format!("video · {}", parts.join(" · "))),
            }
        }
        Kind::Audio => match f.media.as_ref().and_then(sound_facts) {
            Some(facts) => Some(facts),
            None => duration().map(|d| format!("sound, {d}")),
        },
        _ => {
            let pdf = f.pdf.as_ref()?;
            let pages = match pdf.pages {
                1 => "1 page".to_owned(),
                n => format!("{n} pages"),
            };
            Some(match pdf.has_text {
                Some(false) => format!("{pages}, no text"),
                _ => pages,
            })
        }
    }
}

/// A picture codec as people know it: "H.264", "HEVC", "AV1".
fn video_codec_name(c: &kiln_media::video::VideoCodec) -> String {
    use kiln_media::video::VideoCodec;
    match c {
        VideoCodec::H264 => "H.264".to_owned(),
        VideoCodec::Hevc => "HEVC".to_owned(),
        VideoCodec::Av1 => "AV1".to_owned(),
        VideoCodec::Vp9 => "VP9".to_owned(),
        VideoCodec::Mpeg4 => "MPEG-4".to_owned(),
        VideoCodec::Mjpeg => "MJPEG".to_owned(),
        VideoCodec::Other(name) => name.clone(),
    }
}

/// What Kiln's own engine found in a sound file: "MP3 · 128 kbps · 3:41",
/// or for lossless sound "FLAC · 24-bit · 96 kHz · 3:41". What isn't known
/// is left out. `None` without a codec, when only FFprobe read the file.
fn sound_facts(m: &kiln_core::MediaInfo) -> Option<String> {
    // A codec nothing could name reads "an unknown format": not a code.
    let codec = m
        .codec
        .as_deref()
        .filter(|c| !c.is_empty() && *c != "an unknown format")?;
    let mut parts = vec![codec.to_owned()];
    match m.bits {
        // Lossless: its depth and rate say more than a bitrate.
        Some(bits) => {
            parts.push(format!("{bits}-bit"));
            if let Some(rate) = m.rate.filter(|&r| r > 0) {
                parts.push(format!("{} kHz", crate::tune_sheet::khz(rate)));
            }
        }
        None => {
            if let Some(kbps) = m.kbps.filter(|&k| k > 0) {
                parts.push(format!("{kbps} kbps"));
            }
        }
    }
    if let Some(d) = m.duration.filter(|d| *d > 0.0) {
        parts.push(crate::quality::clock(d));
    }
    Some(parts.join(" · "))
}

/// What happened when paths were added, for the status line.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Added {
    pub added: usize,
    pub duplicates: usize,
    /// Folders and paths that don't exist.
    pub skipped: usize,
}

/// Removing at least this many rows at once can be undone, and says so.
pub const UNDO_FROM: usize = 2;

/// The footer after rows were removed: "500 files removed."
pub fn removed_line(count: usize) -> String {
    format!("{count} files removed.")
}

/// The footer after Remove was undone: "500 files are back."
pub fn restored_line(count: usize) -> String {
    format!("{count} files are back.")
}

/// The footer while more than one row is selected: "3 selected".
pub fn selected_line(selected: usize) -> Option<String> {
    (selected > 1).then(|| format!("{selected} selected"))
}

#[derive(Debug, Default)]
pub struct FileList {
    entries: Vec<Entry>,
    /// The selected rows.
    selected: BTreeSet<usize>,
    /// Where a Shift-click or Shift+arrow range starts.
    anchor: Option<usize>,
    /// The row keyboard actions start from. It can be unselected.
    focus: Option<usize>,
    /// Whether the focus shows its ring: after a key, not after a click.
    focus_visible: bool,
    /// Counts the changes to the rows and the selection: see `revision`.
    rev: u64,
}

impl FileList {
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// A number that moves whenever the rows or the selection may have
    /// changed: a row added, removed or moved, its job, facts, results or
    /// formats, or which rows are selected. The focus and its ring don't
    /// move it. What is built from the list alone, the inspector, is built
    /// again only when it moves.
    pub fn revision(&self) -> u64 {
        self.rev
    }

    /// Note a change `revision` follows.
    fn touch(&mut self) {
        self.rev = self.rev.wrapping_add(1);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The one row a single-row action takes: the focus, when it's
    /// selected.
    pub fn selected(&self) -> Option<usize> {
        self.focus.filter(|i| self.selected.contains(i))
    }

    /// Every selected row, in list order.
    pub fn selection(&self) -> Vec<usize> {
        self.selected.iter().copied().collect()
    }

    /// The selected rows, for drawing.
    pub fn selected_rows(&self) -> &BTreeSet<usize> {
        &self.selected
    }

    pub fn is_selected(&self, i: usize) -> bool {
        self.selected.contains(&i)
    }

    pub fn focus(&self) -> Option<usize> {
        self.focus
    }

    /// The focus, while its ring should show. Like focus-visible on the
    /// web: a key shows it, and stays shown when Escape clears the
    /// selection; a click on the list hides it until the next key.
    pub fn visible_focus(&self) -> Option<usize> {
        self.focus.filter(|_| self.focus_visible)
    }

    /// Put the focus on row `i` and leave the selection as it is, as
    /// assistive technology moving its focus does.
    pub fn set_focus(&mut self, i: usize) {
        if i < self.entries.len() {
            self.focus = Some(i);
        }
    }

    /// A key moved through or acted on the list: the focus ring shows.
    pub fn keyboard_used(&mut self) {
        self.focus_visible = true;
    }

    /// The pointer clicked in the list: the focus ring hides.
    pub fn pointer_used(&mut self) {
        self.focus_visible = false;
    }

    /// Rows were just added from row `first` on: every new row is
    /// selected (UX-05) and the first takes the focus, so the list shows
    /// where the new files start and Enter converts them all. The ring shows when a key added them
    /// (Ctrl+O, Ctrl+V) and hides when the pointer did (a drop, the Add
    /// files button).
    pub fn added(&mut self, first: usize, keys: bool) {
        if first >= self.entries.len() {
            return;
        }
        self.click(first);
        // Every file just added is selected, not only the first (UX-05):
        // Convert acts on them all.
        self.selected = (first..self.entries.len()).collect();
        self.focus_visible = keys;
    }

    /// More rows of the same add came in from row `first` on, after
    /// [`FileList::added`]: they join the selection, and the focus stays
    /// where the add's first row put it.
    pub fn also_added(&mut self, first: usize) {
        if first >= self.entries.len() {
            return;
        }
        self.touch();
        self.selected.extend(first..self.entries.len());
    }

    /// A plain click: only row `i` is selected.
    pub fn click(&mut self, i: usize) {
        if i >= self.entries.len() {
            return;
        }
        self.touch();
        self.selected = BTreeSet::from([i]);
        self.anchor = Some(i);
        self.focus = Some(i);
    }

    /// A plain press on row `i`. Inside a selection of two or more rows it
    /// takes the focus and the anchor and keeps the selection, so it can
    /// be dragged; it returns true, and the release without a drag is the
    /// click. Anywhere else it's the click now, and it returns false.
    pub fn press(&mut self, i: usize) -> bool {
        if self.selected.len() > 1 && self.selected.contains(&i) {
            self.focus = Some(i);
            self.anchor = Some(i);
            true
        } else {
            self.click(i);
            false
        }
    }

    /// A Ctrl-press on row `i`, as in Explorer. On a selected row it takes
    /// the focus and the anchor and keeps the selection, so it can be
    /// dragged; it returns true, and the release without a drag is the
    /// Ctrl-click. On any other row it's the Ctrl-click now, and it
    /// returns false.
    pub fn ctrl_press(&mut self, i: usize) -> bool {
        if self.selected.contains(&i) {
            self.focus = Some(i);
            self.anchor = Some(i);
            true
        } else {
            self.ctrl_click(i);
            false
        }
    }

    /// A Ctrl-click: row `i` goes in or out of the selection, and ranges
    /// start from it.
    pub fn ctrl_click(&mut self, i: usize) {
        if i >= self.entries.len() {
            return;
        }
        self.touch();
        if !self.selected.remove(&i) {
            self.selected.insert(i);
        }
        self.anchor = Some(i);
        self.focus = Some(i);
    }

    /// A Shift-click: the rows from the anchor to `i` are the selection.
    pub fn shift_click(&mut self, i: usize) {
        if i >= self.entries.len() {
            return;
        }
        let Some(anchor) = self.anchor else {
            return self.click(i);
        };
        self.touch();
        self.selected = (anchor.min(i)..=anchor.max(i)).collect();
        self.focus = Some(i);
    }

    /// Ctrl+A. The focus stays where it is, or goes to the first row.
    pub fn select_all(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        self.touch();
        self.selected = (0..self.entries.len()).collect();
        self.focus = self.focus.or(Some(0));
        self.anchor = self.anchor.or(self.focus);
    }

    /// Escape: nothing is selected, but the focus stays.
    pub fn clear_selection(&mut self) {
        if !self.selected.is_empty() {
            self.touch();
        }
        self.selected.clear();
    }

    /// Where the focus goes `delta` rows on, clamped to the list: from no
    /// focus, the first row going down or the last going up.
    fn stepped(&self, delta: isize) -> Option<usize> {
        let last = self.entries.len().checked_sub(1)? as isize;
        Some(match self.focus {
            None if delta >= 0 => 0,
            None => last as usize,
            Some(i) => (i as isize + delta).clamp(0, last) as usize,
        })
    }

    /// An arrow key: the focus moves `delta` rows and only it is selected.
    pub fn step(&mut self, delta: isize) {
        if let Some(i) = self.stepped(delta) {
            self.click(i);
        }
    }

    /// Shift with an arrow key: the focus moves `delta` rows and the
    /// selection runs from the anchor to it.
    pub fn extend(&mut self, delta: isize) {
        let Some(i) = self.stepped(delta) else {
            return;
        };
        if self.anchor.is_none() {
            self.anchor = self.focus.or(Some(i));
        }
        self.shift_click(i);
    }

    /// A right-click on row `i`: a row outside the selection becomes the
    /// whole selection; one inside it keeps the selection and takes the
    /// focus. Returns the rows the menu acts on.
    pub fn context_target(&mut self, i: usize) -> Vec<usize> {
        if i < self.entries.len() {
            if self.selected.contains(&i) {
                self.focus = Some(i);
            } else {
                self.click(i);
            }
        }
        self.selection()
    }

    /// Take row `i` out of the list. The rows after it move up one, and so
    /// do their places in the selection. When the last selected row goes,
    /// the row that takes the focus's place is selected, so Delete can be
    /// pressed again.
    pub fn remove(&mut self, i: usize) -> Option<Entry> {
        if i >= self.entries.len() {
            return None;
        }
        self.touch();
        let removed = self.entries.remove(i);
        let was_selected = self.selected.remove(&i);
        let len = self.entries.len();
        let shift = |k: usize| if k > i { k - 1 } else { k };
        self.selected = self.selected.iter().map(|&k| shift(k)).collect();
        // A focus or anchor on the removed row stays at its place, now the
        // next row's, or the last row's at the end.
        let follow = |k: Option<usize>| {
            k.map(|k| {
                if k == i {
                    k.min(len.wrapping_sub(1))
                } else {
                    shift(k)
                }
            })
            .filter(|&k| k < len)
        };
        self.focus = follow(self.focus);
        self.anchor = follow(self.anchor);
        if was_selected && self.selected.is_empty() {
            if let Some(f) = self.focus {
                self.click(f);
            }
        }
        Some(removed)
    }

    pub fn add(
        &mut self,
        kiln: &Kiln,
        available: &Availability,
        paths: impl IntoIterator<Item = PathBuf>,
    ) -> Added {
        self.touch();
        let mut result = Added::default();
        for path in paths {
            let Ok(meta) = std::fs::metadata(&path) else {
                result.skipped += 1;
                continue;
            };
            if !meta.is_file() {
                result.skipped += 1;
                continue;
            }
            let path = std::path::absolute(&path).unwrap_or(path);
            if self.entries.iter().any(|e| same_path(&e.path, &path)) {
                result.duplicates += 1;
                continue;
            }
            self.entries.push(entry_for(
                kiln,
                available,
                path,
                meta.len(),
                meta.modified().ok(),
            ));
            result.added += 1;
        }
        result
    }

    /// Add files a walk has already read ([`walk_dropped`]), in order,
    /// leaving out those the list has. Reads nothing from the disk: the
    /// size, time and look at the first bytes are the walk's. Nothing is
    /// skipped here (`skipped` is 0); the walk's [`Gathered::unopened`]
    /// counts what didn't open.
    pub fn add_read(
        &mut self,
        kiln: &Kiln,
        available: &Availability,
        files: impl IntoIterator<Item = ReadFile>,
    ) -> Added {
        self.add_read_with(&mut ListedPaths::default(), kiln, available, files)
    }

    /// [`add_read`](Self::add_read), looking duplicates up in `listed`: a
    /// walk adds its files a share at a time to a list that may hold
    /// thousands, and keeps `listed` from one share to the next. It is
    /// made again from the rows only when the list changed meanwhile.
    pub fn add_read_with(
        &mut self,
        listed: &mut ListedPaths,
        kiln: &Kiln,
        available: &Availability,
        files: impl IntoIterator<Item = ReadFile>,
    ) -> Added {
        if listed.rev != Some(self.rev) {
            listed.keys = self.entries.iter().map(|e| path_key(&e.path)).collect();
        }
        self.touch();
        let mut result = Added::default();
        for f in files {
            if !listed.keys.insert(path_key(&f.path)) {
                result.duplicates += 1;
                continue;
            }
            self.entries.push(entry_checked(
                kiln, available, f.path, f.size, f.modified, f.check,
            ));
            result.added += 1;
        }
        listed.rev = Some(self.rev);
        result
    }

    /// The held results by row index: finished jobs not yet exported.
    pub fn held(&self) -> Vec<(usize, Vec<PathBuf>)> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| Some((i, e.held()?.to_vec())))
            .collect()
    }

    /// Every held result file of the rows `rows` picks, once each, in list
    /// order.
    pub fn held_files(&self, rows: impl Fn(usize) -> bool) -> Vec<PathBuf> {
        let mut seen = HashSet::new();
        let mut files: Vec<PathBuf> = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            let Some(held) = e.held().filter(|_| rows(i)) else {
                continue;
            };
            for p in held {
                if seen.insert(p) {
                    files.push(p.clone());
                }
            }
        }
        files
    }

    /// What leaving the rows `rows` (removing them, or starting something
    /// new on them) would lose: their held files that no other row holds.
    pub fn held_only_by(&self, rows: impl Fn(usize) -> bool) -> Vec<PathBuf> {
        let others = self.held_files(|i| !rows(i));
        self.held_files(rows)
            .into_iter()
            .filter(|p| !others.contains(p))
            .collect()
    }

    /// Record exported results: `moved` maps each held file that went out
    /// to where it went, all into `folder`. A row whose results all went
    /// out is saved there; a row with some left keeps holding those.
    pub fn exported(&mut self, moved: &[(PathBuf, PathBuf)], folder: &Path) {
        self.touch();
        let moved_to = |p: &PathBuf| moved.iter().find(|(from, _)| from == p).map(|(_, to)| to);
        for e in &mut self.entries {
            let Some(held) = e.held() else {
                continue;
            };
            if !held.iter().any(|p| moved_to(p).is_some()) {
                continue;
            }
            let left: Vec<PathBuf> = held
                .iter()
                .filter(|p| moved_to(p).is_none())
                .cloned()
                .collect();
            // Something was saved: whatever went wrong before is over.
            e.save_failed = false;
            let JobState::Done { outputs, saved, .. } = &mut e.job else {
                continue;
            };
            if left.is_empty() {
                *outputs = outputs.iter().filter_map(moved_to).cloned().collect();
                // Where they went: older, or the folder they went to in
                // its place.
                let went = outputs.first().and_then(|p| p.parent());
                *saved = Some(Saved::Folder(went.unwrap_or(folder).to_path_buf()));
            } else {
                *outputs = left;
            }
        }
    }

    /// Note that saving `failed` held files didn't work: their rows offer
    /// to choose another folder.
    pub fn export_failed(&mut self, failed: &[PathBuf]) {
        self.touch();
        for e in &mut self.entries {
            if e.held()
                .is_some_and(|h| h.iter().any(|p| failed.contains(p)))
            {
                e.save_failed = true;
            }
        }
    }

    /// The rows to change in place. Whatever is changed through it counts
    /// as a change to `revision`.
    pub fn entries_mut(&mut self) -> &mut [Entry] {
        self.touch();
        &mut self.entries
    }

    pub fn index_of(&self, path: &Path) -> Option<usize> {
        self.entries.iter().position(|e| same_path(&e.path, path))
    }

    /// Whether the row for `path` knows its file holds a subtitle track,
    /// so the wheel offers subtitle formats (spec 3.7).
    pub fn has_subtitles(&self, path: &Path) -> bool {
        self.index_of(path)
            .is_some_and(|i| self.entries[i].has_subtitles())
    }

    /// Why converting the files at `paths` can't start: one of them is
    /// converting already. `None` when nothing stands in the way. The
    /// format choice and the inspector's formats both ask this, so a row
    /// that is converting is never offered another conversion.
    pub fn convert_busy(&self, paths: &[PathBuf]) -> Option<&'static str> {
        let running = paths
            .iter()
            .filter_map(|p| self.index_of(p))
            .filter(|&i| matches!(self.entries[i].job, JobState::Running { .. }))
            .count();
        if running == 0 {
            return None;
        }
        convert_block(paths.len(), running, 0)
    }

    /// Work out the formats of the row at `path` again, now its facts are
    /// in: a video found to have no sound loses its audio formats.
    pub fn refresh_offers_for(&mut self, kiln: &Kiln, available: &Availability, path: &Path) {
        let Some(i) = self.index_of(path) else {
            return;
        };
        self.touch();
        let e = &mut self.entries[i];
        if e.check == FileCheck::Ok {
            (e.targets, e.missing) = offers(kiln, available, &e.path);
        }
        e.ffmpeg_found = available.has("ffmpeg");
    }

    /// Work out every row's formats again, after the helper apps found
    /// changed. Rows the add-time check ruled out stay as they are.
    pub fn refresh_offers(&mut self, kiln: &Kiln, available: &Availability) {
        self.touch();
        for e in &mut self.entries {
            if e.check == FileCheck::Ok {
                (e.targets, e.missing) = offers(kiln, available, &e.path);
            }
            e.ffmpeg_found = available.has("ffmpeg");
        }
    }

    /// The Store extensions rows wait on, by codec ("HEVC"), each once:
    /// every row whose facts say Windows lacks its picture's decoder.
    pub fn awaited_extensions(&self) -> Vec<&'static str> {
        let mut codecs = vec![];
        for e in &self.entries {
            let wanted = e
                .facts
                .as_ref()
                .and_then(|f| f.media.as_ref()?.needs_extension);
            if let Some((codec, _)) = wanted.filter(|(c, _)| !codecs.contains(c)) {
                codecs.push(codec);
            }
        }
        codecs
    }

    /// The decoders for `installed` ("HEVC") are there now: the rows that
    /// waited on them stop saying so. Their paths, for their facts to
    /// be read again.
    pub fn extensions_installed(&mut self, installed: &[&str]) -> Vec<PathBuf> {
        let mut cleared = vec![];
        for e in &mut self.entries {
            let Some(m) = e.facts.as_mut().and_then(|f| f.media.as_mut()) else {
                continue;
            };
            if m.needs_extension
                .is_some_and(|(c, _)| installed.contains(&c))
            {
                m.needs_extension = None;
                cleared.push(e.path.clone());
            }
        }
        if !cleared.is_empty() {
            self.touch();
        }
        cleared
    }

    /// Put `entry` back as row `i` (or last, if the list is shorter now),
    /// after an undo. Rows from there on move down one.
    pub fn insert(&mut self, i: usize, entry: Entry) {
        self.touch();
        let i = i.min(self.entries.len());
        self.entries.insert(i, entry);
        let shift = |k: usize| if k >= i { k + 1 } else { k };
        self.selected = self.selected.iter().map(|&k| shift(k)).collect();
        self.focus = self.focus.map(shift);
        self.anchor = self.anchor.map(shift);
    }

    /// Select exactly `rows`, the focus on the last and the anchor on the
    /// first.
    pub fn select_only(&mut self, rows: &[usize]) {
        self.touch();
        self.selected = rows
            .iter()
            .copied()
            .filter(|&i| i < self.entries.len())
            .collect();
        self.anchor = self.selected.iter().next().copied();
        self.focus = self.selected.iter().next_back().copied();
    }

    pub fn clear(&mut self) {
        self.touch();
        self.entries.clear();
        self.selected.clear();
        self.anchor = None;
        self.focus = None;
    }

    /// Store what `inspect` found for the row at `path`, and its
    /// suggestion; its second line fades over from now, unless the row
    /// already had facts, so a repeated result never fades again. False,
    /// changing nothing, when no row has that path.
    pub fn set_facts(&mut self, path: &Path, facts: Facts, suggestion: Option<Suggestion>) -> bool {
        let Some(i) = self.index_of(path) else {
            return false;
        };
        self.touch();
        let e = &mut self.entries[i];
        if e.facts.is_none() {
            e.facts_at = Some(Instant::now());
        }
        e.facts = Some(facts);
        e.suggestion = suggestion;
        true
    }

    /// A preview of `path` has arrived, made from the file as it was at
    /// `modified`. Returns the row to keep it for, or `None` to drop it:
    /// the file is no longer listed, or the picture is older than the file
    /// the row knows. A newer time means the file changed since it was
    /// listed, and the row takes it.
    ///
    /// An unknown time (`None`) counts as older than any known one. A row
    /// that couldn't tell its file's time learns it from a preview that
    /// could; a preview without a time is kept only by a row without one,
    /// as a row with a time would never look it up.
    pub fn preview_arrived(&mut self, path: &Path, modified: Option<SystemTime>) -> Option<usize> {
        let i = self.index_of(path)?;
        let e = &mut self.entries[i];
        if modified < e.modified {
            return None;
        }
        e.modified = modified;
        Some(i)
    }

    /// Drop every row's facts and suggestion, as when Suggestions is
    /// turned off.
    pub fn forget_facts(&mut self) {
        self.touch();
        for e in &mut self.entries {
            e.facts = None;
            e.suggestion = None;
            e.facts_at = None;
        }
    }

    /// The facts of every file in `paths`, in order. `None` until each of
    /// them is listed and has facts.
    pub fn facts_for(&self, paths: &[PathBuf]) -> Option<Vec<Facts>> {
        paths
            .iter()
            .map(|p| self.entries[self.index_of(p)?].facts.clone())
            .collect()
    }
}

/// The helper app's name, for a backend id.
pub fn helper_name(backend: &str) -> &'static str {
    match backend {
        "ffmpeg" => "FFmpeg",
        "pandoc" => "Pandoc",
        "libreoffice" => "LibreOffice",
        "calibre" => "Calibre",
        "audio-mf" => "Windows' Media Feature Pack",
        _ => "a helper app",
    }
}

/// The link that opens a Microsoft Store extension's page, named for what
/// it adds: "Get HEVC extension". Never "Get it": the link says what it gets.
pub fn store_label(product: &str) -> &'static str {
    match product {
        "9N4WGH0Z6VHQ" => "Get HEVC extension",
        "9MVZQVXJBQ9V" => "Get AV1 extension",
        "9N4D0MSMP0PT" => "Get VP9 extension",
        "9PMMSR1CGPWG" => "Get HEIF extension",
        _ => "Get extension",
    }
}

/// The Microsoft Store page of an extension Kiln names: HEVC, AV1 or VP9
/// video, or HEIF images. It takes only these four products, so Get it can
/// never open anything else. `None` for any other.
pub fn store_page(product: &str) -> Option<String> {
    const PRODUCTS: [&str; 4] = [
        "9N4WGH0Z6VHQ",
        "9MVZQVXJBQ9V",
        "9N4D0MSMP0PT",
        "9PMMSR1CGPWG",
    ];
    PRODUCTS
        .contains(&product)
        .then(|| format!("ms-windows-store://pdp/?ProductId={product}"))
}

/// The row link that opens a helper's download page: "Get FFmpeg". Windows'
/// Media Feature Pack is added from Settings instead, and its link says so
/// by name: "Get Media Feature Pack".
pub fn get_label(backend: &str) -> &'static str {
    match backend {
        "ffmpeg" => "Get FFmpeg",
        "pandoc" => "Get Pandoc",
        "libreoffice" => "Get LibreOffice",
        "calibre" => "Get Calibre",
        "audio-mf" => "Get Media Feature Pack",
        _ => "Get helper",
    }
}

/// Whether `backend` only ever serves sound: Kiln's own engine, plain or
/// with Windows' Media Feature Pack.
fn sound_helper(backend: &str) -> bool {
    matches!(backend, "audio" | "audio-mf")
}

/// Settings' Optional features page, where Windows' Media Feature Pack is
/// added on an N edition.
pub const MEDIA_FEATURE_PACK: &str = "ms-settings:optionalfeatures";

/// The one address a helper's link may open: a helper app's official
/// download page, or Settings for the Media Feature Pack. `None` for
/// anything else, so a link can't open an address that isn't one of these.
pub fn helper_page(backend: &str) -> Option<&'static str> {
    use crate::settings::Engine;
    let engine = match backend {
        "ffmpeg" => Engine::Ffmpeg,
        "pandoc" => Engine::Pandoc,
        "libreoffice" => Engine::LibreOffice,
        "calibre" => Engine::Calibre,
        "audio-mf" => return Some(MEDIA_FEATURE_PACK),
        _ => return None,
    };
    engine.download_page()
}

/// The link that gets `backend`'s helper ("Get Pandoc"), when it has a
/// page to open: FFmpeg comes with Kiln and has none.
pub fn get_link(backend: &str) -> Option<&'static str> {
    helper_page(backend).map(|_| get_label(backend))
}

/// The footer's one notice a run that a helper app some new files need
/// is missing. Short enough for the footer of the default window:
/// the rows carry the rest. Part of Windows, installed from Settings: the
/// spec's own line, as the row and the wheel say it.
pub fn helper_notice(backend: &str) -> String {
    let name = helper_name(backend);
    if backend == "audio-mf" {
        "Needs Windows' Media Feature Pack, which isn't installed.".to_owned()
    } else if get_link(backend).is_some() {
        format!("{name} isn't installed. Use Get {name} to download it.")
    } else {
        format!("{name} isn't installed, so some files can't be converted yet.")
    }
}

/// The link of a row a conversion left out for the missing helper
/// `backend`: "Get Pandoc", or "Get Media Feature Pack" for Windows'
/// Media Feature Pack. FFmpeg, which has no download page, is got from
/// Settings. `None` for a backend Kiln can't point to.
pub fn waits_on_link(backend: &str) -> Option<&'static str> {
    match backend {
        "audio-mf" | "ffmpeg" => Some(get_label(backend)),
        b => helper_page(b).map(|_| get_label(b)),
    }
}

/// The line when a helper's page wouldn't open.
pub fn helper_page_failed(backend: &str) -> String {
    match backend {
        "audio-mf" => "Couldn't open Settings.".to_owned(),
        b => format!("Couldn't open {}'s download page.", helper_name(b)),
    }
}

/// Looking for Media Foundation's codecs again once "Get Media Feature Pack"
/// has opened Settings for the Media Feature Pack, so its rows clear without a restart
///. The window asks [`MfRecheck::activated`] each time it is
/// activated, runs the look off its thread when told to, and hands the
/// answer to [`MfRecheck::finished`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MfRecheck {
    /// Get Media Feature Pack was pressed and the pack hasn't turned up yet.
    pending: bool,
    /// A look is under way.
    running: bool,
}

impl MfRecheck {
    /// A helper's link was followed and its page opened. Only the Media
    /// Feature Pack's link starts the watch.
    pub fn helper_opened(&mut self, backend: &str) {
        if backend == "audio-mf" {
            self.pending = true;
        }
    }

    /// The window was activated: whether to look now. Never while a look
    /// is under way.
    pub fn activated(&mut self) -> bool {
        if self.pending && !self.running {
            self.running = true;
            true
        } else {
            false
        }
    }

    /// A look came back: the codecs `before` it and what it found. Whether
    /// the offers must be worked out again. Until something has changed,
    /// the next activation looks again, since adding the pack takes a while
    /// and the person may come back to Kiln before it is done.
    pub fn finished(&mut self, before: kiln_media::MfCodecs, found: kiln_media::MfCodecs) -> bool {
        self.running = false;
        let changed = before != found;
        if changed {
            self.pending = false;
        }
        changed
    }
}

/// Looking again for the decoders of Microsoft's video extensions once Get
/// it has opened the Store, so the rows that waited on one clear without
/// adding the file again, as [`MfRecheck`] does for the Media
/// Feature Pack. The window asks [`ExtensionRecheck::activated`] each time
/// it is activated, runs the look off its thread when told to, and hands
/// the answer to [`ExtensionRecheck::finished`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ExtensionRecheck {
    /// Get it opened the Store and rows still wait on an extension.
    pending: bool,
    /// A look is under way.
    running: bool,
}

impl ExtensionRecheck {
    /// Get it opened an extension's Store page.
    pub fn store_opened(&mut self) {
        self.pending = true;
    }

    /// The window was activated: whether to look now. Never while a look
    /// is under way.
    pub fn activated(&mut self) -> bool {
        if self.pending && !self.running {
            self.running = true;
            true
        } else {
            false
        }
    }

    /// A look came back. `cleared` says whether rows stopped waiting, and
    /// `waiting` whether any still wait. Whether the rows must be read
    /// again. While rows wait, the next activation looks again, since an
    /// install from the Store takes a while.
    pub fn finished(&mut self, cleared: bool, waiting: bool) -> bool {
        self.running = false;
        self.pending = waiting;
        cleared
    }
}

/// The picture codec a Store extension decodes, by the name the rows use.
pub fn extension_codec(name: &str) -> Option<kiln_media::video::VideoCodec> {
    use kiln_media::video::VideoCodec;
    match name {
        "HEVC" => Some(VideoCodec::Hevc),
        "AV1" => Some(VideoCodec::Av1),
        "VP9" => Some(VideoCodec::Vp9),
        _ => None,
    }
}

/// A helper app some of a file's formats need and Kiln can't find.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Missing {
    /// Its backend ("libreoffice"): the helper of the waiting format that
    /// comes first in the wheel's order (`wheel::Order`), so a DOCX names
    /// LibreOffice, which makes its PDF, not Pandoc (UX-12).
    pub backend: String,
    /// How many formats wait on this helper. Others may wait on another
    /// missing one, and aren't counted.
    pub count: usize,
    /// This helper alone would make the file's PDF, which waits on it.
    pub makes_pdf: bool,
}

impl Missing {
    /// `count` formats wait on `backend`, none of them a PDF it makes.
    pub fn new(backend: &str, count: usize) -> Missing {
        Missing {
            backend: backend.to_owned(),
            count,
            makes_pdf: false,
        }
    }
}

/// What `path`'s formats are with the helpers in `available`: the codes
/// that can run, and the helper of the best-ranked format that can't,
/// with how many formats wait on that helper. Reads nothing from the
/// disk.
fn offers(kiln: &Kiln, available: &Availability, path: &Path) -> (Vec<String>, Option<Missing>) {
    let (usable, blocked) = kiln.targets_split(path, available);
    let order = crate::wheel::Order::of(kiln, &[path.to_path_buf()]);
    let best = blocked.iter().min_by(|(a, _), (b, _)| {
        order
            .rank(&a.ext)
            .cmp(&order.rank(&b.ext))
            .then_with(|| a.ext.cmp(&b.ext))
    });
    let missing = best.map(|(_, backend)| {
        let pdf_waits = blocked.iter().any(|(f, _)| f.ext == "pdf");
        let makes_pdf = pdf_waits
            && kiln
                .targets_split(path, &available.clone().with(backend))
                .0
                .iter()
                .any(|f| f.ext == "pdf");
        Missing {
            backend: backend.clone(),
            count: blocked.iter().filter(|(_, b)| b == backend).count(),
            makes_pdf,
        }
    });
    (usable.iter().map(|f| f.code()).collect(), missing)
}

/// The results `job` holds, if it holds any: a finished job that hasn't
/// been exported.
pub(crate) fn held_in(job: &JobState) -> Option<&[PathBuf]> {
    match job {
        JobState::Done {
            outputs,
            saved: None,
            ..
        } => Some(outputs),
        _ => None,
    }
}

pub(crate) fn entry_for(
    kiln: &Kiln,
    available: &Availability,
    path: PathBuf,
    size: u64,
    modified: Option<SystemTime>,
) -> Entry {
    let ext = kiln.registry().format_of(&path).map(|f| f.ext.clone());
    let check = kiln_core::check_file(&path, ext.as_deref(), size);
    entry_checked(kiln, available, path, size, modified, check)
}

/// `entry_for`, with the look at the file's first bytes already taken.
/// Reads nothing from the disk.
fn entry_checked(
    kiln: &Kiln,
    available: &Availability,
    path: PathBuf,
    size: u64,
    modified: Option<SystemTime>,
    check: FileCheck,
) -> Entry {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let folder = path
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let code = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_uppercase())
        .unwrap_or_default();
    // A file that can't be what it says, or read, is offered nothing.
    let (targets, missing) = if check == FileCheck::Ok {
        offers(kiln, available, &path)
    } else {
        (vec![], None)
    };
    Entry {
        path,
        name,
        folder,
        code,
        size,
        modified,
        targets,
        check,
        missing,
        save_failed: false,
        job: JobState::Idle,
        facts: None,
        suggestion: None,
        facts_at: None,
        last: None,
        from_drag: false,
        job_note: None,
        result_bytes: None,
        waits_on: None,
        ffmpeg_found: available.has("ffmpeg"),
    }
}

/// Windows paths are case-insensitive.
/// The list's paths as [`FileList::add_read_with`] looks them up, and the
/// list's revision they were taken at.
#[derive(Debug, Default)]
pub struct ListedPaths {
    rev: Option<u64>,
    keys: HashSet<OsString>,
}

/// `p` as [`same_path`] compares it, to look up in a set.
fn path_key(p: &Path) -> OsString {
    if cfg!(windows) {
        p.as_os_str().to_ascii_lowercase()
    } else {
        p.as_os_str().to_owned()
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        a.as_os_str().eq_ignore_ascii_case(b.as_os_str())
    } else {
        a == b
    }
}

/// More files than this from folders asks first.
pub const FOLDER_ASK_ABOVE: usize = 200;

/// What dropped or chosen paths come to once folders are opened.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Expanded {
    /// The files to add, in the order met.
    pub files: Vec<PathBuf>,
    /// How many of them came out of folders.
    pub from_folders: usize,
    /// The folders opened.
    pub folders: Vec<String>,
    /// What was left out, by name and why: "notes (hidden)".
    pub skipped: Vec<String>,
}

impl Expanded {
    /// Where the files came from, for "Added 12 files from Holiday.": the
    /// folder's name for one, a count for several. `None` with only
    /// loose files.
    pub fn source(&self) -> Option<String> {
        source_of(&self.folders, self.from_folders, self.files.len())
    }
}

/// Where `files` files came from, `from_folders` of them out of `folders`:
/// what [`Expanded::source`] and [`Gathered::source`] say.
pub fn source_of(folders: &[String], from_folders: usize, files: usize) -> Option<String> {
    match folders {
        [] => None,
        [one] if from_folders == files => Some(one.clone()),
        [one] => Some(format!("{one} and elsewhere")),
        many => Some(format!("{} folders", many.len())),
    }
}

/// Whether a directory entry is one Kiln leaves out of a folder: hidden or
/// system, or a dot name.
fn hidden_or_system(name: &str, meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const HIDDEN: u32 = 0x2;
        const SYSTEM: u32 = 0x4;
        if meta.file_attributes() & (HIDDEN | SYSTEM) != 0 {
            return true;
        }
    }
    let _ = meta;
    name.starts_with('.')
}

/// Open the folders among `paths`. A folder gives its files, and those of
/// the folders directly inside it, but nothing hidden or system and
/// nothing deeper. What is left out is named with the reason, so a drop
/// never says "Skipped 2 items" and stops.
///
/// This reads the disk on the calling thread; the window walks with
/// [`walk_dropped`] instead, which reads on a thread of its own.
pub fn expand_dropped(paths: Vec<PathBuf>) -> Expanded {
    let mut out = Expanded::default();
    visit_dropped(paths, &|| false, &mut |met| match met {
        Met::File {
            path, from_folder, ..
        } => {
            out.files.push(path);
            out.from_folders += usize::from(from_folder);
        }
        // `FileList::add` says it didn't open.
        Met::Other(path) => out.files.push(path),
        Met::Skipped(why) => out.skipped.push(why),
        Met::Folder(name) => out.folders.push(name),
    });
    out
}

/// What a walk of dropped paths meets, in order.
enum Met {
    /// A file: loose, or found in a folder.
    File {
        path: PathBuf,
        meta: std::fs::Metadata,
        from_folder: bool,
    },
    /// A loose path that is there but is neither a file nor a folder.
    Other(PathBuf),
    /// Something left out, by name and why: "notes (hidden)".
    Skipped(String),
    /// A dropped folder, once its files have been met.
    Folder(String),
}

/// Walk `paths` as [`expand_dropped`] describes, telling `met` what it meets,
/// until `stop` says to stop.
fn visit_dropped(paths: Vec<PathBuf>, stop: &dyn Fn() -> bool, met: &mut dyn FnMut(Met)) {
    let name_of = |p: &Path| {
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| p.display().to_string())
    };
    for path in paths {
        if stop() {
            return;
        }
        let Ok(meta) = std::fs::metadata(&path) else {
            met(Met::Skipped(format!("{} (not found)", name_of(&path))));
            continue;
        };
        if meta.is_file() {
            met(Met::File {
                path,
                meta,
                from_folder: false,
            });
            continue;
        }
        if !meta.is_dir() {
            met(Met::Other(path));
            continue;
        }
        if !visit_folder(&path, 0, stop, met) {
            return;
        }
        met(Met::Folder(name_of(&path)));
    }
}

/// One folder's part of [`visit_dropped`]. False when `stop` stopped it.
fn visit_folder(
    dir: &Path,
    depth: usize,
    stop: &dyn Fn() -> bool,
    met: &mut dyn FnMut(Met),
) -> bool {
    let Ok(read) = std::fs::read_dir(dir) else {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        met(Met::Skipped(format!("{name} (can't be opened)")));
        return true;
    };
    let mut entries: Vec<_> = read.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        if stop() {
            return false;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(meta) = e.metadata() else {
            met(Met::Skipped(format!("{name} (can't be read)")));
            continue;
        };
        if hidden_or_system(&name, &meta) {
            met(Met::Skipped(format!("{name} (hidden)")));
        } else if meta.is_dir() {
            if depth == 0 {
                if !visit_folder(&e.path(), 1, stop, met) {
                    return false;
                }
            } else {
                met(Met::Skipped(format!("{name} (folder inside a folder)")));
            }
        } else if meta.is_file() {
            met(Met::File {
                path: e.path(),
                meta,
                from_folder: true,
            });
        }
    }
    true
}

/// One file as a folder walk read it: all [`FileList::add_read`] needs, so
/// adding it touches no disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadFile {
    /// The file, as an absolute path.
    pub path: PathBuf,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// What a look at its first bytes found, as `kiln_core::check_file`
    /// says.
    pub check: FileCheck,
}

impl ReadFile {
    /// Read what a row needs of `path`, a file whose metadata is `meta`:
    /// its size and time from `meta`, and a look at its first bytes. It
    /// reads the disk, so it runs on the walk's thread.
    pub fn read(registry: &Registry, path: PathBuf, meta: &std::fs::Metadata) -> ReadFile {
        let path = std::path::absolute(&path).unwrap_or(path);
        let size = meta.len();
        let ext = registry.format_of(&path).map(|f| f.ext.as_str());
        let check = kiln_core::check_file(&path, ext, size);
        ReadFile {
            path,
            size,
            modified: meta.modified().ok(),
            check,
        }
    }
}

/// What a walk of dropped paths found: the whole walk once its batches are
/// put together with [`Gathered::absorb`], or one batch's share of it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Gathered {
    /// The files to add, in the order met.
    pub files: Vec<ReadFile>,
    /// How many of them came out of folders.
    pub from_folders: usize,
    /// The folders opened.
    pub folders: Vec<String>,
    /// What was left out, by name and why: "notes (hidden)".
    pub skipped: Vec<String>,
    /// Loose paths that are there but are neither a file nor a folder: the
    /// "didn't open" count, which `FileList::add` gives as `skipped`.
    pub unopened: usize,
}

impl Gathered {
    /// Add a later batch's share to what came before.
    pub fn absorb(&mut self, part: Gathered) {
        self.files.extend(part.files);
        self.from_folders += part.from_folders;
        self.folders.extend(part.folders);
        self.skipped.extend(part.skipped);
        self.unopened += part.unopened;
    }

    /// Where the files came from, as [`Expanded::source`] says.
    pub fn source(&self) -> Option<String> {
        source_of(&self.folders, self.from_folders, self.files.len())
    }

    fn is_empty(&self) -> bool {
        self.files.is_empty()
            && self.folders.is_empty()
            && self.skipped.is_empty()
            && self.unopened == 0
    }
}

/// One report from a walk of dropped paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkBatch {
    /// What was found since the last batch.
    pub part: Gathered,
    /// The files from folders found so far, all batches together: what the
    /// big-folder prompt counts against [`FOLDER_ASK_ABOVE`].
    pub from_folders: usize,
    /// Every file found so far, loose or from folders.
    pub found: usize,
    /// How many files from folders the walk reads in all. The folders are
    /// listed before any file is read, and the first batch, sent then,
    /// already carries it: the big-folder prompt asks with it, so it names
    /// the whole count while the reading goes on.
    pub listed: usize,
    /// The last batch: the walk is over and nothing more comes.
    pub done: bool,
}

/// A walk sends a batch once it has this many new files...
pub const WALK_BATCH_FILES: usize = 200;
/// ...or once this long has passed since the last batch, whichever is first.
pub const WALK_BATCH_EVERY: Duration = Duration::from_millis(100);

/// Walk the folders among `paths` on a thread of its own, as
/// [`expand_dropped`] does, reading each file's metadata and first bytes
/// there too (see [`ReadFile`]).
///
/// `send` is called on that thread with a [`WalkBatch`] whenever
/// [`WALK_BATCH_FILES`] new files are found or [`WALK_BATCH_EVERY`] has
/// passed with something new, and once more with `done` when the walk is
/// over. The folders are listed first, and the first batch, with no files
/// yet, says how many they hold ([`WalkBatch::listed`]). Batches come in
/// order and each carries the running counts, so the window can ask before
/// adding a big folder while the walk goes on.
///
/// Once `cancel` is cancelled the walk stops at the next entry and sends
/// nothing more; a batch already sent may still be on its way, so the
/// window drops batches for a walk it cancelled. The thread holds nothing
/// of the window's: `registry` is its own copy.
pub fn walk_dropped(
    paths: Vec<PathBuf>,
    registry: kiln_core::Registry,
    cancel: CancelToken,
    mut send: impl FnMut(WalkBatch) + Send + 'static,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("kiln-walk".into())
        .spawn(move || walk_here(paths, &registry, &cancel, &mut send))
}

/// [`walk_dropped`]'s walk, on the calling thread.
pub fn walk_here(
    paths: Vec<PathBuf>,
    registry: &Registry,
    cancel: &CancelToken,
    send: &mut dyn FnMut(WalkBatch),
) {
    let stop = || cancel.is_cancelled();
    // The folders are listed first: a listing gives each entry's name,
    // attributes and size without opening it, so the whole count is known
    // early. Reading each file's first bytes is the slow part, after.
    let mut listing = vec![];
    visit_dropped(paths, &stop, &mut |met| listing.push(met));
    if stop() {
        return;
    }
    let listed = listing
        .iter()
        .filter(|m| {
            matches!(
                m,
                Met::File {
                    from_folder: true,
                    ..
                }
            )
        })
        .count();
    send(WalkBatch {
        part: Gathered::default(),
        from_folders: 0,
        found: 0,
        listed,
        done: false,
    });
    let mut part = Gathered::default();
    let mut from_folders = 0;
    let mut found = 0;
    let mut sent = Instant::now();
    for met in listing {
        if stop() {
            return;
        }
        match met {
            Met::File {
                path,
                meta,
                from_folder,
            } => {
                part.files.push(ReadFile::read(registry, path, &meta));
                found += 1;
                if from_folder {
                    part.from_folders += 1;
                    from_folders += 1;
                }
            }
            Met::Other(_) => part.unopened += 1,
            Met::Skipped(why) => part.skipped.push(why),
            Met::Folder(name) => part.folders.push(name),
        }
        let full = part.files.len() >= WALK_BATCH_FILES;
        if (full || sent.elapsed() >= WALK_BATCH_EVERY) && !part.is_empty() && !stop() {
            send(WalkBatch {
                part: std::mem::take(&mut part),
                from_folders,
                found,
                listed,
                done: false,
            });
            sent = Instant::now();
        }
    }
    if stop() {
        return;
    }
    send(WalkBatch {
        part,
        from_folders,
        found,
        listed,
        done: true,
    });
}

/// What a drop said, with folders opened: "Added 12 files from Holiday.
/// Skipped 2 items: Thumbs.db (hidden), old (folder inside a folder)."
/// Without folders or names it is [`status_line`].
pub fn drop_line(
    total: usize,
    added: usize,
    duplicates: usize,
    from: Option<&str>,
    skipped: &[String],
) -> String {
    if from.is_none() && skipped.is_empty() {
        return status_line(total, added, duplicates, 0);
    }
    let n = |k: usize, one: &str, many: &str| format!("{k} {}", if k == 1 { one } else { many });
    let mut parts = vec![];
    if added > 0 {
        let source = from.map(|f| format!(" from {f}")).unwrap_or_default();
        parts.push(format!("Added {}{source}.", n(added, "file", "files")));
    }
    if duplicates > 0 {
        parts.push(format!(
            "{} already in the list.",
            n(duplicates, "file was", "files were")
        ));
    }
    if !skipped.is_empty() {
        let shown: Vec<&str> = skipped.iter().take(3).map(String::as_str).collect();
        let more = match skipped.len() - shown.len() {
            0 => String::new(),
            k => format!(" and {k} more"),
        };
        parts.push(format!(
            "Skipped {}: {}{more}.",
            n(skipped.len(), "item", "items"),
            shown.join(", ")
        ));
    }
    if parts.is_empty() {
        parts.push(format!("{}.", n(total, "file", "files")));
    }
    parts.join(" ")
}

/// "3 files.", or what a drop just did: "Added 2 files. Skipped 1 item
/// that isn't a file."
pub fn status_line(total: usize, added: usize, duplicates: usize, skipped: usize) -> String {
    let n = |k: usize, one: &str, many: &str| format!("{k} {}", if k == 1 { one } else { many });
    let mut parts = vec![];
    if added > 0 {
        parts.push(format!("Added {}.", n(added, "file", "files")));
    }
    if duplicates > 0 {
        parts.push(format!(
            "{} already in the list.",
            n(duplicates, "file was", "files were")
        ));
    }
    if skipped > 0 {
        parts.push(format!(
            "Skipped {}.",
            n(skipped, "item that isn't a file", "items that aren't files")
        ));
    }
    if parts.is_empty() {
        parts.push(format!("{}.", n(total, "file", "files")));
    }
    parts.join(" ")
}

/// "932 bytes", "14.2 KB", "3.1 MB". Decimal units, like Explorer's
/// rounding but without pretending to be exact.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return if bytes == 1 {
            "1 byte".to_owned()
        } else {
            format!("{bytes} bytes")
        };
    }
    let mut v = bytes as f64 / 1000.0;
    let mut unit = 0;
    while v >= 1000.0 && unit < UNITS.len() - 1 {
        v /= 1000.0;
        unit += 1;
    }
    if v < 10.0 {
        format!("{v:.1} {}", UNITS[unit])
    } else {
        format!("{v:.0} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiln_core::Suggested;
    use std::fs;

    fn setup() -> (tempfile::TempDir, Kiln) {
        (tempfile::tempdir().unwrap(), Kiln::new())
    }

    #[test]
    fn a_dropped_folder_gives_its_files_one_level_down_and_names_what_it_skips() {
        let dir = tempfile::tempdir().unwrap();
        let holiday = dir.path().join("Holiday");
        fs::create_dir_all(holiday.join("beach").join("deep")).unwrap();
        fs::write(holiday.join("a.jpg"), b"x").unwrap();
        fs::write(holiday.join(".secret.jpg"), b"x").unwrap();
        fs::write(holiday.join("beach").join("b.jpg"), b"x").unwrap();
        fs::write(holiday.join("beach").join("deep").join("c.jpg"), b"x").unwrap();
        let loose = dir.path().join("loose.png");
        fs::write(&loose, b"x").unwrap();
        let gone = dir.path().join("gone.png");

        let one = expand_dropped(vec![holiday.clone()]);
        let names: Vec<String> = one
            .files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["a.jpg", "b.jpg"]);
        assert_eq!(one.from_folders, 2);
        assert_eq!(one.source().as_deref(), Some("Holiday"));
        assert!(one.skipped.contains(&".secret.jpg (hidden)".to_owned()));
        assert!(one
            .skipped
            .contains(&"deep (folder inside a folder)".to_owned()));
        assert_eq!(
            drop_line(2, 2, 0, one.source().as_deref(), &one.skipped),
            "Added 2 files from Holiday. Skipped 2 items: .secret.jpg (hidden), deep (folder inside a folder)."
        );

        // Loose files and a missing path come through by name.
        let mixed = expand_dropped(vec![loose.clone(), gone, holiday]);
        assert_eq!(mixed.files.len(), 3);
        assert_eq!(mixed.source().as_deref(), Some("Holiday and elsewhere"));
        assert!(mixed.skipped.contains(&"gone.png (not found)".to_owned()));
        // With no folder and nothing skipped, the old line stands.
        assert_eq!(drop_line(1, 1, 0, None, &[]), "Added 1 file.");
        assert_eq!(
            drop_line(0, 0, 0, None, &["x (hidden)".into()]),
            "Skipped 1 item: x (hidden)."
        );
    }

    #[test]
    fn a_big_folder_asks_first() {
        let p = big_folder_prompt(1240);
        assert_eq!(p.title, "That's 1240 files. Add them all?");
        assert_eq!(p.buttons, [PromptButton::AddAll, PromptButton::Cancel]);
        assert_eq!(FOLDER_ASK_ABOVE, 200);
    }

    /// A folder of `n` small PNGs, named so they sort in number order.
    fn folder_of(dir: &Path, name: &str, n: usize) -> PathBuf {
        let folder = dir.join(name);
        fs::create_dir_all(&folder).unwrap();
        for i in 0..n {
            fs::write(folder.join(format!("{i:04}.png")), b"\x89PNG\r\n\x1a\nrest").unwrap();
        }
        folder
    }

    /// The batches a walk of `paths` sends, from a thread of its own.
    fn walk_all(paths: Vec<PathBuf>, cancel: CancelToken) -> Vec<WalkBatch> {
        let (tx, rx) = std::sync::mpsc::channel();
        let walk = walk_dropped(paths, Registry::builtin(), cancel, move |b| {
            let _ = tx.send(b);
        })
        .unwrap();
        walk.join().unwrap();
        rx.into_iter().collect()
    }

    #[test]
    fn a_walk_of_a_big_folder_reports_in_batches_and_passes_the_limit_before_done() {
        let dir = tempfile::tempdir().unwrap();
        let big = folder_of(dir.path(), "Big", 1000);
        let batches = walk_all(vec![big.clone()], CancelToken::new());
        assert!(batches.len() >= 5, "{} batches", batches.len());
        // The folder is listed first: the first batch names the whole
        // count before a file is read, and every batch carries it.
        assert!(batches[0].part.files.is_empty());
        assert!(batches.iter().all(|b| b.listed == 1000));
        // Only the last is done, and the counts only grow.
        assert!(batches.last().unwrap().done);
        assert!(batches[..batches.len() - 1].iter().all(|b| !b.done));
        assert!(batches
            .windows(2)
            .all(|w| w[0].from_folders <= w[1].from_folders));
        assert!(batches
            .iter()
            .all(|b| b.part.files.len() <= WALK_BATCH_FILES));
        // The window can ask about the big folder before the walk is over.
        let asks = batches
            .iter()
            .position(|b| b.from_folders > FOLDER_ASK_ABOVE)
            .unwrap();
        assert!(!batches[asks].done);
        // Put together, the batches are the whole folder, in order, read.
        let mut all = Gathered::default();
        for b in batches {
            all.absorb(b.part);
        }
        assert_eq!(all.files.len(), 1000);
        assert_eq!(all.from_folders, 1000);
        assert_eq!(all.folders, ["Big"]);
        assert_eq!(all.source().as_deref(), Some("Big"));
        assert!(all.skipped.is_empty());
        let first = &all.files[0];
        assert_eq!(
            first.path,
            std::path::absolute(big.join("0000.png")).unwrap()
        );
        assert_eq!(first.size, 12);
        assert!(first.modified.is_some());
        assert_eq!(first.check, FileCheck::Ok);
        assert!(all.files.windows(2).all(|w| w[0].path < w[1].path));
    }

    #[test]
    fn a_walk_finds_what_expand_dropped_finds() {
        let dir = tempfile::tempdir().unwrap();
        let holiday = dir.path().join("Holiday");
        fs::create_dir_all(holiday.join("beach").join("deep")).unwrap();
        fs::write(holiday.join("a.jpg"), b"x").unwrap();
        fs::write(holiday.join(".secret.jpg"), b"x").unwrap();
        fs::write(holiday.join("beach").join("b.jpg"), b"x").unwrap();
        fs::write(holiday.join("beach").join("deep").join("c.jpg"), b"x").unwrap();
        let loose = dir.path().join("loose.png");
        fs::write(&loose, b"\x89PNG\r\n\x1a\n").unwrap();
        let empty = dir.path().join("empty.png");
        fs::write(&empty, b"").unwrap();
        let gone = dir.path().join("gone.png");
        let paths = vec![loose, gone, holiday, empty];

        let old = expand_dropped(paths.clone());
        let mut new = Gathered::default();
        for b in walk_all(paths, CancelToken::new()) {
            new.absorb(b.part);
        }
        let found: Vec<&PathBuf> = new.files.iter().map(|f| &f.path).collect();
        let want: Vec<PathBuf> = old
            .files
            .iter()
            .map(|p| std::path::absolute(p).unwrap())
            .collect();
        assert_eq!(found, want.iter().collect::<Vec<_>>());
        assert_eq!(new.from_folders, old.from_folders);
        assert_eq!(new.folders, old.folders);
        assert_eq!(new.skipped, old.skipped);
        assert_eq!(new.source(), old.source());
        assert_eq!(new.unopened, 0);
        // The looks at the first bytes are taken on the walk: "x" is no
        // JPEG, and an empty file is empty.
        let check = |name: &str| {
            new.files
                .iter()
                .find(|f| f.path.ends_with(name))
                .unwrap()
                .check
        };
        assert_eq!(check("a.jpg"), FileCheck::Mismatch);
        assert_eq!(check("loose.png"), FileCheck::Ok);
        assert_eq!(check("empty.png"), FileCheck::Empty);
    }

    #[test]
    fn a_cancelled_walk_stops_and_sends_nothing_more() {
        let dir = tempfile::tempdir().unwrap();
        let big = folder_of(dir.path(), "Big", 1000);
        // Cancelled once the first batch arrives, as Cancel on the prompt does.
        let cancel = CancelToken::new();
        let (tx, rx) = std::sync::mpsc::channel();
        let stop = cancel.clone();
        let walk = walk_dropped(vec![big.clone()], Registry::builtin(), cancel, move |b| {
            stop.cancel();
            let _ = tx.send(b);
        })
        .unwrap();
        walk.join().unwrap();
        let batches: Vec<WalkBatch> = rx.into_iter().collect();
        assert_eq!(batches.len(), 1);
        assert!(!batches[0].done);
        assert!(batches[0].from_folders < 1000);
        // Cancelled before it starts, it sends nothing at all.
        let cancel = CancelToken::new();
        cancel.cancel();
        assert!(walk_all(vec![big], cancel).is_empty());
    }

    #[test]
    fn a_walks_listed_paths_are_kept_across_shares_until_the_list_changes() {
        let (dir, kiln) = setup();
        let read = |name: &str| ReadFile {
            path: dir.path().join(name),
            size: 1,
            modified: None,
            check: FileCheck::Ok,
        };
        let all = Availability::all();
        let mut list = FileList::default();
        let mut listed = ListedPaths::default();
        let r = list.add_read_with(&mut listed, &kiln, &all, vec![read("a.png")]);
        assert_eq!(r.added, 1);
        // Kept: the next share finds a.png without looking at the rows.
        let kept = listed.rev;
        let r = list.add_read_with(&mut listed, &kiln, &all, vec![read("A.PNG"), read("b.png")]);
        assert_eq!((r.added, r.duplicates), (1, 1));
        assert_ne!(listed.rev, kept);
        // The list changed between shares: the paths are taken again, so
        // a row removed meanwhile can be added, and one added elsewhere
        // counts as already there.
        list.clear();
        fs::write(dir.path().join("c.png"), b"x").unwrap();
        assert_eq!(
            list.add(&kiln, &all, vec![dir.path().join("c.png")]).added,
            1
        );
        let r = list.add_read_with(&mut listed, &kiln, &all, vec![read("a.png"), read("c.png")]);
        assert_eq!((r.added, r.duplicates), (1, 1));
    }

    #[test]
    fn adding_what_a_walk_read_touches_no_disk() {
        let (dir, kiln) = setup();
        // Two of these are nowhere on the disk, and the third says other
        // than the file there does: the rows are made from what the walk
        // said alone.
        let real = dir.path().join("real.png");
        fs::write(&real, b"\x89PNG\r\n\x1a\nrest").unwrap();
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let read = |path: PathBuf, size, check| ReadFile {
            path,
            size,
            modified: Some(at),
            check,
        };
        let files = vec![
            read(dir.path().join("nowhere.png"), 4096, FileCheck::Ok),
            read(real.clone(), 99, FileCheck::Mismatch),
            read(dir.path().join("NOWHERE.PNG"), 1, FileCheck::Ok),
        ];
        let mut list = FileList::default();
        let rev = list.revision();
        let r = list.add_read(&kiln, &Availability::all(), files);
        assert_eq!((r.added, r.duplicates, r.skipped), (2, 1, 0));
        assert_ne!(list.revision(), rev);
        let e = &list.entries()[0];
        assert_eq!(e.path, dir.path().join("nowhere.png"));
        assert_eq!(
            (e.size, e.modified, e.check),
            (4096, Some(at), FileCheck::Ok)
        );
        assert!(!e.targets.is_empty());
        // The walk's look stands, though the file on disk is a fine PNG.
        let e = &list.entries()[1];
        assert_eq!((e.size, e.check), (99, FileCheck::Mismatch));
        assert!(e.targets.is_empty());
        // The same file again is a duplicate.
        let r = list.add_read(&kiln, &Availability::all(), [read(real, 99, FileCheck::Ok)]);
        assert_eq!((r.added, r.duplicates), (0, 1));
    }

    #[test]
    fn a_running_row_says_how_far_and_how_long() {
        let sec = Duration::from_secs;
        // Too early to say how long.
        assert_eq!(time_left(sec(2), 0.5), None);
        assert_eq!(time_left(sec(30), 0.0), None);
        assert_eq!(time_left(sec(30), 1.0), None);
        // Half done after a minute: about a minute more.
        assert_eq!(time_left(sec(60), 0.5), Some(sec(60)));
        assert_eq!(time_left(sec(60), 0.25), Some(sec(180)));
        assert_eq!(left_text(sec(3)), "a few seconds");
        assert_eq!(left_text(sec(33)), "about 35 sec");
        assert_eq!(left_text(sec(60)), "about 1 min");
        assert_eq!(left_text(sec(120)), "about 2 min");
        assert_eq!(left_text(sec(3000)), "about 50 min");
        assert_eq!(left_text(sec(5400)), "about 1 h 30 min");
        assert_eq!(left_text(sec(7200)), "about 2 h");
        assert_eq!(left_text(sec(4800)), "about 1 h 20 min");
        assert_eq!(
            progress_line("Converting to WEBM", Some(0.38), Some(sec(120))),
            "Converting to WEBM · 38% · about 2 min"
        );
        assert_eq!(
            progress_line("Converting to WEBM", Some(0.38), None),
            "Converting to WEBM · 38%"
        );
        // A helper that can't say how far.
        assert_eq!(
            progress_line("Converting to WEBM", None, None),
            "Converting to WEBM · Working…"
        );
    }

    #[test]
    fn a_row_line_follows_the_job() {
        let mut e = row("a.mp4");
        e.job = JobState::Running {
            action: Action::Convert("webm".into()),
            fraction: None,
            started: None,
        };
        // Task 18 (mock screen 5): the line says what it is making; how far
        // along is at the row's right end and in the side pane.
        assert_eq!(e.line().0, "Converting to WEBM");
        e.job = JobState::Running {
            action: Action::Convert("webm".into()),
            fraction: Some(0.5),
            started: Some(Instant::now() - Duration::from_secs(60)),
        };
        assert_eq!(e.line().0, "Converting to WEBM");
        assert_eq!(e.mark(), RowMark::Progress(Some(0.5)));
    }

    #[test]
    fn each_finish_is_said_in_a_sentence() {
        assert_eq!(job_outcome("a.png", true, false), "a.png is ready.");
        assert_eq!(
            job_outcome("a.png", false, false),
            "a.png couldn't be converted."
        );
        assert_eq!(job_outcome("a.png", false, true), "a.png was cancelled.");
    }

    #[test]
    fn a_batch_and_the_taskbar_count_what_is_done() {
        assert_eq!(batch_progress(12, 50), "12 of 50 done.");
        // Nothing done yet: what is under way, never "0 of 1 done." beside
        // a row an earlier batch saved.
        assert_eq!(batch_progress(0, 1), "Working on 1 file.");
        assert_eq!(batch_progress(0, 3), "Working on 3 files.");
        assert_eq!(taskbar_progress(0, 0, &[]), TaskbarProgress::None);
        assert_eq!(taskbar_progress(3, 3, &[]), TaskbarProgress::None);
        assert_eq!(
            taskbar_progress(2, 0, &[None, None]),
            TaskbarProgress::Indeterminate
        );
        // One of four done, two half done: 2 of 4.
        assert_eq!(
            taskbar_progress(4, 1, &[Some(0.5), Some(0.5), None]),
            TaskbarProgress::Fraction(0.5)
        );
        assert_eq!(
            taskbar_progress(4, 1, &[None]),
            TaskbarProgress::Fraction(0.25)
        );
    }

    #[test]
    fn finishing_while_away_names_the_result() {
        let n =
            |ok, failed, cancelled, first| finish_notice(ok, failed, cancelled, first).map(|t| t.1);
        assert_eq!(
            n(1, 0, 0, Some("photo.jpg")).as_deref(),
            Some("photo.jpg is ready.")
        );
        assert_eq!(n(12, 0, 0, None).as_deref(), Some("12 files are ready."));
        assert_eq!(
            n(0, 1, 0, None).as_deref(),
            Some("A file couldn't be converted. Open Kiln to see why.")
        );
        assert_eq!(
            n(3, 2, 0, None).as_deref(),
            Some("3 ready, 2 couldn't be converted.")
        );
        // Only cancelled: nothing to announce.
        assert_eq!(n(0, 0, 0, None), None);
    }

    #[test]
    fn the_finish_notice_says_where_saved_results_went() {
        let saved = |names: &[&str]| Saves {
            saved: names.iter().map(|s| (*s).to_owned()).collect(),
            ..Saves::default()
        };
        // Saved next to the original: Kiln's notice says so, and a click
        // shows the result there.
        assert_eq!(
            finish_notice_saved(1, 0, 0, Some("photo.webp"), &saved(&["photo.webp"])),
            Some((
                "Kiln".into(),
                "Saved photo.webp next to the original.".into(),
                true
            ))
        );
        assert_eq!(
            finish_notice_saved(3, 0, 1, None, &saved(&["a.jpg", "b.jpg", "c.jpg"])),
            Some((
                "Kiln".into(),
                "Saved 3 files next to the originals.".into(),
                true
            ))
        );
        // Kept: it says so, and a click opens Kiln on it.
        let kept = Saves {
            kept: vec![("DCIM".into(), "photo.webp".into())],
            ..Saves::default()
        };
        assert_eq!(
            finish_notice_saved(1, 0, 0, Some("photo.webp"), &kept),
            Some((
                "Kiln".into(),
                "Couldn't save photo.webp. Kiln tries again the next time it starts.".into(),
                false
            ))
        );
        // A failure reads as it did, and so does a result that is held.
        assert_eq!(
            finish_notice_saved(1, 1, 0, None, &saved(&["a.jpg"])),
            Some((
                "Kiln".into(),
                "1 ready, 1 couldn't be converted.".into(),
                false
            ))
        );
        let held = Saves {
            held: 1,
            ..Saves::default()
        };
        assert_eq!(
            finish_notice_saved(1, 0, 0, Some("photo.jpg"), &held),
            Some(("Kiln".into(), "photo.jpg is ready.".into(), false))
        );
        assert_eq!(finish_notice_saved(0, 0, 2, None, &saved(&[])), None);
    }

    #[test]
    fn finish_notifies_with_the_result_name() {
        let webp = Action::Convert("webp".into());
        let ok = |name: &str| (name.to_owned(), None);
        let bad = |name: &str| (name.to_owned(), Some("FFmpeg isn't installed.".to_owned()));
        let n = |action: &Action, jobs: &[(String, Option<String>)]| {
            drag_notice(action, jobs).map(|t| (t.1, t.2))
        };
        // One file: named.
        assert_eq!(
            n(&webp, &[ok("photo.jpg")]),
            Some(("Converted photo.jpg to WEBP.".into(), 0))
        );
        assert_eq!(
            n(&webp, &[bad("photo.jpg")]),
            Some((
                "Couldn't convert photo.jpg: FFmpeg isn't installed.".into(),
                0
            ))
        );
        assert_eq!(
            drag_notice(&webp, &[ok("photo.jpg")])
                .map(|t| t.0)
                .as_deref(),
            Some("Kiln")
        );
        // Several: one notice for the drop, counted. A click opens the
        // first that failed, or else the first.
        let three = [ok("a.png"), ok("b.png"), ok("c.png")];
        assert_eq!(
            n(&webp, &three),
            Some(("Converted 3 files to WEBP.".into(), 0))
        );
        assert_eq!(
            n(&webp, &[ok("a.png"), bad("b.png"), ok("c.png")]),
            Some(("Converted 2 of 3 files to WEBP.".into(), 1))
        );
        assert_eq!(
            n(&webp, &[bad("a.png"), bad("b.png")]),
            Some(("Couldn't convert 2 files to WEBP.".into(), 0))
        );
        // A tool in its own words.
        let borders = Action::Tool(Tool::RemoveEdges, Kind::Image);
        assert_eq!(
            n(&borders, &[ok("photo.png")]),
            Some(("Removed borders from photo.png.".into(), 0))
        );
        assert_eq!(
            n(&borders, &[bad("photo.png")]),
            Some((
                "Couldn't remove borders from photo.png: FFmpeg isn't installed.".into(),
                0
            ))
        );
        assert_eq!(
            n(&borders, &three),
            Some(("Removed borders from 3 files.".into(), 0))
        );
        assert_eq!(
            n(&borders, &[ok("a.png"), ok("b.png"), bad("c.png")]),
            Some(("Removed borders from 2 of 3 files.".into(), 2))
        );
        let quiet = Action::Tool(Tool::RemoveQuietEnds, Kind::Audio);
        assert_eq!(
            n(&quiet, &[bad("a.wav"), bad("b.wav")]),
            Some(("Couldn't remove silence at the ends of 2 files.".into(), 0))
        );
        let halves = Action::Tool(Tool::CutInHalf, Kind::Video);
        assert_eq!(
            n(&halves, &[ok("talk.mp4")]),
            Some(("Cut talk.mp4 in half.".into(), 0))
        );
        let smaller = Action::Tool(Tool::Smaller, Kind::Video);
        assert_eq!(
            n(&smaller, &[ok("a.mp4"), bad("b.mp4")]),
            Some(("Made 1 of 2 files smaller.".into(), 1))
        );
        assert_eq!(
            n(&Action::Tool(Tool::Merge, Kind::Document), &[ok("3 files")]),
            Some(("Merged 3 files.".into(), 0))
        );
        // Every job cancelled: nothing to tell.
        assert_eq!(n(&webp, &[]), None);
    }

    #[test]
    fn every_result_goes_beside_from_any_way_in_with_a_holding_folder() {
        // With a holding folder each result is delivered as soon as it is
        // ready; without one it was written in its folder already.
        assert_eq!(autosave_for(true), AutoSave::Beside);
        assert_eq!(autosave_for(false), AutoSave::Hold);
        assert_eq!(AutoSave::Beside.held(3), 0);
    }

    #[test]
    fn the_start_says_where_last_times_results_went_not_what_settings_say_now() {
        let pictures = PathBuf::from(r"C:\Users\me\Pictures");
        let converted = PathBuf::from(r"D:\Converted");
        let results = PathBuf::from(r"C:\Users\me\Documents\Kiln results");
        let report = |beside, fallback, kept, folders: &[&PathBuf]| kiln_core::Report {
            beside,
            fallback,
            kept,
            folders: folders.iter().map(|f| (*f).clone()).collect(),
        };
        let said = |r: &kiln_core::Report| delivered_info(r, Some(&results));
        assert_eq!(said(&report(0, 0, 0, &[])), None);
        // Last time's folder, whatever Settings say now.
        assert_eq!(
            said(&report(3, 0, 0, &[&converted])).as_deref(),
            Some("Saved 3 converted files from last time in Converted.")
        );
        assert_eq!(
            said(&report(1, 0, 0, &[&pictures])).as_deref(),
            Some("Saved 1 converted file from last time in Pictures.")
        );
        // Several folders, said by how many.
        let docs = PathBuf::from(r"C:\Users\me\Documents");
        assert_eq!(
            said(&report(2, 0, 0, &[&pictures, &docs])).as_deref(),
            Some("Saved 2 converted files from last time in 2 folders.")
        );
        // Some went to the fallback.
        assert_eq!(
            said(&report(2, 1, 0, &[&pictures, &results])).as_deref(),
            Some(
                "Saved 3 converted files from last time: 2 in Pictures, \
                 1 in Documents\\Kiln results."
            )
        );
        assert_eq!(
            said(&report(0, 2, 0, &[&results])).as_deref(),
            Some("Saved 2 converted files from last time in Documents\\Kiln results.")
        );
        assert_eq!(
            said(&report(0, 0, 1, &[])).as_deref(),
            Some(
                "1 converted file from last time couldn't be saved. \
                 Kiln tries again the next time it starts."
            )
        );
        assert_eq!(
            said(&report(1, 0, 2, &[&pictures])).as_deref(),
            Some(
                "Saved 1 converted file from last time in Pictures. \
                 2 couldn't be saved. Kiln tries again the next time it starts."
            )
        );
    }

    #[test]
    fn a_row_says_where_its_result_went_when_its_folder_refused_it() {
        assert_eq!(
            fallback_line("DCIM"),
            "Saved in Documents\\Kiln results. Kiln can't save in DCIM."
        );
        assert_eq!(
            KEPT_LINE,
            "Couldn't save this. Kiln tries again the next time it starts."
        );
    }

    #[test]
    fn notices_say_where() {
        let names = |n: &[&str]| n.iter().map(|s| (*s).to_owned()).collect::<Vec<String>>();
        assert_eq!(
            saved_notice(&names(&["photo.webp"]), None),
            "Saved photo.webp next to the original."
        );
        assert_eq!(
            saved_notice(&names(&["a.webp", "b.webp", "c.webp"]), None),
            "Saved 3 files next to the originals."
        );
        // Into the folder chosen in Settings.
        assert_eq!(
            saved_notice(&names(&["photo.webp"]), Some("Converted")),
            "Saved photo.webp in Converted."
        );
        assert_eq!(
            saved_notice(&names(&["a.webp", "b.webp"]), Some("Converted")),
            "Saved 2 files in Converted."
        );
        assert_eq!(
            kept_notice("photo.webp"),
            "Couldn't save photo.webp. Kiln tries again the next time it starts."
        );
    }

    #[test]
    fn a_notice_says_where_its_results_went_once_they_are_in() {
        let saves = |saved: &[&str], kept: &[(&str, &str)], held| Saves {
            saved: saved.iter().map(|s| (*s).to_owned()).collect(),
            kept: kept
                .iter()
                .map(|(f, n)| ((*f).to_owned(), (*n).to_owned()))
                .collect(),
            first_kept: None,
            held,
            ..Saves::default()
        };
        // Every result saved: where they are, and a click shows them.
        let one = saves(&["photo.webp"], &[], 0);
        assert_eq!(
            one.notice().as_deref(),
            Some("Saved photo.webp next to the original.")
        );
        assert!(one.all_saved());
        let three = saves(&["a.webp", "b.webp", "c.webp"], &[], 0);
        assert_eq!(
            three.notice().as_deref(),
            Some("Saved 3 files next to the originals.")
        );
        assert!(three.all_saved());
        // One couldn't be saved: that is the news, and it is in Kiln.
        let kept = saves(&["a.webp"], &[("DCIM", "photo.webp")], 0);
        assert_eq!(
            kept.notice().as_deref(),
            Some("Couldn't save photo.webp. Kiln tries again the next time it starts.")
        );
        assert!(!kept.all_saved());
        let card = saves(&[], &[("DCIM", "a.webp"), ("DCIM", "b.webp")], 0);
        assert_eq!(
            card.notice().as_deref(),
            Some("Couldn't save 2 files. Kiln tries again the next time it starts.")
        );
        assert!(!card.all_saved());
        // Nothing went next to the original, or some results are held
        // anyway: the notice reads as it did.
        for s in [
            saves(&[], &[], 0),
            saves(&[], &[], 2),
            saves(&["a.webp"], &[], 1),
        ] {
            assert_eq!(s.notice(), None, "{s:?}");
            assert!(!s.all_saved(), "{s:?}");
        }
    }

    #[test]
    fn saved_results_show_together_only_from_one_folder() {
        let mut s = Saves::default();
        assert_eq!(s.together(), None);
        s.paths = vec![PathBuf::from(r"D:\C\a.jpg"), PathBuf::from(r"D:\C\b.jpg")];
        assert_eq!(
            s.together(),
            Some((PathBuf::from(r"D:\C"), s.paths.clone()))
        );
        s.paths.push(PathBuf::from(r"E:\x\c.jpg"));
        assert_eq!(s.together(), None);
    }

    #[test]
    fn a_kept_notice_opens_the_kept_row() {
        let card = PathBuf::from(r"E:\DCIM\b.png");
        let kept = Saves {
            saved: vec!["a.webp".into()],
            kept: vec![("DCIM".into(), "b.webp".into())],
            first_kept: Some(card.clone()),
            held: 0,
            ..Saves::default()
        };
        // It says b.webp is kept: a click opens b.png's row.
        assert_eq!(kept.opens(), Some(card.as_path()));
        // A notice about saved results, or one reading as it did, opens
        // the row it names.
        let saved = Saves {
            saved: vec!["a.webp".into()],
            ..Saves::default()
        };
        assert_eq!(saved.opens(), None);
        let held = Saves { held: 1, ..kept };
        assert_eq!(held.opens(), None);
    }

    #[test]
    fn no_prompt_asks_about_saving_results() {
        // The only questions left are about stopping running jobs and big
        // folders: none is about saving, keeping or throwing away results.
        let mut asks: Vec<Prompt> = [Leave::Close, Leave::Clear, Leave::Remove]
            .into_iter()
            .filter_map(|v| leave_prompt(1, v))
            .collect();
        asks.push(big_folder_prompt(1240));
        for p in &asks {
            for word in ["next to the original", "save", "Save", "Discard", "lost"] {
                assert!(!p.title.contains(word), "{}", p.title);
                for b in &p.buttons {
                    assert!(!b.label().contains(word), "{b:?}");
                }
            }
        }
    }

    #[test]
    fn rows_say_what_became_of_the_result() {
        let mut e = row("a.png");
        e.path = PathBuf::from(r"C:\photos\a.png");
        e.job = JobState::Done {
            action: Action::Convert("jpg".into()),
            outputs: vec![PathBuf::from(r"held\a.jpg")],
            saved: None,
        };
        // On its way to its folder: nothing to click, nothing to drag.
        assert_eq!(e.line(), ("Saving…".to_owned(), Tone::Muted));
        assert_eq!(e.link(false), None);
        assert!(!e.tooltip("", false).contains("Drag"));
        // Saved beside the original, or elsewhere, with the full path on
        // hover.
        e.job = JobState::Done {
            action: Action::Convert("jpg".into()),
            outputs: vec![PathBuf::from(r"C:\photos\a.jpg")],
            saved: Some(Saved::Folder(PathBuf::from(r"C:\photos"))),
        };
        // Task 18 (mock screen 5): "Saved as" the new file's name, beside
        // the original or elsewhere; Show in folder takes you to it.
        assert_eq!(e.line().0, "Saved as a.jpg");
        assert_eq!(e.link(false), Some("Show in folder"));
        e.job = JobState::Done {
            action: Action::Convert("jpg".into()),
            outputs: vec![PathBuf::from(r"D:\Out\a.jpg")],
            saved: Some(Saved::Folder(PathBuf::from(r"D:\Out"))),
        };
        assert_eq!(e.line().0, "Saved as a.jpg");
        assert!(
            e.tooltip("", false).ends_with(r"Saved to D:\Out"),
            "{}",
            e.tooltip("", false)
        );
    }

    #[test]
    fn removed_rows_come_back_where_they_were_and_selected() {
        let mut list = list_of(6);
        // Remove rows 1, 2 and 4, as the app does, one at a time.
        let mut gone: Vec<(usize, Entry)> = [1usize, 2, 4]
            .iter()
            .map(|&i| (i, list.entries()[i].clone()))
            .collect();
        gone.sort_by_key(|(i, _)| *i);
        for name in ["1.png", "2.png", "4.png"] {
            let at = list
                .index_of(Path::new("photos").join(name).as_path())
                .unwrap();
            list.remove(at);
        }
        assert_eq!(list.len(), 3);
        let mut back = vec![];
        for (i, e) in gone {
            list.insert(i, e.restored());
            back.push(i);
        }
        let names: Vec<&str> = list.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            ["0.png", "1.png", "2.png", "3.png", "4.png", "5.png"]
        );
        list.select_only(&back);
        assert_eq!(list.selection(), [1, 2, 4]);
        assert_eq!(removed_line(500), "500 files removed.");
        assert_eq!(restored_line(500), "500 files are back.");
    }

    #[test]
    fn a_restored_row_forgets_a_job_that_was_stopped_but_keeps_a_save() {
        let mut running = row("a.png");
        running.job = JobState::Running {
            action: Action::Convert("jpg".into()),
            fraction: None,
            started: None,
        };
        assert_eq!(running.restored().job, JobState::Idle);
        let saved = JobState::Done {
            action: Action::Convert("jpg".into()),
            outputs: vec![PathBuf::from(r"C:\Out\a.jpg")],
            saved: Some(Saved::Folder(PathBuf::from(r"C:\Out"))),
        };
        let mut e = row("a.png");
        e.job = saved.clone();
        assert_eq!(e.restored().job, saved);
        // A held result may have been discarded with it: back to idle.
        let mut held = row("a.png");
        held.job = JobState::Done {
            action: Action::Convert("jpg".into()),
            outputs: vec![PathBuf::from("held.jpg")],
            saved: None,
        };
        assert_eq!(held.restored().job, JobState::Idle);
        // A row a conversion left out for a missing helper lost its note
        // when it was removed: back, it has no Get it.
        let mut waiting = row("b.avi");
        waiting.waits_on = Some("ffmpeg".into());
        assert_eq!(waiting.link(false), Some("Get FFmpeg"));
        let back = waiting.restored();
        assert_eq!(back.waits_on, None);
        assert_ne!(back.link(false), Some("Get FFmpeg"));
    }

    #[test]
    fn a_get_link_always_names_what_it_gets() {
        // never "Get it".
        for backend in ["pandoc", "libreoffice", "calibre", "ffmpeg"] {
            let link = waits_on_link(backend).unwrap();
            assert_eq!(link, format!("Get {}", helper_name(backend)));
            assert_ne!(link, "Get it");
        }
        assert_eq!(store_label("9N4WGH0Z6VHQ"), "Get HEVC extension");
        assert_eq!(store_label("9MVZQVXJBQ9V"), "Get AV1 extension");
        assert_eq!(store_label("9N4D0MSMP0PT"), "Get VP9 extension");
        assert_eq!(store_label("9PMMSR1CGPWG"), "Get HEIF extension");
        assert_eq!(store_label("unknown"), "Get extension");
    }

    #[test]
    fn a_row_waiting_on_a_helper_links_to_get_it() {
        // Get it for a helper with a page or FFmpeg (Settings),
        // "Get Media Feature Pack" for Windows' Media Feature Pack, nothing
        // otherwise.
        // Always named, never "Get it".
        assert_eq!(waits_on_link("pandoc"), Some("Get Pandoc"));
        assert_eq!(waits_on_link("libreoffice"), Some("Get LibreOffice"));
        assert_eq!(waits_on_link("calibre"), Some("Get Calibre"));
        assert_eq!(waits_on_link("ffmpeg"), Some("Get FFmpeg"));
        assert_eq!(waits_on_link("audio-mf"), Some("Get Media Feature Pack"));
        assert_eq!(waits_on_link("office"), None);
        let mut e = row("a.md");
        e.waits_on = Some("audio-mf".into());
        assert_eq!(e.link(false), Some("Get Media Feature Pack"));
        // Only while nothing has been done to it.
        e.job = JobState::Running {
            action: Action::Convert("docx".into()),
            fraction: None,
            started: None,
        };
        assert_eq!(e.link(false), None);
    }

    #[test]
    fn a_row_reads_as_labelled_parts() {
        let mut e = row("photo.png");
        e.targets = vec!["JPG".into(), "PDF".into()];
        // Before facts: its type, the size and the folder, apart (Task 18:
        // the row says what the file is, mock screen 2).
        assert_eq!(e.spoken_detail(), "PNG. File size 3.1 MB. Folder photos");
        // With facts, the sizes are read as "by"; a suggestion is the side
        // pane's to put first, not the row's.
        e.facts = Some(picture(facts("png", Kind::Image), 1600, 1200, Look::Photo));
        e.suggestion = Some(Suggestion {
            action: Suggested::Convert("jpg".into()),
            estimate: Some(890_000),
            reason: String::new(),
        });
        assert_eq!(
            e.spoken_detail(),
            "1600 by 1200 photo. File size 3.1 MB. Folder photos"
        );
        // A file ruled out says why, then where it is.
        e.check = FileCheck::Empty;
        assert!(
            e.spoken_detail().starts_with("Empty file (0 bytes). "),
            "{}",
            e.spoken_detail()
        );
    }

    #[test]
    fn a_file_that_cannot_work_says_so_and_offers_nothing() {
        let (dir, kiln) = setup();
        let w = |name: &str, bytes: &[u8]| {
            let p = dir.path().join(name);
            fs::write(&p, bytes).unwrap();
            p
        };
        let empty = w("empty.png", b"");
        let fake = w("fake.docx", &[7u8; 2000]);
        let good = w("good.docx", b"PK\x03\x04rest of a zip");
        let mut list = FileList::default();
        list.add(&kiln, &Availability::all(), [empty, fake, good]);
        let (e, f, g) = (&list.entries()[0], &list.entries()[1], &list.entries()[2]);
        assert_eq!(e.check, FileCheck::Empty);
        assert!(e.targets.is_empty());
        assert!(
            e.plain_detail().starts_with("Empty file (0 bytes)"),
            "{}",
            e.plain_detail()
        );
        assert_eq!(e.line().1, Tone::Crack);
        assert_eq!(f.check, FileCheck::Mismatch);
        assert!(f.targets.is_empty());
        assert!(
            f.detail().starts_with("Isn't really a DOCX file"),
            "{}",
            f.detail()
        );
        // Neither offers a way to start; a real DOCX does.
        assert_eq!(e.link(false), None);
        assert_eq!(f.link(false), None);
        assert_eq!(g.check, FileCheck::Ok);
        assert!(!g.targets.is_empty());
        assert_eq!(g.link(false), Some("Convert"));
    }

    #[test]
    fn a_silent_video_loses_its_audio_formats_once_its_facts_are_in() {
        let (dir, kiln) = setup();
        let mp4 = dir.path().join("clip.mp4");
        fs::write(&mp4, b"\0\0\0\x18ftypmp42").unwrap();
        let none = Availability::all().without(&["ffmpeg"]);
        let mut list = FileList::default();
        list.add(&kiln, &none, [mp4.clone()]);
        // Until it is looked at, its sound is Kiln's own work.
        assert!(list.entries()[0].targets.iter().any(|t| t == "MP3"));
        let mut facts = facts("mp4", Kind::Video);
        facts.silent = true;
        facts.audio_problem = Some("This video has no sound to convert.".into());
        kiln.learn(&mp4, &facts);
        assert!(list.set_facts(&mp4, facts, None));
        list.refresh_offers_for(&kiln, &none, &mp4);
        let v = &list.entries()[0];
        // Its facts stay beside what waits on FFmpeg.
        assert!(v.facts.is_some());
        // Its picture is Kiln's own work among MP4, MOV, M4V and MKV, so
        // the row offers those and no sound.
        assert_eq!(v.targets, ["M4V", "MKV", "MOV"]);
        // Task 18: the row says what the file is, then how many formats
        // wait on FFmpeg.
        assert!(
            v.detail().ends_with(" · 14 formats need FFmpeg"),
            "{}",
            v.detail()
        );
        // a screen reader hears it too.
        assert!(
            v.spoken_detail().contains(". 14 formats need FFmpeg. "),
            "{}",
            v.spoken_detail()
        );
        // It has something to convert to, so the row offers that.
        assert_eq!(v.link(false), Some("Convert"));
        list.refresh_offers_for(&kiln, &Availability::all(), &mp4);
        let v = &list.entries()[0];
        assert!(!v.targets.is_empty() && !v.targets.iter().any(|t| t == "MP3"));
    }

    #[test]
    fn a_missing_helper_shows_on_the_row_with_a_get_link() {
        let (dir, kiln) = setup();
        // An AVI: Kiln reads the sound of an MP4 itself, but not of this.
        let avi = dir.path().join("clip.avi");
        fs::write(&avi, b"RIFF\x04\0\0\0AVI ").unwrap();
        let png = dir.path().join("p.png");
        fs::write(&png, b"\x89PNG\r\n\x1a\nxx").unwrap();
        let none = Availability::all().without(&["ffmpeg"]);
        let mut list = FileList::default();
        list.add(&kiln, &none, [avi.clone(), png]);
        let (v, p) = (&list.entries()[0], &list.entries()[1]);
        assert!(v.targets.is_empty());
        assert_eq!(
            v.missing.as_ref().map(|m| m.backend.as_str()),
            Some("ffmpeg")
        );
        assert!(
            v.detail().starts_with("Needs FFmpeg to convert"),
            "{}",
            v.detail()
        );
        // FFmpeg comes with Kiln: there is no page to send anyone to.
        assert_eq!(v.link(false), None);
        // The picture needs no helper for most of its formats, and says
        // how many wait on FFmpeg, if any do.
        assert!(!p.targets.is_empty());
        assert_eq!(p.link(false), Some("Convert"));
        // FFmpeg turns up: the row offers its formats again.
        list.refresh_offers(&kiln, &Availability::all());
        let v = &list.entries()[0];
        assert!(!v.targets.is_empty() && v.missing.is_none());
        assert_eq!(v.link(false), Some("Convert"));
        assert!(!v.detail().contains("FFmpeg"), "{}", v.detail());
    }

    #[test]
    fn a_row_names_the_helper_of_the_format_most_people_want() {
        let (dir, kiln) = setup();
        let docx = dir.path().join("report.docx");
        fs::write(&docx, b"PK\x03\x04rest of a zip").unwrap();
        let md = dir.path().join("notes.md");
        fs::write(&md, b"# Notes\n").unwrap();
        let heic = dir.path().join("photo.heic");
        fs::write(&heic, b"\0\0\0\x18ftypheic\0\0\0\0mif1heic").unwrap();
        let neither = Availability::all().without(&["office", "libreoffice", "pandoc", "ffmpeg"]);
        let mut list = FileList::default();
        list.add(&kiln, &neither, [docx.clone(), md.clone(), heic.clone()]);
        let row = |list: &FileList, p: &Path| list.entries()[list.index_of(p).unwrap()].clone();

        // A DOCX: PDF comes first, and LibreOffice makes it, not Pandoc.
        let d = row(&list, &docx);
        assert!(d.targets.is_empty(), "{:?}", d.targets);
        assert_eq!(
            d.missing.as_ref().map(|m| m.backend.as_str()),
            Some("libreoffice")
        );
        assert!(
            d.detail().starts_with("Needs LibreOffice to make a PDF"),
            "{}",
            d.detail()
        );
        // A Markdown file needs Pandoc first, whatever it becomes; Pandoc
        // alone doesn't make its PDF, so the row doesn't promise one.
        let m = row(&list, &md);
        assert_eq!(
            m.missing.as_ref().map(|m| m.backend.as_str()),
            Some("pandoc")
        );
        assert!(
            m.detail().starts_with("Needs Pandoc to convert"),
            "{}",
            m.detail()
        );
        // A HEIC names FFmpeg, which brings FFprobe, never FFprobe.
        let h = row(&list, &heic);
        assert!(h.detail().contains("FFmpeg"), "{}", h.detail());
        assert!(!h.detail().contains("FFprobe"), "{}", h.detail());

        // With Pandoc, the DOCX converts to its formats and says PDF waits
        // on LibreOffice.
        let pandoc = Availability::all().without(&["office", "libreoffice", "ffmpeg"]);
        list.refresh_offers(&kiln, &pandoc);
        let d = row(&list, &docx);
        assert!(!d.targets.is_empty());
        assert!(
            d.detail().contains("formats need LibreOffice")
                || d.detail().contains("format needs LibreOffice"),
            "{}",
            d.detail()
        );
    }

    #[test]
    fn more_need_counts_only_the_formats_waiting_on_the_named_helper() {
        let (dir, kiln) = setup();
        let docx = dir.path().join("report.docx");
        fs::write(&docx, b"PK\x03\x04rest of a zip").unwrap();
        // Pandoc runs; Microsoft Office, LibreOffice and Calibre aren't there.
        let some = Availability::all().without(&["office", "libreoffice", "calibre"]);
        let (usable, blocked) = kiln.targets_split(&docx, &some);
        let waits: Vec<(String, &str)> = blocked
            .iter()
            .map(|(f, b)| (f.code(), b.as_str()))
            .collect();
        let on_lo = waits.iter().filter(|(_, b)| *b == "libreoffice").count();
        assert!(!usable.is_empty(), "{waits:?}");
        assert!(on_lo > 0 && on_lo < waits.len(), "{waits:?}");
        let mut list = FileList::default();
        list.add(&kiln, &some, [docx]);
        let d = &list.entries()[0];
        assert_eq!(
            d.missing.as_ref().map(|m| (m.backend.as_str(), m.count)),
            Some(("libreoffice", on_lo)),
            "{waits:?}"
        );
        // Task 18: what the file is and its size, then how many formats
        // wait on the helper.
        let need = if on_lo == 1 {
            "format needs"
        } else {
            "formats need"
        };
        let want = format!("Word document · 17 bytes · {on_lo} {need} LibreOffice");
        assert_eq!(d.detail(), want, "{waits:?}");
        // One is one format.
        let mut one = d.clone();
        one.missing.as_mut().unwrap().count = 1;
        assert!(
            one.detail().ends_with(" · 1 format needs LibreOffice"),
            "{}",
            one.detail()
        );
    }

    #[test]
    fn files_are_added_with_their_conversions() {
        let (dir, kiln) = setup();
        let png = dir.path().join("photo.png");
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.resize(1500, 0);
        fs::write(&png, bytes).unwrap();
        let mut list = FileList::default();
        let r = list.add(&kiln, &Availability::all(), [png.clone()]);
        assert_eq!(
            r,
            Added {
                added: 1,
                duplicates: 0,
                skipped: 0
            }
        );
        let e = &list.entries()[0];
        assert_eq!(e.name, "photo.png");
        assert_eq!(e.code, "PNG");
        assert_eq!(
            e.targets,
            [
                "AVIF", "BMP", "GIF", "ICO", "JPG", "JXL", "PDF", "PPM", "QOI", "TGA", "TIFF",
                "WEBP"
            ]
        );
        // Task 18: its type and size until its facts are in.
        assert_eq!(e.detail(), "PNG · 1.5 KB");
    }

    #[test]
    fn a_file_is_listed_with_the_time_it_was_last_changed() {
        let (dir, kiln) = setup();
        let a = dir.path().join("a.png");
        fs::write(&a, b"x").unwrap();
        let changed = fs::metadata(&a).unwrap().modified().unwrap();
        let mut list = FileList::default();
        list.add(&kiln, &Availability::all(), [a.clone()]);
        assert_eq!(list.entries()[0].modified, Some(changed));

        // An undone Remove keeps the time.
        let back = list.remove(0).unwrap().restored();
        assert_eq!(back.modified, Some(changed));
    }

    #[test]
    fn a_preview_is_kept_only_for_a_listed_file_as_new_as_its_row() {
        let t1 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let t2 = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000);
        let t3 = SystemTime::UNIX_EPOCH + Duration::from_secs(3_000);
        let mut list = FileList::default();
        list.insert(0, row("a.png"));
        list.insert(
            1,
            Entry {
                modified: Some(t2),
                ..row("b.png")
            },
        );
        let b = PathBuf::from("photos").join("b.png");

        // No longer listed: dropped.
        let gone = PathBuf::from("photos").join("gone.png");
        assert_eq!(list.preview_arrived(&gone, Some(t2)), None);
        // Made before the file the row knows: dropped, and the row keeps
        // its time.
        assert_eq!(list.preview_arrived(&b, Some(t1)), None);
        assert_eq!(list.entries()[1].modified, Some(t2));
        // As new as the row: kept for it.
        assert_eq!(list.preview_arrived(&b, Some(t2)), Some(1));
        assert_eq!(list.entries()[1].modified, Some(t2));
        // Newer: the file changed since it was listed, and the row takes
        // the new time.
        assert_eq!(list.preview_arrived(&b, Some(t3)), Some(1));
        assert_eq!(list.entries()[1].modified, Some(t3));
        // The other row is untouched by any of it.
        assert_eq!(list.entries()[0].modified, None);
    }

    #[test]
    fn an_unknown_time_counts_as_older_than_any_known_one() {
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let a = PathBuf::from("photos").join("a.png");
        let b = PathBuf::from("photos").join("b.png");
        let mut list = FileList::default();
        list.insert(0, row("a.png"));
        list.insert(
            1,
            Entry {
                modified: Some(t),
                ..row("b.png")
            },
        );

        // A row that couldn't tell its file's time learns it from a
        // preview that could.
        assert_eq!(list.preview_arrived(&a, Some(t)), Some(0));
        assert_eq!(list.entries()[0].modified, Some(t));
        // A preview with no time is no use to a row that has one: the
        // window would never look it up. Dropped, and the row keeps its
        // time.
        assert_eq!(list.preview_arrived(&b, None), None);
        assert_eq!(list.entries()[1].modified, Some(t));
        // Unknown against unknown is as new.
        list.entries_mut()[0].modified = None;
        assert_eq!(list.preview_arrived(&a, None), Some(0));
        assert_eq!(list.entries()[0].modified, None);
    }

    #[test]
    fn duplicates_and_folders_are_skipped() {
        let (dir, kiln) = setup();
        let a = dir.path().join("a.mp3");
        fs::write(&a, b"x").unwrap();
        let mut list = FileList::default();
        list.add(&kiln, &Availability::all(), [a.clone()]);
        let r = list.add(
            &kiln,
            &Availability::all(),
            [a, dir.path().to_path_buf(), dir.path().join("missing.wav")],
        );
        assert_eq!(
            r,
            Added {
                added: 0,
                duplicates: 1,
                skipped: 2
            }
        );
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn unknown_types_are_listed_but_offer_nothing() {
        let (dir, kiln) = setup();
        let f = dir.path().join("notes.xyz");
        fs::write(&f, b"x").unwrap();
        let mut list = FileList::default();
        list.add(&kiln, &Availability::all(), [f]);
        assert!(list.entries()[0].targets.is_empty());
        assert!(list.entries()[0]
            .detail()
            .starts_with("Unknown type (.XYZ) · "));
        assert_eq!(list.entries()[0].link(false), None);
    }

    /// A list of `n` rows that aren't on disk: "0.png", "1.png"…
    fn list_of(n: usize) -> FileList {
        FileList {
            entries: (0..n).map(|i| row(&format!("{i}.png"))).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn a_click_selects_only_that_row() {
        let mut list = list_of(4);
        assert_eq!(list.selection(), Vec::<usize>::new());
        assert_eq!(list.selected(), None);
        list.click(1);
        list.click(2);
        assert_eq!(list.selection(), [2]);
        assert_eq!(list.selected(), Some(2));
        assert_eq!(list.focus(), Some(2));
        assert!(list.is_selected(2) && !list.is_selected(1));
        // Past the end is no row.
        list.click(9);
        assert_eq!(list.selection(), [2]);
    }

    #[test]
    fn ctrl_click_toggles_a_row() {
        let mut list = list_of(4);
        list.click(0);
        list.ctrl_click(2);
        assert_eq!(list.selection(), [0, 2]);
        assert_eq!(list.focus(), Some(2));
        list.ctrl_click(0);
        assert_eq!(list.selection(), [2]);
        // The focus follows the click even when it unselects.
        assert_eq!(list.focus(), Some(0));
        assert_eq!(list.selected(), None);
        list.ctrl_click(2);
        assert!(list.selection().is_empty());
        list.ctrl_click(7);
        assert!(list.selection().is_empty());
    }

    #[test]
    fn shift_click_selects_from_the_anchor() {
        let mut list = list_of(6);
        list.click(2);
        list.shift_click(4);
        assert_eq!(list.selection(), [2, 3, 4]);
        assert_eq!(list.focus(), Some(4));
        // The anchor stays put, and the range replaces the last one.
        list.shift_click(0);
        assert_eq!(list.selection(), [0, 1, 2]);
        // A Ctrl-click moves the anchor.
        list.ctrl_click(5);
        list.shift_click(3);
        assert_eq!(list.selection(), [3, 4, 5]);
        // With no anchor yet, it's a plain click.
        let mut fresh = list_of(3);
        fresh.shift_click(1);
        assert_eq!(fresh.selection(), [1]);
        fresh.shift_click(9);
        assert_eq!(fresh.selection(), [1]);
    }

    #[test]
    fn select_all_takes_every_row() {
        let mut list = list_of(3);
        list.select_all();
        assert_eq!(list.selection(), [0, 1, 2]);
        assert_eq!(list.focus(), Some(0));
        list.click(1);
        list.select_all();
        assert_eq!(list.selection(), [0, 1, 2]);
        assert_eq!(list.selected(), Some(1));
        let mut empty = FileList::default();
        empty.select_all();
        assert!(empty.selection().is_empty());
        assert_eq!(empty.focus(), None);
    }

    #[test]
    fn arrows_move_and_select_one_row() {
        let mut list = list_of(3);
        list.step(1);
        assert_eq!(list.selected(), Some(0));
        list.step(5);
        assert_eq!(list.selection(), [2]);
        list.select_all();
        list.step(-1);
        assert_eq!(list.selection(), [1]);
        let mut fresh = list_of(3);
        fresh.step(-1);
        assert_eq!(fresh.selection(), [2]);
        let mut empty = FileList::default();
        empty.step(1);
        assert_eq!(empty.selected(), None);
    }

    #[test]
    fn shift_arrows_extend_from_the_anchor() {
        let mut list = list_of(5);
        list.click(2);
        list.extend(1);
        list.extend(1);
        assert_eq!(list.selection(), [2, 3, 4]);
        assert_eq!(list.focus(), Some(4));
        // Clamped at the end.
        list.extend(1);
        assert_eq!(list.selection(), [2, 3, 4]);
        // Back past the anchor.
        list.extend(-3);
        assert_eq!(list.selection(), [1, 2]);
        assert_eq!(list.focus(), Some(1));
        // A plain arrow afterwards selects only where it lands.
        list.step(1);
        assert_eq!(list.selection(), [2]);
        // From nothing, it starts at the first row.
        let mut fresh = list_of(3);
        fresh.extend(1);
        assert_eq!(fresh.selection(), [0]);
        fresh.extend(1);
        assert_eq!(fresh.selection(), [0, 1]);
        let mut empty = FileList::default();
        empty.extend(1);
        assert_eq!(empty.focus(), None);
    }

    #[test]
    fn the_focus_ring_shows_after_keys_and_hides_after_clicks() {
        let mut list = list_of(5);
        assert_eq!(list.visible_focus(), None, "nothing before any input");
        // A click focuses the row but shows no ring.
        list.pointer_used();
        list.click(2);
        assert_eq!(list.focus(), Some(2));
        assert_eq!(list.visible_focus(), None);
        // An arrow shows it.
        list.keyboard_used();
        list.step(1);
        assert_eq!(list.visible_focus(), Some(3));
        // Another click hides it again.
        list.pointer_used();
        list.ctrl_click(1);
        assert_eq!(list.visible_focus(), None);
        // Escape after a click shows it on the kept focus, with nothing
        // selected.
        list.keyboard_used();
        list.clear_selection();
        assert!(list.selection().is_empty());
        assert_eq!(list.visible_focus(), Some(1));
        // And Escape again keeps it showing.
        list.keyboard_used();
        list.clear_selection();
        assert_eq!(list.visible_focus(), Some(1));
    }

    #[test]
    fn rows_added_by_a_key_show_the_focus_ring() {
        // Ctrl+O or Ctrl+V: the first new row takes the focus, ring and all,
        // so the list shows where the new files start and Enter converts.
        let mut list = list_of(5);
        list.pointer_used();
        list.click(0);
        list.added(3, true);
        // Every new row is selected (UX-05, Task 18), the first focused.
        assert_eq!(list.selection(), [3, 4]);
        assert_eq!(list.visible_focus(), Some(3));
        // A drop or the Add files button: the focus moves, the ring hides.
        list.added(4, false);
        assert_eq!(list.focus(), Some(4));
        assert_eq!(list.visible_focus(), None);
        // Nothing listed, nothing to focus.
        let mut empty = FileList::default();
        empty.added(0, true);
        assert_eq!(empty.focus(), None);
    }

    #[test]
    fn escape_clears_the_selection_but_keeps_the_focus() {
        let mut list = list_of(4);
        list.click(1);
        list.shift_click(3);
        list.clear_selection();
        assert!(list.selection().is_empty());
        assert_eq!(list.selected(), None);
        assert_eq!(list.focus(), Some(3));
        // An arrow carries on from the focus.
        list.step(-1);
        assert_eq!(list.selection(), [2]);
    }

    #[test]
    fn right_click_keeps_a_selection_it_lands_in() {
        let mut list = list_of(5);
        list.click(1);
        list.shift_click(3);
        assert_eq!(list.context_target(2), [1, 2, 3]);
        assert_eq!(list.selection(), [1, 2, 3]);
        assert_eq!(list.focus(), Some(2));
        // Outside it, only that row.
        assert_eq!(list.context_target(4), [4]);
        assert_eq!(list.selection(), [4]);
        assert_eq!(list.context_target(9), [4]);
    }

    #[test]
    fn removing_a_row_shifts_the_selection() {
        let mut list = list_of(6);
        list.click(1);
        list.ctrl_click(3);
        list.ctrl_click(5);
        // 0 1 2 3 4 5, with 1 3 5 selected, the anchor and focus on 5.
        assert_eq!(list.remove(2).map(|e| e.name), Some("2.png".to_owned()));
        assert_eq!(list.selection(), [1, 2, 4]);
        assert_eq!(list.focus(), Some(4));
        assert_eq!(list.entries()[2].name, "3.png");
        // A Shift-click still ranges from the anchor, now at 4.
        list.shift_click(3);
        assert_eq!(list.selection(), [3, 4]);
        assert!(list.remove(9).is_none());
        assert_eq!(list.len(), 5);
    }

    #[test]
    fn removing_the_anchor_row_moves_the_anchor_to_the_next() {
        let mut list = list_of(5);
        list.click(1);
        list.shift_click(3);
        list.remove(1);
        // Rows 0 2 3 4 now; the old 2 and 3 stay selected, at 1 and 2.
        assert_eq!(list.selection(), [1, 2]);
        assert_eq!(list.anchor, Some(1));
        assert_eq!(list.entries()[1].name, "2.png");
        list.shift_click(2);
        assert_eq!(list.selection(), [1, 2]);
        list.shift_click(3);
        assert_eq!(list.selection(), [1, 2, 3]);
        // With four rows, the old last row is past the end: nothing changes.
        let mut four = list_of(4);
        four.click(1);
        four.shift_click(3);
        four.remove(1);
        four.shift_click(3);
        assert_eq!(four.selection(), [1, 2]);
        assert_eq!(four.anchor, Some(1));
    }

    #[test]
    fn removing_the_selection_leaves_the_next_row_selected() {
        let mut list = list_of(5);
        list.click(1);
        list.shift_click(2);
        // Removed as Remove does: one row at a time, by file.
        list.remove(1);
        assert_eq!(list.selection(), [1]);
        list.remove(1);
        assert_eq!(list.entries()[1].name, "3.png");
        assert_eq!(list.selection(), [1]);
        assert_eq!(list.focus(), Some(1));
        // Removing a row outside the selection leaves it be.
        list.remove(0);
        assert_eq!(list.selection(), [0]);
        assert_eq!(list.entries()[0].name, "3.png");
        // The last row going takes the focus to the one before.
        list.click(1);
        list.remove(1);
        assert_eq!(list.selection(), [0]);
        list.remove(0);
        assert!(list.is_empty());
        assert_eq!(list.selected(), None);
        assert_eq!(list.focus(), None);
        list.step(-1);
        assert_eq!(list.selected(), None);
    }

    #[test]
    fn clearing_forgets_the_selection() {
        let mut list = list_of(3);
        list.select_all();
        list.clear();
        assert!(list.selection().is_empty());
        assert_eq!(list.focus(), None);
        list.shift_click(0);
        assert!(list.selection().is_empty());
    }

    #[test]
    fn several_selected_rows_say_how_many() {
        assert_eq!(selected_line(0), None);
        assert_eq!(selected_line(1), None);
        assert_eq!(selected_line(3).as_deref(), Some("3 selected"));
    }

    #[test]
    fn status_copy_is_plain() {
        assert_eq!(status_line(3, 0, 0, 0), "3 files.");
        assert_eq!(status_line(1, 1, 0, 0), "Added 1 file.");
        assert_eq!(
            status_line(4, 2, 1, 1),
            "Added 2 files. 1 file was already in the list. Skipped 1 item that isn't a file."
        );
        assert_eq!(
            status_line(0, 0, 2, 2),
            "2 files were already in the list. Skipped 2 items that aren't files."
        );
        for s in [status_line(0, 0, 0, 0), status_line(9, 9, 9, 9)] {
            assert!(!s.contains('!'));
        }
    }

    #[test]
    fn rows_follow_their_job() {
        let (dir, kiln) = setup();
        let f = dir.path().join("a.png");
        fs::write(&f, b"x").unwrap();
        let mut list = FileList::default();
        list.add(&kiln, &Availability::all(), [f]);
        let e = &mut list.entries_mut()[0];
        let jpg = Action::Convert("jpg".into());
        e.job = JobState::Running {
            action: jpg.clone(),
            fraction: Some(0.5),
            started: None,
        };
        assert_eq!(e.line(), ("Converting to JPG".to_owned(), Tone::Muted));
        e.job = JobState::Done {
            action: jpg,
            outputs: vec![dir.path().join("a.jpg")],
            saved: None,
        };
        assert_eq!(e.line(), ("Saving…".to_owned(), Tone::Muted));
        e.job = JobState::Done {
            action: Action::Tool(Tool::CutInHalf, Kind::Audio),
            outputs: vec!["a part 1.wav".into(), "a part 2.wav".into()],
            saved: None,
        };
        assert_eq!(e.line().0, "Saving…");
        e.job = JobState::Failed {
            message:
                "Needs Pandoc, which isn't installed.\nSettings > Helper apps says where to get it."
                    .into(),
            details: String::new(),
            engine: None,
            store_product: None,
        };
        // One line, whole: the row cuts it short only where it must.
        assert_eq!(
            e.line(),
            (
                "Needs Pandoc, which isn't installed. Settings > Helper apps says where to get it."
                    .to_owned(),
                Tone::Crack
            )
        );
    }

    #[test]
    fn batch_summaries_are_calm_and_specific() {
        let jpg = Action::Convert("jpg".into());
        assert_eq!(batch_summary(&jpg, 3, 3, 0, 0), "Converted 3 files to JPG.");
        assert_eq!(batch_summary(&jpg, 1, 1, 0, 0), "Converted 1 file to JPG.");
        assert_eq!(
            batch_summary(&jpg, 3, 2, 1, 0),
            "Converted 2 of 3 files to JPG. 1 didn't work. The row says why."
        );
        assert_eq!(
            batch_summary(&jpg, 1, 0, 1, 0),
            "Couldn't convert the file to JPG. The row says why."
        );
        assert_eq!(
            batch_summary(&jpg, 3, 0, 2, 1),
            "Couldn't convert 2 files to JPG. Each row says why. 1 cancelled."
        );
        assert_eq!(batch_summary(&jpg, 2, 0, 0, 2), "Cancelled.");
        let trim = Action::Tool(Tool::RemoveQuietEnds, Kind::Video);
        assert_eq!(
            batch_summary(&trim, 2, 2, 0, 0),
            "Removed black at the ends of 2 files."
        );
        assert_eq!(
            batch_summary(&trim, 1, 0, 1, 0),
            "Couldn't remove black at the ends of the file. The row says why."
        );
        assert_eq!(
            batch_summary(&trim, 2, 1, 0, 1),
            "Removed black at the ends of 1 of 2 files. 1 cancelled."
        );
        let smaller = Action::Tool(Tool::Smaller, Kind::Image);
        assert_eq!(batch_summary(&smaller, 1, 1, 0, 0), "Made 1 file smaller.");
        assert_eq!(
            batch_summary(&smaller, 3, 0, 3, 0),
            "Couldn't make 3 files smaller. Each row says why."
        );
        let merge = Action::Tool(Tool::Merge, Kind::Document);
        assert_eq!(
            batch_summary(&merge, 3, 1, 0, 0),
            "Merged 3 files into one."
        );
        assert_eq!(
            batch_summary(&merge, 3, 0, 1, 0),
            "Couldn't merge the files. The row says why."
        );
        for s in [
            batch_summary(&jpg, 9, 0, 9, 0),
            batch_summary(&merge, 2, 1, 0, 0),
        ] {
            assert!(!s.contains('!'));
        }
    }

    #[test]
    fn rows_say_what_a_tool_is_doing_and_did() {
        let words = |t: Tool, k: Kind| {
            let a = Action::Tool(t, k);
            (doing(&a), done(&a))
        };
        let w = |a: &str, b: &str| (a.to_owned(), b.to_owned());
        assert_eq!(
            words(Tool::Smaller, Kind::Video),
            w("Making smaller", "Made smaller")
        );
        assert_eq!(words(Tool::Resize, Kind::Image), w("Resizing", "Resized"));
        assert_eq!(
            words(Tool::RemoveLocation, Kind::Image),
            w("Removing location data", "Removed location data")
        );
        assert_eq!(
            words(Tool::RemoveEdges, Kind::Image),
            w("Removing borders", "Removed borders")
        );
        assert_eq!(
            words(Tool::RemoveEdges, Kind::Video),
            w("Removing black bars", "Removed black bars")
        );
        assert_eq!(
            words(Tool::RemoveQuietEnds, Kind::Audio),
            w(
                "Removing silence at the ends",
                "Removed silence at the ends"
            )
        );
        assert_eq!(
            words(Tool::RemoveQuietEnds, Kind::Video),
            w("Removing black at the ends", "Removed black at the ends")
        );
        assert_eq!(
            words(Tool::CutInHalf, Kind::Audio),
            w("Cutting in half", "Cut in half")
        );
        assert_eq!(
            words(Tool::SaveEachPage, Kind::Document),
            w("Saving each page as a PDF", "Saved each page as a PDF")
        );
        assert_eq!(words(Tool::Merge, Kind::Audio), w("Merging", "Merged"));
    }

    fn facts(ext: &str, kind: Kind) -> Facts {
        Facts {
            ext: ext.to_owned(),
            kind,
            bytes: 3_100_000,
            picture: None,
            media: None,
            pdf: None,
            bars: None,
            estimates: Default::default(),
            compressed: None,
            audio_problem: None,
            silent: false,
        }
    }

    #[test]
    fn a_file_already_in_the_format_is_left_alone() {
        let kiln = Kiln::new();
        let (jpg, png, jpeg) = (
            PathBuf::from(r"C:\in\a.jpg"),
            PathBuf::from(r"C:\in\b.png"),
            PathBuf::from(r"C:\in\c.JPEG"),
        );
        let all = vec![jpg.clone(), png.clone(), jpeg.clone()];
        let (convert, already) = already_in(&kiln, &all, "jpg");
        assert_eq!(convert, [png]);
        assert_eq!(already, [jpg, jpeg]);
        assert_eq!(already_line("jpg"), "Already JPG");
        assert_eq!(already_line("webp"), "Already WEBP");
        // None already: all of them go.
        let (convert, already) = already_in(&kiln, &all, "webp");
        assert_eq!((convert, already.len()), (all, 0));
    }

    #[test]
    fn a_row_knows_its_subtitle_track_once_its_facts_are_in() {
        let mut files = FileList::default();
        let mut e = row("clip.mkv");
        e.path = PathBuf::from(r"C:\in\clip.mkv");
        files.insert(0, e);
        let path = PathBuf::from(r"C:\in\clip.mkv");
        // No facts yet: none is known.
        assert!(!files.has_subtitles(&path));
        let mut f = facts("mkv", Kind::Video);
        f.media = Some(kiln_core::MediaInfo::default());
        files.entries_mut()[0].facts = Some(f.clone());
        assert!(!files.has_subtitles(&path));
        f.media.as_mut().unwrap().subtitle_tracks = 2;
        files.entries_mut()[0].facts = Some(f);
        assert!(files.has_subtitles(&path));
        // A file not in the list has none.
        assert!(!files.has_subtitles(Path::new(r"C:\in\other.mkv")));
    }

    fn picture(mut f: Facts, width: u32, height: u32, look: Look) -> Facts {
        f.picture = Some(kiln_core::Picture {
            width,
            height,
            alpha: false,
            animated: false,
            look,
        });
        f
    }

    fn lasting(mut f: Facts, seconds: f64) -> Facts {
        f.media = Some(kiln_core::MediaInfo {
            duration: Some(seconds),
            width: None,
            height: None,
            fps: None,
            has_audio: true,
            ..Default::default()
        });
        f
    }

    fn pages(pages: u32, has_text: Option<bool>) -> Facts {
        let mut f = facts("pdf", Kind::Document);
        f.pdf = Some(kiln_core::PdfFacts { pages, has_text });
        f
    }

    fn photo() -> Facts {
        picture(facts("png", Kind::Image), 4032, 3024, Look::Photo)
    }

    /// A row in "photos", 3.1 MB, that converts to 12 formats.
    fn row(name: &str) -> Entry {
        Entry {
            path: PathBuf::from("photos").join(name),
            name: name.to_owned(),
            folder: "photos".to_owned(),
            code: "PNG".to_owned(),
            size: 3_100_000,
            modified: None,
            targets: (0..12).map(|i| format!("F{i}")).collect(),
            job: JobState::Idle,
            facts: None,
            suggestion: None,
            facts_at: None,
            last: None,
            from_drag: false,
            check: FileCheck::Ok,
            missing: None,
            save_failed: false,
            job_note: None,
            result_bytes: None,
            waits_on: None,
            ffmpeg_found: true,
        }
    }

    #[test]
    fn describe_says_what_a_file_is() {
        let video = lasting(
            picture(facts("mp4", Kind::Video), 1280, 720, Look::Photo),
            90.0,
        );
        let said: Vec<Option<String>> = [
            photo(),
            picture(facts("png", Kind::Image), 1200, 800, Look::Graphic),
            video,
            lasting(facts("mp3", Kind::Audio), 222.0),
            pages(12, Some(true)),
            pages(1, None),
            pages(12, Some(false)),
        ]
        .iter()
        .map(describe)
        .collect();
        assert_eq!(
            said,
            [
                "4032 × 3024 photo",
                "1200 × 800 graphic",
                "1280 × 720 video · 1:30",
                "sound, 3:42",
                "12 pages",
                "1 page",
                "12 pages, no text",
            ]
            .map(|s| Some(s.to_owned()))
        );
        assert_eq!(describe(&facts("txt", Kind::Document)), None);
    }

    /// An audio file's facts as Kiln's own engine reads them.
    fn heard(
        ext: &str,
        codec: &str,
        kbps: Option<u32>,
        bits: Option<u16>,
        rate: Option<u32>,
        secs: Option<f64>,
    ) -> Facts {
        let mut f = facts(ext, Kind::Audio);
        f.media = Some(kiln_core::MediaInfo {
            duration: secs,
            has_audio: true,
            codec: Some(codec.to_owned()),
            kbps,
            channels: Some(2),
            rate,
            bits,
            ..Default::default()
        });
        f
    }

    #[test]
    fn audio_row_facts() {
        let said = |f: Facts| describe(&f).unwrap();
        let mp3 = heard("mp3", "MP3", Some(128), None, Some(44_100), Some(221.0));
        assert_eq!(said(mp3), "MP3 · 128 kbps · 3:41");
        let flac = heard(
            "flac",
            "FLAC",
            Some(2_900),
            Some(24),
            Some(96_000),
            Some(221.0),
        );
        assert_eq!(said(flac), "FLAC · 24-bit · 96 kHz · 3:41");
        let cd = heard("wav", "PCM", None, Some(16), Some(44_100), Some(3_725.0));
        assert_eq!(said(cd), "PCM · 16-bit · 44.1 kHz · 1:02:05");
        // kHz reads as on Advanced's Sample rate.
        let low = heard("wav", "PCM", None, Some(16), Some(22_050), Some(61.0));
        assert_eq!(said(low), "PCM · 16-bit · 22.05 kHz · 1:01");
        // What isn't known is left out, never shown as zero.
        let vbr = heard("ogg", "Vorbis", None, None, Some(48_000), Some(61.0));
        assert_eq!(said(vbr), "Vorbis · 1:01");
        let cut = heard("mp3", "MP3", Some(192), None, None, None);
        assert_eq!(said(cut), "MP3 · 192 kbps");
        // FFprobe's facts alone, with no codec: as before.
        assert_eq!(
            said(lasting(facts("mp3", Kind::Audio), 222.0)),
            "sound, 3:42"
        );
        // A screen reader hears the units whole.
        let mut e = row("song.flac");
        e.facts = Some(heard(
            "flac",
            "FLAC",
            None,
            Some(24),
            Some(44_100),
            Some(221.0),
        ));
        // Task 18: a row says "Sound" and its length (mock screen 2).
        assert!(
            e.spoken_detail().starts_with("Sound. 3:41. "),
            "{}",
            e.spoken_detail()
        );
    }

    #[test]
    fn the_hevc_extension_row_keeps_convert() {
        use kiln_media::video::{Hdr, VideoCodec};
        // An HDR HEVC MKV on a PC with neither Microsoft's HEVC Video
        // Extensions nor FFmpeg.
        let mut e = row("clip.mkv");
        e.code = "MKV".to_owned();
        e.targets = ["MP4", "MOV", "M4V", "MKV", "M4A"]
            .map(String::from)
            .to_vec();
        e.missing = Some(crate::model::Missing::new("ffmpeg", 9));
        e.ffmpeg_found = false;
        let mut f = facts("mkv", Kind::Video);
        f.media = Some(kiln_core::MediaInfo {
            duration: Some(42.0),
            width: Some(1920),
            height: Some(1080),
            video_codec: Some(VideoCodec::Hevc),
            hdr: Some(Hdr::Pq),
            needs_extension: Some(("HEVC", "9N4WGH0Z6VHQ")),
            ..Default::default()
        });
        e.facts = Some(f);
        // Before any conversion. Every format copies the picture, so
        // only a changed picture needs the extension, and the row keeps
        // Convert; the inspector has Get it.
        let sentence = "Needs Microsoft's HEVC Video Extensions to re-encode HEVC video.";
        assert_eq!(e.line(), (sentence.to_owned(), Tone::Muted));
        assert_eq!(e.link(false), Some("Convert"));
        assert_eq!(e.store_product(), Some("9N4WGH0Z6VHQ"));
        assert!(e
            .spoken_detail()
            .starts_with(&sentence[..sentence.len() - 1]));
        assert_eq!(
            store_page("9N4WGH0Z6VHQ").as_deref(),
            Some("ms-windows-store://pdp/?ProductId=9N4WGH0Z6VHQ")
        );
        assert!(store_page("9MVZQVXJBQ9V").is_some() && store_page("9N4D0MSMP0PT").is_some());
        // A HEIC names the HEIF Image Extensions when Windows can't open it.
        assert_eq!(
            store_page("9PMMSR1CGPWG").as_deref(),
            Some("ms-windows-store://pdp/?ProductId=9PMMSR1CGPWG")
        );
        assert_eq!(store_page("9NBLGGH4NNS1"), None);
        // A turned picture can't be copied into MKV: that format needs it.
        let media = |e: &mut Entry| e.facts.as_mut().unwrap().media.as_mut().unwrap().rotation = 90;
        media(&mut e);
        assert_eq!(
            e.line().0,
            "Needs Microsoft's HEVC Video Extensions to convert HEVC video to MKV."
        );
        assert_eq!(e.link(false), Some("Convert"));
        // With nothing to convert to and no helper app to get, Get it is
        // all there is.
        let targets = std::mem::take(&mut e.targets);
        let missing = e.missing.take();
        assert_eq!(e.link(false), Some("Get HEVC extension"));
        (e.targets, e.missing) = (targets, missing);
        // With FFmpeg the file converts through it: the row says
        // what it is, with its codec and HDR. It is FFmpeg that counts,
        // not which helper the formats name first.
        e.missing = Some(crate::model::Missing::new("pandoc", 1));
        e.ffmpeg_found = true;
        assert_eq!(e.store_product(), None);
        // Task 18: the row's facts are the mock's (size, length); the
        // codec is in Show details' world, not the row's.
        assert_eq!(
            e.line().0,
            "1920 × 1080 video · 0:42 · 3.1 MB · 1 format needs Pandoc"
        );
        assert_eq!(e.link(false), Some("Convert"));
        // A job that failed for want of an extension offers it too.
        e.job = JobState::Failed {
            message: "Needs Microsoft's HEVC Video Extensions to write HEVC video.".into(),
            details: String::new(),
            engine: None,
            store_product: Some("9N4WGH0Z6VHQ"),
        };
        assert_eq!(e.link(false), Some("Get HEVC extension"));
        assert_eq!(e.store_product(), Some("9N4WGH0Z6VHQ"));
    }

    #[test]
    fn the_av1_extension_row_names_the_formats_that_need_it() {
        use kiln_media::video::VideoCodec;
        // AV1 copies into MKV only: MP4, MOV and M4V decode it.
        let mut e = row("clip.mp4");
        e.code = "MP4".to_owned();
        e.targets = ["MKV", "MP4", "MOV", "M4V", "MP3"]
            .map(String::from)
            .to_vec();
        e.ffmpeg_found = false;
        let mut f = facts("mp4", Kind::Video);
        f.media = Some(kiln_core::MediaInfo {
            duration: Some(12.0),
            width: Some(1280),
            height: Some(720),
            fps: Some(30.0),
            video_codec: Some(VideoCodec::Av1),
            needs_extension: Some(("AV1", "9MVZQVXJBQ9V")),
            ..Default::default()
        });
        e.facts = Some(f);
        assert_eq!(
            e.extension_line().as_deref(),
            Some("Needs Microsoft's AV1 Video Extensions to convert AV1 video to MP4, MOV or M4V.")
        );
        assert_eq!(e.link(false), Some("Convert"));
        // Only MP4 and MOV offered.
        e.targets = ["MKV", "MP4", "MOV"].map(String::from).to_vec();
        assert_eq!(
            e.extension_line().as_deref(),
            Some("Needs Microsoft's AV1 Video Extensions to convert AV1 video to MP4 or MOV.")
        );
    }

    #[test]
    fn a_video_whose_sound_kiln_cant_read_says_so_and_keeps_convert() {
        use kiln_media::video::VideoCodec;
        // AC-3 in an MKV, which Kiln names but can't decode, with no
        // FFmpeg found: the row says so before any conversion, as for DTS.
        let mut e = row("film.mkv");
        e.code = "MKV".to_owned();
        e.targets = ["MP4", "MOV"].map(String::from).to_vec();
        e.ffmpeg_found = false;
        let mut f = facts("mkv", Kind::Video);
        f.media = Some(kiln_core::MediaInfo {
            duration: Some(42.0),
            width: Some(1280),
            height: Some(720),
            video_codec: Some(VideoCodec::H264),
            has_audio: true,
            codec: Some("AC3".into()),
            ..Default::default()
        });
        let sentence = "This file's audio is AC3, which Kiln can't read yet.";
        f.audio_problem = Some(sentence.into());
        e.facts = Some(f);
        assert_eq!(e.line(), (sentence.to_owned(), Tone::Muted));
        assert!(e
            .spoken_detail()
            .starts_with(&sentence[..sentence.len() - 1]));
        assert_eq!(e.link(false), Some("Convert"));
        // With FFmpeg it converts through it: nothing to say.
        e.ffmpeg_found = true;
        assert_eq!(e.sound_line(), None);
        assert!(e.line().0.starts_with("1280 × 720 video"), "{}", e.line().0);
        // Sound found damaged isn't a codec Kiln can't read.
        e.ffmpeg_found = false;
        e.facts.as_mut().unwrap().audio_problem =
            Some("This MKV is damaged and can't be read.".into());
        assert_eq!(e.sound_line(), None);
    }

    #[test]
    fn a_finished_row_says_its_note() {
        let mut e = row("clip.mov");
        e.job = JobState::Done {
            action: Action::Convert("mp4".into()),
            outputs: vec![PathBuf::from("clip.mp4")],
            saved: None,
        };
        e.job_note = Some(kiln_core::ON_THE_PROCESSOR.to_owned());
        assert_eq!(e.line().0, "Saving… · Encoded on the processor.");
        // A row nothing has been done to has no job to note.
        e.job = JobState::Idle;
        assert!(!e.line().0.contains("processor"));
    }

    #[test]
    fn a_rows_finished_line_carries_its_drops() {
        let mut e = row("clip.mkv");
        e.job = JobState::Done {
            action: Action::Convert("mp4".into()),
            outputs: vec![PathBuf::from("clip.mp4")],
            saved: None,
        };
        e.job_note = Some(kiln_core::first_of_tracks(2));
        // After the saved line, as a job's own note is.
        assert_eq!(e.line().0, "Saving… · Keeps the first of 2 sound tracks.");
        // Saved next to the original, it still follows.
        e.job = JobState::Done {
            action: Action::Convert("mp4".into()),
            outputs: vec![PathBuf::from("clip.mp4")],
            saved: Some(Saved::Folder(PathBuf::from(&e.folder))),
        };
        assert!(
            e.line()
                .0
                .ends_with(" · Keeps the first of 2 sound tracks."),
            "{}",
            e.line().0
        );
        // A job that failed or was stopped has converted nothing to say of.
        for job in [JobState::Cancelled, JobState::Idle] {
            e.job = job;
            assert!(!e.line().0.contains("sound tracks"), "{}", e.line().0);
        }
    }

    #[test]
    fn a_running_rows_line_carries_its_drops() {
        let mut e = row("clip.mkv");
        e.job = JobState::Running {
            action: Action::Convert("mp4".into()),
            fraction: Some(0.4),
            started: None,
        };
        let bare = e.line().0;
        e.job_note = Some("Keeps the first of 2 sound tracks. · Subtitles aren't kept.".into());
        assert_eq!(
            e.line().0,
            format!("{bare} · Keeps the first of 2 sound tracks. · Subtitles aren't kept.")
        );
    }

    #[test]
    fn a_jobs_note_is_what_it_drops_then_its_own() {
        let drops = [
            kiln_core::first_of_tracks(2),
            kiln_core::NO_SUBTITLES.to_owned(),
        ];
        assert_eq!(job_note(&[], None), None);
        assert_eq!(
            job_note(&[], Some(kiln_core::ON_THE_PROCESSOR)).as_deref(),
            Some("Encoded on the processor.")
        );
        assert_eq!(
            job_note(&drops, None).as_deref(),
            Some("Keeps the first of 2 sound tracks. · Subtitles aren't kept.")
        );
        assert_eq!(
            job_note(&drops[..1], Some(kiln_core::ON_THE_PROCESSOR)).as_deref(),
            Some("Keeps the first of 2 sound tracks. · Encoded on the processor.")
        );
    }

    #[test]
    fn a_drag_notice_adds_its_drops() {
        let drops = [
            kiln_core::first_of_tracks(2),
            kiln_core::NO_SUBTITLES.to_owned(),
        ];
        // The drop's own sentence stands as it was; the drops follow it.
        assert_eq!(
            with_drops("Converted clip.mkv to MP4.", &drops),
            "Converted clip.mkv to MP4. Keeps the first of 2 sound tracks. Subtitles aren't kept."
        );
        assert_eq!(
            with_drops("Saved clip.mp4 next to the original.", &drops[1..]),
            "Saved clip.mp4 next to the original. Subtitles aren't kept."
        );
        assert_eq!(
            with_drops("Converted clip.mkv to MP4.", &[]),
            "Converted clip.mkv to MP4."
        );
        // Several files say only what every one of them is told: another
        // file's sound tracks are no news about this one's.
        let a = vec![
            kiln_core::first_of_tracks(2),
            kiln_core::NO_SUBTITLES.to_owned(),
        ];
        let b = vec![
            kiln_core::first_of_tracks(3),
            kiln_core::NO_SUBTITLES.to_owned(),
        ];
        let c: Vec<String> = vec![];
        assert_eq!(shared_notes(std::slice::from_ref(&a)), a);
        assert_eq!(
            shared_notes(&[a.clone(), b.clone()]),
            [kiln_core::NO_SUBTITLES]
        );
        // Every one: a sentence two of three files carry is not said.
        assert_eq!(
            shared_notes(&[a.clone(), a.clone(), b]),
            [kiln_core::NO_SUBTITLES]
        );
        assert!(shared_notes(&[a, c]).is_empty());
        assert!(shared_notes::<Vec<String>>(&[]).is_empty());
    }

    #[test]
    fn a_target_is_text_only_when_it_is_for_every_file() {
        let kiln = Kiln::new();
        let at = |names: &[&str], to: &str| {
            let paths: Vec<PathBuf> = names.iter().map(PathBuf::from).collect();
            text_only_for(&kiln, &paths, to)
        };
        assert!(at(&["a.pdf"], "docx"));
        assert!(at(&["a.pdf", "b.PDF"], "odt"));
        assert!(at(&["a.pdf"], ".epub"));
        // A file that isn't a PDF becomes an ODT as itself, not as its
        // text: the label is for all the files or none.
        assert!(!at(&["a.pdf", "b.md"], "odt"));
        assert!(!at(&["b.md", "a.pdf"], "odt"));
        assert!(!at(&["b.md"], "odt"));
        // A PDF's pages, its text, and what Kiln doesn't know.
        assert!(!at(&["a.pdf"], "png"));
        assert!(!at(&["a.pdf"], "txt"));
        assert!(!at(&["a.pdf"], "zzz"));
        assert!(!at(&["a.zzz"], "docx"));
        assert!(!at(&[], "docx"));
    }

    #[test]
    fn the_sheet_opens_with_the_notes_the_rows_facts_give() {
        use kiln_core::{first_of_tracks, MediaInfo, KEEPS_TEXT_ONLY, NO_SUBTITLES};
        let kiln = Kiln::new();
        let mut files = FileList::default();
        let (clip, doc, other) = (
            PathBuf::from(r"C:\in\clip.mkv"),
            PathBuf::from(r"C:\in\doc.pdf"),
            PathBuf::from(r"C:\in\other.mkv"),
        );
        let mut rows = vec![];
        for path in [&clip, &doc, &other] {
            let mut e = row(&path.file_name().unwrap().to_string_lossy());
            e.path = path.clone();
            rows.push(e);
        }
        // The facts of the first video carry its tracks, as the inspector
        // read them; the second has none yet.
        let mut clip_facts = facts("mkv", Kind::Video);
        clip_facts.media = Some(MediaInfo {
            sound_tracks: 2,
            subtitle_tracks: 1,
            ..MediaInfo::default()
        });
        rows[0].facts = Some(clip_facts);
        for (i, e) in rows.into_iter().enumerate() {
            files.insert(i, e);
        }
        let seed = |paths: &[&PathBuf], to: &str| {
            let paths: Vec<PathBuf> = paths.iter().map(|p| (*p).clone()).collect();
            notes_from_rows(&kiln, &files, &paths, to)
        };
        assert_eq!(
            seed(&[&clip], "mp4"),
            [first_of_tracks(2), NO_SUBTITLES.to_owned()]
        );
        assert_eq!(seed(&[&clip], "mp3"), [first_of_tracks(2)]);
        // A PDF needs no facts.
        assert_eq!(seed(&[&doc], "docx"), [KEEPS_TEXT_ONLY]);
        // A file whose facts haven't arrived says nothing yet, and so
        // nothing is said of the two: only what both are told.
        assert!(seed(&[&clip, &other], "mp4").is_empty());
        assert!(seed(&[&other], "mp4").is_empty());
        // A file the list doesn't hold has none either.
        assert!(notes_from_rows(&kiln, &files, &[PathBuf::from("gone.mkv")], "mp4").is_empty());
    }

    #[test]
    fn the_sheets_notes_are_the_notes_the_job_sends() {
        use kiln_core::{first_of_tracks, MediaInfo, KEEPS_TEXT_ONLY, NO_SUBTITLES};
        let kiln = Kiln::new();
        let one = |name: &str| vec![PathBuf::from(name)];
        let tracks = |sound_tracks, subtitle_tracks, later_sound| MediaInfo {
            sound_tracks,
            subtitle_tracks,
            later_sound,
            ..MediaInfo::default()
        };
        // A PDF's text laid out as a document needs nothing read.
        assert_eq!(
            sheet_notes(&kiln, &one("a.pdf"), "docx", None),
            [KEEPS_TEXT_ONLY]
        );
        assert!(sheet_notes(&kiln, &one("a.pdf"), "png", None).is_empty());
        // A video's tracks once its file is read, and nothing before.
        let read = [tracks(2, 1, false)];
        assert!(sheet_notes(&kiln, &one("a.avi"), "mp4", None).is_empty());
        assert_eq!(
            sheet_notes(&kiln, &one("a.avi"), "mp4", Some(&read)),
            [first_of_tracks(2), NO_SUBTITLES.to_owned()]
        );
        assert_eq!(
            sheet_notes(&kiln, &one("a.avi"), "mp3", Some(&read)),
            [first_of_tracks(2)]
        );
        assert!(sheet_notes(&kiln, &one("a.avi"), "gif", Some(&read)).is_empty());
        // Where Kiln's own engines read a later track, as the job does not
        // say "the first".
        let skipped = [tracks(2, 1, true)];
        assert_eq!(
            sheet_notes(&kiln, &one("a.mkv"), "mp4", Some(&skipped)),
            [NO_SUBTITLES]
        );
        assert!(sheet_notes(&kiln, &one("a.mkv"), "mp3", Some(&skipped)).is_empty());
        assert_eq!(
            sheet_notes(&kiln, &one("a.mkv"), "webm", Some(&skipped)),
            [first_of_tracks(2), NO_SUBTITLES.to_owned()]
        );
        // Several files: what each of them is told. Files not all read
        // (more than are looked at) have only what needs no reading.
        let both = vec![PathBuf::from("a.mkv"), PathBuf::from("b.mkv")];
        let reads = [tracks(2, 1, false), tracks(3, 1, false)];
        assert_eq!(
            sheet_notes(&kiln, &both, "webm", Some(&reads)),
            [NO_SUBTITLES]
        );
        assert!(sheet_notes(&kiln, &both, "webm", Some(&reads[..1])).is_empty());
        let pdfs = vec![PathBuf::from("a.pdf"), PathBuf::from("b.PDF")];
        assert_eq!(
            sheet_notes(&kiln, &pdfs, "epub", Some(&[])),
            [KEEPS_TEXT_ONLY]
        );
        assert!(sheet_notes(&kiln, &[], "mp4", None).is_empty());
        // A PDF with a Markdown file: the Markdown file keeps all it has.
        let mixed = vec![PathBuf::from("a.pdf"), PathBuf::from("b.md")];
        assert!(sheet_notes(&kiln, &mixed, "odt", None).is_empty());
    }

    #[test]
    fn media_pack_link() {
        assert_eq!(get_label("audio-mf"), "Get Media Feature Pack");
        assert_eq!(helper_name("audio-mf"), "Windows' Media Feature Pack");
        assert_eq!(
            helper_page("audio-mf"),
            Some("ms-settings:optionalfeatures")
        );
        assert_eq!(helper_page("ffmpeg"), None);
        assert_eq!(
            helper_page("pandoc"),
            Some("https://pandoc.org/installing.html")
        );
        assert_eq!(helper_page("nothing"), None);
        // A row whose formats all wait on it says so, and its link installs it.
        let mut e = row("song.wma");
        e.targets.clear();
        e.missing = Some(crate::model::Missing::new("audio-mf", 6));
        assert_eq!(
            e.reach(),
            "Needs Windows' Media Feature Pack, which isn't installed."
        );
        assert_eq!(e.link(false), Some("Get Media Feature Pack"));
    }

    #[test]
    fn get_it_looks_for_the_extension_again_on_activation() {
        use kiln_media::video::VideoCodec;
        let mut r = ExtensionRecheck::default();
        // Nothing pressed: activation doesn't look.
        assert!(!r.activated());
        // Get it opened the Store: the next activation looks, once,
        // however often the window is activated while the look runs.
        r.store_opened();
        assert!(r.activated());
        assert!(!r.activated());
        // Nothing yet, rows still waiting: the next activation looks again.
        assert!(!r.finished(false, true));
        assert!(r.activated());
        // It is there: the rows are read again, and with none left
        // waiting the watch ends.
        assert!(r.finished(true, false));
        assert!(!r.activated());
        assert_eq!(r, ExtensionRecheck::default());
        assert_eq!(extension_codec("HEVC"), Some(VideoCodec::Hevc));
        assert_eq!(extension_codec("AV1"), Some(VideoCodec::Av1));
        assert_eq!(extension_codec("VP9"), Some(VideoCodec::Vp9));
        assert_eq!(extension_codec("ProRes"), None);
    }

    #[test]
    fn rows_waiting_on_an_extension_clear_once_it_is_there() {
        let waiting = |name: &str, codec: &'static str, product: &'static str| {
            let mut e = row(name);
            e.ffmpeg_found = false;
            let mut f = facts("mp4", Kind::Video);
            f.media = Some(kiln_core::MediaInfo {
                needs_extension: Some((codec, product)),
                ..Default::default()
            });
            e.facts = Some(f);
            e
        };
        let mut list = FileList::default();
        list.insert(0, waiting("a.mp4", "HEVC", "9N4WGH0Z6VHQ"));
        list.insert(1, waiting("b.mkv", "AV1", "9MVZQVXJBQ9V"));
        list.insert(2, waiting("c.mp4", "HEVC", "9N4WGH0Z6VHQ"));
        list.insert(3, row("d.png"));
        assert_eq!(list.awaited_extensions(), ["HEVC", "AV1"]);
        // HEVC's decoder is there now: its two rows clear, AV1's waits.
        let cleared = list.extensions_installed(&["HEVC"]);
        assert_eq!(
            cleared,
            [PathBuf::from("photos/a.mp4"), PathBuf::from("photos/c.mp4")]
        );
        assert_eq!(list.entries()[0].extension_line(), None);
        assert!(list.entries()[1].extension_line().is_some());
        assert_eq!(list.awaited_extensions(), ["AV1"]);
        assert!(list.extensions_installed(&["HEVC"]).is_empty());
    }

    #[test]
    fn install_it_looks_for_the_pack_again_on_activation() {
        use kiln_media::MfCodecs;
        let none = MfCodecs::default();
        let all = MfCodecs {
            aac_enc: true,
            wma_enc: true,
            ac3_enc: true,
            wma_dec: true,
            ac3_dec: true,
            heaac_dec: true,
        };
        let mut r = MfRecheck::default();
        // Nothing pressed: activation doesn't look.
        assert!(!r.activated());
        // Another helper's link doesn't start the watch either.
        r.helper_opened("ffmpeg");
        assert!(!r.activated());
        // Get Media Feature Pack: the next activation looks, once, however often the
        // window is activated while the look runs.
        r.helper_opened("audio-mf");
        assert!(r.activated());
        assert!(!r.activated());
        // Nothing yet: the rows stay, and the next activation looks again.
        assert!(!r.finished(none, none));
        assert!(r.activated());
        // The pack is there: the offers are worked out again, and the
        // watch ends.
        assert!(r.finished(none, all));
        assert!(!r.activated());
        assert_eq!(r, MfRecheck::default());
    }

    #[test]
    fn a_row_waiting_on_the_pack_clears_once_it_is_there() {
        let (dir, kiln) = setup();
        let wma = dir.path().join("song.wma");
        fs::write(&wma, b"not really wma").unwrap();
        let no_mf = Availability::all().without(&["audio-mf"]);
        let mut list = FileList::default();
        list.add(&kiln, &no_mf, [wma]);
        let e = &list.entries()[0];
        assert_eq!(e.needs_helper(), Some("audio-mf"));
        assert_eq!(e.link(false), Some("Get Media Feature Pack"));
        // What the window does when a look finds the pack: the offers are
        // worked out again with the engines looked for afresh.
        list.refresh_offers(&kiln, &Availability::all());
        let e = &list.entries()[0];
        assert_eq!(e.needs_helper(), None);
        assert!(!e.targets.is_empty());
    }

    #[test]
    fn a_row_with_unreadable_sound_says_why() {
        let mut e = row("broken.mp3");
        e.targets.clear();
        e.missing = Some(crate::model::Missing::new("audio-mf", 6));
        let mut f = facts("mp3", Kind::Audio);
        f.audio_problem = Some("This MP3 is damaged and can't be read.".to_owned());
        e.facts = Some(f);
        assert_eq!(e.reach(), "This MP3 is damaged and can't be read.");
        // Installing the Media Feature Pack wouldn't mend it: no link.
        assert_eq!(e.needs_helper(), None);
        assert_eq!(e.link(false), None);
    }

    #[test]
    fn row_line_before_and_after_facts() {
        // Task 18 (mock screen 2): its type and size before the facts, what
        // it is and its size after; never a suggestion or the folder.
        let mut e = row("a.png");
        assert_eq!(e.detail(), "PNG · 3.1 MB");
        e.facts = Some(photo());
        e.suggestion = Some(Suggestion {
            action: Suggested::Convert("jpg".into()),
            reason: String::new(),
            estimate: Some(2_040_000),
        });
        assert_eq!(e.detail(), "4032 × 3024 photo · 3.1 MB");
        assert_eq!(e.line(), (e.detail(), Tone::Muted));
        assert_eq!(e.plain_detail(), "PNG · 3.1 MB");

        let mut txt = row("notes.txt");
        txt.code = "TXT".to_owned();
        txt.facts = Some(facts("txt", Kind::Document));
        assert_eq!(txt.detail(), "TXT · 3.1 MB");
    }

    #[test]
    fn the_row_tooltip_has_the_cut_line_then_the_full_folder() {
        let mut e = row("photo.png");
        e.folder = r"C:\kiln\target\samples".to_owned();
        // Nothing cut: where the file is, in full.
        let line = e.line().0;
        assert_eq!(e.tooltip(&line, false), r"C:\kiln\target\samples");
        // A cut line comes whole, then the path on a line of its own.
        assert_eq!(
            e.tooltip(&line, true),
            format!("{line}\nC:\\kiln\\target\\samples")
        );
        // A failed row's sentence comes as it was written, unfolded.
        e.job = JobState::Failed {
            message: "Needs Pandoc, which isn't installed.\nSettings > Helper apps says where to get it.\n"
                .into(),
            details: String::new(),
            engine: None,
            store_product: None,
        };
        let line = e.line().0;
        assert_eq!(
            e.tooltip(&line, true),
            "Needs Pandoc, which isn't installed.\nSettings > Helper apps says where to get it.\nC:\\kiln\\target\\samples"
        );
        assert_eq!(e.tooltip(&line, false), r"C:\kiln\target\samples");
    }

    #[test]
    fn a_finished_rows_results_are_named_with_how_many_more() {
        let mut e = row("p.png");
        // Nothing finished, nothing to name.
        assert_eq!(e.results_named(), None);
        let jpg = Action::Convert("jpg".into());
        let done = |outputs: &[&str]| JobState::Done {
            action: jpg.clone(),
            outputs: outputs.iter().map(PathBuf::from).collect(),
            saved: None,
        };
        e.job = done(&[]);
        assert_eq!(e.results_named(), None);
        e.job = done(&["held/p.jpg"]);
        assert_eq!(e.results_named().as_deref(), Some("p.jpg"));
        e.job = done(&["held/p 1.jpg", "held/p 2.jpg"]);
        assert_eq!(e.results_named().as_deref(), Some("p 1.jpg and 1 more"));
        e.job = done(&["held/p 1.jpg", "held/p 2.jpg", "held/p 3.jpg"]);
        assert_eq!(e.results_named().as_deref(), Some("p 1.jpg and 2 more"));
        // While they are saved, the row says only that.
        assert_eq!(e.line().0, "Saving…");
        e.job = JobState::Cancelled;
        assert_eq!(e.results_named(), None);
    }

    #[test]
    fn facts_for_a_removed_row_are_dropped() {
        let (dir, kiln) = setup();
        let a = dir.path().join("a.png");
        fs::write(&a, b"x").unwrap();
        let mut list = FileList::default();
        list.add(&kiln, &Availability::all(), [a]);
        let before = list.entries().to_vec();
        assert!(!list.set_facts(&dir.path().join("gone.png"), photo(), None));
        assert_eq!(list.entries(), before);
    }

    #[test]
    fn facts_for_waits_for_every_file() {
        let (dir, kiln) = setup();
        let paths: Vec<PathBuf> = ["a.png", "b.png"]
            .iter()
            .map(|n| {
                let p = dir.path().join(n);
                fs::write(&p, b"x").unwrap();
                p
            })
            .collect();
        let mut list = FileList::default();
        list.add(&kiln, &Availability::all(), paths);
        let listed: Vec<PathBuf> = list.entries().iter().map(|e| e.path.clone()).collect();
        assert_eq!(list.facts_for(&listed), None);
        assert!(list.set_facts(&listed[0], photo(), None));
        let first = list.entries()[0].facts_at;
        assert!(first.is_some());
        // The same facts again, as when a stale result lands: no new fade.
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(list.set_facts(&listed[0], photo(), None));
        assert_eq!(list.entries()[0].facts_at, first);
        assert_eq!(list.facts_for(&listed), None);
        let graphic = picture(facts("png", Kind::Image), 1200, 800, Look::Graphic);
        assert!(list.set_facts(&listed[1], graphic.clone(), None));
        assert_eq!(list.facts_for(&listed), Some(vec![photo(), graphic]));
        assert_eq!(list.facts_for(&[dir.path().join("c.png")]), None);
        list.forget_facts();
        assert_eq!(list.facts_for(&listed), None);
        assert!(list.entries().iter().all(|e| e.facts_at.is_none()));
    }

    #[test]
    fn a_jpg_cut_short_says_so_in_its_row() {
        // A JPG cut short says so: the fixture, cut in half as a copy that
        // didn't finish leaves it.
        let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/sample.jpg");
        let bytes = fs::read(sample).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cut.jpg");
        fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
        // On adding: the walk's look at it finds it, and it is offered
        // nothing.
        let meta = fs::metadata(&path).unwrap();
        let read = ReadFile::read(&Registry::builtin(), path.clone(), &meta);
        assert_eq!(read.check, FileCheck::CutShort);
        let mut e = row("cut.jpg");
        e.code = "JPG".to_owned();
        e.check = read.check;
        e.targets = vec![];
        assert_eq!(
            e.line(),
            (
                format!(
                    "Looks cut short. Try copying or downloading it again.{DETAIL_SEPARATOR}{}",
                    human_size(e.size)
                ),
                Tone::Crack
            )
        );
        // A job that reads it all the same, as one cut short after it was
        // added: Kiln's refusal is the row's sentence.
        let err = kiln_core::Kiln::new()
            .convert(&path, "png", &kiln_core::Options::default())
            .unwrap_err();
        let err = kiln_core::JobError::from(err);
        e.last = Some(Action::Convert("png".into()));
        e.job = JobState::Failed {
            message: err.message,
            details: err.details,
            engine: err.engine,
            store_product: err.store_product,
        };
        assert_eq!(
            e.failure_sentence(),
            "This JPG looks cut short. Try copying or downloading it again."
        );
        assert_eq!(e.failure_title(), "Couldn't read this JPG");
        // Trying again reads the same bytes.
        assert!(!e.retryable());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn retry_is_for_cancelled_rows_and_failures_trying_again_can_fix() {
        let failed = |message: &str| JobState::Failed {
            message: message.into(),
            details: String::new(),
            engine: None,
            store_product: None,
        };
        let mut e = row("a.png");
        e.job = JobState::Cancelled;
        // Nothing known about what it was doing: nothing to run again.
        assert!(!e.retryable());
        e.last = Some(Action::Convert("jpg".into()));
        assert!(e.retryable());
        e.job = failed("The conversion stopped before it finished. Try again.");
        assert!(e.retryable());
        e.job = failed(
            "This file looks damaged, or isn't really a PNG. Try opening it in another app.",
        );
        assert!(!e.retryable());
        e.job = JobState::Idle;
        assert!(!e.retryable());
    }

    fn held_row(name: &str, outputs: &[&str], saved: Option<&str>) -> Entry {
        let mut e = row(name);
        e.job = JobState::Done {
            action: Action::Convert("jpg".into()),
            outputs: outputs.iter().map(PathBuf::from).collect(),
            saved: saved.map(|f| Saved::Folder(f.into())),
        };
        e
    }

    #[test]
    fn a_finished_drag_wheel_conversion_offers_options() {
        let mut held = held_row("a.png", &["held/photo.jpg"], None);
        // Converted over Kiln's window: the sheet was already chosen.
        assert_eq!(held.options_for(), None);
        // From a drop on the drag-wheel: Options redoes it to the same
        // format, held or saved.
        held.from_drag = true;
        assert_eq!(held.options_for(), Some("jpg"));
        let mut saved = held_row("a.png", &[r"C:\pics\photo.jpg"], Some(r"C:\pics"));
        saved.from_drag = true;
        assert_eq!(saved.options_for(), Some("jpg"));
        // A tool, a failure, a cancelled or running job: none.
        let mut tool = held.clone();
        tool.job = JobState::Done {
            action: Action::Tool(Tool::RemoveEdges, Kind::Image),
            outputs: vec![PathBuf::from("held/photo cropped.png")],
            saved: None,
        };
        assert_eq!(tool.options_for(), None);
        let mut failed = held.clone();
        failed.job = JobState::Failed {
            message: "It broke.".into(),
            details: String::new(),
            engine: None,
            store_product: None,
        };
        assert_eq!(failed.options_for(), None);
        let mut other = held.clone();
        for job in [
            JobState::Cancelled,
            JobState::Idle,
            JobState::Running {
                action: Action::Convert("jpg".into()),
                fraction: None,
                started: None,
            },
        ] {
            other.job = job;
            assert_eq!(other.options_for(), None);
        }
    }

    /// Row `a.png` mid-redo: it held `prior`, and its redo has just ended
    /// as `end`.
    fn redone(prior: &Entry, end: JobState) -> (Entry, Vec<PathBuf>) {
        let mut e = prior.clone();
        e.job = end;
        let gone = e.end_redo(prior.job.clone());
        (e, gone)
    }

    fn redo_done(output: &str) -> JobState {
        JobState::Done {
            action: Action::Convert("jpg".into()),
            outputs: vec![PathBuf::from(output)],
            saved: None,
        }
    }

    #[test]
    fn a_redo_that_succeeds_replaces_the_held_result() {
        let mut held = held_row("a.png", &["held/photo.jpg"], None);
        held.from_drag = true;
        let (e, gone) = redone(&held, redo_done("held/photo 2.jpg"));
        // The new result is the row's; the old one goes only now, as a
        // replace's Discard takes it.
        assert_eq!(e.held(), Some(&[PathBuf::from("held/photo 2.jpg")][..]));
        assert_eq!(gone, [PathBuf::from("held/photo.jpg")]);
        assert_eq!(e.options_for(), Some("jpg"));
        // Never the file the new result is.
        let (_, gone) = redone(&held, redo_done("held/photo.jpg"));
        assert!(gone.is_empty());
    }

    #[test]
    fn a_redo_that_fails_or_is_cancelled_gives_back_the_held_result() {
        let mut held = held_row("a.png", &["held/photo.jpg"], None);
        held.from_drag = true;
        for end in [
            JobState::Failed {
                message: "It broke.".into(),
                details: String::new(),
                engine: None,
                store_product: None,
            },
            JobState::Cancelled,
        ] {
            let (e, gone) = redone(&held, end.clone());
            // Untouched, held as before: Options again, and still there
            // for the next start to deliver.
            assert!(gone.is_empty(), "{end:?}");
            assert_eq!(e.job, held.job, "{end:?}");
            assert_eq!(e.options_for(), Some("jpg"));
            let mut list = FileList::default();
            list.insert(0, e);
            assert_eq!(list.held_files(|_| true).len(), 1, "{end:?}");
            assert_eq!(
                list.held_only_by(|_| true),
                [PathBuf::from("held/photo.jpg")]
            );
        }
    }

    #[test]
    fn a_retried_drop_is_still_a_drop() {
        let jpg = Action::Convert("jpg".into());
        let mut e = row("a.png");
        e.from_drag = true;
        e.last = Some(jpg.clone());
        e.job = JobState::Cancelled;
        assert!(e.retries_drop(&jpg));
        // Something else on it is not a retry.
        assert!(!e.retries_drop(&Action::Convert("webp".into())));
        // Nor a conversion over Kiln's window, nor a finished row.
        let mut window = e.clone();
        window.from_drag = false;
        assert!(!window.retries_drop(&jpg));
        let mut done = e.clone();
        done.job = JobState::Done {
            action: jpg.clone(),
            outputs: vec![PathBuf::from("held/a.jpg")],
            saved: None,
        };
        assert!(!done.retries_drop(&jpg));
    }

    #[test]
    fn a_redo_never_takes_a_result_already_saved() {
        // Saved next to the original: the person's file.
        let mut saved = held_row("a.png", &[r"C:\pics\photo.jpg"], Some(r"C:\pics"));
        saved.from_drag = true;
        let (e, gone) = redone(&saved, redo_done("held/photo.jpg"));
        assert!(gone.is_empty());
        assert_eq!(e.held(), Some(&[PathBuf::from("held/photo.jpg")][..]));
        // And a failed redo leaves it saved, as it was.
        let (e, gone) = redone(&saved, JobState::Cancelled);
        assert!(gone.is_empty());
        assert_eq!(e.job, saved.job);
    }

    #[test]
    fn held_rows_say_they_are_being_saved() {
        let e = held_row("a.png", &["held/photo.jpg"], None);
        assert_eq!(e.line(), ("Saving…".to_owned(), Tone::Muted));
        let e = held_row("a.pdf", &["p 1.png", "p 2.png"], None);
        assert_eq!(e.line().0, "Saving…");
    }

    #[test]
    fn saved_rows_name_the_folder() {
        let e = held_row("a.png", &["held/photo.jpg"], Some(r"C:\Users\me\Pictures"));
        // Task 18: the new file's name; Show in folder goes to the folder.
        assert_eq!(e.line(), ("Saved as photo.jpg".to_owned(), Tone::Result));
        let e = held_row("a.png", &["held/photo.jpg"], Some(r"D:\"));
        assert_eq!(e.line().0, "Saved as photo.jpg");
        assert!(e.tooltip("", false).ends_with(r"Saved to D:\"));
    }

    #[test]
    fn a_missing_held_result_is_gone() {
        let held = held_row("a.png", &["held/photo.jpg"], None);
        assert!(held.is_gone(|_| false));
        assert!(!held.is_gone(|_| true));
        assert_eq!(
            held.line_checked(|_| false),
            (
                "The result is no longer there. Convert it again.".to_owned(),
                Tone::Crack
            )
        );
        assert_eq!(held.line_checked(|_| true), held.line());
        // One missing part of several is enough.
        let many = held_row("a.pdf", &["p 1.png", "p 2.png"], None);
        assert!(many.is_gone(|p| !p.ends_with("p 1.png")));
        // Saved results are somewhere the user put them, not ours to check.
        let saved = held_row("a.png", &["held/photo.jpg"], Some("Pictures"));
        assert!(!saved.is_gone(|_| false));
    }

    #[test]
    fn rows_link_to_details_or_export() {
        let mut failed = row("a.png");
        failed.job = JobState::Failed {
            message: "FFmpeg couldn't convert the file.".into(),
            details: "engine output".into(),
            engine: Some("FFmpeg"),
            store_product: None,
        };
        assert_eq!(failed.link(false), Some("Show details"));
        assert_eq!(
            failed.details(false),
            Some((
                "What FFmpeg said".to_owned(),
                "FFmpeg couldn't convert the file.".to_owned(),
                "engine output".to_owned()
            ))
        );
        failed.job = JobState::Failed {
            message: "There's no plain border to remove.".into(),
            details: String::new(),
            engine: None,
            store_product: None,
        };
        // UX-13: every failure has Show details, with Kiln's own words when
        // the engine had none.
        assert_eq!(failed.link(false), Some("Show details"));
        assert!(failed.details(false).is_some());
        assert_eq!(failed.link(true), Some("Show details"));
        assert_eq!(
            failed.details(true),
            Some((
                "What happened".to_owned(),
                "There's no plain border to remove.".to_owned(),
                String::new()
            ))
        );
        let held = held_row("a.png", &["held/a.jpg"], None);
        assert_eq!(held.link(false), None);
        assert_eq!(held.link(true), None);
        assert_eq!(held.details(true), None);
        assert_eq!(held.saved(), None);
        let saved = held_row("a.png", &[r"C:\Out\a.jpg"], Some(r"C:\Out"));
        assert_eq!(saved.link(false), Some("Show in folder"));
        assert_eq!(
            saved.saved(),
            Some((Path::new(r"C:\Out"), &[PathBuf::from(r"C:\Out\a.jpg")][..]))
        );
        assert_eq!(row("idle.png").link(true), Some("Convert"));
    }

    #[test]
    fn the_details_dialog_says_what_happened_then_the_engines_words() {
        // D37 / mock screen 8: "What happened", a plain line naming the
        // file, then what the engine said.
        let mut e = row("scan-2019.pdf");
        e.code = "PDF".into();
        e.job = JobState::Failed {
            message:
                "This file looks damaged, or isn't really a PDF. Try opening it in another app."
                    .into(),
            details: "the PDF couldn't be read (xref table not found).\n".into(),
            engine: Some("Kiln"),
            store_product: None,
        };
        assert_eq!(
            e.details(false),
            Some((
                DETAILS_TITLE.to_owned(),
                "Kiln couldn't read scan-2019.pdf. It looks damaged, or isn't really a PDF. \
                 Try opening it in another app."
                    .to_owned(),
                "the PDF couldn't be read (xref table not found).".to_owned()
            ))
        );
        assert_eq!(DETAILS_TITLE, "What happened");
    }

    #[test]
    fn the_details_log_drops_its_first_line_when_it_repeats_the_sentence() {
        let mut e = row("doc.pdf");
        e.code = "PDF".into();
        let failed = |details: &str| JobState::Failed {
            message: "The PDF has no text to take out. Its pages may be scanned images.".into(),
            details: details.into(),
            engine: Some("Kiln"),
            store_product: None,
        };
        let lead = "The PDF has no text to take out. Its pages may be scanned images.";
        // The sentence again in lower case is not said twice.
        e.job = failed("the PDF has no text to take out. Its pages may be scanned images.\n");
        assert_eq!(
            e.details(false),
            Some((DETAILS_TITLE.to_owned(), lead.to_owned(), String::new()))
        );
        // Only that line goes; the lines after it stay.
        e.job = failed("the PDF has no text to take out. Its pages may be scanned images.\nxref table not found\n");
        assert_eq!(e.details(false).unwrap().2, "xref table not found");
        // A first line that says something else stays.
        e.job = failed("the PDF couldn't be read\nxref table not found");
        assert_eq!(
            e.details(false).unwrap().2,
            "the PDF couldn't be read\nxref table not found"
        );
    }

    #[test]
    fn the_details_dialog_names_the_helper_whose_words_it_shows() {
        // when a helper app's own words are in the card, the title
        // says whose they are; Kiln's own, or none, keep "What happened".
        let mut e = row("clip.mp4");
        e.code = "MP4".into();
        let failed = |details: &str, engine| JobState::Failed {
            message:
                "This file looks damaged, or isn't really an MP4. Try opening it in another app."
                    .into(),
            details: details.into(),
            engine,
            store_product: None,
        };
        e.job = failed(
            "moov atom not found
",
            Some("FFmpeg"),
        );
        assert_eq!(e.details(false).unwrap().0, "What FFmpeg said");
        e.job = failed("source file could not be loaded", Some("LibreOffice"));
        assert_eq!(e.details(false).unwrap().0, "What LibreOffice said");
        e.job = failed(
            "  
",
            Some("FFmpeg"),
        );
        assert_eq!(e.details(false).unwrap().0, DETAILS_TITLE);
        e.job = failed("the PDF couldn't be read", Some("Kiln"));
        assert_eq!(e.details(false).unwrap().0, DETAILS_TITLE);
        e.job = failed("something", None);
        assert_eq!(e.details(false).unwrap().0, DETAILS_TITLE);
    }

    #[test]
    fn copy_says_copied_for_a_moment() {
        use std::time::Duration;
        assert_eq!(copy_label(None), "Copy");
        assert_eq!(copy_label(Some((true, Duration::ZERO))), "Copied");
        assert_eq!(
            copy_label(Some((true, Duration::from_millis(1199)))),
            "Copied"
        );
        assert_eq!(
            copy_label(Some((true, Duration::from_millis(1200)))),
            "Copy"
        );
        assert_eq!(copy_label(Some((false, Duration::ZERO))), "Couldn't copy");
        assert_eq!(
            copy_label(Some((false, Duration::from_millis(1200)))),
            "Copy"
        );
        assert_eq!(clipboard_lines("a\nb\r\nc"), "a\r\nb\r\nc");
    }

    #[test]
    fn a_merged_result_counts_once() {
        // Merge gives both its rows the same one result.
        let list = FileList {
            entries: vec![
                held_row("a.mp3", &["held/a merged.mp3"], None),
                held_row("b.mp3", &["held/a merged.mp3"], None),
                held_row("c.png", &["held/c.jpg"], None),
            ],
            ..Default::default()
        };
        assert_eq!(list.held_files(|_| true).len(), 2);
        assert_eq!(
            list.held_files(|i| i < 2),
            vec![PathBuf::from("held/a merged.mp3")]
        );
        // Removing one merge row loses nothing: the other still holds it.
        assert!(list.held_only_by(|i| i == 0).is_empty());
        assert_eq!(
            list.held_only_by(|i| i != 2),
            vec![PathBuf::from("held/a merged.mp3")]
        );
        assert_eq!(
            list.held_only_by(|i| i == 2),
            vec![PathBuf::from("held/c.jpg")]
        );
    }

    #[test]
    fn exported_rows_are_saved_and_the_rest_stay_held() {
        let mut list = FileList {
            entries: vec![
                held_row("a.png", &["held/a.jpg"], None),
                held_row("b.pdf", &["held/p 1.png", "held/p 2.png"], None),
                held_row("c.mp3", &["held/m.mp3"], None),
                held_row("d.mp3", &["held/m.mp3"], None),
                held_row("e.png", &["held/e.jpg"], None),
            ],
            ..Default::default()
        };
        let out = Path::new(r"C:\Out");
        let moved = vec![
            ("held/a.jpg".into(), out.join("a.jpg")),
            ("held/p 1.png".into(), out.join("p 1.png")),
            ("held/m.mp3".into(), out.join("m (1).mp3")),
        ];
        list.exported(&moved, out);
        let e = list.entries();
        assert_eq!(
            e[0].job,
            JobState::Done {
                action: Action::Convert("jpg".into()),
                outputs: vec![out.join("a.jpg")],
                saved: Some(Saved::Folder(out.to_path_buf())),
            }
        );
        // Part of a split went out: the row keeps holding the rest.
        assert_eq!(e[1].held(), Some(&[PathBuf::from("held/p 2.png")][..]));
        // Both merge rows follow their shared result.
        for row in &e[2..4] {
            assert!(row.held().is_none());
            assert_eq!(row.line().0, "Saved as m (1).mp3");
        }
        // Untouched rows stay as they were.
        assert_eq!(e[4].held(), Some(&[PathBuf::from("held/e.jpg")][..]));
        assert_eq!(list.held_files(|_| true).len(), 2);
    }

    #[test]
    fn a_press_inside_the_selection_waits_for_release() {
        let mut list = list_of(4);
        list.click(0);
        list.ctrl_click(2);
        assert!(list.press(2));
        assert_eq!(list.selection(), [0, 2]);
        assert_eq!(list.focus(), Some(2));
        assert_eq!(list.anchor, Some(2));
        // Release without a drag: the win layer calls click(2).
        list.click(2);
        assert_eq!(list.selection(), [2]);
        // A press outside the selection, or with one row selected, is a click.
        assert!(!list.press(1));
        assert_eq!(list.selection(), [1]);
        assert!(!list.press(1));
        assert_eq!(list.selection(), [1]);
        // Past the end is no row.
        assert!(!list.press(9));
        assert_eq!(list.selection(), [1]);
    }

    #[test]
    fn a_ctrl_press_on_a_selected_row_waits_for_release() {
        let mut list = list_of(4);
        list.click(0);
        list.ctrl_click(2);
        // Kept at press, so the selection can be dragged out.
        assert!(list.ctrl_press(0));
        assert_eq!(list.selection(), [0, 2]);
        assert_eq!(list.focus(), Some(0));
        assert_eq!(list.anchor, Some(0));
        // Release without a drag: the win layer calls ctrl_click(0).
        list.ctrl_click(0);
        assert_eq!(list.selection(), [2]);
        // The only selected row is kept at press too.
        assert!(list.ctrl_press(2));
        assert_eq!(list.selection(), [2]);
        list.ctrl_click(2);
        assert!(list.selection().is_empty());
    }

    #[test]
    fn a_ctrl_press_on_an_unselected_row_adds_it_at_once() {
        let mut list = list_of(4);
        list.click(0);
        assert!(!list.ctrl_press(3));
        assert_eq!(list.selection(), [0, 3]);
        assert_eq!(list.focus(), Some(3));
        assert_eq!(list.anchor, Some(3));
        // The release does nothing more.
        assert_eq!(list.selection(), [0, 3]);
        // Past the end is no row.
        assert!(!list.ctrl_press(9));
        assert_eq!(list.selection(), [0, 3]);
    }

    fn prompt(running: usize, verb: Leave) -> Option<(String, Vec<PromptButton>)> {
        leave_prompt(running, verb).map(|p| (p.title, p.buttons))
    }

    #[test]
    fn nothing_running_means_no_prompt() {
        for verb in [Leave::Close, Leave::Clear, Leave::Remove] {
            assert_eq!(leave_prompt(0, verb), None);
        }
    }

    #[test]
    fn a_leave_waits_for_any_dialog_to_close() {
        // An export finishing under the Save dialog or a picker doesn't
        // prompt or close on top of it.
        assert_eq!(leave_due(true, true, true, false), Due::Nothing);
        assert_eq!(leave_due(true, false, true, false), Due::Nothing);
        // With the dialog closed, the leave goes ahead first; a close
        // waits for the last export.
        assert_eq!(leave_due(false, true, false, false), Due::Resume);
        assert_eq!(leave_due(false, true, true, false), Due::Resume);
        assert_eq!(leave_due(false, false, true, false), Due::Close);
        assert_eq!(leave_due(false, false, true, true), Due::Nothing);
        assert_eq!(leave_due(false, false, false, false), Due::Nothing);
    }

    #[test]
    fn the_sticky_warning_stands_in_for_the_plain_count() {
        let warn = Some(NO_HOLDING);
        assert_eq!(footer_status("", "2 files.", warn), NO_HOLDING);
        assert_eq!(footer_status("2 files.", "2 files.", warn), NO_HOLDING);
        assert_eq!(
            footer_status("Converted 2 files to JPG.", "2 files.", warn),
            "Converted 2 files to JPG."
        );
        assert_eq!(footer_status("2 files.", "2 files.", None), "2 files.");
    }

    #[cfg(windows)]
    #[test]
    fn no_room_names_the_holding_drive() {
        let held = Path::new(r"C:\Users\a\AppData\Local\Kiln\held\1-2");
        let full = Path::new(r"c:\Users\a\AppData\Local\Kiln\held\1-2\photo.jpg");
        assert_eq!(
            no_room_line(full, held).as_deref(),
            Some("There isn't room on C: to hold the result.")
        );
        // Out of room somewhere else isn't about holding the result.
        assert_eq!(no_room_line(Path::new(r"D:\Temp\Kiln\x.tmp"), held), None);
        assert_eq!(no_room_line(Path::new("relative.tmp"), held), None);
    }

    #[test]
    fn running_jobs_ask_to_stop() {
        // Keep converting comes first, as the mock has it (phase 3 put
        // the in-app dialog's accent first; it was Stop, then Cancel).
        for (verb, label) in [
            (Leave::Close, "Close"),
            (Leave::Clear, "Clear list"),
            (Leave::Remove, "Remove from list"),
        ] {
            let (title, buttons) = prompt(2, verb).unwrap();
            assert!(
                title.starts_with("2 files are still converting."),
                "{title}"
            );
            assert_eq!(buttons, [PromptButton::Cancel, PromptButton::Stop(verb)]);
            assert_eq!(buttons[1].label(), label);
            assert!(prompt(1, verb)
                .unwrap()
                .0
                .starts_with("1 file is still converting."));
        }
    }

    #[test]
    fn the_leave_dialog_asks_in_the_mocks_words() {
        let p = leave_prompt(1, Leave::Close).unwrap();
        assert_eq!(p.dialog_title(), "Stop converting and close?");
        assert_eq!(
            p.title,
            "1 file is still converting. If you close Kiln now, it stops and no new file is made."
        );
        let labels: Vec<_> = p.buttons.iter().map(|&b| p.label(b)).collect();
        assert_eq!(labels, ["Keep converting", "Close"]);
        // Enter keeps converting.
        assert_eq!(p.accent(), Some(0));
        assert_eq!(p.buttons[0], PromptButton::Cancel);
        assert_eq!(
            leave_prompt(2, Leave::Close).unwrap().title,
            "2 files are still converting. If you close Kiln now, they stop and no new file is made."
        );

        for (verb, stop) in [
            (Leave::Clear, "Stop converting and clear the list?"),
            (Leave::Remove, "Stop converting and remove from the list?"),
        ] {
            let p = leave_prompt(2, verb).unwrap();
            assert_eq!(p.dialog_title(), stop);
            assert_eq!(p.title, "2 files are still converting.");
            assert_eq!(p.label(PromptButton::Cancel), "Keep converting");
        }

        // The big folder prompt says it all in one line, and Cancel is the
        // accent.
        let p = big_folder_prompt(1240);
        assert_eq!(p.heading, None);
        assert_eq!(p.dialog_title(), "That's 1240 files. Add them all?");
        assert_eq!(p.dialog_text(), None);
        assert_eq!(p.accent(), Some(1));
        assert_eq!(p.label(PromptButton::Cancel), "Cancel");
        // A prompt with no Cancel has no accent.
        let p = Prompt {
            heading: None,
            title: "x".into(),
            buttons: vec![PromptButton::Stop(Leave::Close)],
        };
        assert_eq!(p.accent(), None);
    }

    #[test]
    fn prompt_buttons_are_labelled_in_sentence_case() {
        let labels: Vec<_> = [
            PromptButton::Stop(Leave::Clear),
            PromptButton::AddAll,
            PromptButton::Cancel,
        ]
        .iter()
        .map(|b| b.label())
        .collect();
        assert_eq!(labels, ["Clear list", "Add them all", "Cancel"]);
    }

    #[test]
    fn convert_is_blocked_for_nothing_selected_and_for_a_busy_row() {
        assert_eq!(convert_block(0, 0, 0), Some("Select a file to convert."));
        assert!(convert_block(1, 1, 0).unwrap().contains("still converting"));
        assert!(convert_block(3, 1, 0).is_some());
        assert_eq!(convert_block(1, 0, 0), None);
        assert_eq!(convert_block(500, 0, 0), None);
        // a result being copied to another drive says so, in its own
        // words; a conversion running says it first.
        assert_eq!(convert_block(1, 0, 1), Some(COPYING_BLOCK));
        assert!(COPYING_BLOCK.contains("copied to another drive"));
        assert!(convert_block(2, 1, 1).unwrap().contains("still converting"));
    }

    #[test]
    fn the_revision_moves_with_the_rows_and_the_selection_but_not_the_focus() {
        let mut list = list_of(3);
        let mut seen = list.revision();
        let mut moved = |list: &FileList| {
            let now = list.revision();
            let moved = now != seen;
            seen = now;
            moved
        };
        // The selection.
        list.click(1);
        assert!(moved(&list));
        list.ctrl_click(2);
        assert!(moved(&list));
        list.shift_click(0);
        assert!(moved(&list));
        list.select_all();
        assert!(moved(&list));
        list.clear_selection();
        assert!(moved(&list));
        list.select_only(&[2]);
        assert!(moved(&list));
        // The focus and its ring are no change to what is selected.
        list.set_focus(0);
        list.keyboard_used();
        list.pointer_used();
        assert!(!moved(&list));
        // Nor is clearing a selection that is already empty.
        list.clear_selection();
        list.clear_selection();
        assert!(moved(&list));
        list.clear_selection();
        assert!(!moved(&list));
        // A row's job, its facts, its results.
        list.entries_mut()[0].job = JobState::Cancelled;
        assert!(moved(&list));
        let path = list.entries()[0].path.clone();
        let facts = kiln_core::Facts {
            ext: "png".into(),
            kind: kiln_core::Kind::Image,
            bytes: 1,
            picture: None,
            media: None,
            pdf: None,
            bars: None,
            estimates: Default::default(),
            compressed: None,
            audio_problem: None,
            silent: false,
        };
        assert!(list.set_facts(&path, facts, None));
        assert!(moved(&list));
        list.forget_facts();
        assert!(moved(&list));
        list.exported(&[], Path::new("out"));
        assert!(moved(&list));
        // A preview is no change to what the list says.
        assert_eq!(list.preview_arrived(&path, None), Some(0));
        assert!(!moved(&list));
        // Rows coming and going.
        let gone = list.remove(2).unwrap();
        assert!(moved(&list));
        list.insert(2, gone);
        assert!(moved(&list));
        list.clear();
        assert!(moved(&list));
    }

    #[test]
    fn a_file_that_is_converting_is_not_converted_again() {
        let mut list = list_of(3);
        let paths: Vec<PathBuf> = list.entries().iter().map(|e| e.path.clone()).collect();
        assert_eq!(list.convert_busy(&paths), None);
        assert_eq!(list.convert_busy(&paths[..1]), None);
        list.entries_mut()[1].job = JobState::Running {
            action: Action::Convert("jpg".into()),
            fraction: None,
            started: None,
        };
        // Alone or among others, the converting file stops it, with the
        // footer's own words.
        let busy = Some("A selected file is still converting. Wait for it, or cancel it.");
        assert_eq!(list.convert_busy(&paths), busy);
        assert_eq!(list.convert_busy(&paths[1..2]), busy);
        assert_eq!(list.convert_busy(&paths[..1]), None);
        // Finished, it can be converted again.
        list.entries_mut()[1].job = JobState::Cancelled;
        assert_eq!(list.convert_busy(&paths), None);
        // Nothing asked for, and a file no longer listed, stop nothing.
        assert_eq!(list.convert_busy(&[]), None);
        assert_eq!(list.convert_busy(&[PathBuf::from("gone.png")]), None);
    }

    #[test]
    fn a_new_file_offers_convert_as_its_link() {
        let e = row("b.png");
        assert_eq!(e.link(false), Some("Convert"));
        let mut running = row("b.png");
        running.job = JobState::Cancelled;
        assert_eq!(running.link(false), None);
    }

    #[test]
    fn tab_goes_round_the_list_and_the_buttons() {
        use FocusButton::*;
        let walk = |back: bool, listed: bool| {
            let mut at = tab_stop(None, back, listed, false, &[]);
            let mut seen = vec![at];
            while seen.len() < 8 {
                at = tab_stop(at, back, listed, false, &[]);
                seen.push(at);
            }
            seen
        };
        // From the list: the footer left to right, then the header.
        assert_eq!(
            walk(false, true)[..5],
            [
                Some(AddFiles),
                Some(Clear),
                Some(Help),
                Some(Settings),
                None
            ]
        );
        // Shift+Tab goes the other way.
        assert_eq!(
            walk(true, true)[..5],
            [
                Some(Settings),
                Some(Help),
                Some(Clear),
                Some(AddFiles),
                None
            ]
        );
        // The empty window: Help, Settings, then its Add files, round.
        assert_eq!(
            walk(false, false)[..4],
            [Some(Help), Some(Settings), Some(AddFiles), Some(Help)]
        );
        assert_eq!(tab_stop(None, true, false, false, &[]), Some(AddFiles));
        // A button that's gone starts the path again.
        assert_eq!(tab_stop(Some(Clear), false, false, false, &[]), Some(Help));
    }

    #[test]
    fn tab_reaches_the_top_infobars_dismiss_between_the_header_and_the_list() {
        use FocusButton::*;
        let go = |at, back| tab_stop_with(at, back, true, true, true, &[]);
        assert_eq!(go(Some(Settings), false), Some(InfoDismiss));
        assert_eq!(go(Some(InfoDismiss), false), None);
        assert_eq!(go(None, true), Some(InfoDismiss));
        assert_eq!(go(Some(InfoDismiss), true), Some(Settings));
        // Without the InfoBar, Settings goes straight to the list.
        assert_eq!(
            tab_stop_with(Some(Settings), false, true, true, false, &[]),
            None
        );
    }

    #[test]
    fn tab_reaches_the_first_run_cards_dismiss_after_add_files() {
        use FocusButton::*;
        let walk = |back: bool, first_run: bool| {
            let mut at = None;
            let mut seen = vec![];
            for _ in 0..5 {
                at = empty_tab_stop(at, back, first_run, false);
                seen.push(at);
            }
            seen
        };
        assert_eq!(
            walk(false, true),
            [
                Some(Help),
                Some(Settings),
                Some(AddFiles),
                Some(Dismiss),
                Some(Help)
            ]
        );
        assert_eq!(
            walk(true, true),
            [
                Some(Dismiss),
                Some(AddFiles),
                Some(Settings),
                Some(Help),
                Some(Dismiss)
            ]
        );
        // Dismissed, the path is as it was.
        assert_eq!(
            walk(false, false)[..4],
            [Some(Help), Some(Settings), Some(AddFiles), Some(Help)]
        );
        // Dismiss gone from under the keyboard: Tab starts again.
        assert_eq!(
            empty_tab_stop(Some(Dismiss), false, false, false),
            Some(Help)
        );
        // An InfoBar at the top: its Dismiss after the title bar's.
        assert_eq!(
            empty_tab_stop(Some(Settings), false, false, true),
            Some(InfoDismiss)
        );
        assert_eq!(
            empty_tab_stop(Some(InfoDismiss), false, false, true),
            Some(AddFiles)
        );
        assert_eq!(
            empty_tab_stop(Some(AddFiles), true, false, true),
            Some(InfoDismiss)
        );
        // The same path as `tab_stop` gives the empty window without a card.
        for at in [None, Some(Help), Some(Settings), Some(AddFiles)] {
            for back in [false, true] {
                assert_eq!(
                    empty_tab_stop(at, back, false, false),
                    tab_stop(at, back, false, false, &[])
                );
            }
        }
    }

    #[test]
    fn the_footer_tabs_left_to_right_as_it_is_drawn_ending_at_convert() {
        use FocusButton::*;
        // Add files, Clear list, Convert: as drawn.
        let mut at = None;
        let mut seen = vec![];
        for _ in 0..4 {
            at = tab_stop(at, false, true, true, &[]);
            seen.push(at);
        }
        assert_eq!(
            seen,
            [Some(AddFiles), Some(Clear), Some(Convert), Some(Help)]
        );
        assert_eq!(tab_stop(Some(Convert), true, true, true, &[]), Some(Clear));
        assert_eq!(tab_stop(Some(Help), true, true, true, &[]), Some(Convert));
        assert_eq!(tab_stop(Some(AddFiles), true, true, true, &[]), None);
        // Disabled, it isn't a stop at all.
        assert_eq!(tab_stop(Some(Clear), false, true, false, &[]), Some(Help));
        // The empty window has no Convert, and starts at the header.
        assert_eq!(tab_stop(None, false, false, true, &[]), Some(Help));
    }

    #[test]
    fn the_credit_link_is_a_tab_stop_where_it_is_drawn() {
        use FocusButton::*;
        // In the footer, at its left: after the list, before Add files.
        let mut at = None;
        let mut seen = vec![];
        for _ in 0..5 {
            at = tab_stop_all(at, false, true, true, false, true, &[]);
            seen.push(at);
        }
        assert_eq!(
            seen,
            [
                Some(Credit),
                Some(AddFiles),
                Some(Clear),
                Some(Convert),
                Some(Help)
            ]
        );
        assert_eq!(
            tab_stop_all(Some(AddFiles), true, true, true, false, true, &[]),
            Some(Credit)
        );
        assert_eq!(
            tab_stop_all(Some(Credit), true, true, true, false, true, &[]),
            None
        );
        // Not shown, it isn't a stop.
        assert_eq!(
            tab_stop_all(None, false, true, true, false, false, &[]),
            Some(AddFiles)
        );
        // The empty window: the last stop, after the first-run card.
        assert_eq!(
            empty_tab_stop_all(Some(Dismiss), false, true, false, true),
            Some(Credit)
        );
        assert_eq!(
            empty_tab_stop_all(Some(AddFiles), false, false, false, true),
            Some(Credit)
        );
        assert_eq!(
            empty_tab_stop_all(Some(Credit), false, false, false, true),
            Some(Help)
        );
        assert_eq!(
            empty_tab_stop_all(Some(Help), true, false, false, true),
            Some(Credit)
        );
        assert_eq!(
            empty_tab_stop_all(Some(AddFiles), false, false, false, false),
            Some(Help)
        );
    }

    #[test]
    fn tab_runs_header_list_inspector_footer() {
        use crate::inspector::InspectorControl as C;
        use FocusButton::*;
        let inspector = [C::Target(0), C::More];
        let go = |at, back| tab_stop(at, back, true, true, &inspector);
        // From the list: the inspector's controls, then the footer.
        assert_eq!(go(None, false), Some(Inspector(C::Target(0))));
        assert_eq!(
            go(Some(Inspector(C::Target(0))), false),
            Some(Inspector(C::More))
        );
        assert_eq!(go(Some(Inspector(C::More)), false), Some(AddFiles));
        // Shift+Tab goes back through them to the list.
        assert_eq!(go(Some(AddFiles), true), Some(Inspector(C::More)));
        assert_eq!(
            go(Some(Inspector(C::More)), true),
            Some(Inspector(C::Target(0)))
        );
        assert_eq!(go(Some(Inspector(C::Target(0))), true), None);
        // The whole round, with Convert off, from Help.
        let mut at = Some(Help);
        let mut seen = vec![];
        for _ in 0..8 {
            at = tab_stop(at, false, true, false, &inspector);
            seen.push(at);
        }
        assert_eq!(
            seen,
            [
                Some(Settings),
                None,
                Some(Inspector(C::Target(0))),
                Some(Inspector(C::More)),
                Some(AddFiles),
                Some(Clear),
                Some(Help),
                Some(Settings),
            ]
        );
        // A control that is gone hands the focus back to the list.
        assert_eq!(
            tab_stop(Some(Inspector(C::Retry)), false, true, true, &inspector),
            None
        );
        // The empty window has no inspector to stop at.
        assert_eq!(
            tab_stop(Some(Settings), false, false, false, &inspector),
            Some(AddFiles)
        );
    }

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(human_size(0), "0 bytes");
        assert_eq!(human_size(1), "1 byte");
        assert_eq!(human_size(999), "999 bytes");
        assert_eq!(human_size(14_200), "14 KB");
        assert_eq!(human_size(3_140_000), "3.1 MB");
        assert_eq!(human_size(2_000_000_000_000_000), "2000 TB");
    }
    #[test]
    fn notes_move_their_revision_with_every_change_and_only_then() {
        let mut notes = Notes::default();
        let a = PathBuf::from("a.png");
        let r0 = notes.revision();
        // Nothing to remove or clear: nothing changes.
        notes.remove(&a);
        notes.clear();
        assert_eq!(notes.revision(), r0);

        notes.insert(a.clone(), (GONE.to_owned(), Tone::Crack));
        let r1 = notes.revision();
        assert_ne!(r1, r0);
        assert!(notes.gone(&a));
        assert_eq!(notes.get(&a), Some(&(GONE.to_owned(), Tone::Crack)));
        // The same note again is no change.
        notes.insert(a.clone(), (GONE.to_owned(), Tone::Crack));
        assert_eq!(notes.revision(), r1);
        // Another note for the same row is.
        notes.insert(
            a.clone(),
            ("Couldn't save to photos.".to_owned(), Tone::Crack),
        );
        let r2 = notes.revision();
        assert_ne!(r2, r1);
        assert!(!notes.gone(&a));

        notes.remove(&a);
        assert_ne!(notes.revision(), r2);
        assert!(notes.get(&a).is_none());
        notes.insert(a, (GONE.to_owned(), Tone::Crack));
        let r3 = notes.revision();
        notes.clear();
        assert_ne!(notes.revision(), r3);
        assert!(notes.by_file().is_empty());
    }

    // Task 18: rows in the new look (mock screens 2, 5 and 8b).

    /// `name` in `C:\v`, as a row of `code` and `size`, with `facts`.
    fn row18(name: &str, code: &str, size: u64, facts: Option<Facts>) -> Entry {
        let mut e = row(name);
        e.path = PathBuf::from(r"C:\v").join(name);
        e.folder = r"C:\v".to_owned();
        e.code = code.to_owned();
        e.size = size;
        e.facts = facts;
        e
    }

    #[test]
    fn a_rows_facts_say_what_it_is_then_its_size() {
        let photo = picture(facts("jpg", Kind::Image), 3000, 2000, Look::Photo);
        let e = row18("friends-portrait.jpg", "JPG", 3_200_000, Some(photo));
        assert_eq!(
            e.line(),
            ("3000 × 2000 photo · 3.2 MB".to_owned(), Tone::Muted)
        );
        let video = picture(
            lasting(facts("mp4", Kind::Video), 20.0),
            1920,
            800,
            Look::Photo,
        );
        let e = row18("tos-1080p.mp4", "MP4", 31_000_000, Some(video));
        assert_eq!(e.line().0, "1920 × 800 video · 0:20 · 31 MB");
        let sound = heard("mp3", "MP3", Some(128), None, Some(44_100), Some(60.0));
        let e = row18("concert-60s.mp3", "MP3", 1_400_000, Some(sound));
        assert_eq!(e.line().0, "Sound · 1:00 · 1.4 MB");
        let e = row18("scan-2019.pdf", "PDF", 57_000, Some(pages(3, Some(true))));
        assert_eq!(e.line().0, "PDF · 3 pages · 57 KB");
        let docx = facts("docx", Kind::Document);
        let e = row18("quarterly-report.docx", "DOCX", 11_000, Some(docx));
        assert_eq!(e.line().0, "Word document · 11 KB");
        // Before its facts are in: its type and size. Never the folder, and
        // never a suggestion: the side pane puts that first.
        let mut e = row18("skyline.png", "PNG", 445_000, None);
        assert_eq!(e.line().0, "PNG · 445 KB");
        e.suggestion = Some(Suggestion {
            action: Suggested::Convert("webp".into()),
            reason: String::new(),
            estimate: Some(90_000),
        });
        e.facts = Some(picture(facts("png", Kind::Image), 1600, 682, Look::Photo));
        assert_eq!(e.line().0, "1600 × 682 photo · 445 KB");
        // A screen reader hears the same, labelled.
        assert_eq!(
            e.spoken_detail(),
            "1600 by 682 photo. File size 445 KB. Folder v"
        );
    }

    #[test]
    fn a_converting_row_says_what_it_is_making_and_shows_its_bar() {
        let mut e = row18("tos-1080p.mp4", "MP4", 31_000_000, None);
        e.job = JobState::Running {
            action: Action::Convert("mp4".into()),
            fraction: Some(0.38),
            started: None,
        };
        // The percentage and the time left are at the right and in the
        // side pane, not in the line.
        assert_eq!(e.line(), ("Converting to MP4".to_owned(), Tone::Muted));
        assert_eq!(e.mark(), RowMark::Progress(Some(0.38)));
        assert_eq!(percent_text(0.38), "38%");
        e.job = JobState::Running {
            action: Action::Convert("mp4".into()),
            fraction: None,
            started: None,
        };
        assert_eq!(e.mark(), RowMark::Progress(None));
    }

    #[test]
    fn a_saved_row_names_the_new_file_and_how_much_smaller_it_is() {
        let mut e = row18("tos-1080p.mp4", "MP4", 31_000_000, None);
        e.job = JobState::Done {
            action: Action::Convert("mp4".into()),
            outputs: vec![PathBuf::from(r"C:\v\tos-1080p (1).mp4")],
            saved: Some(Saved::Folder(PathBuf::from(r"C:\v"))),
        };
        e.result_bytes = Some(9_600_000);
        assert_eq!(
            e.line(),
            (
                "Saved as tos-1080p (1).mp4 · 31 MB → 9.6 MB, 69% smaller".to_owned(),
                Tone::Result
            )
        );
        assert_eq!(e.mark(), RowMark::Done);
        assert_eq!(e.link(false), Some(SHOW_IN_FOLDER));
        // Show in folder stays on the row while the side pane shows.
        assert!(link_on_row(SHOW_IN_FOLDER, false));
        assert!(!link_on_row("Show details", false));
        assert!(link_on_row("Show details", true));
        // Saved in another folder, it says the same: Show in folder goes there.
        e.job = JobState::Done {
            action: Action::Convert("mp4".into()),
            outputs: vec![PathBuf::from(r"D:\out\tos-1080p.mp4")],
            saved: Some(Saved::Folder(PathBuf::from(r"D:\out"))),
        };
        assert_eq!(
            e.line().0,
            "Saved as tos-1080p.mp4 · 31 MB → 9.6 MB, 69% smaller"
        );
        // With the result's size unknown, just its name.
        e.result_bytes = None;
        assert_eq!(e.line().0, "Saved as tos-1080p.mp4");
        // Still held:
        e.job = JobState::Done {
            action: Action::Convert("mp4".into()),
            outputs: vec![PathBuf::from(r"C:\held\tos-1080p.mp4")],
            saved: None,
        };
        e.result_bytes = Some(9_600_000);
        // On its way to its folder, it says so until it is there.
        assert_eq!(e.line().0, "Saving…");
        assert_eq!(e.link(false), None);
        assert_eq!(e.mark(), RowMark::Done);
    }

    #[test]
    fn a_saved_row_has_a_short_result_for_a_narrow_list() {
        // the name is on the line above, so the result drops it and the
        // size it came from: "Saved · 29 MB, about the same size".
        let mut e = row18("tos-1080p.mov", "MOV", 29_000_000, None);
        assert_eq!(e.short_line(), None, "nothing saved yet");
        e.job = JobState::Done {
            action: Action::Convert("mov".into()),
            outputs: vec![PathBuf::from(r"C:\v\tos-1080p (7).mov")],
            saved: Some(Saved::Folder(PathBuf::from(r"C:\v"))),
        };
        e.result_bytes = Some(29_000_000);
        assert_eq!(
            e.line().0,
            "Saved as tos-1080p (7).mov · 29 MB → 29 MB, about the same size"
        );
        assert_eq!(
            e.short_line().as_deref(),
            Some("Saved · 29 MB, about the same size")
        );
        e.size = 31_000_000;
        e.result_bytes = Some(9_600_000);
        assert_eq!(
            e.short_line().as_deref(),
            Some("Saved · 9.6 MB, 69% smaller")
        );
        // The result's size not read yet: just that it is saved.
        e.result_bytes = None;
        assert_eq!(e.short_line().as_deref(), Some("Saved"));
        // The job's note still follows it.
        e.job_note = Some("2 files left out".to_owned());
        assert_eq!(e.short_line().as_deref(), Some("Saved · 2 files left out"));
        // A result not saved says what it is, as the full line does.
        e.job = JobState::Done {
            action: Action::Convert("mov".into()),
            outputs: vec![PathBuf::from(r"C:\held.mov")],
            saved: None,
        };
        assert_eq!(e.short_line(), None);
    }

    #[test]
    fn a_files_size_is_split_from_the_reason_it_has_nothing_to_offer() {
        // the reason is in the critical colour, the size after it in
        // the secondary one, so the row draws them as two parts.
        let mut e = row18("clip.jpg", "JPG", 2_700_000, None);
        e.check = FileCheck::CutShort;
        let (line, tone) = e.line();
        assert_eq!(tone, Tone::Crack);
        assert_eq!(
            line,
            "Looks cut short. Try copying or downloading it again. · 2.7 MB"
        );
        let (head, tail) = e.line_parts();
        assert_eq!(
            head,
            "Looks cut short. Try copying or downloading it again."
        );
        assert_eq!(tail.as_deref(), Some(" · 2.7 MB"));
        assert_eq!(format!("{head}{}", tail.unwrap()), line);
        // Nothing in it: no size to split off. A plain line is all head.
        e.check = FileCheck::Empty;
        assert_eq!(e.line_parts().1, None);
        let ok = row18("a.jpg", "JPG", 3_200_000, None);
        assert_eq!(ok.line_parts(), (ok.line().0, None));
    }

    #[test]
    fn a_failed_row_says_what_happened_and_what_to_do_next() {
        let failed = |message: &str| JobState::Failed {
            message: message.to_owned(),
            details: String::new(),
            engine: None,
            store_product: None,
        };
        // UX-13: a damaged video, sound file or PDF says so plainly, with
        // somewhere else to try it, and no Retry: it would fail again.
        let video = picture(
            lasting(facts("mp4", Kind::Video), 20.0),
            1920,
            800,
            Look::Photo,
        );
        let mut e = row18("clip.mp4", "MP4", 31_000_000, Some(video));
        e.last = Some(Action::Convert("webm".into()));
        e.job = failed("This MP4 is damaged and can't be read.");
        assert_eq!(
            e.line(),
            (
                "This video looks damaged. Try playing it in another app.".to_owned(),
                Tone::Crack
            )
        );
        assert_eq!(e.failure_title(), "Couldn't read this video");
        assert!(!e.retryable());
        assert_eq!(e.mark(), RowMark::Failed);
        // Show details is always there, even with nothing more from the
        // engine than the sentence.
        assert_eq!(e.link(false), Some("Show details"));
        assert_eq!(
            e.details(false),
            Some((
                "What happened".to_owned(),
                "Kiln couldn't read clip.mp4. It looks damaged. Try playing it in another app."
                    .to_owned(),
                String::new()
            ))
        );
        let sound = heard("wav", "PCM", None, Some(16), Some(44_100), Some(2.0));
        let mut e = row18("corrupt.wav", "WAV", 21, Some(sound));
        e.last = Some(Action::Convert("mp3".into()));
        e.job = failed("This WAV is damaged and can't be read.");
        assert_eq!(
            e.line().0,
            "This sound file looks damaged. Try playing it in another app."
        );
        assert_eq!(e.failure_title(), "Couldn't read this sound file");
        let mut e = row18("scan-2019.pdf", "PDF", 57_000, None);
        e.last = Some(Action::Convert("docx".into()));
        e.job = failed(
            "This file looks damaged, or isn't really a PDF. Try opening it in another app.",
        );
        assert_eq!(
            e.line().0,
            "This PDF looks damaged, or isn't really a PDF. Try opening it in another app."
        );
        assert_eq!(e.failure_title(), "Couldn't read this PDF");
        assert!(!e.retryable());
        // A job that stopped can pass a second time: Retry.
        e.job = failed("The conversion stopped before it finished. Try again.");
        assert_eq!(
            e.line().0,
            "The conversion stopped before it finished. Try again."
        );
        assert_eq!(e.failure_title(), "Couldn't convert this file");
        assert!(e.retryable());
        assert_eq!(e.link(false), Some("Show details"));
    }

    #[test]
    fn results_are_saved_never_exported() {
        // Every string in the app's own sources says Save, not Export
        // (spec 2.4). Paths, such as a folder called Exports in a test,
        // aren't words a person reads.
        let word = "xport";
        let mut found = vec![];
        let mut stack = vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|x| x.to_str()) != Some("rs") {
                    continue;
                }
                let text = fs::read_to_string(&path).unwrap();
                for (n, line) in text.lines().enumerate() {
                    if line.trim_start().starts_with("//") {
                        continue;
                    }
                    for literal in string_literals(line) {
                        let says = literal.contains(&format!("E{word}"))
                            || literal.contains(&format!("e{word}"));
                        // Kiln's own working files' names aren't words either.
                        let file = literal.contains(".kiln-");
                        if says && !file && !literal.contains(['\\', '/']) {
                            found.push(format!("{}:{}: {literal}", path.display(), n + 1));
                        }
                    }
                }
            }
        }
        assert!(found.is_empty(), "{}", found.join("\n"));
    }

    /// The string literals on one line of Rust: what is between each pair
    /// of unescaped double quotes outside a char literal.
    fn string_literals(line: &str) -> Vec<String> {
        let mut out = vec![];
        let mut chars = line.chars();
        let mut prev = ' ';
        while let Some(c) = chars.next() {
            if c == '"' && prev != '\'' {
                let mut s = String::new();
                let mut escaped = false;
                for d in chars.by_ref() {
                    if escaped {
                        escaped = false;
                    } else if d == '\\' {
                        escaped = true;
                    } else if d == '"' {
                        break;
                    }
                    s.push(d);
                }
                out.push(s);
            }
            prev = c;
        }
        out
    }

    // Task 18: the footer (mock screen 2) and UX-05.

    #[test]
    fn adding_five_files_selects_all_five() {
        // Two rows were there, the first selected; five more come in.
        let mut list = list_of(7);
        list.pointer_used();
        list.click(0);
        list.added(2, false);
        // Every file just added is selected, not only the first (UX-05),
        // and the first of them takes the focus and the anchor.
        assert_eq!(list.selection(), [2, 3, 4, 5, 6]);
        assert_eq!(list.focus(), Some(2));
        list.shift_click(3);
        assert_eq!(list.selection(), [2, 3]);
        // By a key: the ring shows on the first of them.
        let mut list = list_of(3);
        list.added(1, true);
        assert_eq!(list.selection(), [1, 2]);
        assert_eq!(list.visible_focus(), Some(1));
        // A walk's later shares join them.
        let mut list = list_of(6);
        list.click(1);
        list.also_added(3);
        assert_eq!(list.selection(), [1, 3, 4, 5]);
        assert_eq!(list.focus(), Some(1));
    }

    #[test]
    fn the_footers_convert_says_how_many_files() {
        assert_eq!(convert_label(0, None), "Convert");
        assert_eq!(convert_label(1, None), "Convert");
        assert_eq!(convert_label(5, None), "Convert 5 files");
        assert_eq!(convert_label(120, None), "Convert 120 files");
    }

    #[test]
    fn with_the_pane_the_footers_convert_says_what_it_converts_to() {
        // "Convert to JPG", for several "Convert 5 files to JPG".
        assert_eq!(convert_label(1, Some("jpg")), "Convert to JPG");
        assert_eq!(convert_label(1, Some(".webp")), "Convert to WEBP");
        assert_eq!(convert_label(0, Some("png")), "Convert to PNG");
        assert_eq!(convert_label(5, Some("jpg")), "Convert 5 files to JPG");
    }
}
