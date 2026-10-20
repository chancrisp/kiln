//! The list of dropped files. Platform-independent so it can be tested
//! anywhere.

use std::path::{Path, PathBuf};
use std::time::Instant;

use kiln_core::{about, Facts, Kiln, Kind, Look, Suggested, Suggestion, Tool};

use crate::actions::FileAction;
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
    },
    Done {
        action: Action,
        outputs: Vec<PathBuf>,
        /// The folder the results were exported to. `None` means they are
        /// still held in Kiln's private folder.
        saved: Option<PathBuf>,
    },
    Failed {
        message: String,
        details: String,
    },
    Cancelled,
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

/// "Converting to JPG", "Trimming".
pub fn doing(action: &Action) -> String {
    match action {
        Action::Convert(ext) => format!("Converting to {}", ext.to_ascii_uppercase()),
        Action::Tool(t) => match t {
            Tool::Compress => "Compressing",
            Tool::Crop => "Cropping",
            Tool::Trim => "Trimming",
            Tool::Split => "Splitting",
            Tool::Merge => "Merging",
        }
        .to_owned(),
        Action::File(FileAction::Move) => "Moving".to_owned(),
        Action::File(a) => a.label().to_owned(),
    }
}

/// "Converted to JPG", "Trimmed".
pub fn done(action: &Action) -> String {
    match action {
        Action::Convert(ext) => format!("Converted to {}", ext.to_ascii_uppercase()),
        Action::Tool(t) => past(*t).to_owned(),
        Action::File(FileAction::Move) => "Moved".to_owned(),
        Action::File(a) => a.label().to_owned(),
    }
}

fn past(t: Tool) -> &'static str {
    match t {
        Tool::Compress => "Compressed",
        Tool::Crop => "Cropped",
        Tool::Trim => "Trimmed",
        Tool::Split => "Split",
        Tool::Merge => "Merged",
    }
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
    if let Action::File(_) = action {
        return format!("{} {}.", done(action), n(ok));
    }
    let merge = matches!(action, Action::Tool(Tool::Merge));
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
            Action::Tool(t) => format!("Couldn't {} {what}.", t.id()),
            Action::File(_) => unreachable!("handled above"),
        };
        if cancelled > 0 {
            s.push_str(&format!(" {cancelled} cancelled."));
        }
        return s;
    }
    let mut s = match action {
        Action::Tool(Tool::Merge) if ok > 0 => format!("Merged {} into one.", n(files)),
        Action::Tool(Tool::Merge) => "Couldn't merge the files.".to_owned(),
        Action::Convert(ext) if ok == files => {
            format!("Converted {} to {}.", n(ok), ext.to_ascii_uppercase())
        }
        Action::Convert(ext) => format!(
            "Converted {ok} of {} to {}.",
            n(files),
            ext.to_ascii_uppercase()
        ),
        Action::Tool(t) if ok == files => format!("{} {}.", past(*t), n(ok)),
        Action::Tool(t) => format!("{} {ok} of {}.", past(*t), n(files)),
        Action::File(_) => unreachable!("handled above"),
    };
    if !merge {
        if failed > 0 {
            s.push_str(&format!(" {failed} didn't work."));
        }
        if cancelled > 0 {
            s.push_str(&format!(" {cancelled} cancelled."));
        }
    }
    s
}

/// A held result that someone removed by hand.
pub const GONE: &str = "The result is no longer there. Convert it again.";

/// Footer after a batch that left `held` result files to export:
/// "Converted 3 files to JPG. Export to keep them." With nothing held,
/// the summary stands as it is.
pub fn with_export_hint(summary: String, held: usize) -> String {
    match held {
        0 => summary,
        1 => format!("{summary} Export to keep it."),
        _ => format!("{summary} Export to keep them."),
    }
}

/// The footer button: "Export all (3)".
pub fn export_all_label(held: usize) -> String {
    format!("Export all ({held})")
}

/// A row's line when exporting to `folder` failed.
pub fn save_failed_line(folder: &Path) -> String {
    format!(
        "Couldn't save to {}. Pick another folder.",
        folder.display()
    )
}

/// The footer after exporting `total` files, `saved` of which went through.
pub fn export_summary(saved: usize, total: usize) -> String {
    let n = |k: usize| format!("{k} file{}", if k == 1 { "" } else { "s" });
    if saved == total {
        format!("Saved {}.", n(saved))
    } else {
        format!(
            "Saved {saved} of {total}. {} couldn't be saved.",
            total - saved
        )
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
    ExportAll,
    /// Throw the held results away. Also stops any running jobs.
    Discard,
    /// Stop the running jobs and go ahead.
    Stop(Leave),
    Cancel,
}

impl PromptButton {
    pub fn label(self) -> &'static str {
        match self {
            PromptButton::ExportAll => "Export all",
            PromptButton::Discard => "Discard",
            PromptButton::Stop(Leave::Close) => "Stop and close",
            PromptButton::Stop(Leave::Clear) => "Stop and clear",
            PromptButton::Stop(Leave::Remove) => "Stop and remove",
            PromptButton::Cancel => "Cancel",
        }
    }
}

/// The question asked before closing, clearing or removing a row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    pub title: String,
    pub buttons: Vec<PromptButton>,
}

/// What to ask before `verb` loses `held` unexported results or stops
/// `running` jobs. `None` when nothing is at stake.
pub fn leave_prompt(held: usize, running: usize, verb: Leave) -> Option<Prompt> {
    let mut sentences = vec![];
    if held > 0 {
        sentences.push(match held {
            1 => "1 result isn't saved yet.".to_owned(),
            n => format!("{n} results aren't saved yet."),
        });
    }
    if running > 0 {
        sentences.push(match running {
            1 => "1 file is still converting.".to_owned(),
            n => format!("{n} files are still converting."),
        });
    }
    if sentences.is_empty() {
        return None;
    }
    let buttons = if held > 0 {
        vec![
            PromptButton::ExportAll,
            PromptButton::Discard,
            PromptButton::Cancel,
        ]
    } else {
        vec![PromptButton::Stop(verb), PromptButton::Cancel]
    };
    Some(Prompt {
        title: sentences.join(" "),
        buttons,
    })
}

/// Where an export dialog opens: the last folder exported to in this run,
/// else the Start exports in setting, else the source's own folder.
pub fn export_start(last: Option<&Path>, setting: Option<&Path>, source: &Path) -> PathBuf {
    last.or(setting)
        .map(Path::to_path_buf)
        .or_else(|| source.parent().map(Path::to_path_buf))
        .unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub folder: String,
    /// Format code shown in monospace: "PNG". Empty if there's no extension.
    pub code: String,
    pub size: u64,
    /// Codes Kiln can convert this file to, from formats.toml.
    pub targets: Vec<String>,
    pub job: JobState,
    /// What `inspect` found, once it has.
    pub facts: Option<Facts>,
    /// What Kiln puts forward for this file alone, from its facts.
    pub suggestion: Option<Suggestion>,
    /// When the facts arrived, for the second line's crossfade.
    pub facts_at: Option<Instant>,
}

impl Entry {
    /// The second line of the row and its tone, following the job.
    pub fn line(&self) -> (String, Tone) {
        match &self.job {
            JobState::Idle => (self.detail(), Tone::Muted),
            JobState::Running { action, .. } => (doing(action), Tone::Muted),
            JobState::Done {
                action: Action::File(FileAction::Move),
                ..
            } => (
                format!(
                    "Moved to {}",
                    crate::actions::folder_name(Path::new(&self.folder))
                ),
                Tone::Result,
            ),
            JobState::Done {
                saved: Some(folder),
                ..
            } => (
                format!("Saved to {}.", crate::actions::folder_name(folder)),
                Tone::Result,
            ),
            JobState::Done { outputs, .. } => {
                let first = outputs
                    .first()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let more = match outputs.len() {
                    0 | 1 => String::new(),
                    2 => " and 1 more".to_owned(),
                    k => format!(" and {} more", k - 1),
                };
                (format!("Ready: {first}{more}"), Tone::Result)
            }
            JobState::Failed { message, .. } => (message.clone(), Tone::Crack),
            JobState::Cancelled => ("Cancelled.".to_owned(), Tone::Muted),
        }
    }

    /// The results this row holds, if it holds any: a finished job that
    /// hasn't been exported. The Move file action's row isn't one.
    pub fn held(&self) -> Option<&[PathBuf]> {
        match &self.job {
            JobState::Done {
                action: Action::File(_),
                ..
            } => None,
            JobState::Done {
                outputs,
                saved: None,
                ..
            } => Some(outputs),
            _ => None,
        }
    }

    /// True when the row holds results and any of them is no longer
    /// there. `exists` says whether a path is still on disk.
    pub fn is_gone(&self, exists: impl Fn(&Path) -> bool) -> bool {
        self.held()
            .is_some_and(|outputs| outputs.iter().any(|p| !exists(p)))
    }

    /// [`Entry::line`], except a held result that has gone says so.
    pub fn line_checked(&self, exists: impl Fn(&Path) -> bool) -> (String, Tone) {
        if self.is_gone(exists) {
            (GONE.to_owned(), Tone::Crack)
        } else {
            self.line()
        }
    }

    /// The secondary line under the file name, before any job: what
    /// Kiln found in the file and what it suggests, once it knows.
    pub fn detail(&self) -> String {
        let description = self.facts.as_ref().and_then(describe);
        if description.is_none() && self.suggestion.is_none() {
            return self.plain_detail();
        }
        let mut parts = vec![self.folder.clone()];
        parts.extend(description);
        parts.push(human_size(self.size));
        if let Some(s) = &self.suggestion {
            let label = match &s.action {
                Suggested::Convert(ext) => ext.to_ascii_uppercase(),
                Suggested::Tool(t) => t.label().to_owned(),
            };
            parts.push(match s.estimate {
                Some(bytes) => format!("Suggested: {label}, about {}", about(bytes)),
                None => format!("Suggested: {label}"),
            });
        }
        parts.join("  ·  ")
    }

    /// The secondary line before facts arrive: folder, size and how many
    /// formats the file converts to.
    pub fn plain_detail(&self) -> String {
        let reach = match self.targets.len() {
            0 => "No conversions for this type".to_owned(),
            1 => format!("Converts to {}", self.targets[0]),
            n => format!("Converts to {n} formats"),
        };
        format!("{}  ·  {}  ·  {reach}", self.folder, human_size(self.size))
    }
}

/// What a file is, in a few words: "4032 × 3024 photo", "1280 × 720
/// video, 1:30", "audio, 3:42", "12 pages, no text". `None` when the
/// facts say nothing worth showing.
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
            match (frame, duration()) {
                (Some((w, h)), Some(d)) => Some(format!("{w} × {h} video, {d}")),
                (Some((w, h)), None) => Some(format!("{w} × {h} video")),
                (None, Some(d)) => Some(format!("video, {d}")),
                (None, None) => None,
            }
        }
        Kind::Audio => duration().map(|d| format!("audio, {d}")),
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

/// What happened when paths were added, for the status line.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Added {
    pub added: usize,
    pub duplicates: usize,
    /// Folders and paths that don't exist.
    pub skipped: usize,
}

#[derive(Debug, Default)]
pub struct FileList {
    entries: Vec<Entry>,
    selected: Option<usize>,
}

impl FileList {
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub fn add(&mut self, kiln: &Kiln, paths: impl IntoIterator<Item = PathBuf>) -> Added {
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
            self.entries.push(entry_for(kiln, path, meta.len()));
            result.added += 1;
        }
        result
    }

    pub fn select(&mut self, index: Option<usize>) {
        self.selected = index.filter(|&i| i < self.entries.len());
    }

    /// Move the selection by `delta` rows, clamped to the list.
    pub fn step(&mut self, delta: isize) {
        if self.entries.is_empty() {
            return;
        }
        let last = self.entries.len() as isize - 1;
        let next = match self.selected {
            None if delta >= 0 => 0,
            None => last,
            Some(i) => (i as isize + delta).clamp(0, last),
        };
        self.selected = Some(next as usize);
    }

    /// Remove the selected entry and keep a sensible selection.
    pub fn remove_selected(&mut self) -> Option<Entry> {
        let i = self.selected?;
        let removed = self.entries.remove(i);
        self.selected = if self.entries.is_empty() {
            None
        } else {
            Some(i.min(self.entries.len() - 1))
        };
        Some(removed)
    }

    /// The held results by row index: finished jobs not yet exported.
    pub fn held(&self) -> Vec<(usize, Vec<PathBuf>)> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| Some((i, e.held()?.to_vec())))
            .collect()
    }

    /// How many result files are held (not rows: a split counts its parts).
    pub fn held_count(&self) -> usize {
        self.entries
            .iter()
            .filter_map(Entry::held)
            .map(<[PathBuf]>::len)
            .sum()
    }

    pub fn entries_mut(&mut self) -> &mut [Entry] {
        &mut self.entries
    }

    pub fn index_of(&self, path: &Path) -> Option<usize> {
        self.entries.iter().position(|e| same_path(&e.path, path))
    }

    /// Point the entry for `old` at `new` after the file moved. Returns
    /// its index.
    pub fn relocate(&mut self, kiln: &Kiln, old: &Path, new: PathBuf) -> Option<usize> {
        let i = self.index_of(old)?;
        let e = &mut self.entries[i];
        let fresh = entry_for(kiln, new, e.size);
        e.path = fresh.path;
        e.name = fresh.name;
        e.folder = fresh.folder;
        Some(i)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.selected = None;
    }

    /// Store what `inspect` found for the row at `path`, and its
    /// suggestion; its second line fades over from now, unless the row
    /// already had facts, so a repeated result never fades again. False,
    /// changing nothing, when no row has that path.
    pub fn set_facts(&mut self, path: &Path, facts: Facts, suggestion: Option<Suggestion>) -> bool {
        let Some(i) = self.index_of(path) else {
            return false;
        };
        let e = &mut self.entries[i];
        if e.facts.is_none() {
            e.facts_at = Some(Instant::now());
        }
        e.facts = Some(facts);
        e.suggestion = suggestion;
        true
    }

    /// Drop every row's facts and suggestion, as when Suggestions is
    /// turned off.
    pub fn forget_facts(&mut self) {
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

fn entry_for(kiln: &Kiln, path: PathBuf, size: u64) -> Entry {
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
    let targets = kiln.targets_for(&path).iter().map(|f| f.code()).collect();
    Entry {
        path,
        name,
        folder,
        code,
        size,
        targets,
        job: JobState::Idle,
        facts: None,
        suggestion: None,
        facts_at: None,
    }
}

/// Windows paths are case-insensitive.
fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        a.as_os_str().eq_ignore_ascii_case(b.as_os_str())
    } else {
        a == b
    }
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
    use std::fs;

    fn setup() -> (tempfile::TempDir, Kiln) {
        (tempfile::tempdir().unwrap(), Kiln::new())
    }

    #[test]
    fn files_are_added_with_their_conversions() {
        let (dir, kiln) = setup();
        let png = dir.path().join("photo.png");
        fs::write(&png, [0u8; 1500]).unwrap();
        let mut list = FileList::default();
        let r = list.add(&kiln, [png.clone()]);
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
        assert!(
            e.detail().ends_with("1.5 KB  ·  Converts to 12 formats"),
            "{}",
            e.detail()
        );
    }

    #[test]
    fn duplicates_and_folders_are_skipped() {
        let (dir, kiln) = setup();
        let a = dir.path().join("a.mp3");
        fs::write(&a, b"x").unwrap();
        let mut list = FileList::default();
        list.add(&kiln, [a.clone()]);
        let r = list.add(
            &kiln,
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
        list.add(&kiln, [f]);
        assert!(list.entries()[0].targets.is_empty());
        assert!(list.entries()[0]
            .detail()
            .ends_with("No conversions for this type"));
    }

    #[test]
    fn selection_moves_and_survives_removal() {
        let (dir, kiln) = setup();
        let paths: Vec<_> = (0..3)
            .map(|i| {
                let p = dir.path().join(format!("{i}.wav"));
                fs::write(&p, b"x").unwrap();
                p
            })
            .collect();
        let mut list = FileList::default();
        list.add(&kiln, paths);
        list.step(1);
        assert_eq!(list.selected(), Some(0));
        list.step(5);
        assert_eq!(list.selected(), Some(2));
        list.remove_selected();
        assert_eq!(list.selected(), Some(1));
        list.select(Some(0));
        list.remove_selected();
        list.remove_selected();
        assert_eq!(list.selected(), None);
        assert!(list.is_empty());
        list.step(-1);
        assert_eq!(list.selected(), None);
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
        list.add(&kiln, [f]);
        let e = &mut list.entries_mut()[0];
        let jpg = Action::Convert("jpg".into());
        e.job = JobState::Running {
            action: jpg.clone(),
            fraction: Some(0.5),
        };
        assert_eq!(e.line(), ("Converting to JPG".to_owned(), Tone::Muted));
        e.job = JobState::Done {
            action: jpg,
            outputs: vec![dir.path().join("a.jpg")],
            saved: None,
        };
        assert_eq!(e.line(), ("Ready: a.jpg".to_owned(), Tone::Result));
        e.job = JobState::Done {
            action: Action::Tool(Tool::Split),
            outputs: vec!["a part 1.wav".into(), "a part 2.wav".into()],
            saved: None,
        };
        assert_eq!(e.line().0, "Ready: a part 1.wav and 1 more");
        e.job = JobState::Failed {
            message: "FFmpeg isn't installed. Get it from Settings > Engines.".into(),
            details: String::new(),
        };
        assert_eq!(e.line().1, Tone::Crack);
    }

    #[test]
    fn batch_summaries_are_calm_and_specific() {
        let jpg = Action::Convert("jpg".into());
        assert_eq!(batch_summary(&jpg, 3, 3, 0, 0), "Converted 3 files to JPG.");
        assert_eq!(batch_summary(&jpg, 1, 1, 0, 0), "Converted 1 file to JPG.");
        assert_eq!(
            batch_summary(&jpg, 3, 2, 1, 0),
            "Converted 2 of 3 files to JPG. 1 didn't work."
        );
        assert_eq!(
            batch_summary(&jpg, 1, 0, 1, 0),
            "Couldn't convert the file to JPG."
        );
        assert_eq!(
            batch_summary(&jpg, 3, 0, 2, 1),
            "Couldn't convert 2 files to JPG. 1 cancelled."
        );
        assert_eq!(batch_summary(&jpg, 2, 0, 0, 2), "Cancelled.");
        let trim = Action::Tool(Tool::Trim);
        assert_eq!(batch_summary(&trim, 2, 2, 0, 0), "Trimmed 2 files.");
        assert_eq!(batch_summary(&trim, 1, 0, 1, 0), "Couldn't trim the file.");
        assert_eq!(
            batch_summary(&trim, 2, 1, 0, 1),
            "Trimmed 1 of 2 files. 1 cancelled."
        );
        let merge = Action::Tool(Tool::Merge);
        assert_eq!(
            batch_summary(&merge, 3, 1, 0, 0),
            "Merged 3 files into one."
        );
        assert_eq!(
            batch_summary(&merge, 3, 0, 1, 0),
            "Couldn't merge the files."
        );
        for s in [
            batch_summary(&jpg, 9, 0, 9, 0),
            batch_summary(&merge, 2, 1, 0, 0),
        ] {
            assert!(!s.contains('!'));
        }
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
        }
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
            targets: (0..12).map(|i| format!("F{i}")).collect(),
            job: JobState::Idle,
            facts: None,
            suggestion: None,
            facts_at: None,
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
                "1280 × 720 video, 1:30",
                "audio, 3:42",
                "12 pages",
                "1 page",
                "12 pages, no text",
            ]
            .map(|s| Some(s.to_owned()))
        );
        assert_eq!(describe(&facts("txt", Kind::Document)), None);
    }

    #[test]
    fn row_line_before_and_after_facts() {
        let mut e = row("a.png");
        assert_eq!(e.detail(), "photos  ·  3.1 MB  ·  Converts to 12 formats");
        e.facts = Some(photo());
        e.suggestion = Some(Suggestion {
            action: Suggested::Convert("jpg".into()),
            reason: String::new(),
            estimate: Some(2_040_000),
        });
        assert_eq!(
            e.detail(),
            "photos  ·  4032 × 3024 photo  ·  3.1 MB  ·  Suggested: JPG, about 2 MB"
        );
        assert_eq!(e.line(), (e.detail(), Tone::Muted));
        assert_eq!(
            e.plain_detail(),
            "photos  ·  3.1 MB  ·  Converts to 12 formats"
        );
        e.suggestion = None;
        assert_eq!(e.detail(), "photos  ·  4032 × 3024 photo  ·  3.1 MB");

        let mut txt = row("notes.txt");
        txt.facts = Some(facts("txt", Kind::Document));
        assert_eq!(txt.detail(), "photos  ·  3.1 MB  ·  Converts to 12 formats");
    }

    #[test]
    fn row_shows_tool_when_no_convert() {
        let mut e = row("clip.mp4");
        e.facts = Some(facts("mp4", Kind::Video));
        e.suggestion = Some(Suggestion {
            action: Suggested::Tool(Tool::Compress),
            reason: String::new(),
            estimate: Some(183_400_000),
        });
        assert!(
            e.detail()
                .ends_with("  ·  Suggested: Compress, about 180 MB"),
            "{}",
            e.detail()
        );
    }

    #[test]
    fn facts_for_a_removed_row_are_dropped() {
        let (dir, kiln) = setup();
        let a = dir.path().join("a.png");
        fs::write(&a, b"x").unwrap();
        let mut list = FileList::default();
        list.add(&kiln, [a]);
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
        list.add(&kiln, paths);
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

    fn held_row(name: &str, outputs: &[&str], saved: Option<&str>) -> Entry {
        let mut e = row(name);
        e.job = JobState::Done {
            action: Action::Convert("jpg".into()),
            outputs: outputs.iter().map(PathBuf::from).collect(),
            saved: saved.map(PathBuf::from),
        };
        e
    }

    #[test]
    fn held_rows_say_ready() {
        let e = held_row("a.png", &["held/photo.jpg"], None);
        assert_eq!(e.line(), ("Ready: photo.jpg".to_owned(), Tone::Result));
        let e = held_row("a.pdf", &["p 1.png", "p 2.png"], None);
        assert_eq!(e.line().0, "Ready: p 1.png and 1 more");
        let e = held_row("a.pdf", &["p 1.png", "p 2.png", "p 3.png"], None);
        assert_eq!(e.line().0, "Ready: p 1.png and 2 more");
    }

    #[test]
    fn saved_rows_name_the_folder() {
        let e = held_row("a.png", &["held/photo.jpg"], Some(r"C:\Users\me\Pictures"));
        assert_eq!(e.line(), ("Saved to Pictures.".to_owned(), Tone::Result));
        let e = held_row("a.png", &["held/photo.jpg"], Some(r"D:\"));
        assert_eq!(e.line().0, r"Saved to D:\.");
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
        // The Move action's row isn't a held result.
        let mut moved = row("a.png");
        moved.job = JobState::Done {
            action: Action::File(FileAction::Move),
            outputs: vec!["Pictures/a.png".into()],
            saved: None,
        };
        assert!(!moved.is_gone(|_| false));
        assert_eq!(moved.line_checked(|_| false), moved.line());
    }

    #[test]
    fn held_counts_files_not_rows() {
        let mut moved = row("m.png");
        moved.job = JobState::Done {
            action: Action::File(FileAction::Move),
            outputs: vec!["Pictures/m.png".into()],
            saved: None,
        };
        let list = FileList {
            entries: vec![
                held_row("a.png", &["a.jpg"], None),
                row("idle.png"),
                held_row("b.pdf", &["p 1.png", "p 2.png", "p 3.png"], None),
                held_row("c.png", &["c.jpg"], Some("Pictures")),
                moved,
            ],
            selected: None,
        };
        assert_eq!(
            list.held(),
            vec![
                (0, vec![PathBuf::from("a.jpg")]),
                (
                    2,
                    vec![
                        PathBuf::from("p 1.png"),
                        PathBuf::from("p 2.png"),
                        PathBuf::from("p 3.png")
                    ]
                ),
            ]
        );
        assert_eq!(list.held_count(), 4);
        assert_eq!(FileList::default().held_count(), 0);
        assert!(FileList::default().held().is_empty());
    }

    #[test]
    fn the_footer_points_at_export() {
        let jpg = Action::Convert("jpg".into());
        let s = batch_summary(&jpg, 3, 3, 0, 0);
        assert_eq!(
            with_export_hint(s.clone(), 3),
            "Converted 3 files to JPG. Export to keep them."
        );
        assert_eq!(
            with_export_hint(batch_summary(&jpg, 1, 1, 0, 0), 1),
            "Converted 1 file to JPG. Export to keep it."
        );
        // Nothing held, nothing to export.
        assert_eq!(with_export_hint(s, 0), "Converted 3 files to JPG.");
    }

    #[test]
    fn export_copy_is_plain() {
        assert_eq!(export_all_label(3), "Export all (3)");
        assert_eq!(
            save_failed_line(Path::new(r"D:\Locked")),
            r"Couldn't save to D:\Locked. Pick another folder."
        );
        assert_eq!(export_summary(5, 5), "Saved 5 files.");
        assert_eq!(export_summary(1, 1), "Saved 1 file.");
        assert_eq!(export_summary(4, 5), "Saved 4 of 5. 1 couldn't be saved.");
        assert_eq!(export_summary(0, 2), "Saved 0 of 2. 2 couldn't be saved.");
    }

    fn prompt(held: usize, running: usize, verb: Leave) -> Option<(String, Vec<PromptButton>)> {
        leave_prompt(held, running, verb).map(|p| (p.title, p.buttons))
    }

    #[test]
    fn nothing_to_lose_means_no_prompt() {
        for verb in [Leave::Close, Leave::Clear, Leave::Remove] {
            assert_eq!(leave_prompt(0, 0, verb), None);
        }
    }

    #[test]
    fn held_results_ask_export_discard_or_cancel() {
        let keep = vec![
            PromptButton::ExportAll,
            PromptButton::Discard,
            PromptButton::Cancel,
        ];
        for verb in [Leave::Close, Leave::Clear, Leave::Remove] {
            assert_eq!(
                prompt(3, 0, verb),
                Some(("3 results aren't saved yet.".to_owned(), keep.clone()))
            );
            assert_eq!(
                prompt(1, 0, verb),
                Some(("1 result isn't saved yet.".to_owned(), keep.clone()))
            );
        }
    }

    #[test]
    fn running_jobs_ask_to_stop() {
        for (verb, label) in [
            (Leave::Close, "Stop and close"),
            (Leave::Clear, "Stop and clear"),
            (Leave::Remove, "Stop and remove"),
        ] {
            let (title, buttons) = prompt(0, 2, verb).unwrap();
            assert_eq!(title, "2 files are still converting.");
            assert_eq!(buttons, [PromptButton::Stop(verb), PromptButton::Cancel]);
            assert_eq!(buttons[0].label(), label);
            assert_eq!(prompt(0, 1, verb).unwrap().0, "1 file is still converting.");
        }
    }

    #[test]
    fn both_at_once_is_one_prompt() {
        for verb in [Leave::Close, Leave::Clear, Leave::Remove] {
            assert_eq!(
                prompt(3, 2, verb),
                Some((
                    "3 results aren't saved yet. 2 files are still converting.".to_owned(),
                    vec![
                        PromptButton::ExportAll,
                        PromptButton::Discard,
                        PromptButton::Cancel
                    ]
                ))
            );
        }
        assert_eq!(
            prompt(1, 1, Leave::Close).unwrap().0,
            "1 result isn't saved yet. 1 file is still converting."
        );
    }

    #[test]
    fn prompt_buttons_are_labelled_in_sentence_case() {
        let labels: Vec<_> = [
            PromptButton::ExportAll,
            PromptButton::Discard,
            PromptButton::Cancel,
        ]
        .iter()
        .map(|b| b.label())
        .collect();
        assert_eq!(labels, ["Export all", "Discard", "Cancel"]);
    }

    #[test]
    fn exports_start_where_the_user_was() {
        let last = Path::new(r"C:\Exports");
        let setting = Path::new(r"C:\Converted");
        let source = Path::new(r"C:\photos\a.png");
        assert_eq!(export_start(Some(last), Some(setting), source), last);
        assert_eq!(export_start(None, Some(setting), source), setting);
        assert_eq!(
            export_start(None, None, source),
            PathBuf::from(r"C:\photos")
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
}
