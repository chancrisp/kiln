//! What the inspector says. The window shows a panel beside the file list
//! for what is selected. This is that panel's content as plain data: the
//! words for each selection, its controls in reading order, and nothing
//! about where any of it is drawn.

use std::path::Path;

use kiln_core::{about, describe_format, Facts, Habits, Kind, Suggested};

use crate::a11y::percent;
use crate::model::{human_size, left_text, time_left, Entry, FileList, JobState};
use crate::wheel::{offer_sentence, Action, Offer, WheelKind};

/// A control in the inspector, by what it is. The same names stand for
/// the Tab order, the keyboard focus and what assistive tools are told.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InspectorControl {
    /// The nth target shown, from 0.
    Target(usize),
    /// "More formats…", which opens the format choice.
    More,
    Cancel,
    Save,
    ShowInFolder,
    Retry,
    ShowDetails,
    /// The nth kind of file offered when the selection shares no format:
    /// choosing it selects just those files.
    Kind(usize),
}

/// A format the inspector offers to convert the selection to.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    /// The extension, lowercase: "webp".
    pub ext: String,
    /// What the format is for, in a few words: "a small image for the web".
    pub why: Option<&'static str>,
    /// The likely size of the result in bytes, where Kiln has an estimate.
    pub estimate: Option<u64>,
    /// Whether this is what Kiln suggests for the file.
    pub suggested: bool,
}

/// What the inspector says for the selection.
#[derive(Debug, Clone, PartialEq)]
pub enum Inspector {
    Nothing,
    /// One file that nothing has been done to.
    Waiting {
        row: usize,
        /// What the format is, in a few words.
        what: Option<&'static str>,
        /// Labelled facts, in reading order: Size, Pixels, Length, Folder.
        /// Size and Folder are always there; the others when known.
        facts: Vec<(&'static str, String)>,
        /// At most [`MAX_TARGETS`], the suggestion first.
        targets: Vec<Target>,
        /// Whether there are more formats than `targets` shows.
        more: bool,
    },
    Converting {
        row: usize,
        /// Whole percent, once the engine can say how far along it is.
        percent: Option<u32>,
        /// "about 2 min", once there is enough to go on.
        left: Option<String>,
    },
    Done {
        row: usize,
        /// The result's file name.
        result: String,
        /// "2.4 MB → 410 KB, 83% smaller", where both sizes are known.
        change: Option<String>,
        /// Where the result is, and so what can be done with it.
        kept: Kept,
    },
    /// A job that failed or was cancelled.
    Failed {
        row: usize,
        /// The row's whole sentence.
        sentence: String,
        /// Whether trying again may help.
        retry: bool,
        /// Whether there is more to show than the sentence.
        details: bool,
    },
    Several {
        /// The first three selected rows, for the stack of previews.
        rows: Vec<usize>,
        /// The kinds in words: "3 images, 1 video".
        kinds: String,
        /// The formats every selected file can become, none suggested.
        targets: Vec<Target>,
        /// When `targets` is empty: why, and what to do about it.
        none_shared: Option<String>,
        /// When `targets` is empty and there is more than one kind of file:
        /// each kind's type code and how many files, most files first and a
        /// tie in the order they were met. Empty otherwise.
        offered: Vec<(String, usize)>,
    },
}

/// Where a finished file's result is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kept {
    /// Still in Kiln's holding folder: it can be saved.
    Held,
    /// Saved or moved to a folder that can be shown.
    InFolder,
    /// Dragged out and dropped somewhere Kiln doesn't know: there is
    /// nothing to save and no folder to show.
    Dropped,
}

/// The most targets the inspector lists before More.
pub const MAX_TARGETS: usize = 3;

/// What the inspector can't tell from the list alone.
pub struct Lookups<'a> {
    /// What kind of file a row is.
    pub kind_of: &'a dyn Fn(&Entry) -> Option<Kind>,
    /// How big the file at a path is, if it can be read.
    pub size_of: &'a dyn Fn(&Path) -> Option<u64>,
}

/// What the inspector says for the files selected in `files`. `habits`
/// order the formats a file is offered after the suggestion.
pub fn inspect(files: &FileList, look: &Lookups, habits: &Habits) -> Inspector {
    let entries = files.entries();
    let rows: Vec<usize> = files
        .selection()
        .into_iter()
        .filter(|&i| i < entries.len())
        .collect();
    match rows.as_slice() {
        [] => Inspector::Nothing,
        &[row] => one(row, &entries[row], look, habits),
        _ => several(&rows, entries, look),
    }
}

fn one(row: usize, e: &Entry, look: &Lookups, habits: &Habits) -> Inspector {
    match &e.job {
        JobState::Idle => waiting(row, e, habits),
        JobState::Running {
            fraction, started, ..
        } => Inspector::Converting {
            row,
            percent: fraction.map(percent),
            left: started
                .zip(*fraction)
                .and_then(|(t, f)| time_left(t.elapsed(), f))
                .map(left_text),
        },
        JobState::Done {
            action,
            outputs,
            saved,
        } => {
            // A file action (Move) leaves nothing in the holding folder:
            // the file is where it went, and its size is no result to compare.
            let in_place = matches!(action, Action::File(_));
            let kept = if in_place {
                // The moved file's own folder.
                if e.folder.is_empty() {
                    Kept::Dropped
                } else {
                    Kept::InFolder
                }
            } else if e.saved().is_some() {
                Kept::InFolder
            } else if saved.is_some() {
                Kept::Dropped
            } else {
                Kept::Held
            };
            // Several results are no single size to set against the original.
            let change = match outputs.as_slice() {
                [only] if !in_place => (look.size_of)(only).map(|after| change_line(e.size, after)),
                _ => None,
            };
            Inspector::Done {
                row,
                // As the row names it, or the file itself when there is
                // nothing else to call it.
                result: e.results_named().unwrap_or_else(|| e.name.clone()),
                change,
                kept,
            }
        }
        // A cancelled file has the same way forward as a failed one.
        JobState::Failed { .. } | JobState::Cancelled => Inspector::Failed {
            row,
            sentence: e.line().0,
            retry: e.retryable(),
            details: e.details(false).is_some(),
        },
    }
}

/// `ext`, lowercase, once into `order` when the file can be converted to
/// it (`offered`).
fn take(order: &mut Vec<String>, offered: &[String], ext: &str) {
    let ext = ext.to_ascii_lowercase();
    if offered.contains(&ext) && !order.contains(&ext) {
        order.push(ext);
    }
}

fn waiting(row: usize, e: &Entry, habits: &Habits) -> Inspector {
    let facts: Option<&Facts> = e.facts.as_ref();
    let mut lines = vec![("Size", human_size(e.size))];
    if let Some(p) = facts.and_then(|f| f.picture.as_ref()) {
        lines.push(("Pixels", format!("{} × {}", p.width, p.height)));
    }
    let length = facts
        .and_then(|f| f.media.as_ref())
        .and_then(|m| m.duration)
        .filter(|d| *d > 0.0);
    if let Some(d) = length {
        lines.push(("Length", crate::quality::clock(d)));
    }
    lines.push(("Folder", e.folder_name()));

    // The suggestion first, then what is usually picked for this type of
    // file, then the rest as the row has them. The row lists codes
    // ("WEBP"); the rest of Kiln speaks extensions.
    let offered: Vec<String> = e.targets.iter().map(|t| t.to_ascii_lowercase()).collect();
    let suggested = match e.suggestion.as_ref().map(|s| &s.action) {
        Some(Suggested::Convert(ext)) => {
            Some(ext.to_ascii_lowercase()).filter(|ext| offered.contains(ext))
        }
        _ => None,
    };
    let mut order: Vec<String> = vec![];
    if let Some(ext) = &suggested {
        take(&mut order, &offered, ext);
    }
    // Habits are kept under the canonical type, so a .jpeg file's are
    // under "jpg": the facts know that, the row's code may not.
    let from = facts.map_or_else(|| e.code.to_ascii_lowercase(), |f| f.ext.clone());
    for ext in habits.ranked(&from) {
        take(&mut order, &offered, &ext);
    }
    for ext in &offered {
        take(&mut order, &offered, ext);
    }

    let more = order.len() > MAX_TARGETS;
    let targets = order
        .into_iter()
        .take(MAX_TARGETS)
        .map(|ext| {
            let is_suggestion = suggested.as_deref() == Some(ext.as_str());
            let estimate = e
                .suggestion
                .as_ref()
                .filter(|_| is_suggestion)
                .and_then(|s| s.estimate)
                .or_else(|| facts.and_then(|f| f.estimates.get(&ext).copied()));
            Target {
                why: describe_format(&ext),
                ext,
                estimate,
                suggested: is_suggestion,
            }
        })
        .collect();
    Inspector::Waiting {
        row,
        what: describe_format(&e.code),
        facts: lines,
        targets,
        more,
    }
}

fn several(rows: &[usize], entries: &[Entry], look: &Lookups) -> Inspector {
    let picked: Vec<&Entry> = rows.iter().map(|&i| &entries[i]).collect();
    let kinds: Vec<Option<Kind>> = picked.iter().map(|e| (look.kind_of)(e)).collect();

    // What every file can become, in the first file's order.
    let lower =
        |e: &Entry| -> Vec<String> { e.targets.iter().map(|t| t.to_ascii_lowercase()).collect() };
    let mut shared: Vec<String> = vec![];
    for ext in lower(picked[0]) {
        if !shared.contains(&ext) {
            shared.push(ext);
        }
    }
    for e in &picked[1..] {
        let theirs = lower(e);
        shared.retain(|ext| theirs.contains(ext));
    }

    // With nothing in common, the kinds present, most files first and a
    // tie in the order they were met, as the wheel says it. With only one
    // kind there is none else to narrow to, so none is offered.
    let (none_shared, offered) = if shared.is_empty() {
        let mut codes: Vec<(String, usize)> = vec![];
        for e in &picked {
            let code = if e.code.is_empty() { "Other" } else { &e.code };
            match codes.iter_mut().find(|(c, _)| c == code) {
                Some((_, n)) => *n += 1,
                None => codes.push((code.to_owned(), 1)),
            }
        }
        codes.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
        match codes.as_slice() {
            [(code, _)] => (
                offer_sentence(WheelKind::Convert, &Offer::Unsupported(code.clone())),
                vec![],
            ),
            _ => (
                offer_sentence(WheelKind::Convert, &Offer::Mixed(codes.clone())),
                codes,
            ),
        }
    } else {
        (None, vec![])
    };

    Inspector::Several {
        rows: rows.iter().copied().take(3).collect(),
        kinds: kinds_line(&kinds),
        targets: shared
            .into_iter()
            .take(MAX_TARGETS)
            .map(|ext| Target {
                why: describe_format(&ext),
                ext,
                estimate: None,
                suggested: false,
            })
            .collect(),
        none_shared,
        offered,
    }
}

impl Inspector {
    /// The controls it has, in reading order. That is the Tab order, and
    /// the order assistive tools meet them in.
    pub fn controls(&self) -> Vec<InspectorControl> {
        use InspectorControl as C;
        match self {
            Inspector::Nothing => vec![],
            Inspector::Waiting { targets, more, .. } => (0..targets.len())
                .map(C::Target)
                .chain((*more).then_some(C::More))
                .collect(),
            // The shared formats, or with none, each kind to narrow to.
            Inspector::Several {
                targets, offered, ..
            } => match targets.is_empty() {
                false => (0..targets.len()).map(C::Target).collect(),
                true => (0..offered.len()).map(C::Kind).collect(),
            },
            Inspector::Converting { .. } => vec![C::Cancel],
            Inspector::Done { kept, .. } => match kept {
                Kept::Held => vec![C::Save],
                Kept::InFolder => vec![C::ShowInFolder],
                Kept::Dropped => vec![],
            },
            Inspector::Failed { retry, details, .. } => (*retry)
                .then_some(C::Retry)
                .into_iter()
                .chain((*details).then_some(C::ShowDetails))
                .collect(),
        }
    }
}

impl Target {
    /// What a screen reader calls it: "Convert to WEBP, a small image for
    /// the web, about 410 KB".
    pub fn name(&self) -> String {
        let mut name = format!("Convert to {}", self.ext.to_ascii_uppercase());
        if let Some(why) = self.why {
            name.push_str(", ");
            name.push_str(why);
        }
        if let Some(bytes) = self.estimate {
            name.push_str(&format!(", about {}", about(bytes)));
        }
        name
    }
}

/// The kinds of file in a selection, in words, most first and a tie in the
/// order they were met: "3 images, 1 video". `None` is a file of no kind
/// Kiln knows.
pub fn kinds_line(kinds: &[Option<Kind>]) -> String {
    let mut counts: Vec<(Option<Kind>, usize)> = vec![];
    for &kind in kinds {
        match counts.iter_mut().find(|(k, _)| *k == kind) {
            Some((_, n)) => *n += 1,
            None => counts.push((kind, 1)),
        }
    }
    // The sort is stable, so a tie keeps the order they were met in.
    counts.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
    counts
        .iter()
        .map(|&(kind, n)| {
            let (one, many) = match kind {
                Some(Kind::Image) => ("image", "images"),
                Some(Kind::Video) => ("video", "videos"),
                Some(Kind::Audio) => ("audio file", "audio files"),
                Some(Kind::Document) => ("document", "documents"),
                Some(Kind::Spreadsheet) => ("spreadsheet", "spreadsheets"),
                Some(Kind::Presentation) => ("presentation", "presentations"),
                Some(Kind::Subtitle) => ("subtitle file", "subtitle files"),
                None => ("other file", "other files"),
            };
            format!("{n} {}", if n == 1 { one } else { many })
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// A result's size against the original's: "2.4 MB → 410 KB, 83% smaller".
/// Within one per cent either way it is "about the same size".
pub fn change_line(before: u64, after: u64) -> String {
    let sizes = format!("{} → {}", human_size(before), human_size(after));
    if before == 0 {
        // Nothing to take a share of.
        return sizes;
    }
    if u128::from(before.abs_diff(after)) * 100 <= u128::from(before) {
        return format!("{sizes}, about the same size");
    }
    let ratio = after as f64 / before as f64;
    if after < before {
        format!(
            "{sizes}, {}% smaller",
            ((1.0 - ratio) * 100.0).round() as u64
        )
    } else {
        format!(
            "{sizes}, {}% larger",
            ((ratio - 1.0) * 100.0).round() as u64
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Saved;
    use crate::wheel::Action;
    use kiln_core::{FileCheck, Look, Picture, Suggested, Suggestion};
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    /// A waiting row in "photos", with the targets given.
    fn entry(name: &str, code: &str, size: u64, targets: &[&str]) -> Entry {
        Entry {
            path: PathBuf::from("photos").join(name),
            name: name.to_owned(),
            folder: r"C:\photos".to_owned(),
            code: code.to_owned(),
            size,
            modified: None,
            targets: targets.iter().map(|t| (*t).to_owned()).collect(),
            check: FileCheck::Ok,
            missing: None,
            save_failed: false,
            job: JobState::Idle,
            facts: None,
            suggestion: None,
            facts_at: None,
            last: None,
        }
    }

    fn harbour_facts() -> Facts {
        Facts {
            ext: "png".to_owned(),
            kind: Kind::Image,
            bytes: 2_400_000,
            picture: Some(Picture {
                width: 1600,
                height: 1200,
                alpha: true,
                animated: false,
                look: Look::Graphic,
            }),
            media: None,
            pdf: None,
            bars: None,
            estimates: BTreeMap::from([
                ("avif".to_owned(), 380_000),
                ("jpg".to_owned(), 300_000),
                ("webp".to_owned(), 410_000),
            ]),
            compressed: None,
        }
    }

    /// harbour.png: 2.4 MB, 1600 × 1200, five targets, WEBP suggested.
    fn harbour() -> Entry {
        let mut e = entry(
            "harbour.png",
            "PNG",
            2_400_000,
            &["jpg", "webp", "avif", "gif", "pdf"],
        );
        e.facts = Some(harbour_facts());
        e.suggestion = Some(Suggestion {
            action: Suggested::Convert("webp".to_owned()),
            reason: "Small for the web.".to_owned(),
            estimate: Some(410_000),
        });
        e
    }

    fn kind_by_code(e: &Entry) -> Option<Kind> {
        match e.code.as_str() {
            "PNG" | "JPG" => Some(Kind::Image),
            "MP4" => Some(Kind::Video),
            "DOCX" => Some(Kind::Document),
            _ => None,
        }
    }

    fn no_size(_: &Path) -> Option<u64> {
        None
    }

    fn selecting(entries: Vec<Entry>, rows: &[usize]) -> FileList {
        let mut list = FileList::default();
        for (i, e) in entries.into_iter().enumerate() {
            list.insert(i, e);
        }
        list.select_only(rows);
        list
    }

    fn inspect_plain(list: &FileList, habits: &Habits) -> Inspector {
        let look = Lookups {
            kind_of: &kind_by_code,
            size_of: &no_size,
        };
        inspect(list, &look, habits)
    }

    fn picked(habits: &mut Habits, from: &str, to: &str, times: usize) {
        for _ in 0..times {
            habits.record(from, to);
        }
    }

    #[test]
    fn one_waiting_file_offers_the_suggestion_then_habits_then_the_rest() {
        let list = selecting(vec![harbour()], &[0]);
        let mut habits = Habits::default();
        picked(&mut habits, "png", "avif", 3);
        let i = inspect_plain(&list, &habits);
        let Inspector::Waiting {
            row,
            what,
            facts,
            targets,
            more,
        } = i
        else {
            panic!("{i:?}")
        };
        assert_eq!(row, 0);
        assert_eq!(what, Some("a lossless image with transparency"));
        assert_eq!(facts[0], ("Size", "2.4 MB".to_owned()));
        assert_eq!(facts[1], ("Pixels", "1600 × 1200".to_owned()));
        assert_eq!(facts.last().unwrap(), &("Folder", "photos".to_owned()));
        assert_eq!(facts.len(), 3);
        let exts: Vec<_> = targets.iter().map(|t| t.ext.as_str()).collect();
        assert_eq!(exts, ["webp", "avif", "jpg"]);
        assert!(targets[0].suggested && !targets[1].suggested && !targets[2].suggested);
        assert_eq!(targets[0].estimate, Some(410_000));
        // The others have the estimate the facts hold for them.
        assert_eq!(targets[1].estimate, Some(380_000));
        assert_eq!(targets[2].estimate, Some(300_000));
        assert!(more);
        assert_eq!(
            targets[0].name(),
            "Convert to WEBP, a small image for the web, about 410 KB"
        );
        assert_eq!(
            targets[1].name(),
            "Convert to AVIF, a very small, modern image, about 380 KB"
        );
    }

    #[test]
    fn a_target_is_named_with_what_it_has_to_say() {
        let target = |why, estimate| Target {
            ext: "xyz".to_owned(),
            why,
            estimate,
            suggested: false,
        };
        assert_eq!(target(None, None).name(), "Convert to XYZ");
        assert_eq!(
            target(None, Some(380_000)).name(),
            "Convert to XYZ, about 380 KB"
        );
        assert_eq!(
            target(Some("a made-up format"), None).name(),
            "Convert to XYZ, a made-up format"
        );
    }

    #[test]
    fn without_a_suggestion_or_habit_the_targets_keep_their_order() {
        let mut e = harbour();
        e.suggestion = None;
        e.facts = None;
        let list = selecting(vec![e], &[0]);
        let i = inspect_plain(&list, &Habits::default());
        let Inspector::Waiting {
            what,
            facts,
            targets,
            more,
            ..
        } = i
        else {
            panic!("{i:?}")
        };
        assert_eq!(what, Some("a lossless image with transparency"));
        // Without facts, only the size and the folder are known.
        let names: Vec<_> = facts.iter().map(|f| f.0).collect();
        assert_eq!(names, ["Size", "Folder"]);
        let exts: Vec<_> = targets.iter().map(|t| t.ext.as_str()).collect();
        assert_eq!(exts, ["jpg", "webp", "avif"]);
        assert!(targets.iter().all(|t| !t.suggested && t.estimate.is_none()));
        assert!(more);
        // Three targets or fewer are all shown, and there is no More.
        let mut e = harbour();
        e.targets.truncate(3);
        let list = selecting(vec![e], &[0]);
        let Inspector::Waiting { targets, more, .. } = inspect_plain(&list, &Habits::default())
        else {
            panic!()
        };
        assert_eq!(targets.len(), 3);
        assert!(!more);
    }

    #[test]
    fn a_suggestion_or_habit_the_file_cannot_take_is_left_out() {
        let mut e = harbour();
        e.suggestion = Some(Suggestion {
            action: Suggested::Convert("jxl".to_owned()),
            reason: String::new(),
            estimate: Some(100_000),
        });
        let list = selecting(vec![e], &[0]);
        let mut habits = Habits::default();
        picked(&mut habits, "png", "tiff", 4);
        picked(&mut habits, "png", "gif", 2);
        let Inspector::Waiting { targets, .. } = inspect_plain(&list, &habits) else {
            panic!()
        };
        let exts: Vec<_> = targets.iter().map(|t| t.ext.as_str()).collect();
        assert_eq!(exts, ["gif", "jpg", "webp"]);
        assert!(targets.iter().all(|t| !t.suggested));
    }

    #[test]
    fn targets_are_extensions_however_the_row_spells_its_codes() {
        // The list holds codes ("WEBP"); the inspector speaks extensions.
        let mut e = harbour();
        e.targets = ["JPG", "WEBP", "AVIF", "GIF", "PDF"]
            .map(str::to_owned)
            .to_vec();
        // A .jpeg file's habits are kept under the canonical type.
        e.code = "JPEG".to_owned();
        e.facts.as_mut().unwrap().ext = "jpg".to_owned();
        let list = selecting(vec![e], &[0]);
        let mut habits = Habits::default();
        picked(&mut habits, "jpg", "gif", 3);
        let Inspector::Waiting { targets, .. } = inspect_plain(&list, &habits) else {
            panic!()
        };
        let exts: Vec<_> = targets.iter().map(|t| t.ext.as_str()).collect();
        assert_eq!(exts, ["webp", "gif", "jpg"]);
        assert!(targets[0].suggested);
    }

    #[test]
    fn a_video_says_its_length_after_its_pixels() {
        let mut e = entry("clip.mp4", "MP4", 40_000_000, &["mkv"]);
        let mut f = harbour_facts();
        f.ext = "mp4".to_owned();
        f.kind = Kind::Video;
        f.media = Some(kiln_core::MediaInfo {
            duration: Some(134.0),
            width: Some(1920),
            height: Some(1080),
            fps: None,
            has_audio: true,
        });
        f.picture.as_mut().unwrap().width = 1920;
        f.picture.as_mut().unwrap().height = 1080;
        e.facts = Some(f);
        let list = selecting(vec![e], &[0]);
        let Inspector::Waiting { facts, .. } = inspect_plain(&list, &Habits::default()) else {
            panic!()
        };
        assert_eq!(
            facts,
            [
                ("Size", "40 MB".to_owned()),
                ("Pixels", "1920 × 1080".to_owned()),
                ("Length", "2:14".to_owned()),
                ("Folder", "photos".to_owned()),
            ]
        );
    }

    #[test]
    fn a_converting_file_shows_its_percent() {
        let running = |fraction, started| JobState::Running {
            action: Action::Convert("webp".to_owned()),
            fraction,
            started,
        };
        let inspect_job = |job| {
            let mut e = harbour();
            e.job = job;
            inspect_plain(&selecting(vec![e], &[0]), &Habits::default())
        };
        assert_eq!(
            inspect_job(running(Some(0.38), None)),
            Inspector::Converting {
                row: 0,
                percent: Some(38),
                left: None
            }
        );
        // Ten seconds in and 38% done: about 16 s to go.
        let started = Instant::now().checked_sub(Duration::from_secs(10));
        assert_eq!(
            inspect_job(running(Some(0.38), started)),
            Inspector::Converting {
                row: 0,
                percent: Some(38),
                left: Some("about 15 sec".to_owned())
            }
        );
        // An engine that can't say how far along it is.
        assert_eq!(
            inspect_job(running(None, started)),
            Inspector::Converting {
                row: 0,
                percent: None,
                left: None
            }
        );
    }

    #[test]
    fn a_done_file_shows_before_and_after() {
        assert_eq!(
            change_line(2_400_000, 410_000),
            "2.4 MB → 410 KB, 83% smaller"
        );
        assert_eq!(
            change_line(1_000_000, 1_120_000),
            "1.0 MB → 1.1 MB, 12% larger"
        );
        assert_eq!(
            change_line(1_000_000, 1_004_000),
            "1.0 MB → 1.0 MB, about the same size"
        );
        // Within one per cent either way is the same size.
        assert_eq!(
            change_line(1_000_000, 990_000),
            "1.0 MB → 990 KB, about the same size"
        );
        assert_eq!(
            change_line(1_000_000, 1_020_000),
            "1.0 MB → 1.0 MB, 2% larger"
        );
        // Nothing to compare against.
        assert_eq!(change_line(0, 5_000), "0 bytes → 5.0 KB");

        // A Done row, held: the output's name, and the size it has now.
        let out = PathBuf::from("held").join("harbour.webp");
        let mut e = harbour();
        e.job = JobState::Done {
            action: Action::Convert("webp".to_owned()),
            outputs: vec![out.clone()],
            saved: None,
        };
        let list = selecting(vec![e.clone()], &[0]);
        let seen = |p: &Path| (p == out).then_some(410_000);
        let look = Lookups {
            kind_of: &kind_by_code,
            size_of: &seen,
        };
        assert_eq!(
            inspect(&list, &look, &Habits::default()),
            Inspector::Done {
                row: 0,
                result: "harbour.webp".to_owned(),
                change: Some("2.4 MB → 410 KB, 83% smaller".to_owned()),
                kept: Kept::Held
            }
        );
        // Without the output's size there is no before and after.
        assert_eq!(
            inspect_plain(&list, &Habits::default()),
            Inspector::Done {
                row: 0,
                result: "harbour.webp".to_owned(),
                change: None,
                kept: Kept::Held
            }
        );
        let kept_of = |e: &Entry| {
            let list = selecting(vec![e.clone()], &[0]);
            let Inspector::Done { kept, .. } = inspect_plain(&list, &Habits::default()) else {
                panic!()
            };
            kept
        };
        // Exported to a folder: it can be shown there.
        e.job = JobState::Done {
            action: Action::Convert("webp".to_owned()),
            outputs: vec![out.clone()],
            saved: Some(Saved::Folder(PathBuf::from("photos"))),
        };
        assert_eq!(kept_of(&e), Kept::InFolder);
        // Dragged out and dropped somewhere Kiln doesn't know: no folder to
        // show, and nothing held to save.
        e.job = JobState::Done {
            action: Action::Convert("webp".to_owned()),
            outputs: vec![out.clone()],
            saved: Some(Saved::Dropped),
        };
        assert_eq!(kept_of(&e), Kept::Dropped);
        // A moved file has nothing held to save, no conversion to compare,
        // and a folder to show.
        e.job = JobState::Done {
            action: Action::File(crate::actions::FileAction::Move),
            outputs: vec![out.clone()],
            saved: None,
        };
        assert_eq!(kept_of(&e), Kept::InFolder);
        let list = selecting(vec![e.clone()], &[0]);
        let Inspector::Done { change, .. } = inspect(&list, &look, &Habits::default()) else {
            panic!()
        };
        assert_eq!(change, None);

        // Several results are named as the row names them, and are no one
        // size to set against the original.
        let many: Vec<PathBuf> = (1..=3)
            .map(|i| PathBuf::from("held").join(format!("harbour {i}.png")))
            .collect();
        e.job = JobState::Done {
            action: Action::Convert("png".to_owned()),
            outputs: many,
            saved: None,
        };
        let sized = |_: &Path| Some(1_000);
        let look = Lookups {
            kind_of: &kind_by_code,
            size_of: &sized,
        };
        let list = selecting(vec![e.clone()], &[0]);
        let Inspector::Done { result, change, .. } = inspect(&list, &look, &Habits::default())
        else {
            panic!()
        };
        assert_eq!(result, "harbour 1.png and 2 more");
        assert_eq!(result, e.results_named().unwrap());
        assert_eq!(change, None);
        // No outputs at all: the file's own name.
        e.job = JobState::Done {
            action: Action::Convert("png".to_owned()),
            outputs: vec![],
            saved: None,
        };
        let list = selecting(vec![e], &[0]);
        let Inspector::Done { result, .. } = inspect_plain(&list, &Habits::default()) else {
            panic!()
        };
        assert_eq!(result, "harbour.png");
    }
    #[test]
    fn a_failed_file_says_why_and_offers_retry_when_it_may_help() {
        let failed = |message: &str, last: Option<Action>, details: &str| {
            let mut e = harbour();
            e.job = JobState::Failed {
                message: message.to_owned(),
                details: details.to_owned(),
                engine: Some("FFmpeg"),
            };
            e.last = last;
            let want = (e.line().0, e.retryable());
            let list = selecting(vec![e], &[0]);
            (inspect_plain(&list, &Habits::default()), want)
        };
        let again = Some(Action::Convert("webp".to_owned()));
        let (i, (sentence, retry)) = failed(
            "The conversion stopped before it finished. Try again.",
            again.clone(),
            "",
        );
        assert!(retry);
        assert_eq!(
            i,
            Inspector::Failed {
                row: 0,
                sentence,
                retry: true,
                details: false
            }
        );
        // A damaged file would fail the same way: no Retry.
        let (i, (sentence, retry)) = failed(
            "This file looks damaged. Try opening it in another app.",
            again.clone(),
            "",
        );
        assert!(!retry);
        assert_eq!(
            i,
            Inspector::Failed {
                row: 0,
                sentence,
                retry: false,
                details: false
            }
        );
        // Nothing known to repeat: no Retry either.
        let (i, (_, retry)) = failed("Try again.", None, "");
        assert!(!retry);
        assert!(matches!(i, Inspector::Failed { retry: false, .. }));
        // The engine's own words are something to show.
        let (i, _) = failed(
            "The conversion stopped before it finished. Try again.",
            again.clone(),
            "Error while decoding stream #0:0",
        );
        let Inspector::Failed { details, .. } = &i else {
            panic!("{i:?}")
        };
        assert!(*details);
        assert_eq!(
            i.controls(),
            [InspectorControl::Retry, InspectorControl::ShowDetails]
        );
        // A cancelled file is shown the same way, and can be tried again.
        // It has nothing to show details of.
        let mut e = harbour();
        e.job = JobState::Cancelled;
        e.last = again;
        let list = selecting(vec![e], &[0]);
        let i = inspect_plain(&list, &Habits::default());
        assert_eq!(
            i,
            Inspector::Failed {
                row: 0,
                sentence: "Cancelled.".to_owned(),
                retry: true,
                details: false
            }
        );
        assert_eq!(i.controls(), [InspectorControl::Retry]);
    }
    #[test]
    fn several_files_name_their_kinds_most_first() {
        use Kind::*;
        assert_eq!(
            kinds_line(&[Some(Image), Some(Video), Some(Image), Some(Image)]),
            "3 images, 1 video"
        );
        assert_eq!(
            kinds_line(&[Some(Audio), None]),
            "1 audio file, 1 other file"
        );
        // A tie keeps the order the kinds were met in.
        assert_eq!(
            kinds_line(&[Some(Document), Some(Image), Some(Document), Some(Image)]),
            "2 documents, 2 images"
        );
        assert_eq!(
            kinds_line(&[
                Some(Spreadsheet),
                Some(Presentation),
                Some(Subtitle),
                Some(Subtitle),
                None,
                None,
                None
            ]),
            "3 other files, 2 subtitle files, 1 spreadsheet, 1 presentation"
        );
        assert_eq!(kinds_line(&[Some(Video), Some(Video)]), "2 videos");
    }

    #[test]
    fn several_files_show_up_to_three_and_their_shared_targets() {
        let png = |name: &str, targets: &[&str]| entry(name, "PNG", 1_000, targets);
        let list = selecting(
            vec![
                png("a.png", &["jpg", "webp", "avif", "gif"]),
                png("b.png", &["gif", "webp", "jpg", "avif", "pdf"]),
                png("c.png", &["avif", "jpg", "webp", "gif"]),
                png("d.png", &["webp", "gif", "jpg", "avif"]),
            ],
            &[0, 1, 2, 3],
        );
        let i = inspect_plain(&list, &Habits::default());
        let Inspector::Several {
            rows,
            kinds,
            targets,
            none_shared,
            offered,
        } = i
        else {
            panic!("{i:?}")
        };
        assert_eq!(rows, [0, 1, 2]);
        assert_eq!(kinds, "4 images");
        // The first row's order, the first three, none suggested.
        let exts: Vec<_> = targets.iter().map(|t| t.ext.as_str()).collect();
        assert_eq!(exts, ["jpg", "webp", "avif"]);
        assert!(targets.iter().all(|t| !t.suggested && t.estimate.is_none()));
        assert_eq!(
            targets[1].name(),
            "Convert to WEBP, a small image for the web"
        );
        assert_eq!(none_shared, None);
        // Sharing formats, there is no kind to choose between.
        assert!(offered.is_empty());
    }

    #[test]
    fn several_files_with_nothing_shared_explain_it_and_offer_each_kind() {
        let mp4 = |name: &str| entry(name, "MP4", 1_000, &["mkv", "webm"]);
        let docx = entry("c.docx", "DOCX", 1_000, &["pdf", "odt"]);
        let list = selecting(vec![mp4("a.mp4"), docx.clone(), mp4("b.mp4")], &[0, 1, 2]);
        let i = inspect_plain(&list, &Habits::default());
        let Inspector::Several {
            rows,
            kinds,
            targets,
            none_shared,
            offered,
        } = &i
        else {
            panic!("{i:?}")
        };
        assert_eq!(rows, &[0, 1, 2]);
        assert_eq!(kinds, "2 videos, 1 document");
        assert!(targets.is_empty());
        // The phase 3 sentence, with the kinds most files first.
        let mixed = Offer::Mixed(vec![("MP4".to_owned(), 2), ("DOCX".to_owned(), 1)]);
        assert_eq!(
            none_shared,
            &crate::wheel::offer_sentence(WheelKind::Convert, &mixed)
        );
        assert!(none_shared
            .as_ref()
            .unwrap()
            .starts_with("These files have no format in common"));
        // And each kind is offered, most files first, as a control.
        assert_eq!(offered, &[("MP4".to_owned(), 2), ("DOCX".to_owned(), 1)]);
        assert_eq!(
            i.controls(),
            [InspectorControl::Kind(0), InspectorControl::Kind(1)]
        );

        // One of each: the order they were met in.
        let list = selecting(vec![mp4("a.mp4"), docx.clone()], &[0, 1]);
        let Inspector::Several { offered, .. } = inspect_plain(&list, &Habits::default()) else {
            panic!()
        };
        assert_eq!(offered, [("MP4".to_owned(), 1), ("DOCX".to_owned(), 1)]);
        // A kind with more files comes first, whatever was met first.
        let list = selecting(vec![docx, mp4("a.mp4"), mp4("b.mp4")], &[0, 1, 2]);
        let Inspector::Several { offered, .. } = inspect_plain(&list, &Habits::default()) else {
            panic!()
        };
        assert_eq!(offered, [("MP4".to_owned(), 2), ("DOCX".to_owned(), 1)]);
        // Files of one kind that share nothing have no other kind to
        // narrow to, so offer none.
        let odd = |name: &str| entry(name, "MP4", 1_000, &[]);
        let list = selecting(vec![odd("a.mp4"), odd("b.mp4")], &[0, 1]);
        let i = inspect_plain(&list, &Habits::default());
        let Inspector::Several {
            offered,
            none_shared,
            ..
        } = &i
        else {
            panic!("{i:?}")
        };
        assert!(offered.is_empty());
        assert_eq!(
            none_shared.as_deref(),
            Some("Kiln can't convert MP4 files.")
        );
        assert_eq!(i.controls(), []);
    }
    #[test]
    fn nothing_selected_is_nothing() {
        let habits = Habits::default();
        assert_eq!(
            inspect_plain(&FileList::default(), &habits),
            Inspector::Nothing
        );
        // Rows in the list, none of them selected.
        let list = selecting(vec![harbour()], &[]);
        assert_eq!(inspect_plain(&list, &habits), Inspector::Nothing);
    }

    #[test]
    fn controls_follow_the_reading_order() {
        use InspectorControl as C;
        let target = |ext: &str| Target {
            ext: ext.to_owned(),
            why: None,
            estimate: None,
            suggested: false,
        };
        let waiting = |n: usize, more: bool| Inspector::Waiting {
            row: 0,
            what: None,
            facts: vec![],
            targets: (0..n).map(|i| target(&format!("f{i}"))).collect(),
            more,
        };
        assert_eq!(
            waiting(3, true).controls(),
            [C::Target(0), C::Target(1), C::Target(2), C::More]
        );
        assert_eq!(waiting(2, false).controls(), [C::Target(0), C::Target(1)]);
        assert_eq!(waiting(0, false).controls(), []);
        assert_eq!(
            Inspector::Converting {
                row: 0,
                percent: Some(38),
                left: None
            }
            .controls(),
            [C::Cancel]
        );
        let done = |kept| Inspector::Done {
            row: 0,
            result: "a.webp".to_owned(),
            change: None,
            kept,
        };
        assert_eq!(done(Kept::Held).controls(), [C::Save]);
        assert_eq!(done(Kept::InFolder).controls(), [C::ShowInFolder]);
        // Dragged out: nothing to save, and no folder to show.
        assert_eq!(done(Kept::Dropped).controls(), []);
        let failed = |retry, details| Inspector::Failed {
            row: 0,
            sentence: String::new(),
            retry,
            details,
        };
        assert_eq!(failed(true, true).controls(), [C::Retry, C::ShowDetails]);
        assert_eq!(failed(true, false).controls(), [C::Retry]);
        assert_eq!(failed(false, true).controls(), [C::ShowDetails]);
        assert_eq!(failed(false, false).controls(), []);
        let several = |targets: Vec<Target>, offered: Vec<(String, usize)>| Inspector::Several {
            rows: vec![0, 1],
            kinds: String::new(),
            targets,
            none_shared: None,
            offered,
        };
        assert_eq!(
            several(vec![target("jpg"), target("gif")], vec![]).controls(),
            [C::Target(0), C::Target(1)]
        );
        // With nothing shared, each kind is a control instead.
        assert_eq!(
            several(vec![], vec![("MP4".to_owned(), 2), ("DOCX".to_owned(), 1)]).controls(),
            [C::Kind(0), C::Kind(1)]
        );
        assert_eq!(Inspector::Nothing.controls(), []);
    }
}
