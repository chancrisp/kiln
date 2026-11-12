//! What the side pane says. The window shows a pane beside the file list
//! for what is selected (mock screens 2, 5 and 8b). This is that pane's
//! content as plain data: the words for each selection, its controls in
//! reading order, and nothing about where any of it is drawn.

use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use kiln_core::{about, describe_format, Facts, Kind, Suggested};

use crate::a11y::percent;
use crate::controls::Severity;
use crate::model::{
    doing, get_link, human_size, left_text, store_label, time_left, Entry, FileList, JobState,
    Saved, Tone, GONE,
};
use crate::wheel::{offer_sentence, Offer, Order, WheelKind, SUBTITLES};

/// A control in the side pane, by what it is. The same names stand for
/// the Tab order, the keyboard focus and what assistive tools are told.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InspectorControl {
    /// The nth format shown, from 0: under Convert to, or under Convert
    /// again once a file is done.
    Target(usize),
    /// "More formats and tools", which opens the format choice.
    More,
    /// A finished drag-wheel conversion's Options, as the row's: the
    /// options sheet for its pair, to convert it again.
    Options,
    ShowInFolder,
    Retry,
    ShowDetails,
    /// The nth kind of file offered when the selection shares no format:
    /// choosing it selects just those files.
    Kind(usize),
    /// "Get FFmpeg": the download page of the helper app a file can't be
    /// converted without, where its formats would be.
    GetHelper,
    /// Make smaller, first under Convert to when it applies (spec 2.2).
    Smaller,
    /// Open a finished file's result in the app Windows opens it with.
    Open,
    /// The Folder fact of a waiting file, as a link that opens the folder.
    Folder,
}

/// The smallest result Kiln gives an estimate for, in bytes. Under it a
/// guess is off by more than the size itself ("about 1 KB" for a 12 KB
/// file), so none is shown.
pub const MIN_ESTIMATE: u64 = 10_000;

/// A format the side pane offers to convert the selection to.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    /// The extension, lowercase: "webp".
    pub ext: String,
    /// What the format is for, in a few words: "Small image for the web".
    pub why: Option<&'static str>,
    /// The likely size of the result in bytes, where Kiln has an estimate.
    pub estimate: Option<u64>,
    /// Whether this is what Kiln suggests for the file.
    pub suggested: bool,
}

/// What the side pane says for the selection.
#[derive(Debug, Clone, PartialEq)]
pub enum Inspector {
    Nothing,
    /// One file that nothing has been done to.
    Waiting {
        row: usize,
        /// Labelled facts, in reading order: Size, Dimensions, Length, Folder.
        /// Size and Folder are always there; the others when known.
        facts: Vec<(&'static str, String)>,
        /// At most [`MAX_TARGETS`], the suggestion first.
        targets: Vec<Target>,
        /// Whether there are more formats than `targets` shows.
        more: bool,
        /// When the file has no formats because the helper app it needs
        /// isn't installed: the button that opens its download page, as
        /// the row's button says it ("Get FFmpeg").
        helper: Option<&'static str>,
        /// When the file has nothing to offer, not even a helper app to
        /// get: why, as its row says it ("Isn't really a DOCX file"), and
        /// the tone the row says it in. A healthy file has no line here: the row already says what it is.
        reason: Option<(String, Tone)>,
        /// Make smaller comes first under Convert to (spec 2.2).
        smaller: bool,
    },
    Converting {
        row: usize,
        /// What it is doing: "Converting to MP4".
        doing: String,
        /// Whole percent, once the engine can say how far along it is.
        percent: Option<u32>,
        /// How long is left, "About 40 s left", once there is enough to go
        /// on.
        left: Option<String>,
    },
    Done {
        row: usize,
        /// The result's file name.
        result: String,
        /// "83% smaller", "9.5 times larger", where both sizes are known.
        change: Option<String>,
        /// Where the result is, and so what can be done with it.
        kept: Kept,
        /// The line that stands in for the row's own, as the row shows it,
        /// and its tone: a result that's gone, or a save that failed.
        note: Option<(String, Tone)>,
        /// A drag-wheel conversion offers Options (`Entry::options_for`).
        options: bool,
        /// Where a saved result is, by its folder: "in Pictures". Empty
        /// while it is on its way there.
        place: String,
        /// Make smaller comes first under Convert again where it applies
        /// (mock screen 5), and then one format fewer follows.
        smaller: bool,
        /// The first formats the file can become, under Convert again.
        again: Vec<Target>,
    },
    /// A job that failed or was cancelled.
    Failed {
        row: usize,
        /// The InfoBar's title: "Couldn't read this PDF", or "Cancelled".
        title: String,
        /// The row's whole sentence, and the tone the row says it in.
        sentence: String,
        tone: Tone,
        /// Whether trying again may help.
        retry: bool,
        /// Whether there is more to show than the sentence.
        details: bool,
        /// It was cancelled rather than failed: an InfoBar of no alarm.
        cancelled: bool,
    },
    Several {
        /// The first three selected rows, for the stack of previews.
        rows: Vec<usize>,
        /// How many files are selected.
        count: usize,
        /// The kinds in words: "All photos" for one kind, else "3 images,
        /// 1 video".
        kinds: String,
        /// Whether they are of more than one kind.
        mixed: bool,
        /// At most [`MAX_TARGETS`] of the formats the selected files can
        /// become, as the wheel offers them (any of them for one kind,
        /// every one for several), none suggested, in the wheel's order.
        targets: Vec<Target>,
        /// Whether they share more formats than `targets` shows.
        more: bool,
        /// When `targets` is empty for files of one kind: why, and what
        /// to do about it. Several kinds get no sentence here: the
        /// InfoBar at the top of the list says it.
        none_shared: Option<String>,
        /// When `targets` is empty and there is more than one kind of file:
        /// each kind's type code and how many files, most files first and a
        /// tie in the order they were met. Empty otherwise.
        offered: Vec<(String, usize)>,
        /// When every file waits on the same helper app that isn't
        /// installed: the button that opens its download page ("Get
        /// FFmpeg"), in place of the kinds.
        helper: Option<&'static str>,
        /// Make smaller comes first under Convert them to.
        smaller: bool,
    },
}

/// Where a finished file's result is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kept {
    /// Still in Kiln's holding folder: on its way to its folder, or kept
    /// for the next start to save.
    Held,
    /// Saved to a folder that can be shown.
    InFolder,
    /// Kiln held it, and it isn't there any more: nothing to save.
    Gone,
}

/// The most formats the side pane lists under Convert to before More (the
/// mock's five).
pub const MAX_TARGETS: usize = 5;

/// The most it lists under Convert again.
pub const AGAIN_TARGETS: usize = 3;

/// What the side pane says with nothing selected.
pub const NOTHING_TITLE: &str = "Select a file";

/// The line under it.
pub const NOTHING_HINT: &str = "Its formats and details show here.";

/// The heading over the formats one file can become (spec 2.4).
pub const TURN_INTO: &str = "Convert to";

/// The heading over the formats several files can become.
pub const TURN_THEM_INTO: &str = "Convert them to";

/// The heading over a finished file's formats.
pub const CONVERT_AGAIN: &str = "Convert again";

/// The heading over the kinds offered when several files share no format.
pub const ONE_KIND: &str = "Convert one format at a time";

/// The link after the formats, which opens the format choice.
pub const MORE_FORMATS: &str = "More formats and tools";

/// Make smaller's row: its name and what it does (mock `.opt.first`).
pub const SMALLER: &str = "Make smaller";
pub const SMALLER_WHY: &str = "Same format, smaller file";

/// A cancelled file's InfoBar title.
pub const CANCELLED: &str = "Cancelled";

/// How many files a kind on offer holds: "2 files", or "1 file".
pub fn files_count(n: usize) -> String {
    if n == 1 {
        "1 file".to_owned()
    } else {
        format!("{n} files")
    }
}

/// How long is left, as the side pane says it under the percentage (mock
/// screen 5), in the row's words: "About 40 sec left", "A few seconds
/// left".
pub fn left_line(left: Duration) -> String {
    let said = left_text(left);
    let mut cs = said.chars();
    let first = cs
        .next()
        .map(|c| c.to_ascii_uppercase())
        .unwrap_or_default();
    format!("{first}{} left", cs.as_str())
}

/// Several files of one kind, in words: "All photos", "All videos".
pub fn all_of(kind: Kind) -> &'static str {
    match kind {
        Kind::Image => "All photos",
        Kind::Video => "All videos",
        Kind::Audio => "All sound files",
        Kind::Document => "All documents",
        Kind::Spreadsheet => "All spreadsheets",
        Kind::Presentation => "All presentations",
        Kind::Subtitle => "All subtitle files",
    }
}

impl InspectorControl {
    /// What a button says on it. A format, a kind, More and Make smaller
    /// say what they are in their own way, so they have none; Get names
    /// its helper app, which [`Inspector::label`] knows.
    pub fn label(self) -> Option<&'static str> {
        match self {
            InspectorControl::Options => Some("Options"),
            InspectorControl::ShowInFolder => Some(crate::model::SHOW_IN_FOLDER),
            InspectorControl::Retry => Some("Retry"),
            InspectorControl::ShowDetails => Some("Show details"),
            InspectorControl::Open => Some("Open"),
            InspectorControl::Target(_)
            | InspectorControl::More
            | InspectorControl::Kind(_)
            | InspectorControl::GetHelper
            | InspectorControl::Folder
            | InspectorControl::Smaller => None,
        }
    }
}

/// A file's type code as the side pane counts kinds by it: "MP4", or
/// "Other" for a file of no type Kiln knows. Narrowing the selection to a
/// kind goes by the same.
pub fn type_code(e: &Entry) -> &str {
    if e.code.is_empty() {
        "Other"
    } else {
        &e.code
    }
}

/// What the side pane can't tell from the list alone.
pub struct Lookups<'a> {
    /// What kind of file a row is.
    pub kind_of: &'a dyn Fn(&Entry) -> Option<Kind>,
    /// How big the file at a path is, if it can be read.
    pub size_of: &'a dyn Fn(&Path) -> Option<u64>,
    /// The note standing in for a row's own line, by the row's file, and
    /// its tone: see [`crate::model::Notes`].
    pub note_of: &'a dyn Fn(&Path) -> Option<(String, Tone)>,
    /// Whether a row's result, by the row's file, is being saved now.
    pub saving_of: &'a dyn Fn(&Path) -> bool,
    /// Whether Make smaller applies to a row's file.
    pub smaller_of: &'a dyn Fn(&Entry) -> bool,
}

/// What the side pane says for the files selected in `files`.
pub fn inspect(files: &FileList, look: &Lookups) -> Inspector {
    let entries = files.entries();
    let rows: Vec<usize> = files
        .selection()
        .into_iter()
        .filter(|&i| i < entries.len())
        .collect();
    match rows.as_slice() {
        [] => Inspector::Nothing,
        &[row] => one(row, &entries[row], look),
        _ => several(&rows, entries, look),
    }
}

fn one(row: usize, e: &Entry, look: &Lookups) -> Inspector {
    match &e.job {
        JobState::Idle => waiting(row, e, look),
        JobState::Running {
            action,
            fraction,
            started,
        } => Inspector::Converting {
            row,
            doing: doing(action),
            percent: fraction.map(percent),
            left: started
                .zip(*fraction)
                .and_then(|(t, f)| time_left(t.elapsed(), f))
                .map(left_line),
        },
        JobState::Done { outputs, saved, .. } => {
            // The row's note stands in for its line: the pane says it too.
            let note = (look.note_of)(&e.path);
            let gone = note.as_ref().is_some_and(|(line, _)| line == GONE);
            let kept = if e.saved().is_some() {
                Kept::InFolder
            } else if gone {
                Kept::Gone
            } else {
                Kept::Held
            };
            // Several results are no single size to set against the original.
            let change = match outputs.as_slice() {
                [only] => (look.size_of)(only).and_then(|after| change_words(e.size, after)),
                _ => None,
            };
            let place = match saved {
                Some(Saved::Folder(folder)) => {
                    format!("in {}", crate::actions::folder_name(folder))
                }
                _ => String::new(),
            };
            Inspector::Done {
                row,
                // As the row names it, or the file itself when there is
                // nothing else to call it.
                result: e.results_named().unwrap_or_else(|| e.name.clone()),
                change,
                kept,
                note,
                // Not while its result is being saved, as on the row.
                options: e.options_for().is_some() && !(look.saving_of)(&e.path),
                place,
                smaller: smaller_for(e, look),
                again: ordered_targets(e, look, AGAIN_TARGETS - usize::from(smaller_for(e, look)))
                    .0,
            }
        }
        // A cancelled file has the same way forward as a failed one.
        JobState::Failed { .. } | JobState::Cancelled => {
            let (sentence, tone) = e.line();
            let cancelled = matches!(e.job, JobState::Cancelled);
            let title = if cancelled {
                CANCELLED.to_owned()
            } else {
                e.failure_title()
            };
            let sentence = under_title(&title, sentence);
            Inspector::Failed {
                row,
                title,
                sentence,
                tone,
                retry: e.retryable(),
                details: e.details(false).is_some(),
                cancelled,
            }
        }
    }
}

/// A failure's sentence under its InfoBar `title`: where the title names
/// the file ("Couldn't read this PDF") the sentence doesn't name it again,
/// so "This PDF looks damaged, ..." reads "It looks damaged, ..." (mock
/// screen 8b). The row keeps its own sentence.
pub(crate) fn under_title(title: &str, sentence: String) -> String {
    let named = title
        .strip_prefix("Couldn't read this ")
        .map(|noun| format!("This {noun} looks "));
    match named {
        Some(prefix) if sentence.starts_with(&prefix) => {
            format!("It looks {}", &sentence[prefix.len()..])
        }
        _ => sentence,
    }
}

/// The order `entries`' formats come in, as the wheel has them: by
/// their kind (`kind_of`) and their own format.
fn order_of(entries: &[&Entry], look: &Lookups) -> Order {
    let known: Option<Vec<(String, Kind)>> = entries
        .iter()
        .map(|e| (look.kind_of)(e).map(|k| (e.code.to_ascii_lowercase(), k)))
        .collect();
    known.map_or(Order::Any, |k| {
        Order::of_formats(k.iter().map(|(ext, kind)| (ext.as_str(), *kind)))
    })
}

/// `e`'s formats, lowercase, as the wheel offers them: a video's subtitle
/// formats only once its facts found a subtitle track (spec 3.7).
fn offered_codes(e: &Entry, look: &Lookups) -> Vec<String> {
    let keeps_subtitles = e.has_subtitles() || (look.kind_of)(e) == Some(Kind::Subtitle);
    e.targets
        .iter()
        .map(|t| t.to_ascii_lowercase())
        .filter(|t| keeps_subtitles || !SUBTITLES.contains(&t.as_str()))
        .collect()
}

/// `ext`, lowercase, once into `order` when the file can be converted to
/// it (`offered`).
fn take(order: &mut Vec<String>, offered: &[String], ext: &str) {
    let ext = ext.to_ascii_lowercase();
    if offered.contains(&ext) && !order.contains(&ext) {
        order.push(ext);
    }
}

/// One file's formats as the pane lists them: the suggestion first, then
/// the rest as the wheel ranks them, so the two agree; at most `most`,
/// and whether there are more.
fn ordered_targets(e: &Entry, look: &Lookups, most: usize) -> (Vec<Target>, bool) {
    let facts: Option<&Facts> = e.facts.as_ref();
    let offered = offered_codes(e, look);
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
    let mut rest = offered.clone();
    order_of(&[e], look).sort(&mut rest);
    for ext in &rest {
        take(&mut order, &offered, ext);
    }
    let more = order.len() > most;
    let targets = order
        .into_iter()
        .take(most)
        .map(|ext| {
            let is_suggestion = suggested.as_deref() == Some(ext.as_str());
            let estimate = e
                .suggestion
                .as_ref()
                .filter(|_| is_suggestion)
                .and_then(|s| s.estimate)
                .or_else(|| facts.and_then(|f| f.estimates.get(&ext).copied()))
                .filter(|&bytes| bytes >= MIN_ESTIMATE);
            Target {
                why: describe_format(&ext),
                ext,
                estimate,
                suggested: is_suggestion,
            }
        })
        .collect();
    (targets, more)
}

/// Whether Make smaller applies to `e`: as the wheel offers it, and never
/// for a file the look on adding ruled out, such as a JPG cut short.
fn smaller_for(e: &Entry, look: &Lookups) -> bool {
    e.check == kiln_core::FileCheck::Ok && (look.smaller_of)(e)
}

fn waiting(row: usize, e: &Entry, look: &Lookups) -> Inspector {
    let facts: Option<&Facts> = e.facts.as_ref();
    let mut lines = vec![("Size", human_size(e.size))];
    if let Some(p) = facts.and_then(|f| f.picture.as_ref()) {
        lines.push(("Dimensions", format!("{} × {}", p.width, p.height)));
    }
    let length = facts
        .and_then(|f| f.media.as_ref())
        .and_then(|m| m.duration)
        .filter(|d| *d > 0.0);
    if let Some(d) = length {
        lines.push(("Length", crate::quality::clock(d)));
    }
    lines.push(("Folder", e.folder_name()));

    let (targets, more) = ordered_targets(e, look, MAX_TARGETS);
    let order_empty = targets.is_empty();
    // The helper app it waits on, or the Store extension its picture needs, which it says in place of what the format is, as its row does.
    let extension = e.extension_line();
    // A conversion left it out for a missing helper: Get it, as its row
    // has it.
    let helper = e
        .waits_on
        .as_deref()
        .and_then(crate::model::waits_on_link)
        .or_else(|| e.needs_helper().and_then(get_link))
        .or(e.needs_extension().map(|(_, product)| store_label(product)))
        // Some formats wait on a helper and others are offered: Get it
        // follows them, named for the helper.
        .or_else(|| {
            e.partly_needs()
                .filter(|b| crate::model::waits_on_link(b).is_some())
                .map(crate::model::get_label)
        });
    // A file a conversion left out for a missing helper says the row's own
    // line, "Needs FFmpeg to become MOV", where what the format is would
    // be, so Get it says what it gets.
    let left_out = e.waits_on.as_ref().and_then(|_| (look.note_of)(&e.path));
    // With nothing to offer, not even a helper app to get, it says why, as
    // its row does, where what the format is would be; so does a video
    // whose sound Kiln can't read.
    let reason = extension
        .or_else(|| e.sound_line())
        .map(|x| (x, Tone::Muted))
        .or(left_out)
        .or_else(|| (order_empty && helper.is_none()).then(|| (e.reach(), e.line().1)));
    Inspector::Waiting {
        row,
        facts: lines,
        targets,
        more,
        helper,
        reason,
        smaller: smaller_for(e, look),
    }
}

fn several(rows: &[usize], entries: &[Entry], look: &Lookups) -> Inspector {
    let picked: Vec<&Entry> = rows.iter().map(|&i| &entries[i]).collect();
    let kinds: Vec<Option<Kind>> = picked.iter().map(|e| (look.kind_of)(e)).collect();

    // What they can become, as the wheel offers it: for files of one kind,
    // what any of them can become (UX-06); for several kinds, what every
    // one can. Each file's codes are lowercased once, into a set the
    // shared ones are checked against, and once nothing is shared the rest
    // aren't looked at. Then they are ranked as the wheel ranks them.
    let one_kind = kinds[0].is_some() && kinds.iter().all(|k| *k == kinds[0]);
    let mut shared: Vec<String> = vec![];
    for ext in offered_codes(picked[0], look) {
        if !shared.contains(&ext) {
            shared.push(ext);
        }
    }
    let mut theirs: HashSet<String> = HashSet::new();
    for e in &picked[1..] {
        if one_kind {
            for ext in offered_codes(e, look) {
                if !shared.contains(&ext) {
                    shared.push(ext);
                }
            }
            continue;
        }
        if shared.is_empty() {
            break;
        }
        theirs.clear();
        theirs.extend(offered_codes(e, look));
        shared.retain(|ext| theirs.contains(ext));
    }
    order_of(&picked, look).sort(&mut shared);

    // Every file waiting on the same helper app that isn't installed: what
    // the wheel says for them, and the button that gets it.
    let first_helper = picked[0].needs_helper();
    let helper = first_helper.filter(|&b| picked.iter().all(|e| e.needs_helper() == Some(b)));

    // With nothing in common, the kinds present, most files first and a
    // tie in the order they were met, as the wheel says it, with no
    // sentence. With only one kind there is none else to narrow to, so
    // none is offered and the sentence says why.
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
            // Kinds that share nothing: only the kinds. The InfoBar at
            // the top of the list says why after a drop.
            _ => (None, codes),
        }
    } else {
        (None, vec![])
    };

    let kinds_said = match (one_kind, kinds[0]) {
        (true, Some(k)) => all_of(k).to_owned(),
        _ => kinds_line(&kinds),
    };
    Inspector::Several {
        rows: rows.iter().copied().take(3).collect(),
        count: rows.len(),
        kinds: kinds_said,
        mixed: !one_kind,
        more: shared.len() > MAX_TARGETS,
        smaller: !shared.is_empty() && one_kind && picked.iter().all(|e| smaller_for(e, look)),
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
        helper: helper.and_then(get_link),
    }
}

/// The files a format chosen in the side pane is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Picks {
    /// The one file it shows, by its row.
    Row(usize),
    /// Every selected file, not just those pictured. Those already in
    /// the format are left alone when it starts (UX-06).
    Selection,
}

impl Inspector {
    /// What choosing control `c` asks for: the format it shows, for the
    /// files it is for. `None` for anything that isn't a format.
    pub fn format_pick(&self, c: InspectorControl) -> Option<(&str, Picks)> {
        let InspectorControl::Target(k) = c else {
            return None;
        };
        match self {
            Inspector::Waiting { row, targets, .. } => {
                Some((targets.get(k)?.ext.as_str(), Picks::Row(*row)))
            }
            Inspector::Done { row, again, .. } => {
                Some((again.get(k)?.ext.as_str(), Picks::Row(*row)))
            }
            Inspector::Several { targets, .. } => {
                Some((targets.get(k)?.ext.as_str(), Picks::Selection))
            }
            _ => None,
        }
    }

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
                smaller,
                ..
            } => std::iter::once(C::Folder)
                .chain(smaller.then_some(C::Smaller))
                .chain((0..targets.len()).map(C::Target))
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
                smaller,
                ..
            } => match (targets.is_empty(), helper) {
                (false, _) => smaller
                    .then_some(C::Smaller)
                    .into_iter()
                    .chain((0..targets.len()).map(C::Target))
                    .chain((*more).then_some(C::More))
                    .collect(),
                (true, Some(_)) => vec![C::GetHelper],
                (true, None) => (0..offered.len()).map(C::Kind).collect(),
            },
            // Progress only: Cancel is on the row.
            Inspector::Converting { .. } => vec![],
            // The success InfoBar's buttons, Options, then Convert again.
            Inspector::Done {
                kept,
                options,
                smaller,
                again,
                ..
            } => match kept {
                Kept::InFolder => vec![C::ShowInFolder, C::Open],
                Kept::Held | Kept::Gone => vec![],
            }
            .into_iter()
            .chain(options.then_some(C::Options))
            .chain(smaller.then_some(C::Smaller))
            .chain((0..again.len()).map(C::Target))
            .collect(),
            // The InfoBar's buttons: Retry where it can help, and Show
            // details. Remove from list is the row menu's and Delete's.
            Inspector::Failed { retry, details, .. } => (*retry)
                .then_some(C::Retry)
                .into_iter()
                .chain((*details).then_some(C::ShowDetails))
                .collect(),
        }
    }

    /// What button `c` says on it: as [`InspectorControl::label`] has it,
    /// and Get as the row's button says it, "Get FFmpeg".
    pub fn label(&self, c: InspectorControl) -> Option<&'static str> {
        match (c, self) {
            (
                InspectorControl::GetHelper,
                Inspector::Waiting { helper, .. } | Inspector::Several { helper, .. },
            ) => *helper,
            _ => c.label(),
        }
    }

    /// Whether control `c` is drawn as the accent button: the Saved
    /// InfoBar's Show in folder (mock screen 5).
    pub fn accent(&self, c: InspectorControl) -> bool {
        matches!(
            (self, c),
            (
                Inspector::Done {
                    kept: Kept::InFolder,
                    ..
                },
                InspectorControl::ShowInFolder
            )
        )
    }

    /// The formats it lists, as `Target(k)` counts them.
    pub fn targets(&self) -> &[Target] {
        match self {
            Inspector::Waiting { targets, .. } | Inspector::Several { targets, .. } => targets,
            Inspector::Done { again, .. } => again,
            _ => &[],
        }
    }
}

/// An InfoBar the side pane shows: its severity, title and message.
#[derive(Debug, Clone, PartialEq)]
pub struct Bar {
    pub severity: Severity,
    /// Empty for none.
    pub title: String,
    pub message: String,
}

/// A finished file's InfoBar title once saved (mock screen 5).
pub const SAVED: &str = "Saved";
/// A finished file's whose result is on its way, has gone, or failed to
/// save.
pub const CONVERTED: &str = "Converted";

/// `s` with its first letter a capital.
fn capital(s: &str) -> String {
    let mut cs = s.chars();
    cs.next()
        .map(|c| c.to_uppercase().chain(cs).collect())
        .unwrap_or_default()
}

impl Inspector {
    /// The listed file a pane for one file is about, whose name is its
    /// title.
    pub fn file_row(&self) -> Option<usize> {
        match self {
            Inspector::Waiting { row, .. }
            | Inspector::Converting { row, .. }
            | Inspector::Done { row, .. }
            | Inspector::Failed { row, .. } => Some(*row),
            Inspector::Nothing | Inspector::Several { .. } => None,
        }
    }

    /// The InfoBar it shows, if any: a finished file's (success), a
    /// failed one's (critical), a cancelled one's (info), or why several
    /// files share no format (info, untitled). Its actions are the pane's
    /// buttons, as [`Inspector::controls`] lists them.
    pub fn bar(&self) -> Option<Bar> {
        match self {
            Inspector::Done {
                result,
                change,
                kept,
                note,
                place,
                ..
            } => {
                // The result's name, then how it changed and where it is,
                // "53% smaller · in Files", then the row's note.
                let said: Vec<&str> = change
                    .iter()
                    .map(String::as_str)
                    .chain((!place.is_empty()).then_some(place.as_str()))
                    .collect();
                let mut message = result.clone();
                if !said.is_empty() {
                    message.push('\n');
                    message.push_str(&capital(&said.join(crate::model::DETAIL_SEPARATOR)));
                }
                if let Some(n) = note.as_ref().map(|(n, _)| n.as_str()) {
                    message.push('\n');
                    message.push_str(n);
                }
                let crack = note.as_ref().is_some_and(|(_, t)| *t == Tone::Crack);
                let (severity, title) = match (kept, crack) {
                    (_, true) | (Kept::Gone, _) => (Severity::Caution, CONVERTED),
                    (Kept::Held, _) => (Severity::Success, CONVERTED),
                    _ => (Severity::Success, SAVED),
                };
                Some(Bar {
                    severity,
                    title: title.to_owned(),
                    message,
                })
            }
            Inspector::Failed {
                title,
                sentence,
                cancelled,
                ..
            } => Some(Bar {
                severity: if *cancelled {
                    Severity::Info
                } else {
                    Severity::Critical
                },
                title: title.clone(),
                // "Cancelled." says nothing its title doesn't.
                message: if *cancelled && sentence.trim_end_matches('.') == title.as_str() {
                    String::new()
                } else {
                    sentence.clone()
                },
            }),
            Inspector::Several {
                none_shared: Some(why),
                ..
            } => Some(Bar {
                severity: Severity::Info,
                title: String::new(),
                message: why.clone(),
            }),
            _ => None,
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

    /// The estimate as the target's row shows it: "≈ 410 KB" (mock
    /// `.est`). `None` without one.
    pub fn estimate_label(&self) -> Option<String> {
        self.estimate
            .map(|bytes| format!("\u{2248} {}", about(bytes)))
    }
}

/// The kinds of file in a selection, in words, most first and a tie in the
/// order they were met: "3 photos, 1 video". `None` is a file of no kind
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
                Some(Kind::Image) => ("photo", "photos"),
                Some(Kind::Video) => ("video", "videos"),
                Some(Kind::Audio) => ("sound file", "sound files"),
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
/// Within one per cent either way it is "about the same size", and past
/// double it is a multiple, "328 KB → 3.1 MB, 9.5 times larger"
/// ([`change_words`]).
pub fn change_line(before: u64, after: u64) -> String {
    let sizes = format!("{} → {}", human_size(before), human_size(after));
    match change_words(before, after) {
        Some(words) => format!("{sizes}, {words}"),
        // Nothing to take a share of.
        None => sizes,
    }
}

/// How a result's size changed, without the sizes: "83% smaller", "12%
/// larger", "9.5 times larger" from double up, "about the same size"
/// within one per cent either way. `None` when the original is empty.
pub fn change_words(before: u64, after: u64) -> Option<String> {
    if before == 0 {
        return None;
    }
    if u128::from(before.abs_diff(after)) * 100 <= u128::from(before) {
        return Some("about the same size".to_owned());
    }
    let ratio = after as f64 / before as f64;
    Some(if after < before {
        // Only an empty result is all gone; a few bytes left are 99%.
        let share = ((1.0 - ratio) * 100.0).round() as u64;
        format!(
            "{}% smaller",
            if after == 0 { share } else { share.min(99) }
        )
    } else if ratio >= 2.0 {
        let times = (ratio * 10.0).round() / 10.0;
        if times.fract() == 0.0 {
            format!("{times:.0} times larger")
        } else {
            format!("{times:.1} times larger")
        }
    } else {
        format!("{}% larger", ((ratio - 1.0) * 100.0).round() as u64)
    })
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
            job_note: None,
            result_bytes: None,
            waits_on: None,
            ffmpeg_found: true,
            job: JobState::Idle,
            facts: None,
            suggestion: None,
            facts_at: None,
            last: None,
            from_drag: false,
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
            audio_problem: None,
            silent: false,
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
            "MP4" | "MKV" => Some(Kind::Video),
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

    fn no_smaller(_: &Entry) -> bool {
        false
    }

    fn not_saving(_: &Path) -> bool {
        false
    }

    #[test]
    fn a_finished_drop_being_saved_offers_no_options_until_it_is() {
        let mut e = entry("a.png", "png", 1_000, &["jpg"]);
        e.job = JobState::Done {
            action: Action::Convert("jpg".to_owned()),
            outputs: vec![PathBuf::from("held/a.jpg")],
            saved: None,
        };
        e.from_drag = true;
        let list = selecting(vec![e], &[0]);
        let options = |saving: bool| {
            let saving_of = |_: &Path| saving;
            let look = Lookups {
                kind_of: &kind_by_code,
                size_of: &no_size,
                note_of: &no_note,
                saving_of: &saving_of,
                smaller_of: &no_smaller,
            };
            match inspect(&list, &look) {
                Inspector::Done { options, .. } => options,
                other => panic!("{other:?}"),
            }
        };
        assert!(options(false));
        assert!(!options(true));
    }

    fn selecting(entries: Vec<Entry>, rows: &[usize]) -> FileList {
        let mut list = FileList::default();
        for (i, e) in entries.into_iter().enumerate() {
            list.insert(i, e);
        }
        list.select_only(rows);
        list
    }

    #[test]
    fn a_video_waiting_on_its_extension_offers_get_it() {
        use kiln_media::video::VideoCodec;
        use InspectorControl as C;
        // An HEVC MKV with neither Microsoft's extension nor FFmpeg: its
        // formats stay (a copy needs no decoder), the sentence stands where
        // what the format is would, and Get it follows them.
        let mut e = entry("clip.mkv", "MKV", 9_000_000, &["mp4", "mov"]);
        e.missing = Some(crate::model::Missing::new("ffmpeg", 9));
        e.ffmpeg_found = false;
        e.facts = Some(Facts {
            ext: "mkv".to_owned(),
            kind: Kind::Video,
            media: Some(kiln_core::MediaInfo {
                video_codec: Some(VideoCodec::Hevc),
                needs_extension: Some(("HEVC", "9N4WGH0Z6VHQ")),
                ..Default::default()
            }),
            ..harbour_facts()
        });
        let list = selecting(vec![e], &[0]);
        let i = inspect_plain(&list);
        let Inspector::Waiting { reason, helper, .. } = &i else {
            panic!("{i:?}")
        };
        assert_eq!(
            reason,
            &Some((
                "Needs Microsoft's HEVC Video Extensions to re-encode HEVC video.".to_owned(),
                Tone::Muted
            ))
        );
        assert_eq!(*helper, Some("Get HEVC extension"));
        assert_eq!(
            i.controls(),
            [C::Folder, C::Target(0), C::Target(1), C::GetHelper]
        );
        assert_eq!(i.label(C::GetHelper), Some("Get HEVC extension"));
    }

    #[test]
    fn a_file_with_formats_waiting_on_ffmpeg_offers_get_ffmpeg() {
        // a WEBM that becomes sound formats on Kiln's own engine, with
        // 14 more waiting on FFmpeg: the pane offers Get FFmpeg after its
        // formats, as the row says what waits.
        use InspectorControl as C;
        let mut e = entry("sample.webm", "WEBM", 4_600, &["mp3", "m4a", "wav"]);
        e.missing = Some(crate::model::Missing::new("ffmpeg", 14));
        let i = inspect_plain(&selecting(vec![e.clone()], &[0]));
        assert!(
            matches!(
                &i,
                Inspector::Waiting {
                    helper: Some("Get FFmpeg"),
                    ..
                }
            ),
            "{i:?}"
        );
        assert_eq!(i.controls().last(), Some(&C::GetHelper));
        assert!(i.controls().contains(&C::Target(0)));
        assert_eq!(i.label(C::GetHelper), Some("Get FFmpeg"));
        // Running, done or failed, it has no Get.
        e.job = JobState::Cancelled;
        let i = inspect_plain(&selecting(vec![e], &[0]));
        assert!(!i.controls().contains(&C::GetHelper));
    }

    #[test]
    fn a_file_left_out_for_a_missing_helper_says_so_where_its_row_does() {
        // A WEBM left out of a conversion to MOV because FFmpeg isn't
        // there: its row says "Needs FFmpeg to become MOV", and the
        // side pane says the same line, in the row's tone, with Get it.
        let mut e = entry("clip.webm", "WEBM", 900_000, &["mp3", "m4a"]);
        e.waits_on = Some("ffmpeg".into());
        let file = e.path.clone();
        let line = (crate::wheel::needs_line("mov", "ffmpeg"), Tone::Muted);
        let noted = |p: &Path| (p == file).then(|| line.clone());
        let list = selecting(vec![e], &[0]);
        let look = Lookups {
            kind_of: &kind_by_code,
            size_of: &no_size,
            note_of: &noted,
            saving_of: &not_saving,
            smaller_of: &no_smaller,
        };
        let i = inspect(&list, &look);
        let Inspector::Waiting { reason, helper, .. } = &i else {
            panic!("{i:?}")
        };
        assert_eq!(
            reason,
            &Some(("Needs FFmpeg to become MOV".to_owned(), Tone::Muted))
        );
        assert_eq!(*helper, Some("Get FFmpeg"));
    }

    fn inspect_plain(list: &FileList) -> Inspector {
        let look = Lookups {
            kind_of: &kind_by_code,
            size_of: &no_size,
            note_of: &no_note,
            saving_of: &not_saving,
            smaller_of: &no_smaller,
        };
        inspect(list, &look)
    }

    #[test]
    fn one_waiting_file_offers_the_suggestion_then_the_rest() {
        let list = selecting(vec![harbour()], &[0]);
        let i = inspect_plain(&list);
        let Inspector::Waiting {
            row,
            facts,
            targets,
            more,
            helper,
            reason,
            smaller,
        } = i
        else {
            panic!("{i:?}")
        };
        assert_eq!(row, 0);
        assert_eq!(helper, None);
        assert_eq!(reason, None);
        assert!(!smaller);
        // nothing under the file's name for a healthy file.
        assert_eq!(facts[0], ("Size", "2.4 MB".to_owned()));
        assert_eq!(facts[1], ("Dimensions", "1600 × 1200".to_owned()));
        assert_eq!(facts.last().unwrap(), &("Folder", "photos".to_owned()));
        assert_eq!(facts.len(), 3);
        let exts: Vec<_> = targets.iter().map(|t| t.ext.as_str()).collect();
        assert_eq!(exts, ["webp", "jpg", "pdf", "avif", "gif"]);
        assert!(targets[0].suggested && targets[1..].iter().all(|t| !t.suggested));
        assert_eq!(targets[0].estimate, Some(410_000));
        // The others have the estimate the facts hold for them.
        assert_eq!(targets[1].estimate, Some(300_000));
        // All five show, so there is no More.
        assert!(!more);
        assert_eq!(
            targets[0].name(),
            "Convert to WEBP, Small image for the web, about 410 KB"
        );
        assert_eq!(
            targets[1].name(),
            "Convert to JPG, Photo that opens anywhere, about 300 KB"
        );
    }

    #[test]
    fn an_estimate_under_ten_kilobytes_is_not_shown() {
        let mut e = harbour();
        e.suggestion = None;
        let facts = e.facts.as_mut().unwrap();
        facts.estimates.insert("jpg".to_owned(), 9_999);
        facts.estimates.insert("webp".to_owned(), MIN_ESTIMATE);
        let list = selecting(vec![e], &[0]);
        let Inspector::Waiting { targets, .. } = inspect_plain(&list) else {
            panic!("a waiting file")
        };
        let estimate = |ext: &str| targets.iter().find(|t| t.ext == ext).unwrap().estimate;
        // Under 10 KB: none, so the row and its name say nothing of one.
        assert_eq!(estimate("jpg"), None);
        let jpg = targets.iter().find(|t| t.ext == "jpg").unwrap();
        assert_eq!(jpg.estimate_label(), None);
        assert_eq!(jpg.name(), "Convert to JPG, Photo that opens anywhere");
        // At 10 KB it is shown.
        assert_eq!(estimate("webp"), Some(10_000));
        assert_eq!(estimate("avif"), Some(380_000));
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
    fn without_a_suggestion_the_targets_come_as_the_wheel_ranks_them() {
        let mut e = harbour();
        e.suggestion = None;
        e.facts = None;
        let list = selecting(vec![e], &[0]);
        let i = inspect_plain(&list);
        let Inspector::Waiting {
            facts,
            targets,
            more,
            reason,
            ..
        } = i
        else {
            panic!("{i:?}")
        };
        assert_eq!(reason, None, "no kind line for a healthy file (D4)");
        // Without facts, only the size and the folder are known.
        let names: Vec<_> = facts.iter().map(|f| f.0).collect();
        assert_eq!(names, ["Size", "Folder"]);
        // A picture's ring's order (spec 3.7), five of them.
        let exts: Vec<_> = targets.iter().map(|t| t.ext.as_str()).collect();
        assert_eq!(exts, ["jpg", "webp", "pdf", "avif", "gif"]);
        assert!(targets.iter().all(|t| !t.suggested && t.estimate.is_none()));
        assert!(!more);
        // Three targets or fewer are all shown, and there is no More.
        let mut e = harbour();
        e.targets.truncate(3);
        let list = selecting(vec![e], &[0]);
        let Inspector::Waiting { targets, more, .. } = inspect_plain(&list) else {
            panic!()
        };
        assert_eq!(targets.len(), 3);
        assert!(!more);
    }

    #[test]
    fn a_suggestion_the_file_cannot_take_is_left_out() {
        let mut e = harbour();
        e.suggestion = Some(Suggestion {
            action: Suggested::Convert("jxl".to_owned()),
            reason: String::new(),
            estimate: Some(100_000),
        });
        let list = selecting(vec![e], &[0]);
        let Inspector::Waiting { targets, .. } = inspect_plain(&list) else {
            panic!()
        };
        let exts: Vec<_> = targets.iter().map(|t| t.ext.as_str()).collect();
        assert_eq!(exts, ["jpg", "webp", "pdf", "avif", "gif"]);
        assert!(targets.iter().all(|t| !t.suggested));
    }

    #[test]
    fn targets_are_extensions_however_the_row_spells_its_codes() {
        // The list holds codes ("WEBP"); the inspector speaks extensions.
        let mut e = harbour();
        e.targets = ["JPG", "WEBP", "AVIF", "GIF", "PDF"]
            .map(str::to_owned)
            .to_vec();
        e.code = "JPEG".to_owned();
        e.facts.as_mut().unwrap().ext = "jpg".to_owned();
        let list = selecting(vec![e], &[0]);
        let Inspector::Waiting { targets, .. } = inspect_plain(&list) else {
            panic!()
        };
        let exts: Vec<_> = targets.iter().map(|t| t.ext.as_str()).collect();
        assert_eq!(exts, ["webp", "jpg", "pdf", "gif", "avif"]);
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
            ..Default::default()
        });
        f.picture.as_mut().unwrap().width = 1920;
        f.picture.as_mut().unwrap().height = 1080;
        e.facts = Some(f);
        let list = selecting(vec![e], &[0]);
        let Inspector::Waiting { facts, .. } = inspect_plain(&list) else {
            panic!()
        };
        assert_eq!(
            facts,
            [
                ("Size", "40 MB".to_owned()),
                ("Dimensions", "1920 × 1080".to_owned()),
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
            inspect_plain(&selecting(vec![e], &[0]))
        };
        // What it is doing, and until there is enough to go on, no time.
        assert_eq!(
            inspect_job(running(Some(0.38), None)),
            Inspector::Converting {
                row: 0,
                doing: "Converting to WEBP".to_owned(),
                percent: Some(38),
                left: None,
            }
        );
        // Ten seconds in and 38% done: about 16 s to go.
        let started = Instant::now().checked_sub(Duration::from_secs(10));
        assert_eq!(
            inspect_job(running(Some(0.38), started)),
            Inspector::Converting {
                row: 0,
                doing: "Converting to WEBP".to_owned(),
                percent: Some(38),
                left: Some("About 15 sec left".to_owned()),
            }
        );
        // An engine that can't say how far along it is.
        assert_eq!(
            inspect_job(running(None, started)),
            Inspector::Converting {
                row: 0,
                doing: "Converting to WEBP".to_owned(),
                percent: None,
                left: None,
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

        // Past double it is a multiple, under double the per cent stays.
        assert_eq!(
            change_line(328_000, 3_100_000),
            "328 KB → 3.1 MB, 9.5 times larger"
        );
        // A result that is a few bytes of a large file is never "100% smaller": that says nothing is left.
        assert_eq!(change_words(145_000_000, 33_000).unwrap(), "99% smaller");
        assert_eq!(change_words(1_000_000, 0).unwrap(), "100% smaller");
        assert_eq!(change_words(1_000_000, 1_990_000).unwrap(), "99% larger");
        assert_eq!(
            change_words(1_000_000, 2_000_000).unwrap(),
            "2 times larger"
        );
        assert_eq!(
            change_words(1_000_000, 2_040_000).unwrap(),
            "2 times larger"
        );
        assert_eq!(
            change_words(1_000_000, 3_500_000).unwrap(),
            "3.5 times larger"
        );
        assert_eq!(
            change_words(1_000_000, 12_000_000).unwrap(),
            "12 times larger"
        );
        assert_eq!(change_words(2_400_000, 410_000).unwrap(), "83% smaller");
        assert_eq!(change_words(0, 5_000), None);

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
            saving_of: &not_saving,
            smaller_of: &no_smaller,
        };
        let said = |i: Inspector| match i {
            Inspector::Done {
                row,
                result,
                change,
                kept,
                note,
                options,
                place,
                ..
            } => (row, result, change, kept, note, options, place),
            other => panic!("{other:?}"),
        };
        assert_eq!(
            said(inspect(&list, &look)),
            (
                0,
                "harbour.webp".to_owned(),
                Some("83% smaller".to_owned()),
                Kept::Held,
                None,
                false,
                String::new(),
            )
        );
        // Without the output's size there is no before and after.
        assert_eq!(said(inspect_plain(&list)).2, None);
        let kept_of = |e: &Entry| {
            let list = selecting(vec![e.clone()], &[0]);
            let Inspector::Done { kept, .. } = inspect_plain(&list) else {
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
            saving_of: &not_saving,
            smaller_of: &no_smaller,
        };
        let list = selecting(vec![e.clone()], &[0]);
        let Inspector::Done { result, change, .. } = inspect(&list, &look) else {
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
        let Inspector::Done { result, .. } = inspect_plain(&list) else {
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
                store_product: None,
            };
            e.last = last;
            let want = (e.line(), e.retryable());
            let list = selecting(vec![e], &[0]);
            (inspect_plain(&list), want)
        };
        let again = Some(Action::Convert("webp".to_owned()));
        let (i, ((sentence, tone), retry)) = failed(
            "The conversion stopped before it finished. Try again.",
            again.clone(),
            "",
        );
        assert!(retry);
        // Said in the row's own words and tone. Show details is on every
        // failure (UX-13, Task 18).
        assert_eq!(tone, Tone::Crack);
        assert_eq!(
            i,
            Inspector::Failed {
                row: 0,
                title: "Couldn't convert this file".to_owned(),
                sentence,
                tone,
                retry: true,
                details: true,
                cancelled: false,
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
                title: "Couldn't read this PNG".to_owned(),
                sentence,
                tone,
                retry: false,
                details: true,
                cancelled: false,
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
        let i = inspect_plain(&list);
        assert_eq!(
            i,
            Inspector::Failed {
                row: 0,
                title: CANCELLED.to_owned(),
                sentence: "Cancelled.".to_owned(),
                tone,
                retry: true,
                details: false,
                cancelled: true,
            }
        );
        assert_eq!(i.controls(), [InspectorControl::Retry]);
    }
    #[test]
    fn a_damaged_pdf_says_it_in_the_mocks_words_with_no_retry() {
        // D33 / mock screen 8b: the title names the PDF, so its line starts
        // "It looks damaged", and there is no Retry.
        let mut e = entry("scan-2019.pdf", "PDF", 57_000, &["docx", "txt"]);
        e.last = Some(Action::Convert("docx".to_owned()));
        e.job = JobState::Failed {
            message:
                "This file looks damaged, or isn't really a PDF. Try opening it in another app."
                    .to_owned(),
            details: "the PDF couldn't be read (xref table not found).".to_owned(),
            engine: Some("Kiln"),
            store_product: None,
        };
        let tone = e.line().1;
        let list = selecting(vec![e], &[0]);
        let i = inspect_plain(&list);
        assert_eq!(
            i,
            Inspector::Failed {
                row: 0,
                title: "Couldn't read this PDF".to_owned(),
                sentence: "It looks damaged, or isn't really a PDF. Try opening it in another app."
                    .to_owned(),
                tone,
                retry: false,
                details: true,
                cancelled: false,
            }
        );
        assert_eq!(i.controls(), [InspectorControl::ShowDetails]);
    }

    #[test]
    fn the_pane_words_are_plain_and_one_word_to_a_kind() {
        assert_eq!(NOTHING_TITLE, "Select a file");
        assert_eq!(NOTHING_HINT, "Its formats and details show here.");
        assert_eq!(ONE_KIND, "Convert one format at a time");
        assert_eq!(SMALLER_WHY, "Same format, smaller file");
        assert_eq!(all_of(Kind::Image), "All photos");
        assert_eq!(all_of(Kind::Audio), "All sound files");
        assert_eq!(kinds_line(&[Some(Kind::Audio)]), "1 sound file");
    }

    #[test]
    fn several_files_name_their_kinds_most_first() {
        use Kind::*;
        assert_eq!(
            kinds_line(&[Some(Image), Some(Video), Some(Image), Some(Image)]),
            "3 photos, 1 video"
        );
        assert_eq!(
            kinds_line(&[Some(Audio), None]),
            "1 sound file, 1 other file"
        );
        // A tie keeps the order the kinds were met in.
        assert_eq!(
            kinds_line(&[Some(Document), Some(Image), Some(Document), Some(Image)]),
            "2 documents, 2 photos"
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
    fn a_jpg_and_a_png_are_offered_what_either_can_become() {
        // UX-06: one kind, so a format any of them can become.
        let list = selecting(
            vec![
                entry("a.jpg", "JPG", 1_000, &["png", "webp", "gif"]),
                entry("b.png", "PNG", 1_000, &["jpg", "webp", "gif"]),
            ],
            &[0, 1],
        );
        let i = inspect_plain(&list);
        let Inspector::Several {
            targets,
            more,
            none_shared,
            ..
        } = &i
        else {
            panic!("{i:?}")
        };
        let exts: Vec<_> = targets.iter().map(|t| t.ext.as_str()).collect();
        assert_eq!(exts, ["jpg", "png", "webp", "gif"]);
        assert!(!*more && none_shared.is_none());
        // Two kinds share only what both can become.
        let list = selecting(
            vec![
                entry("a.jpg", "JPG", 1_000, &["png", "pdf"]),
                entry("b.docx", "DOCX", 1_000, &["pdf", "txt"]),
            ],
            &[0, 1],
        );
        let Inspector::Several { targets, .. } = inspect_plain(&list) else {
            panic!()
        };
        let exts: Vec<_> = targets.iter().map(|t| t.ext.as_str()).collect();
        assert_eq!(exts, ["pdf"]);
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
        let i = inspect_plain(&list);
        let Inspector::Several {
            rows,
            kinds,
            targets,
            more,
            none_shared,
            offered,
            helper,
            ..
        } = i
        else {
            panic!("{i:?}")
        };
        assert_eq!(rows, [0, 1, 2]);
        assert_eq!(kinds, "All photos");
        // What any of them can become (UX-06), PDF from the second, as a
        // picture's ring ranks them (spec 3.7): all five, none suggested,
        // so no More.
        let exts: Vec<_> = targets.iter().map(|t| t.ext.as_str()).collect();
        assert_eq!(exts, ["jpg", "webp", "pdf", "avif", "gif"]);
        assert!(!more);
        assert_eq!(helper, None);
        assert!(targets.iter().all(|t| !t.suggested && t.estimate.is_none()));
        assert_eq!(
            targets[1].name(),
            "Convert to WEBP, Small image for the web"
        );
        assert_eq!(none_shared, None);
        // Sharing formats, there is no kind to choose between.
        assert!(offered.is_empty());
    }

    #[test]
    fn a_format_in_the_panel_opens_its_options_for_the_files_it_is_for() {
        use InspectorControl as C;
        let png = |name: &str| entry(name, "PNG", 1_000, &["jpg", "webp", "gif"]);
        // One file: its own row, whichever format.
        let one = inspect_plain(&selecting(vec![png("a.png"), png("b.png")], &[1]));
        let Inspector::Waiting { targets, .. } = &one else {
            panic!("{one:?}")
        };
        for (k, t) in targets.iter().enumerate() {
            assert_eq!(
                one.format_pick(C::Target(k)),
                Some((t.ext.as_str(), Picks::Row(1)))
            );
        }
        // Several: every selected file.
        let list = selecting(vec![png("a.png"), png("b.png"), png("c.png")], &[0, 2]);
        let several = inspect_plain(&list);
        let Inspector::Several { targets, .. } = &several else {
            panic!("{several:?}")
        };
        assert_eq!(
            several.format_pick(C::Target(1)),
            Some((targets[1].ext.as_str(), Picks::Selection))
        );
        // More, a format past the end, and the other buttons ask for no
        // sheet.
        for c in [C::More, C::Target(9), C::Retry, C::GetHelper] {
            assert_eq!(one.format_pick(c), None, "{c:?}");
            assert_eq!(several.format_pick(c), None, "{c:?}");
        }
        assert_eq!(Inspector::Nothing.format_pick(C::Target(0)), None);
    }

    #[test]
    fn several_files_with_nothing_shared_explain_it_and_offer_each_kind() {
        let mp4 = |name: &str| entry(name, "MP4", 1_000, &["mkv", "webm"]);
        let docx = entry("c.docx", "DOCX", 1_000, &["pdf", "odt"]);
        let list = selecting(vec![mp4("a.mp4"), docx.clone(), mp4("b.mp4")], &[0, 1, 2]);
        let i = inspect_plain(&list);
        let Inspector::Several {
            rows,
            kinds,
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
        assert_eq!(rows, &[0, 1, 2]);
        assert_eq!(kinds, "2 videos, 1 document");
        assert!(targets.is_empty() && !more);
        assert_eq!(*helper, None);
        // No sentence and no InfoBar in the side pane: the InfoBar
        // at the top of the list says it after a drop.
        assert_eq!(*none_shared, None);
        assert_eq!(i.bar(), None);
        // Each kind is offered, most files first, as a control.
        assert_eq!(offered, &[("MP4".to_owned(), 2), ("DOCX".to_owned(), 1)]);
        assert_eq!(
            i.controls(),
            [InspectorControl::Kind(0), InspectorControl::Kind(1)]
        );

        // One of each: the order they were met in.
        let list = selecting(vec![mp4("a.mp4"), docx.clone()], &[0, 1]);
        let Inspector::Several { offered, .. } = inspect_plain(&list) else {
            panic!()
        };
        assert_eq!(offered, [("MP4".to_owned(), 1), ("DOCX".to_owned(), 1)]);
        // A kind with more files comes first, whatever was met first.
        let list = selecting(vec![docx, mp4("a.mp4"), mp4("b.mp4")], &[0, 1, 2]);
        let Inspector::Several { offered, .. } = inspect_plain(&list) else {
            panic!()
        };
        assert_eq!(offered, [("MP4".to_owned(), 2), ("DOCX".to_owned(), 1)]);
        // Files of one kind that share nothing have no other kind to
        // narrow to, so offer none.
        let odd = |name: &str| entry(name, "MP4", 1_000, &[]);
        let list = selecting(vec![odd("a.mp4"), odd("b.mp4")], &[0, 1]);
        let i = inspect_plain(&list);
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
    fn an_mp4_with_no_suggestion_is_offered_what_the_wheel_shows_first() {
        // The registry lists a file's formats alphabetically, which would
        // put 3GP, AAC and AC3 first. The wheel ranks them by what people
        // reach for, and the inspector agrees with it.
        let codes = [
            "3gp", "aac", "ac3", "aiff", "avi", "flac", "flv", "gif", "m4a", "m4v", "mkv", "mov",
            "mp3", "mpg", "ogg", "ogv", "opus", "ts", "wav", "webm", "wma", "wmv",
        ];
        let e = entry("clip.mp4", "MP4", 40_000_000, &codes);
        let list = selecting(vec![e], &[0]);
        let i = inspect_plain(&list);
        let Inspector::Waiting { targets, more, .. } = &i else {
            panic!("{i:?}")
        };
        let exts: Vec<_> = targets.iter().map(|t| t.ext.as_str()).collect();
        // A video's ring (spec 3.7), as the wheel has it.
        assert_eq!(exts, ["mp3", "gif", "mov", "webm", "mkv"]);
        assert!(*more);
        // The same order the wheel sorts the same formats in.
        let mut wheel: Vec<String> = codes.iter().map(|c| (*c).to_owned()).collect();
        crate::wheel::Order::Kind(Kind::Video).sort(&mut wheel);
        assert_eq!(&wheel[..5], exts);
    }

    #[test]
    fn a_video_is_offered_subtitles_only_with_a_subtitle_track() {
        let codes = ["srt", "vtt", "mp4"];
        let mut e = entry("clip.mkv", "MKV", 40_000_000, &codes);
        let exts = |e: &Entry| {
            let i = inspect_plain(&selecting(vec![e.clone()], &[0]));
            let Inspector::Waiting { targets, .. } = i else {
                panic!("{i:?}")
            };
            targets.into_iter().map(|t| t.ext).collect::<Vec<_>>()
        };
        // Until its facts say it has one, none.
        assert_eq!(exts(&e), ["mp4"]);
        e.facts = Some(Facts {
            ext: "mkv".to_owned(),
            kind: Kind::Video,
            media: Some(kiln_core::MediaInfo {
                subtitle_tracks: 1,
                ..Default::default()
            }),
            ..harbour_facts()
        });
        assert_eq!(exts(&e), ["mp4", "srt", "vtt"]);
        // Two videos with none: MP4 alone.
        let other = || entry("other.mkv", "MKV", 1_000, &codes);
        let i = inspect_plain(&selecting(vec![other(), other()], &[0, 1]));
        let Inspector::Several { targets, .. } = i else {
            panic!("{i:?}")
        };
        let exts: Vec<_> = targets.iter().map(|t| t.ext.as_str()).collect();
        assert_eq!(exts, ["mp4"]);
    }

    #[test]
    fn three_pngs_share_their_formats_most_popular_first_and_more() {
        let codes = ["avif", "bmp", "gif", "ico", "jpg", "pdf", "tiff", "webp"];
        let png = |name: &str| entry(name, "PNG", 1_000, &codes);
        let list = selecting(vec![png("a.png"), png("b.png"), png("c.png")], &[0, 1, 2]);
        let i = inspect_plain(&list);
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
        // A picture's ring (spec 3.7), as the wheel has it.
        assert_eq!(exts, ["jpg", "webp", "pdf", "avif", "gif"]);
        assert!(*more && none_shared.is_none() && helper.is_none());
        use InspectorControl as C;
        assert_eq!(
            i.controls(),
            [
                C::Target(0),
                C::Target(1),
                C::Target(2),
                C::Target(3),
                C::Target(4),
                C::More
            ]
        );
        // Three formats or fewer in common are all shown, with no More.
        let few = |name: &str| entry(name, "PNG", 1_000, &["webp", "gif", "jpg"]);
        let list = selecting(vec![few("a.png"), few("b.png")], &[0, 1]);
        let i = inspect_plain(&list);
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
            e.missing = Some(crate::model::Missing::new(backend, 12));
            e
        };
        // Two Markdown files while Pandoc is missing: what the wheel would
        // say, and Get where the kinds would be.
        let list = selecting(
            vec![needs("a.md", "MD", "pandoc"), needs("b.md", "MD", "pandoc")],
            &[0, 1],
        );
        let i = inspect_plain(&list);
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
        let wheel = Offer::NeedsHelper("pandoc".to_owned(), "MD".to_owned());
        assert_eq!(
            none_shared,
            &crate::wheel::offer_sentence(WheelKind::Convert, &wheel)
        );
        assert_eq!(
            none_shared.as_deref(),
            Some("MD files need Pandoc, which isn't installed. Use Get Pandoc to download it.")
        );
        assert_eq!(*helper, Some("Get Pandoc"));
        assert_eq!(i.controls(), [C::GetHelper]);
        assert_eq!(i.label(C::GetHelper), Some("Get Pandoc"));

        // Two types that both wait on Pandoc: one Get still does it.
        let list = selecting(
            vec![
                needs("a.md", "MD", "pandoc"),
                needs("b.rst", "RST", "pandoc"),
            ],
            &[0, 1],
        );
        let i = inspect_plain(&list);
        assert!(matches!(
            &i,
            Inspector::Several { helper: Some("Get Pandoc"), offered, .. } if offered.is_empty()
        ));
        assert_eq!(i.controls(), [C::GetHelper]);

        // Two MP4s while FFmpeg is missing: FFmpeg comes with Kiln, so
        // there is no Get, and the wheel's sentence sends nowhere.
        let list = selecting(
            vec![
                needs("a.mp4", "MP4", "ffmpeg"),
                needs("b.mp4", "MP4", "ffmpeg"),
            ],
            &[0, 1],
        );
        let i = inspect_plain(&list);
        let Inspector::Several {
            none_shared,
            helper,
            ..
        } = &i
        else {
            panic!("{i:?}")
        };
        assert_eq!(
            none_shared.as_deref(),
            Some("MP4 files need FFmpeg, which isn't installed.")
        );
        assert_eq!(*helper, None);
        assert!(!i.controls().contains(&C::GetHelper));

        // Each waiting on a different app: the kinds, with no sentence in
        // the side pane.
        let list = selecting(
            vec![
                needs("a.mkv", "MKV", "ffmpeg"),
                needs("b.docx", "DOCX", "pandoc"),
            ],
            &[0, 1],
        );
        let i = inspect_plain(&list);
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
        assert_eq!(*none_shared, None);
        assert_eq!(i.bar(), None);
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
        let i = inspect_plain(&list);
        assert!(matches!(&i, Inspector::Several { helper: None, .. }));
        assert_eq!(i.controls(), [C::Kind(0), C::Kind(1)]);
    }

    #[test]
    fn a_done_file_whose_result_is_gone_says_so_and_offers_nothing_to_open() {
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
                saving_of: &not_saving,
                smaller_of: &no_smaller,
            };
            inspect(&list, &look)
        };
        // Removed by hand: the row's note, in its tone, and nothing to
        // open or show.
        let gone = (crate::model::GONE.to_owned(), Tone::Crack);
        let i = inspect_noted(Some(gone.clone()));
        let Inspector::Done { kept, note, .. } = &i else {
            panic!("{i:?}")
        };
        assert_eq!(*kept, Kept::Gone);
        assert_eq!(note.as_ref(), Some(&gone));
        // Only Convert again, as its note says.
        use InspectorControl as C;
        assert_eq!(i.controls(), [C::Target(0), C::Target(1), C::Target(2)]);
        // A save that failed: its note is said, and Kiln tries again at the
        // next start, so there is nothing to press.
        let failed = (crate::model::KEPT_LINE.to_owned(), Tone::Crack);
        let i = inspect_noted(Some(failed.clone()));
        let Inspector::Done { kept, note, .. } = &i else {
            panic!("{i:?}")
        };
        assert_eq!(*kept, Kept::Held);
        assert_eq!(note.as_ref(), Some(&failed));
        assert_eq!(i.controls(), [C::Target(0), C::Target(1), C::Target(2)]);
        // With no note, the row says its own line, and so does the panel.
        let i = inspect_noted(None);
        assert!(matches!(
            i,
            Inspector::Done {
                kept: Kept::Held,
                note: None,
                options: false,
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
        let i = inspect_plain(&selecting(vec![ruled_out], &[0]));
        let Inspector::Waiting {
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
        assert!(targets.is_empty() && helper.is_none());
        assert_eq!(i.controls(), [InspectorControl::Folder]);
        // An empty file says so, without the size its facts already give.
        let mut empty = entry("blank.png", "PNG", 0, &[]);
        empty.check = kiln_core::FileCheck::Empty;
        let i = inspect_plain(&selecting(vec![empty], &[0]));
        assert!(matches!(
            &i,
            Inspector::Waiting { reason: Some((r, Tone::Crack)), .. } if r == "Empty file (0 bytes)"
        ));
        // A type Kiln has no formats for, said as quietly as the row does.
        let none = entry("notes.xyz", "XYZ", 1_000, &[]);
        let i = inspect_plain(&selecting(vec![none], &[0]));
        assert!(matches!(
            &i,
            Inspector::Waiting { reason: Some((r, Tone::Muted)), .. } if r == "No formats for this type"
        ));
        // A file with formats, or one with Get, has no reason to give.
        let i = inspect_plain(&selecting(vec![harbour()], &[0]));
        assert!(matches!(i, Inspector::Waiting { reason: None, .. }));
        let mut needs = entry("notes.md", "MD", 1_000, &[]);
        needs.missing = Some(crate::model::Missing::new("pandoc", 12));
        let i = inspect_plain(&selecting(vec![needs], &[0]));
        assert!(matches!(
            i,
            Inspector::Waiting {
                reason: None,
                helper: Some(_),
                ..
            }
        ));
        // One waiting on FFmpeg, which has no Get, says why as its row does.
        let mut needs = entry("clip.mkv", "MKV", 1_000, &[]);
        needs.missing = Some(crate::model::Missing::new("ffmpeg", 12));
        let i = inspect_plain(&selecting(vec![needs], &[0]));
        assert!(matches!(
            &i,
            Inspector::Waiting { reason: Some((r, _)), helper: None, .. }
                if r == "Needs FFmpeg to convert"
        ));
    }

    #[test]
    fn nothing_selected_is_nothing() {
        assert_eq!(inspect_plain(&FileList::default()), Inspector::Nothing);
        // Rows in the list, none of them selected.
        let list = selecting(vec![harbour()], &[]);
        assert_eq!(inspect_plain(&list), Inspector::Nothing);
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
            facts: vec![],
            targets: (0..n).map(|i| target(&format!("f{i}"))).collect(),
            more,
            helper: None,
            reason: None,
            smaller: false,
        };
        assert_eq!(
            waiting(3, true).controls(),
            [C::Folder, C::Target(0), C::Target(1), C::Target(2), C::More]
        );
        assert_eq!(
            waiting(2, false).controls(),
            [C::Folder, C::Target(0), C::Target(1)]
        );
        assert_eq!(waiting(0, false).controls(), [C::Folder]);
        // A file waiting on its helper app has Get in their place.
        let mut needs = waiting(0, false);
        if let Inspector::Waiting { helper, .. } = &mut needs {
            *helper = Some("Get FFmpeg");
        }
        assert_eq!(needs.controls(), [C::Folder, C::GetHelper]);
        assert_eq!(
            Inspector::Converting {
                row: 0,
                doing: String::new(),
                percent: Some(38),
                left: None,
            }
            .controls(),
            []
        );
        let done = |kept| Inspector::Done {
            row: 0,
            result: "a.webp".to_owned(),
            change: None,
            kept,
            note: None,
            options: false,
            place: String::new(),
            smaller: false,
            again: vec![],
        };
        // On its way to its folder: nothing to show yet.
        assert_eq!(done(Kept::Held).controls(), []);
        assert_eq!(done(Kept::InFolder).controls(), [C::ShowInFolder, C::Open]);
        assert_eq!(done(Kept::Gone).controls(), []);
        // A drag-wheel conversion's Options follows the result's button.
        let with_options = |kept| match done(kept) {
            Inspector::Done {
                row,
                result,
                change,
                kept,
                note,
                ..
            } => Inspector::Done {
                row,
                result,
                change,
                kept,
                note,
                options: true,
                place: String::new(),
                smaller: false,
                again: vec![],
            },
            other => other,
        };
        assert_eq!(with_options(Kept::Held).controls(), [C::Options]);
        assert_eq!(
            with_options(Kept::InFolder).controls(),
            [C::ShowInFolder, C::Open, C::Options]
        );
        assert_eq!(C::Options.label(), Some("Options"));
        let failed = |retry, details| Inspector::Failed {
            row: 0,
            title: String::new(),
            sentence: String::new(),
            tone: Tone::Crack,
            retry,
            details,
            cancelled: false,
        };
        // Retry when it can help and Show details, nothing else.
        assert_eq!(failed(true, true).controls(), [C::Retry, C::ShowDetails]);
        assert_eq!(failed(true, false).controls(), [C::Retry]);
        assert_eq!(failed(false, true).controls(), [C::ShowDetails]);
        assert_eq!(failed(false, false).controls(), []);
        let several = |targets: Vec<Target>, offered: Vec<(String, usize)>| Inspector::Several {
            rows: vec![0, 1],
            count: 2,
            kinds: String::new(),
            mixed: false,
            targets,
            none_shared: None,
            offered,
            more: false,
            helper: None,
            smaller: false,
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
        let labels: Vec<_> = [C::ShowInFolder, C::Retry, C::ShowDetails]
            .map(|c| c.label().unwrap())
            .to_vec();
        assert_eq!(labels, ["Show in folder", "Retry", "Show details"]);
        // A format, a kind and More say what they are in their own way.
        assert!([C::Target(0), C::Kind(0), C::More]
            .iter()
            .all(|c| c.label().is_none()));
        assert_eq!(files_count(1), "1 file");
        assert_eq!(files_count(2), "2 files");
        // The inspector says them the same, and Get names its helper app.
        let mut e = entry("notes.md", "MD", 1_000, &[]);
        e.missing = Some(crate::model::Missing::new("pandoc", 12));
        let i = inspect_plain(&selecting(vec![e], &[0]));
        assert_eq!(i.label(C::GetHelper), Some("Get Pandoc"));
        assert_eq!(Inspector::Nothing.label(C::GetHelper), None);
    }

    #[test]
    fn a_file_waiting_on_its_helper_app_offers_to_get_it_where_its_formats_would_be() {
        use InspectorControl as C;
        let mut e = entry("notes.md", "MD", 1_000, &[]);
        e.missing = Some(crate::model::Missing::new("pandoc", 12));
        // Named as the row's link names it, from the same words.
        assert_eq!(e.link(false), Some("Get Pandoc"));
        assert_eq!(e.needs_helper(), Some("pandoc"));
        let i = inspect_plain(&selecting(vec![e.clone()], &[0]));
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
        assert_eq!(*helper, Some("Get Pandoc"));
        assert_eq!(i.controls(), [C::Folder, C::GetHelper]);

        // A file with formats of its own converts to them, and Get for
        // the helper the rest wait on follows them.
        let mut some = entry("clip.mp4", "MP4", 1_000, &["mkv"]);
        some.missing = Some(crate::model::Missing::new("ffmpeg", 2));
        let i = inspect_plain(&selecting(vec![some], &[0]));
        assert!(matches!(
            i,
            Inspector::Waiting {
                helper: Some("Get FFmpeg"),
                ..
            }
        ));
        assert_eq!(i.controls(), [C::Folder, C::Target(0), C::GetHelper]);
        // A file Kiln can't convert at all, with no helper to get, has
        // nothing to offer.
        let none = entry("notes.xyz", "XYZ", 1_000, &[]);
        let i = inspect_plain(&selecting(vec![none], &[0]));
        assert_eq!(i.controls(), [C::Folder]);
        // Nor does one ruled out on adding.
        let mut ruled_out = e;
        ruled_out.check = kiln_core::FileCheck::Empty;
        assert_eq!(ruled_out.needs_helper(), None);
        let i = inspect_plain(&selecting(vec![ruled_out], &[0]));
        assert_eq!(i.controls(), [C::Folder]);
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
        for shared in [true, false] {
            let list = five_hundred(shared);
            let i = inspect_plain(&list);
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
                std::hint::black_box(inspect_plain(std::hint::black_box(&list)));
            }
            let each = start.elapsed() / runs;
            println!(
                "inspect() for 500 selected files, {}: {each:?}",
                if shared { "all sharing" } else { "half video" }
            );
            assert!(each < Duration::from_millis(50), "{each:?}");
        }
    }

    #[test]
    fn make_smaller_comes_first_under_convert_to_where_it_is_offered() {
        use InspectorControl as C;
        let list = selecting(vec![harbour()], &[0]);
        let offered = |_: &Entry| true;
        let look = Lookups {
            kind_of: &kind_by_code,
            size_of: &no_size,
            note_of: &no_note,
            saving_of: &not_saving,
            smaller_of: &offered,
        };
        let i = inspect(&list, &look);
        assert!(matches!(i, Inspector::Waiting { smaller: true, .. }));
        assert_eq!(i.controls()[1..3], [C::Smaller, C::Target(0)]);
        assert_eq!(C::Smaller.label(), None);
        // Several photos: first under Convert them to, too.
        let list = selecting(vec![harbour(), harbour()], &[0, 1]);
        let i = inspect(&list, &look);
        assert_eq!(i.controls()[..2], [C::Smaller, C::Target(0)]);
        // Where the lookup says it doesn't apply, the pane has none.
        assert!(!inspect_plain(&selecting(vec![harbour()], &[0]))
            .controls()
            .contains(&C::Smaller));
    }

    #[test]
    fn a_file_that_looks_cut_short_offers_no_make_smaller() {
        // a JPG cut short can't be made smaller either.
        use InspectorControl as C;
        let mut e = entry("cut.jpg", "JPG", 1_000, &[]);
        e.check = FileCheck::CutShort;
        let offered = |_: &Entry| true;
        let look = Lookups {
            kind_of: &kind_by_code,
            size_of: &no_size,
            note_of: &no_note,
            saving_of: &not_saving,
            smaller_of: &offered,
        };
        let i = inspect(&selecting(vec![e.clone()], &[0]), &look);
        assert!(!i.controls().contains(&C::Smaller), "{i:?}");
        let i = inspect(&selecting(vec![e.clone(), e], &[0, 1]), &look);
        assert!(!i.controls().contains(&C::Smaller), "{i:?}");
    }

    #[test]
    fn the_saved_infobars_show_in_folder_is_the_accent() {
        // D37 / mock screen 5: Show in folder is the accent; Open isn't.
        use InspectorControl as C;
        let mut e = harbour();
        e.job = JobState::Done {
            action: Action::Convert("webp".to_owned()),
            outputs: vec![PathBuf::from("photos").join("harbour.webp")],
            saved: Some(Saved::Folder(PathBuf::from("photos"))),
        };
        let i = inspect_plain(&selecting(vec![e], &[0]));
        assert!(i.accent(C::ShowInFolder));
        assert!(!i.accent(C::Open));
        let mut e = harbour();
        e.job = JobState::Failed {
            message: "The conversion stopped before it finished. Try again.".to_owned(),
            details: String::new(),
            engine: None,
            store_product: None,
        };
        e.last = Some(Action::Convert("webp".to_owned()));
        let i = inspect_plain(&selecting(vec![e], &[0]));
        assert!(i.controls().iter().all(|c| !i.accent(*c)));
    }

    #[test]
    fn convert_again_starts_with_make_smaller_where_it_is_offered() {
        // D37 / mock screen 5: under Convert again, Make smaller first,
        // then two formats: three lines, as the mock's compact list.
        use InspectorControl as C;
        let mut e = harbour();
        e.job = JobState::Done {
            action: Action::Convert("webp".to_owned()),
            outputs: vec![PathBuf::from("photos").join("harbour.webp")],
            saved: Some(Saved::Folder(PathBuf::from("photos"))),
        };
        let list = selecting(vec![e], &[0]);
        let offered = |_: &Entry| true;
        let look = Lookups {
            kind_of: &kind_by_code,
            size_of: &no_size,
            note_of: &no_note,
            saving_of: &not_saving,
            smaller_of: &offered,
        };
        let i = inspect(&list, &look);
        assert!(matches!(&i, Inspector::Done { smaller: true, again, .. } if again.len() == 2));
        assert_eq!(
            i.controls(),
            [
                C::ShowInFolder,
                C::Open,
                C::Smaller,
                C::Target(0),
                C::Target(1)
            ]
        );
        // Where it doesn't apply: three formats, no Make smaller.
        let i = inspect_plain(&list);
        assert!(matches!(&i, Inspector::Done { smaller: false, again, .. } if again.len() == 3));
        assert!(!i.controls().contains(&C::Smaller));
    }

    #[test]
    fn several_of_one_kind_say_so_and_mixed_kinds_have_no_side_pane_infobar() {
        let png = |name: &str| entry(name, "PNG", 1_000, &["jpg", "webp"]);
        let list = selecting(vec![png("a.png"), png("b.png"), png("c.png")], &[0, 1, 2]);
        let i = inspect_plain(&list);
        let Inspector::Several {
            count,
            kinds,
            mixed,
            ..
        } = &i
        else {
            panic!("{i:?}")
        };
        assert_eq!((*count, kinds.as_str(), *mixed), (3, "All photos", false));
        assert_eq!(i.bar(), None);
        assert_eq!(files_count(*count), "3 files");
        // An MP4 and a DOCX share nothing: each kind is offered, and the
        // side pane has no InfoBar; the top of the list says it.
        let mp4 = entry("a.mp4", "MP4", 1_000, &["mkv"]);
        let docx = entry("c.docx", "DOCX", 1_000, &["pdf"]);
        let i = inspect_plain(&selecting(vec![mp4, docx], &[0, 1]));
        let Inspector::Several {
            mixed, none_shared, ..
        } = &i
        else {
            panic!("{i:?}")
        };
        assert!(*mixed);
        assert_eq!(*none_shared, None);
        assert_eq!(i.bar(), None);
        // Files waiting on one helper still say so in an info InfoBar,
        // with no title.
        let md = |name: &str| {
            let mut e = entry(name, "MD", 1_000, &[]);
            e.missing = Some(crate::model::Missing::new("pandoc", 12));
            e
        };
        let i = inspect_plain(&selecting(vec![md("a.md"), md("b.md")], &[0, 1]));
        let bar = i.bar().unwrap();
        assert_eq!(bar.severity, Severity::Info);
        assert!(bar.title.is_empty());
        assert!(bar.message.starts_with("MD files need Pandoc"), "{bar:?}");
    }

    #[test]
    fn a_converting_file_says_how_long_is_left_in_the_panes_words() {
        assert_eq!(left_line(Duration::from_secs(4)), "A few seconds left");
        assert_eq!(left_line(Duration::from_secs(40)), "About 40 sec left");
        assert_eq!(left_line(Duration::from_secs(150)), "About 3 min left");
        assert_eq!(
            left_line(Duration::from_secs(4800)),
            "About 1 h 20 min left"
        );
    }

    #[test]
    fn a_done_file_says_saved_where_and_offers_open_and_convert_again() {
        use InspectorControl as C;
        let out = PathBuf::from("photos").join("harbour.webp");
        let mut e = harbour();
        e.job = JobState::Done {
            action: Action::Convert("webp".to_owned()),
            outputs: vec![out.clone()],
            saved: Some(Saved::Folder(PathBuf::from("photos"))),
        };
        let list = selecting(vec![e], &[0]);
        let seen = |p: &Path| (p == out).then_some(410_000);
        let look = Lookups {
            kind_of: &kind_by_code,
            size_of: &seen,
            note_of: &no_note,
            saving_of: &not_saving,
            smaller_of: &no_smaller,
        };
        let i = inspect(&list, &look);
        let Inspector::Done { place, again, .. } = &i else {
            panic!("{i:?}")
        };
        assert_eq!(place, "in photos");
        let exts: Vec<_> = again.iter().map(|t| t.ext.as_str()).collect();
        assert_eq!(exts, ["webp", "jpg", "pdf"]);
        assert_eq!(
            i.bar(),
            Some(Bar {
                severity: Severity::Success,
                title: SAVED.to_owned(),
                message: "harbour.webp\n83% smaller · in photos".to_owned(),
            })
        );
        assert_eq!(
            i.controls(),
            [
                C::ShowInFolder,
                C::Open,
                C::Target(0),
                C::Target(1),
                C::Target(2)
            ]
        );
        assert_eq!(i.format_pick(C::Target(1)), Some(("jpg", Picks::Row(0))));
        assert_eq!(C::Open.label(), Some("Open"));
        // Held: converted, on its way to its folder, nothing to press.
        let mut e = harbour();
        e.job = JobState::Done {
            action: Action::Convert("webp".to_owned()),
            outputs: vec![PathBuf::from("held").join("harbour.webp")],
            saved: None,
        };
        let i = inspect_plain(&selecting(vec![e], &[0]));
        let bar = i.bar().unwrap();
        assert_eq!(bar.title, CONVERTED);
        assert_eq!(bar.message, "harbour.webp");
        assert!(!i.controls().contains(&C::Open));
        assert!(!i.controls().contains(&C::ShowInFolder));
    }

    #[test]
    fn a_failed_file_has_a_critical_infobar_with_retry_and_show_details() {
        use InspectorControl as C;
        let mut e = harbour();
        e.job = JobState::Failed {
            message: "The conversion stopped before it finished. Try again.".to_owned(),
            details: String::new(),
            engine: None,
            store_product: None,
        };
        e.last = Some(Action::Convert("webp".to_owned()));
        let i = inspect_plain(&selecting(vec![e], &[0]));
        let bar = i.bar().unwrap();
        assert_eq!(bar.severity, Severity::Critical);
        assert_eq!(bar.title, "Couldn't convert this file");
        assert_eq!(
            bar.message,
            "The conversion stopped before it finished. Try again."
        );
        assert_eq!(i.controls(), [C::Retry, C::ShowDetails]);
        // Cancelled: an info InfoBar that says so once.
        let mut e = harbour();
        e.job = JobState::Cancelled;
        let i = inspect_plain(&selecting(vec![e], &[0]));
        let bar = i.bar().unwrap();
        assert_eq!(
            (bar.severity, bar.title.as_str(), bar.message.as_str()),
            (Severity::Info, CANCELLED, "")
        );
    }
}
