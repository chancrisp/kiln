//! The list of dropped files. Platform-independent so it can be tested
//! anywhere.

use std::path::{Path, PathBuf};

use kiln_core::{Kiln, Tool};

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
                (format!("{first}{more}"), Tone::Result)
            }
            JobState::Failed { message, .. } => (message.clone(), Tone::Crack),
            JobState::Cancelled => ("Cancelled.".to_owned(), Tone::Muted),
        }
    }

    /// The secondary line under the file name, before any job.
    pub fn detail(&self) -> String {
        let reach = match self.targets.len() {
            0 => "No conversions for this type".to_owned(),
            1 => format!("Converts to {}", self.targets[0]),
            n => format!("Converts to {n} formats"),
        };
        format!("{}  ·  {}  ·  {reach}", self.folder, human_size(self.size))
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
        assert_eq!(e.targets, ["JPG"]);
        assert!(
            e.detail().ends_with("1.5 KB  ·  Converts to JPG"),
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
        };
        assert_eq!(e.line(), ("a.jpg".to_owned(), Tone::Result));
        e.job = JobState::Done {
            action: Action::Tool(Tool::Split),
            outputs: vec!["a part 1.wav".into(), "a part 2.wav".into()],
        };
        assert_eq!(e.line().0, "a part 1.wav and 1 more");
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
