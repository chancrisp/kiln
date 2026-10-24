//! What the inspector says. The window shows a panel beside the file list
//! for what is selected. This is that panel's content as plain data: the
//! words for each selection, its controls in reading order, and nothing
//! about where any of it is drawn.

use std::collections::HashSet;
use std::path::Path;

use kiln_core::{about, describe_format, Facts, Habits, Kind, Suggested};

use crate::a11y::percent;
use crate::model::{
    doing, get_label, human_size, left_text, time_left, Entry, FileList, JobState, Tone, GONE,
};
use crate::wheel::{offer_sentence, sort_popular, Action, Offer, WheelKind};

/// A control in the inspector, by what it is. The same names stand for
/// the Tab order, the keyboard focus and what assistive tools are told.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InspectorControl {
    /// The nth target shown, from 0.
    Target(usize),
    /// "More formats…", which opens the format choice.
    More,
    Cancel,
    /// Export a held result, as the row's link and Export all do.
    Export,
    ShowInFolder,
    Retry,
    ShowDetails,
    /// The nth kind of file offered when the selection shares no format:
    /// choosing it selects just those files.
    Kind(usize),
    /// "Get FFmpeg": the download page of the helper app a file can't be
    /// converted without, where its formats would be.
    GetHelper,
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
        /// When the file has no formats because the helper app it needs
        /// isn't installed: the button that opens its download page, as
        /// the row's link says it ("Get FFmpeg").
        helper: Option<&'static str>,
        /// When the file has nothing to offer, not even a helper app to
        /// get: why, as its row says it ("Isn't really a DOCX file"), and
        /// the tone the row says it in. It stands where `what` would.
        reason: Option<(String, Tone)>,
    },
    Converting {
        row: usize,
        /// Whole percent, once the engine can say how far along it is.
        percent: Option<u32>,
        /// How long is left, "about 2 min", once there is enough to go on;
        /// until then what it is doing, "Converting to WEBP".
        time: String,
    },
    Done {
        row: usize,
        /// The result's file name.
        result: String,
        /// "2.4 MB → 410 KB, 83% smaller", where both sizes are known.
        change: Option<String>,
        /// Where the result is, and so what can be done with it.
        kept: Kept,
        /// The line that stands in for the row's own, as the row shows it,
        /// and its tone: a result that's gone, or an export that failed.
        note: Option<(String, Tone)>,
    },
    /// A job that failed or was cancelled.
    Failed {
        row: usize,
        /// The row's whole sentence, and the tone the row says it in.
        sentence: String,
        tone: Tone,
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
        /// At most [`MAX_TARGETS`] of the formats every selected file can
        /// become, none suggested, the most popular first.
        targets: Vec<Target>,
        /// Whether they share more formats than `targets` shows.
        more: bool,
        /// When `targets` is empty: why, and what to do about it.
        none_shared: Option<String>,
        /// When `targets` is empty and there is more than one kind of file:
        /// each kind's type code and how many files, most files first and a
        /// tie in the order they were met. Empty otherwise.
        offered: Vec<(String, usize)>,
        /// When every file waits on the same helper app that isn't
        /// installed: the button that opens its download page ("Get
        /// FFmpeg"), in place of the kinds.
        helper: Option<&'static str>,
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
    /// Kiln held it, and it isn't there any more: nothing to export.
    Gone,
}

/// The most targets the inspector lists before More.
pub const MAX_TARGETS: usize = 3;

/// What the inspector says with nothing selected.
pub const NOTHING_TITLE: &str = "Select a file to see what Kiln can do with it";

/// The heading over the formats the selection can become.
pub const TURN_INTO: &str = "Turn it into";

/// The heading over the kinds offered when several files share no format.
pub const ONE_KIND: &str = "Select one kind";

/// The link after the formats, which opens the format choice.
pub const MORE_FORMATS: &str = "More formats…";

/// How many files a kind on offer holds: "2 files", or "1 file".
pub fn files_count(n: usize) -> String {
    if n == 1 {
        "1 file".to_owned()
    } else {
        format!("{n} files")
    }
}

impl InspectorControl {
    /// What a button says on it. A format, a kind and More say what they
    /// are in their own way, so they have none; Get names its helper app,
    /// which [`Inspector::label`] knows.
    pub fn label(self) -> Option<&'static str> {
        match self {
            InspectorControl::Cancel => Some("Cancel"),
            InspectorControl::Export => Some("Export"),
            InspectorControl::ShowInFolder => Some("Show in folder"),
            InspectorControl::Retry => Some("Retry"),
            InspectorControl::ShowDetails => Some("Show details"),
            InspectorControl::Target(_)
            | InspectorControl::More
            | InspectorControl::Kind(_)
            | InspectorControl::GetHelper => None,
        }
    }
}

/// A file's type code as the inspector counts kinds by it: "MP4", or
/// "Other" for a file of no type Kiln knows. Narrowing the selection to a
/// kind goes by the same.
pub fn type_code(e: &Entry) -> &str {
    if e.code.is_empty() {
        "Other"
    } else {
        &e.code
    }
}

/// What the inspector can't tell from the list alone.
pub struct Lookups<'a> {
    /// What kind of file a row is.
    pub kind_of: &'a dyn Fn(&Entry) -> Option<Kind>,
    /// How big the file at a path is, if it can be read.
    pub size_of: &'a dyn Fn(&Path) -> Option<u64>,
    /// The note standing in for a row's own line, by the row's file, and
    /// its tone: see [`crate::model::Notes`].
    pub note_of: &'a dyn Fn(&Path) -> Option<(String, Tone)>,
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
            action,
            fraction,
            started,
        } => Inspector::Converting {
            row,
            percent: fraction.map(percent),
            time: started
                .zip(*fraction)
                .and_then(|(t, f)| time_left(t.elapsed(), f))
                .map_or_else(|| doing(action), left_text),
        },
        JobState::Done {
            action,
            outputs,
            saved,
        } => {
            // A file action (Move) leaves nothing in the holding folder:
            // the file is where it went, and its size is no result to compare.
            let in_place = matches!(action, Action::File(_));
            // The row's note stands in for its line: the panel says it too.
            let note = (look.note_of)(&e.path);
            let gone = note.as_ref().is_some_and(|(line, _)| line == GONE);
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
            } else if gone {
                Kept::Gone
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
                note,
            }
        }
        // A cancelled file has the same way forward as a failed one.
        JobState::Failed { .. } | JobState::Cancelled => {
            let (sentence, tone) = e.line();
            Inspector::Failed {
                row,
                sentence,
                tone,
                retry: e.retryable(),
                details: e.details(false).is_some(),
            }
        }
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
    // file, then the rest as the wheel ranks them, so the two agree. The
    // row lists codes ("WEBP"); the rest of Kiln speaks extensions.
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
    let mut rest = offered.clone();
    sort_popular(&mut rest);
    for ext in &rest {
        take(&mut order, &offered, ext);
    }

    let more = order.len() > MAX_TARGETS;
    let order_empty = order.is_empty();
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
    let helper = e.needs_helper().map(get_label);
    // With nothing to offer, not even a helper app to get, it says why, as
    // its row does, where what the format is would be.
    let reason = (order_empty && helper.is_none()).then(|| (e.reach(), e.line().1));
    Inspector::Waiting {
        row,
        what: describe_format(&e.code).filter(|_| reason.is_none()),
        facts: lines,
        targets,
        more,
        helper,
        reason,
    }
}

fn several(rows: &[usize], entries: &[Entry], look: &Lookups) -> Inspector {
    let picked: Vec<&Entry> = rows.iter().map(|&i| &entries[i]).collect();
    let kinds: Vec<Option<Kind>> = picked.iter().map(|e| (look.kind_of)(e)).collect();

    // What every file can become. Each file's codes are lowercased once,
    // into a set the shared ones are checked against, and once nothing is
    // shared the rest aren't looked at. Then they are ranked as the wheel
    // ranks them.
    let mut shared: Vec<String> = vec![];
    for t in &picked[0].targets {
        let ext = t.to_ascii_lowercase();
        if !shared.contains(&ext) {
            shared.push(ext);
        }
    }
    let mut theirs: HashSet<String> = HashSet::new();
    for e in &picked[1..] {
        if shared.is_empty() {
            break;
        }
        theirs.clear();
        theirs.extend(e.targets.iter().map(|t| t.to_ascii_lowercase()));
        shared.retain(|ext| theirs.contains(ext));
    }
    sort_popular(&mut shared);

    // Every file waiting on the same helper app that isn't installed: what
    // the wheel says for them, and the button that gets it.
    let first_helper = picked[0].needs_helper();
    let helper = first_helper.filter(|&b| picked.iter().all(|e| e.needs_helper() == Some(b)));

    // With nothing in common, the kinds present, most files first and a
    // tie in the order they were met, as the wheel says it. With only one
    // kind there is none else to narrow to, so none is offered.
    let (none_shared, offered) = if let Some(backend) = helper {
        let code = type_code(picked[0]).to_owned();
        let offer = Offer::NeedsHelper(backend.to_owned(), code);
        (offer_sentence(WheelKind::Convert, &offer), vec![])
    } else if shared.is_empty() {
        let mut codes: Vec<(String, usize)> = vec![];
        for e in &picked {
            let code = type_code(e);
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
        more: shared.len() > MAX_TARGETS,
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
        helper: helper.map(get_label),
    }
}

impl Inspector {
    /// The controls it has, in reading order. That is the Tab order, and
    /// the order assistive tools meet them in.
    pub fn controls(&self) -> Vec<InspectorControl> {
        use InspectorControl as C;
        match self {
            Inspector::Nothing => vec![],
            // A file waiting on its helper app has no formats, only Get.
            Inspector::Waiting {
                targets,
                more,
                helper,
                ..
            } => (0..targets.len())
                .map(C::Target)
                .chain((*more).then_some(C::More))
                .chain(helper.map(|_| C::GetHelper))
                .collect(),
            // The shared formats and More; with none, Get for the helper
            // app they all wait on, or else each kind to narrow to.
            Inspector::Several {
                targets,
                more,
                offered,
                helper,
                ..
            } => match (targets.is_empty(), helper) {
                (false, _) => (0..targets.len())
                    .map(C::Target)
                    .chain((*more).then_some(C::More))
                    .collect(),
                (true, Some(_)) => vec![C::GetHelper],
                (true, None) => (0..offered.len()).map(C::Kind).collect(),
            },
            Inspector::Converting { .. } => vec![C::Cancel],
            Inspector::Done { kept, .. } => match kept {
                Kept::Held => vec![C::Export],
                Kept::InFolder => vec![C::ShowInFolder],
                Kept::Dropped | Kept::Gone => vec![],
            },
            Inspector::Failed { retry, details, .. } => (*retry)
                .then_some(C::Retry)
                .into_iter()
                .chain((*details).then_some(C::ShowDetails))
                .collect(),
        }
    }

    /// What button `c` says on it: as [`InspectorControl::label`] has it,
    /// and Get as the row's link says it, "Get FFmpeg".
    pub fn label(&self, c: InspectorControl) -> Option<&'static str> {
        match (c, self) {
            (
                InspectorControl::GetHelper,
                Inspector::Waiting { helper, .. } | Inspector::Several { helper, .. },
            ) => *helper,
            _ => c.label(),
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

    /// The estimate as the target's row shows it: "≈ 410 KB", which reads
    /// as a guess where a bare size would read as exact. `None` without
    /// one.
    pub fn estimate_label(&self) -> Option<String> {
        self.estimate
            .map(|bytes| format!("\u{2248} {}", about(bytes)))
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

    fn no_note(_: &Path) -> Option<(String, Tone)> {
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
            note_of: &no_note,
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
            helper,
            reason,
        } = i
        else {
            panic!("{i:?}")
        };
        assert_eq!(row, 0);
        assert_eq!(helper, None);
        assert_eq!(reason, None);
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
        // Drawn, an estimate is marked as one; without, nothing is drawn.
        assert_eq!(
            target(None, Some(410_000)).estimate_label().as_deref(),
            Some("\u{2248} 410 KB")
        );
        assert_eq!(target(None, None).estimate_label(), None);
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
    fn without_a_suggestion_or_habit_the_targets_come_as_the_wheel_ranks_them() {
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
        // JPG, PDF and WEBP are the wheel's first three of these five.
        let exts: Vec<_> = targets.iter().map(|t| t.ext.as_str()).collect();
        assert_eq!(exts, ["jpg", "pdf", "webp"]);
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
        assert_eq!(exts, ["gif", "jpg", "pdf"]);
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
        // Until there is enough to go on, what it is doing.
        assert_eq!(
            inspect_job(running(Some(0.38), None)),
            Inspector::Converting {
                row: 0,
                percent: Some(38),
                time: "Converting to WEBP".to_owned()
            }
        );
        // Ten seconds in and 38% done: about 16 s to go.
        let started = Instant::now().checked_sub(Duration::from_secs(10));
        assert_eq!(
            inspect_job(running(Some(0.38), started)),
            Inspector::Converting {
                row: 0,
                percent: Some(38),
                time: "about 15 sec".to_owned()
            }
        );
        // An engine that can't say how far along it is.
        assert_eq!(
            inspect_job(running(None, started)),
            Inspector::Converting {
                row: 0,
                percent: None,
                time: "Converting to WEBP".to_owned()
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
            note_of: &no_note,
        };
        assert_eq!(
            inspect(&list, &look, &Habits::default()),
            Inspector::Done {
                row: 0,
                result: "harbour.webp".to_owned(),
                change: Some("2.4 MB → 410 KB, 83% smaller".to_owned()),
                kept: Kept::Held,
                note: None,
            }
        );
        // Without the output's size there is no before and after.
        assert_eq!(
            inspect_plain(&list, &Habits::default()),
            Inspector::Done {
                row: 0,
                result: "harbour.webp".to_owned(),
                change: None,
                kept: Kept::Held,
                note: None,
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
            note_of: &no_note,
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
            let want = (e.line(), e.retryable());
            let list = selecting(vec![e], &[0]);
            (inspect_plain(&list, &Habits::default()), want)
        };
        let again = Some(Action::Convert("webp".to_owned()));
        let (i, ((sentence, tone), retry)) = failed(
            "The conversion stopped before it finished. Try again.",
            again.clone(),
            "",
        );
        assert!(retry);
        // Said in the row's own words and tone.
        assert_eq!(tone, Tone::Crack);
        assert_eq!(
            i,
            Inspector::Failed {
                row: 0,
                sentence,
                tone,
                retry: true,
                details: false
            }
        );
        // A damaged file would fail the same way: no Retry.
        let (i, ((sentence, tone), retry)) = failed(
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
                tone,
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
        let tone = e.line().1;
        let list = selecting(vec![e], &[0]);
        let i = inspect_plain(&list, &Habits::default());
        assert_eq!(
            i,
            Inspector::Failed {
                row: 0,
                sentence: "Cancelled.".to_owned(),
                tone,
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
            more,
            none_shared,
            offered,
            helper,
        } = i
        else {
            panic!("{i:?}")
        };
        assert_eq!(rows, [0, 1, 2]);
        assert_eq!(kinds, "4 images");
        // The four they share, as the wheel ranks them: the first three,
        // none suggested, and More for the fourth.
        let exts: Vec<_> = targets.iter().map(|t| t.ext.as_str()).collect();
        assert_eq!(exts, ["jpg", "webp", "gif"]);
        assert!(more);
        assert_eq!(helper, None);
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
            more,
            none_shared,
            offered,
            helper,
        } = &i
        else {
            panic!("{i:?}")
        };
        assert_eq!(rows, &[0, 1, 2]);
        assert_eq!(kinds, "2 videos, 1 document");
        assert!(targets.is_empty() && !more);
        assert_eq!(*helper, None);
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
    fn an_mp4_with_no_suggestion_or_habits_is_offered_what_the_wheel_shows_first() {
        // The registry lists a file's formats alphabetically, which would
        // put 3GP, AAC and AC3 first. The wheel ranks them by what people
        // reach for, and the inspector agrees with it.
        let codes = [
            "3gp", "aac", "ac3", "aiff", "avi", "flac", "flv", "gif", "m4a", "m4v", "mkv", "mov",
            "mp3", "mpg", "ogg", "ogv", "opus", "ts", "wav", "webm", "wma", "wmv",
        ];
        let e = entry("clip.mp4", "MP4", 40_000_000, &codes);
        let list = selecting(vec![e], &[0]);
        let i = inspect_plain(&list, &Habits::default());
        let Inspector::Waiting { targets, more, .. } = &i else {
            panic!("{i:?}")
        };
        let exts: Vec<_> = targets.iter().map(|t| t.ext.as_str()).collect();
        assert_eq!(exts, ["mp3", "gif", "wav"]);
        assert!(*more);
        // The same order the wheel sorts the same formats in.
        let mut wheel: Vec<String> = codes.iter().map(|c| (*c).to_owned()).collect();
        crate::wheel::sort_popular(&mut wheel);
        assert_eq!(&wheel[..3], exts);
    }

    #[test]
    fn three_pngs_share_their_formats_most_popular_first_and_more() {
        let codes = ["avif", "bmp", "gif", "ico", "jpg", "pdf", "tiff", "webp"];
        let png = |name: &str| entry(name, "PNG", 1_000, &codes);
        let list = selecting(vec![png("a.png"), png("b.png"), png("c.png")], &[0, 1, 2]);
        let i = inspect_plain(&list, &Habits::default());
        let Inspector::Several {
            targets,
            more,
            none_shared,
            helper,
            ..
        } = &i
        else {
            panic!("{i:?}")
        };
        let exts: Vec<_> = targets.iter().map(|t| t.ext.as_str()).collect();
        assert_eq!(exts, ["jpg", "pdf", "webp"]);
        assert!(*more && none_shared.is_none() && helper.is_none());
        use InspectorControl as C;
        assert_eq!(
            i.controls(),
            [C::Target(0), C::Target(1), C::Target(2), C::More]
        );
        // Three formats or fewer in common are all shown, with no More.
        let few = |name: &str| entry(name, "PNG", 1_000, &["webp", "gif", "jpg"]);
        let list = selecting(vec![few("a.png"), few("b.png")], &[0, 1]);
        let i = inspect_plain(&list, &Habits::default());
        let Inspector::Several { targets, more, .. } = &i else {
            panic!("{i:?}")
        };
        let exts: Vec<_> = targets.iter().map(|t| t.ext.as_str()).collect();
        assert_eq!(exts, ["jpg", "webp", "gif"]);
        assert!(!more);
    }

    #[test]
    fn several_files_waiting_on_one_helper_app_offer_to_get_it() {
        use InspectorControl as C;
        let needs = |name: &str, code: &str, backend: &str| {
            let mut e = entry(name, code, 1_000, &[]);
            e.missing = Some((backend.to_owned(), 12));
            e
        };
        // Two MP4s while FFmpeg is missing: what the wheel would say, and
        // Get where the kinds would be.
        let list = selecting(
            vec![
                needs("a.mp4", "MP4", "ffmpeg"),
                needs("b.mp4", "MP4", "ffmpeg"),
            ],
            &[0, 1],
        );
        let i = inspect_plain(&list, &Habits::default());
        let Inspector::Several {
            targets,
            more,
            none_shared,
            offered,
            helper,
            ..
        } = &i
        else {
            panic!("{i:?}")
        };
        assert!(targets.is_empty() && !more && offered.is_empty());
        let wheel = Offer::NeedsHelper("ffmpeg".to_owned(), "MP4".to_owned());
        assert_eq!(
            none_shared,
            &crate::wheel::offer_sentence(WheelKind::Convert, &wheel)
        );
        assert_eq!(
            none_shared.as_deref(),
            Some("MP4 files need FFmpeg, which isn't installed. Use Get FFmpeg to download it.")
        );
        assert_eq!(*helper, Some("Get FFmpeg"));
        assert_eq!(i.controls(), [C::GetHelper]);
        assert_eq!(i.label(C::GetHelper), Some("Get FFmpeg"));

        // Two types that both wait on FFmpeg: one Get still does it.
        let list = selecting(
            vec![
                needs("a.mkv", "MKV", "ffmpeg"),
                needs("b.avi", "AVI", "ffmpeg"),
            ],
            &[0, 1],
        );
        let i = inspect_plain(&list, &Habits::default());
        assert!(matches!(
            &i,
            Inspector::Several { helper: Some("Get FFmpeg"), offered, .. } if offered.is_empty()
        ));
        assert_eq!(i.controls(), [C::GetHelper]);

        // Each waiting on a different app: the explanation and the kinds.
        let list = selecting(
            vec![
                needs("a.mkv", "MKV", "ffmpeg"),
                needs("b.docx", "DOCX", "pandoc"),
            ],
            &[0, 1],
        );
        let i = inspect_plain(&list, &Habits::default());
        let Inspector::Several {
            none_shared,
            offered,
            helper,
            ..
        } = &i
        else {
            panic!("{i:?}")
        };
        assert_eq!(*helper, None);
        assert!(none_shared
            .as_deref()
            .unwrap()
            .starts_with("These files have no format in common"));
        assert_eq!(offered, &[("MKV".to_owned(), 1), ("DOCX".to_owned(), 1)]);
        assert_eq!(i.controls(), [C::Kind(0), C::Kind(1)]);

        // One waiting on FFmpeg beside one Kiln can't convert at all: the
        // helper wouldn't do for both, so no Get.
        let list = selecting(
            vec![
                needs("a.mkv", "MKV", "ffmpeg"),
                entry("b.xyz", "XYZ", 1_000, &[]),
            ],
            &[0, 1],
        );
        let i = inspect_plain(&list, &Habits::default());
        assert!(matches!(&i, Inspector::Several { helper: None, .. }));
        assert_eq!(i.controls(), [C::Kind(0), C::Kind(1)]);
    }

    #[test]
    fn a_done_file_whose_result_is_gone_says_so_and_offers_no_export() {
        let out = PathBuf::from("held").join("harbour.webp");
        let mut e = harbour();
        e.job = JobState::Done {
            action: Action::Convert("webp".to_owned()),
            outputs: vec![out],
            saved: None,
        };
        let file = e.path.clone();
        let list = selecting(vec![e], &[0]);
        let inspect_noted = |note: Option<(String, Tone)>| {
            let noted = |p: &Path| note.clone().filter(|_| p == file);
            let look = Lookups {
                kind_of: &kind_by_code,
                size_of: &no_size,
                note_of: &noted,
            };
            inspect(&list, &look, &Habits::default())
        };
        // Removed by hand: the row's note, in its tone, and no Export,
        // which would have nothing to export.
        let gone = (crate::model::GONE.to_owned(), Tone::Crack);
        let i = inspect_noted(Some(gone.clone()));
        let Inspector::Done { kept, note, .. } = &i else {
            panic!("{i:?}")
        };
        assert_eq!(*kept, Kept::Gone);
        assert_eq!(note.as_ref(), Some(&gone));
        assert_eq!(i.controls(), []);
        // An export that failed: its note is said, and Export is still
        // the way to try again.
        let failed = ("Couldn't save to photos.".to_owned(), Tone::Crack);
        let i = inspect_noted(Some(failed.clone()));
        let Inspector::Done { kept, note, .. } = &i else {
            panic!("{i:?}")
        };
        assert_eq!(*kept, Kept::Held);
        assert_eq!(note.as_ref(), Some(&failed));
        assert_eq!(i.controls(), [InspectorControl::Export]);
        // With no note, the row says its own line, and so does the panel.
        let i = inspect_noted(None);
        assert!(matches!(
            i,
            Inspector::Done {
                kept: Kept::Held,
                note: None,
                ..
            }
        ));
    }

    #[test]
    fn a_file_with_nothing_to_offer_says_why_as_its_row_does() {
        // Ruled out on adding: the row's reason, in the row's tone, where
        // what the format is would be.
        let mut ruled_out = entry("report.docx", "DOCX", 2_000_000, &[]);
        ruled_out.check = kiln_core::FileCheck::Mismatch;
        assert_eq!(ruled_out.line().1, Tone::Crack);
        let i = inspect_plain(&selecting(vec![ruled_out], &[0]), &Habits::default());
        let Inspector::Waiting {
            what,
            reason,
            targets,
            helper,
            ..
        } = &i
        else {
            panic!("{i:?}")
        };
        assert_eq!(
            reason.as_ref(),
            Some(&("Isn't really a DOCX file".to_owned(), Tone::Crack))
        );
        assert_eq!(*what, None);
        assert!(targets.is_empty() && helper.is_none());
        assert_eq!(i.controls(), []);
        // An empty file says so, without the size its facts already give.
        let mut empty = entry("blank.png", "PNG", 0, &[]);
        empty.check = kiln_core::FileCheck::Empty;
        let i = inspect_plain(&selecting(vec![empty], &[0]), &Habits::default());
        assert!(matches!(
            &i,
            Inspector::Waiting { reason: Some((r, Tone::Crack)), .. } if r == "Empty file (0 bytes)"
        ));
        // A type Kiln has no formats for, said as quietly as the row does.
        let none = entry("notes.xyz", "XYZ", 1_000, &[]);
        let i = inspect_plain(&selecting(vec![none], &[0]), &Habits::default());
        assert!(matches!(
            &i,
            Inspector::Waiting { reason: Some((r, Tone::Muted)), .. } if r == "No formats for this type"
        ));
        // A file with formats, or one with Get, has no reason to give.
        let i = inspect_plain(&selecting(vec![harbour()], &[0]), &Habits::default());
        assert!(matches!(i, Inspector::Waiting { reason: None, .. }));
        let mut needs = entry("clip.mkv", "MKV", 1_000, &[]);
        needs.missing = Some(("ffmpeg".to_owned(), 12));
        let i = inspect_plain(&selecting(vec![needs], &[0]), &Habits::default());
        assert!(matches!(
            i,
            Inspector::Waiting {
                reason: None,
                helper: Some(_),
                ..
            }
        ));
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
            helper: None,
            reason: None,
        };
        assert_eq!(
            waiting(3, true).controls(),
            [C::Target(0), C::Target(1), C::Target(2), C::More]
        );
        assert_eq!(waiting(2, false).controls(), [C::Target(0), C::Target(1)]);
        assert_eq!(waiting(0, false).controls(), []);
        // A file waiting on its helper app has Get in their place.
        let mut needs = waiting(0, false);
        if let Inspector::Waiting { helper, .. } = &mut needs {
            *helper = Some("Get FFmpeg");
        }
        assert_eq!(needs.controls(), [C::GetHelper]);
        assert_eq!(
            Inspector::Converting {
                row: 0,
                percent: Some(38),
                time: String::new()
            }
            .controls(),
            [C::Cancel]
        );
        let done = |kept| Inspector::Done {
            row: 0,
            result: "a.webp".to_owned(),
            change: None,
            kept,
            note: None,
        };
        assert_eq!(done(Kept::Held).controls(), [C::Export]);
        assert_eq!(done(Kept::InFolder).controls(), [C::ShowInFolder]);
        // Dragged out: nothing to save, and no folder to show.
        assert_eq!(done(Kept::Dropped).controls(), []);
        let failed = |retry, details| Inspector::Failed {
            row: 0,
            sentence: String::new(),
            tone: Tone::Crack,
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
            more: false,
            helper: None,
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
        // More after the shared formats, as for one file.
        let mut more = several(vec![target("jpg"); 3], vec![]);
        if let Inspector::Several { more, .. } = &mut more {
            *more = true;
        }
        assert_eq!(
            more.controls(),
            [C::Target(0), C::Target(1), C::Target(2), C::More]
        );
        // Files all waiting on one helper app have Get, and no kinds.
        let mut needs = several(vec![], vec![]);
        if let Inspector::Several { helper, .. } = &mut needs {
            *helper = Some("Get FFmpeg");
        }
        assert_eq!(needs.controls(), [C::GetHelper]);
        assert_eq!(needs.label(C::GetHelper), Some("Get FFmpeg"));
        assert_eq!(Inspector::Nothing.controls(), []);
    }

    #[test]
    fn buttons_say_what_they_do_and_kinds_count_their_files() {
        use InspectorControl as C;
        let labels: Vec<_> = [
            C::Cancel,
            C::Export,
            C::ShowInFolder,
            C::Retry,
            C::ShowDetails,
        ]
        .map(|c| c.label().unwrap())
        .to_vec();
        assert_eq!(
            labels,
            [
                "Cancel",
                "Export",
                "Show in folder",
                "Retry",
                "Show details"
            ]
        );
        // A format, a kind and More say what they are in their own way.
        assert!([C::Target(0), C::Kind(0), C::More]
            .iter()
            .all(|c| c.label().is_none()));
        assert_eq!(files_count(1), "1 file");
        assert_eq!(files_count(2), "2 files");
        // The inspector says them the same, and Get names its helper app.
        let mut e = entry("clip.mkv", "MKV", 1_000, &[]);
        e.missing = Some(("ffmpeg".to_owned(), 12));
        let i = inspect_plain(&selecting(vec![e], &[0]), &Habits::default());
        assert_eq!(i.label(C::GetHelper), Some("Get FFmpeg"));
        assert_eq!(i.label(C::Cancel), Some("Cancel"));
        assert_eq!(Inspector::Nothing.label(C::GetHelper), None);
    }

    #[test]
    fn a_file_waiting_on_its_helper_app_offers_to_get_it_where_its_formats_would_be() {
        use InspectorControl as C;
        let mut e = entry("clip.mkv", "MKV", 1_000, &[]);
        e.missing = Some(("ffmpeg".to_owned(), 12));
        // Named as the row's link names it, from the same words.
        assert_eq!(e.link(false), Some("Get FFmpeg"));
        assert_eq!(e.needs_helper(), Some("ffmpeg"));
        let i = inspect_plain(&selecting(vec![e.clone()], &[0]), &Habits::default());
        let Inspector::Waiting {
            targets,
            more,
            helper,
            ..
        } = &i
        else {
            panic!("{i:?}")
        };
        assert!(targets.is_empty() && !more);
        assert_eq!(*helper, Some("Get FFmpeg"));
        assert_eq!(i.controls(), [C::GetHelper]);

        // A file with formats of its own converts to them: no Get, even
        // when some more would need the helper.
        let mut some = entry("clip.mp4", "MP4", 1_000, &["mkv"]);
        some.missing = Some(("ffmpeg".to_owned(), 2));
        let i = inspect_plain(&selecting(vec![some], &[0]), &Habits::default());
        assert!(matches!(i, Inspector::Waiting { helper: None, .. }));
        assert_eq!(i.controls(), [C::Target(0)]);
        // A file Kiln can't convert at all, with no helper to get, has
        // nothing to offer.
        let none = entry("notes.xyz", "XYZ", 1_000, &[]);
        let i = inspect_plain(&selecting(vec![none], &[0]), &Habits::default());
        assert_eq!(i.controls(), []);
        // Nor does one ruled out on adding.
        let mut ruled_out = e;
        ruled_out.check = kiln_core::FileCheck::Empty;
        assert_eq!(ruled_out.needs_helper(), None);
        let i = inspect_plain(&selecting(vec![ruled_out], &[0]), &Habits::default());
        assert_eq!(i.controls(), []);
    }

    #[test]
    fn kinds_are_counted_and_narrowed_by_one_type_code() {
        let mut odd = entry("README", "", 1_000, &[]);
        assert_eq!(type_code(&odd), "Other");
        odd.code = "MP4".to_owned();
        assert_eq!(type_code(&odd), "MP4");
    }

    /// 500 selected rows, as Ctrl+A on a big folder makes, each with the
    /// dozen formats an image has, spelt as the list spells them.
    fn five_hundred(shared: bool) -> FileList {
        let codes = [
            "JPG", "WEBP", "AVIF", "GIF", "PNG", "BMP", "TIFF", "ICO", "PDF", "HEIC", "JXL", "TGA",
        ];
        let entries = (0..500)
            .map(|i| {
                // Every other file a video, which shares none of them.
                if !shared && i % 2 == 1 {
                    return entry(&format!("{i}.mp4"), "MP4", 1_000, &["MKV", "WEBM", "MOV"]);
                }
                let mut targets: Vec<&str> = codes.to_vec();
                // Rotated, so no two neighbours list them in one order.
                targets.rotate_left(i % codes.len());
                entry(&format!("{i}.png"), "PNG", 1_000, &targets)
            })
            .collect();
        let rows: Vec<usize> = (0..500).collect();
        selecting(entries, &rows)
    }

    #[test]
    fn inspecting_five_hundred_selected_files_is_quick() {
        let habits = Habits::default();
        for shared in [true, false] {
            let list = five_hundred(shared);
            let i = inspect_plain(&list, &habits);
            let Inspector::Several {
                targets, offered, ..
            } = &i
            else {
                panic!("{i:?}")
            };
            assert_eq!(targets.is_empty(), !shared);
            assert_eq!(offered.len(), if shared { 0 } else { 2 });
            // The mean of a few runs, to keep a slow first one from
            // deciding it. The bound is generous, for a debug build on a
            // busy machine; the time it took is printed.
            let runs: u32 = 20;
            let start = Instant::now();
            for _ in 0..runs {
                std::hint::black_box(inspect_plain(std::hint::black_box(&list), &habits));
            }
            let each = start.elapsed() / runs;
            println!(
                "inspect() for 500 selected files, {}: {each:?}",
                if shared { "all sharing" } else { "half video" }
            );
            assert!(each < Duration::from_millis(50), "{each:?}");
        }
    }
}
