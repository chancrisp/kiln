//! The Make smaller and Resize dialogs as pure models (spec 3.2):
//! what each offers for the files it is for, which choices are picked and
//! which can't be, what the estimate line says, and the tool options a
//! press of its accent button starts the job with. Nothing here draws or
//! knows of a window; `win::surfaces::smaller` lays the dialog out, takes
//! its keys and asks a worker for the estimate.

use std::path::PathBuf;

use kiln_core::{about, Tool, ToolOptions};

use crate::controls::{ComboState, Expander, Radios, TextBoxState};
use crate::layout::kit::{DialogLayout, ExpanderParts, FlyoutLayout, RadioParts};
use crate::layout::Rect;
use crate::model::human_size;

/// What a Make smaller dialog is for: its presets and More options follow
/// it. Each tool is for one kind of file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shrink {
    Video,
    Photo,
    Pdf,
}

impl Shrink {
    /// The kind of file Make smaller takes `ext` as, for the formats Kiln
    /// writes; `None` for the rest.
    pub fn of(ext: &str) -> Option<Shrink> {
        match ext {
            "mp4" | "mov" | "m4v" | "mkv" => Some(Shrink::Video),
            "jpg" | "png" | "webp" => Some(Shrink::Photo),
            "pdf" => Some(Shrink::Pdf),
            _ => None,
        }
    }

    /// Its three Max size presets, smallest first (the spec's 3.2 table).
    pub fn presets(self) -> [Preset; 3] {
        let p = |bytes, label, note| Preset { bytes, label, note };
        match self {
            Shrink::Video => [
                p(10_000_000, "10 MB", Some("Fits email")),
                p(25_000_000, "25 MB", Some("Fits most chat apps")),
                p(50_000_000, "50 MB", None),
            ],
            Shrink::Photo => [
                p(500_000, "500 KB", None),
                p(1_000_000, "1 MB", None),
                p(2_000_000, "2 MB", None),
            ],
            Shrink::Pdf => [
                p(1_000_000, "1 MB", None),
                p(5_000_000, "5 MB", None),
                p(10_000_000, "10 MB", None),
            ],
        }
    }

    /// The lines under More options: none for a PDF.
    pub fn more_lines(self) -> &'static [MoreLine] {
        match self {
            Shrink::Video => &[MoreLine::PictureSize, MoreLine::Sound],
            Shrink::Photo => &[MoreLine::Resize, MoreLine::RemoveLocation],
            Shrink::Pdf => &[],
        }
    }
}

/// One Max size preset: its size, its label, and what it is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preset {
    pub bytes: u64,
    pub label: &'static str,
    pub note: Option<&'static str>,
}

/// What Custom says under its box while it is empty.
pub const TYPE_A_SIZE: &str = "Type a size in MB.";

/// What a choice that would do nothing says beside it.
pub const ALREADY: &str = "Already smaller than this";

/// One of the files a dialog is for, as its row knows it.
#[derive(Debug, Clone, PartialEq)]
pub struct File {
    pub path: PathBuf,
    pub name: String,
    /// Its format's extension, lowercase.
    pub ext: String,
    pub bytes: u64,
    /// Its picture's width and height, when known.
    pub picture: Option<(u32, u32)>,
    /// A video's frames a second, when known.
    pub fps: Option<f64>,
}

/// What the worker found Make smaller would make of one file, for the
/// options asked.
#[derive(Debug, Clone, PartialEq)]
pub enum Estimate {
    /// Asked, not answered yet.
    Working,
    /// The size it would come to.
    About(u64),
    /// The size can't be reached; the smallest Kiln can make.
    OutOfReach { max_bytes: u64, smallest: u64 },
    /// Nothing to do, in the engine's words.
    Already(String),
    /// It couldn't be worked out.
    Failed,
    /// Not worked out, as there was no thread to work it out on: the
    /// size asked for stands in.
    Plain,
}

/// A line under More options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoreLine {
    /// Video: Largest video size, a ComboBox.
    PictureSize,
    /// Video: Sound, a ToggleSwitch.
    Sound,
    /// Photos: Resize, a ComboBox.
    Resize,
    /// Photos: Remove location data, a ToggleSwitch.
    RemoveLocation,
}

impl MoreLine {
    /// Its label, which is also its name for Narrator.
    pub fn label(self) -> &'static str {
        match self {
            MoreLine::PictureSize => "Largest video size",
            MoreLine::Sound => "Sound",
            MoreLine::Resize => "Resize",
            MoreLine::RemoveLocation => "Remove location data",
        }
    }

    /// The smaller line under the label, from the mock.
    pub fn note(self) -> Option<&'static str> {
        match self {
            MoreLine::PictureSize => Some("Smaller pictures leave more room for quality."),
            MoreLine::Sound => None,
            MoreLine::Resize => Some("Longest side"),
            MoreLine::RemoveLocation => Some("Removes where it was taken and the camera details."),
        }
    }

    /// Whether it is a ToggleSwitch; the others are ComboBoxes.
    pub fn is_toggle(self) -> bool {
        matches!(self, MoreLine::Sound | MoreLine::RemoveLocation)
    }
}

/// Where a result goes, as Settings > Saving says: the estimate card's
/// last sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Saving {
    NextToOriginal,
    /// The folder chosen in Settings (`save_to`).
    Folder,
}

impl Saving {
    /// Where a result goes: `chosen` says a folder is chosen in Settings.
    pub fn of(chosen: bool) -> Self {
        match chosen {
            true => Saving::Folder,
            false => Saving::NextToOriginal,
        }
    }

    fn line(self, several: bool) -> &'static str {
        match (self, several) {
            (Saving::NextToOriginal, false) => "Saved next to the original.",
            (Saving::NextToOriginal, true) => "Saved next to the originals.",
            (Saving::Folder, _) => "Saved in the folder chosen in Settings.",
        }
    }
}

/// Largest video size's choices: Keep, then 1080p, 720p and 480p.
pub const PICTURE_LINES: [Option<u32>; 4] = [None, Some(1080), Some(720), Some(480)];
/// Resize's long sides, largest first, in the dialog and under More
/// options.
pub const LONG_SIDES: [u32; 4] = [3840, 2560, 1920, 1280];
/// The Resize dialog starts on this long side when it can.
pub const RESIZE_DEFAULT: u32 = 1920;
/// Custom's longest text: "9999.9" MB, or 99999 px.
const CUSTOM_LEN: usize = 6;
const CUSTOM_PX_LEN: usize = 5;

/// "10 MB", "2.5 MB", "500 KB": a size as a choice says it.
pub fn size_words(bytes: u64) -> String {
    let s = human_size(bytes);
    s.replacen(".0 ", " ", 1)
}

/// A Custom size typed in MB, "2.5", in bytes (decimal, as the app
/// counts); `None` for nothing, a lone point, or 0.
pub fn custom_bytes(text: &str) -> Option<u64> {
    let mb: f64 = text.trim().parse().ok()?;
    let bytes = (mb * 1_000_000.0).round();
    (bytes.is_finite() && bytes >= 1.0).then_some(bytes as u64)
}

/// A Custom long side typed in pixels; `None` for nothing or 0.
pub fn custom_px(text: &str) -> Option<u32> {
    text.trim().parse().ok().filter(|&px| px > 0)
}

/// "24", "29.97": frames a second as the card says them.
fn fps_words(fps: f64) -> String {
    if (fps - fps.round()).abs() < 0.01 {
        format!("{}", fps.round() as u32)
    } else {
        let s = format!("{fps:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    }
}

/// The picture `(w, h)` with its short side brought down to `lines`,
/// never enlarged, both sides even.
fn with_lines((w, h): (u32, u32), lines: u32) -> (u32, u32) {
    let short = w.min(h);
    if short <= lines || short == 0 {
        return (w, h);
    }
    let scale =
        |x: u32| ((f64::from(x) * f64::from(lines) / f64::from(short) / 2.0).round() as u32) * 2;
    if w <= h {
        (lines, scale(h))
    } else {
        (scale(w), lines)
    }
}

/// The picture `(w, h)` with its long side brought down to `side`, never
/// enlarged.
fn with_long_side((w, h): (u32, u32), side: u32) -> (u32, u32) {
    let long = w.max(h);
    if long <= side || long == 0 {
        return (w, h);
    }
    let scale = |x: u32| (f64::from(x) * f64::from(side) / f64::from(long)).round() as u32;
    (scale(w).max(1), scale(h).max(1))
}

/// "Keeps JPG." for files all of one format, as `saved` says each is saved;
/// "Each keeps its format." for several formats.
fn keeps_line(files: &[File], saved: impl Fn(&str) -> String) -> String {
    let first = files.first().map(|f| f.ext.as_str()).unwrap_or_default();
    if files.iter().all(|f| f.ext == first) {
        let out = saved(first);
        if out == first {
            format!("Keeps {}.", first.to_uppercase())
        } else {
            format!("Saved as {}.", out.to_uppercase())
        }
    } else if files.iter().any(|f| saved(&f.ext) != f.ext) {
        "Each keeps its format; HEIC and AVIF become JPG.".to_owned()
    } else {
        "Each keeps its format.".to_owned()
    }
}

/// The dialog's sub-line: "clip.mp4 · 31 MB. Keeps MP4." for one file, "3
/// files. Keeps JPG." for several.
fn sub_line(files: &[File], fact: impl Fn(&File) -> String, keeps: String) -> String {
    match files {
        [one] => format!("{} · {}. {keeps}", one.name, fact(one)),
        _ => format!("{} files. {keeps}", files.len()),
    }
}

/// The Make smaller dialog's model.
#[derive(Debug, Clone, PartialEq)]
pub struct SmallerDialog {
    pub shrink: Shrink,
    pub files: Vec<File>,
    /// The three presets, then Custom.
    pub radios: Radios,
    /// Which presets would do nothing: at or above every file's size.
    pub disabled: [bool; 3],
    /// Custom's size in MB.
    pub custom: TextBoxState,
    pub more: Expander,
    /// Largest video size, over `picture_choices`.
    pub picture: ComboState,
    picture_choices: Vec<Option<u32>>,
    pub sound: bool,
    /// Resize, over `resize_choices`.
    pub resize: ComboState,
    resize_choices: Vec<Option<u32>>,
    pub remove_location: bool,
    /// The estimates asked for, by the options they were asked with.
    estimates: Vec<(ToolOptions, Estimate)>,
}

impl SmallerDialog {
    /// The dialog for `files`, which must all be of one kind Make smaller
    /// takes; `None` otherwise.
    pub fn new(files: Vec<File>) -> Option<SmallerDialog> {
        let shrink = Shrink::of(&files.first()?.ext)?;
        if files.iter().any(|f| Shrink::of(&f.ext) != Some(shrink)) {
            return None;
        }
        let largest = files.iter().map(|f| f.bytes).max().unwrap_or(0);
        let disabled = shrink.presets().map(|p| p.bytes >= largest);
        // The second preset, else the first, else Custom.
        let picked = [1, 0].into_iter().find(|&i| !disabled[i]).unwrap_or(3);
        // A smaller picture is offered where it is smaller than a file's.
        let short = files
            .iter()
            .map(|f| f.picture.map(|(w, h)| w.min(h)))
            .collect::<Option<Vec<_>>>()
            .map(|s| s.into_iter().max().unwrap_or(0));
        let picture_choices: Vec<Option<u32>> = PICTURE_LINES
            .into_iter()
            .filter(|l| match (l, short) {
                (None, _) | (_, None) => true,
                (Some(l), Some(s)) => *l < s,
            })
            .collect();
        // 720p when it makes the picture smaller, else Keep.
        let helps = short.is_some_and(|s| s > 720);
        let picture_at = if helps {
            picture_choices
                .iter()
                .position(|l| *l == Some(720))
                .unwrap_or(0)
        } else {
            0
        };
        let long = files
            .iter()
            .map(|f| f.picture.map(|(w, h)| w.max(h)))
            .collect::<Option<Vec<_>>>()
            .map(|s| s.into_iter().max().unwrap_or(0));
        let resize_choices: Vec<Option<u32>> = std::iter::once(None)
            .chain(
                LONG_SIDES
                    .into_iter()
                    .filter(|&side| long.is_none_or(|l| side < l))
                    .map(Some),
            )
            .collect();
        Some(SmallerDialog {
            shrink,
            files,
            radios: Radios::new(4, Some(picked)),
            disabled,
            custom: TextBoxState::size("", CUSTOM_LEN),
            more: Expander::default(),
            picture: ComboState::new(picture_choices.len(), picture_at),
            picture_choices,
            sound: true,
            resize: ComboState::new(resize_choices.len(), 0),
            resize_choices,
            remove_location: true,
            estimates: vec![],
        })
    }

    fn several(&self) -> bool {
        self.files.len() > 1
    }

    /// The largest of the files.
    fn largest(&self) -> u64 {
        self.files.iter().map(|f| f.bytes).max().unwrap_or(0)
    }

    /// The size asked: the picked preset's, or Custom's; `None` while
    /// Custom holds no size.
    pub fn max_bytes(&self) -> Option<u64> {
        match self.radios.selected? {
            i @ 0..=2 => Some(self.shrink.presets()[i].bytes),
            _ => custom_bytes(self.custom.text()),
        }
    }

    /// Whether Custom is picked, and its box shows.
    pub fn custom_picked(&self) -> bool {
        self.radios.selected == Some(3)
    }

    /// Whether the size asked is at or above every file's size: nothing
    /// would be done.
    fn already(&self) -> bool {
        self.max_bytes().is_some_and(|m| m >= self.largest())
    }

    /// Largest video size's choices as it shows them.
    pub fn picture_labels(&self) -> Vec<String> {
        self.picture_choices
            .iter()
            .map(|l| l.map_or_else(|| "Keep".to_owned(), |l| format!("{l}p")))
            .collect()
    }

    /// Resize's choices as it shows them.
    pub fn resize_labels(&self) -> Vec<String> {
        self.resize_choices
            .iter()
            .map(|s| s.map_or_else(|| "Keep".to_owned(), |s| format!("{s} px")))
            .collect()
    }

    /// The most lines asked for the picture's short side.
    pub fn picture_lines(&self) -> Option<u32> {
        self.picture_choices
            .get(self.picture.value)
            .copied()
            .flatten()
    }

    /// The long side asked for a photo.
    pub fn long_side(&self) -> Option<u32> {
        self.resize_choices
            .get(self.resize.value)
            .copied()
            .flatten()
    }

    /// What the job runs with; `None` while no size is asked.
    pub fn tool_options(&self) -> Option<ToolOptions> {
        let max = self.max_bytes()?;
        let mut t = ToolOptions {
            max_bytes: Some(max),
            ..ToolOptions::default()
        };
        match self.shrink {
            Shrink::Video => {
                t.picture_lines = self.picture_lines();
                t.keep_sound = (!self.sound).then_some(false);
            }
            Shrink::Photo => {
                t.long_side = self.long_side();
                t.remove_location = self.remove_location;
            }
            Shrink::Pdf => {}
        }
        Some(t)
    }

    /// The estimate for the options asked now, if one was asked.
    pub fn estimate(&self) -> Option<&Estimate> {
        let now = self.tool_options()?;
        self.estimates
            .iter()
            .find(|(o, _)| *o == now)
            .map(|(_, e)| e)
    }

    /// Whether Make smaller can be pressed: a size is asked that isn't at
    /// or above every file's size, and for one file, one the estimate
    /// hasn't found out of reach or already met.
    pub fn can_run(&self) -> bool {
        self.max_bytes().is_some()
            && !self.already()
            && !matches!(
                self.estimate(),
                Some(Estimate::OutOfReach { .. } | Estimate::Already(_))
            )
    }

    /// The file and options the worker should work the estimate out for,
    /// when it is one file, a size is asked that it isn't under, and that
    /// estimate hasn't been asked for yet.
    pub fn wants_estimate(&self) -> Option<(PathBuf, ToolOptions)> {
        let [one] = self.files.as_slice() else {
            return None;
        };
        let opts = self.tool_options()?;
        if self.already() || self.estimates.iter().any(|(o, _)| *o == opts) {
            return None;
        }
        Some((one.path.clone(), opts))
    }

    /// The estimate for `opts` was asked for.
    pub fn asked(&mut self, opts: ToolOptions) {
        self.estimates.retain(|(o, _)| *o != opts);
        self.estimates.push((opts, Estimate::Working));
    }

    /// The worker answered for `opts`.
    pub fn estimated(&mut self, opts: ToolOptions, e: Estimate) {
        match self.estimates.iter_mut().find(|(o, _)| *o == opts) {
            Some(slot) => slot.1 = e,
            None => self.estimates.push((opts, e)),
        }
    }

    /// The sub-line under the title: "clip.mp4 · 31 MB. Keeps MP4."
    pub fn sub_line(&self) -> String {
        let keeps = keeps_line(&self.files, |e| e.to_owned());
        sub_line(&self.files, |f| human_size(f.bytes), keeps)
    }

    /// Preset `i`'s words after its label: what it's for, or "Already
    /// smaller than this" when it would do nothing.
    pub fn tail(&self, i: usize) -> Option<&'static str> {
        if self.disabled.get(i).copied().unwrap_or(false) {
            return Some(ALREADY);
        }
        self.shrink.presets().get(i).and_then(|p| p.note)
    }

    /// What Custom asks while its box is empty: plain words under the
    /// box, in no card, since it is a hint and not an estimate.
    pub fn hint(&self) -> Option<&'static str> {
        (self.custom_picked() && self.max_bytes().is_none()).then_some(TYPE_A_SIZE)
    }

    /// The estimate card's two lines: the size in bold, then the picture
    /// and where it is saved. Empty while [`SmallerDialog::hint`] has the
    /// say instead.
    pub fn estimate_lines(&self, saving: Saving) -> (String, String) {
        let several = self.several();
        let saved = saving.line(several);
        let Some(max) = self.max_bytes() else {
            return (String::new(), String::new());
        };
        if self.already() {
            let what = if several {
                "These files are"
            } else {
                "This file is"
            };
            return (
                format!("{what} already smaller than {}.", about(max)),
                String::new(),
            );
        }
        if several {
            return (format!("Up to {} each", size_words(max)), saved.to_owned());
        }
        let detail = match self.picture_line() {
            Some(p) => format!("{p} {saved}"),
            None => saved.to_owned(),
        };
        match self.estimate() {
            None | Some(Estimate::Working) => ("Working out the size…".to_owned(), detail),
            Some(Estimate::About(n)) => (format!("About {}", size_words(*n)), detail),
            Some(Estimate::OutOfReach {
                max_bytes,
                smallest,
            }) => (
                format!("Can't get this under {}.", about(*max_bytes)),
                format!("The smallest Kiln can make is about {}.", about(*smallest)),
            ),
            Some(Estimate::Already(said)) => (said.clone(), String::new()),
            Some(Estimate::Failed) => ("Kiln couldn't work out the size.".to_owned(), detail),
            Some(Estimate::Plain) => (format!("Up to {}", size_words(max)), detail),
        }
    }

    /// What becomes of one file's picture: "Keeps 1920 × 800 at 24 frames
    /// a second." or "Becomes 1280 × 720." `None` for a PDF, or a
    /// picture whose size isn't known.
    fn picture_line(&self) -> Option<String> {
        let f = self.files.first()?;
        let (w, h) = f.picture?;
        let (to, rate) = match self.shrink {
            Shrink::Video => (
                self.picture_lines()
                    .map_or((w, h), |l| with_lines((w, h), l)),
                f.fps.filter(|r| r.is_finite() && *r > 0.0),
            ),
            Shrink::Photo => (
                self.long_side()
                    .map_or((w, h), |s| with_long_side((w, h), s)),
                None,
            ),
            Shrink::Pdf => return None,
        };
        let verb = if to == (w, h) { "Keeps" } else { "Becomes" };
        let rate = rate.map_or_else(String::new, |r| {
            format!(" at {} frames a second", fps_words(r))
        });
        Some(format!("{verb} {} × {}{rate}.", to.0, to.1))
    }

    /// A More options line's value as its ComboBox shows it.
    pub fn line_value(&self, l: MoreLine) -> String {
        match l {
            MoreLine::PictureSize => self.picture_labels()[self.picture.value].clone(),
            MoreLine::Resize => self.resize_labels()[self.resize.value].clone(),
            MoreLine::Sound | MoreLine::RemoveLocation => String::new(),
        }
    }

    /// A ToggleSwitch line's state.
    pub fn line_on(&self, l: MoreLine) -> bool {
        match l {
            MoreLine::Sound => self.sound,
            MoreLine::RemoveLocation => self.remove_location,
            _ => false,
        }
    }

    /// A ComboBox line's state.
    pub fn combo(&self, l: MoreLine) -> Option<&ComboState> {
        match l {
            MoreLine::PictureSize => Some(&self.picture),
            MoreLine::Resize => Some(&self.resize),
            _ => None,
        }
    }

    pub fn combo_mut(&mut self, l: MoreLine) -> Option<&mut ComboState> {
        match l {
            MoreLine::PictureSize => Some(&mut self.picture),
            MoreLine::Resize => Some(&mut self.resize),
            _ => None,
        }
    }

    /// A ComboBox line's choices.
    pub fn line_choices(&self, l: MoreLine) -> Vec<String> {
        match l {
            MoreLine::PictureSize => self.picture_labels(),
            MoreLine::Resize => self.resize_labels(),
            _ => vec![],
        }
    }

    /// Flip a ToggleSwitch line.
    pub fn toggle(&mut self, l: MoreLine) {
        match l {
            MoreLine::Sound => self.sound = !self.sound,
            MoreLine::RemoveLocation => self.remove_location = !self.remove_location,
            _ => {}
        }
    }
}

/// The Resize dialog's model: one group of long sides and Custom.
#[derive(Debug, Clone, PartialEq)]
pub struct ResizeDialog {
    pub files: Vec<File>,
    /// 3840, 2560, 1920 and 1280 px, then Custom.
    pub radios: Radios,
    /// Which long sides would do nothing: at or above every photo's.
    pub disabled: [bool; 4],
    /// Custom's long side in pixels.
    pub custom: TextBoxState,
}

impl ResizeDialog {
    /// The dialog for `files`, the photos Resize takes; `None` for none.
    pub fn new(files: Vec<File>) -> Option<ResizeDialog> {
        if files.is_empty() {
            return None;
        }
        let long = files
            .iter()
            .map(|f| f.picture.map(|(w, h)| w.max(h)))
            .collect::<Option<Vec<_>>>()
            .map(|s| s.into_iter().max().unwrap_or(0));
        let disabled = LONG_SIDES.map(|side| long.is_some_and(|l| side >= l));
        // 1920, else the largest that does something, else Custom.
        let default = LONG_SIDES.iter().position(|&s| s == RESIZE_DEFAULT);
        let picked = default
            .filter(|&i| !disabled[i])
            .or_else(|| disabled.iter().position(|d| !d))
            .unwrap_or(4);
        Some(ResizeDialog {
            files,
            radios: Radios::new(5, Some(picked)),
            disabled,
            custom: TextBoxState::new("", CUSTOM_PX_LEN, true),
        })
    }

    /// The long side asked; `None` while Custom holds none.
    pub fn long_side(&self) -> Option<u32> {
        match self.radios.selected? {
            i @ 0..=3 => Some(LONG_SIDES[i]),
            _ => custom_px(self.custom.text()),
        }
    }

    pub fn custom_picked(&self) -> bool {
        self.radios.selected == Some(4)
    }

    /// What the job runs with; `None` while no long side is asked.
    pub fn tool_options(&self) -> Option<ToolOptions> {
        Some(ToolOptions {
            long_side: Some(self.long_side()?),
            ..ToolOptions::default()
        })
    }

    /// Whether Resize can be pressed: a long side is asked, and one a
    /// listed side wouldn't do nothing for.
    pub fn can_run(&self) -> bool {
        let picked_off = self
            .radios
            .selected
            .and_then(|i| self.disabled.get(i))
            .copied()
            .unwrap_or(false);
        self.long_side().is_some() && !picked_off
    }

    /// "photo.heic · 4032 × 3024. Saved as JPG."
    pub fn sub_line(&self) -> String {
        let keeps = keeps_line(&self.files, |e| {
            if matches!(e, "heic" | "heif" | "avif") {
                "jpg".to_owned()
            } else {
                e.to_owned()
            }
        });
        sub_line(
            &self.files,
            |f| match f.picture {
                Some((w, h)) => format!("{w} × {h}"),
                None => human_size(f.bytes),
            },
            keeps,
        )
    }

    pub fn tail(&self, i: usize) -> Option<&'static str> {
        self.disabled
            .get(i)
            .copied()
            .unwrap_or(false)
            .then_some(ALREADY)
    }
}

/// Where the keyboard is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    Radios,
    Custom,
    Expander,
    Line(usize),
    Accent,
    Cancel,
}

/// A More options line laid out: its label, its note and its control.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineParts {
    pub line: MoreLine,
    pub label: Rect,
    pub note: Rect,
    pub control: Rect,
}

/// The dialog laid out in a window, its content scrolled.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolLayout {
    /// The ContentDialog: its accent button first, then Cancel.
    pub frame: DialogLayout,
    pub sub: Rect,
    pub heading: Rect,
    pub radios: Vec<RadioParts>,
    /// Where each choice's words after its label go.
    pub tails: Vec<Rect>,
    /// Custom's box and its unit, while Custom is picked.
    pub custom: Option<(Rect, Rect)>,
    /// The estimate card, and its bold and second lines.
    pub card: Option<(Rect, Rect, Rect)>,
    /// The hint under the choices while Custom waits for a size.
    pub hint: Option<Rect>,
    pub expander: Option<ExpanderParts>,
    pub lines: Vec<LineParts>,
    /// An open ComboBox's list.
    pub list: Option<FlyoutLayout>,
    /// How far the content can scroll.
    pub scroll_max: f32,
    /// How far it is scrolled.
    pub scroll: f32,
}

/// Either dialog, as the surface holds it.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolDialog {
    Smaller(SmallerDialog),
    Resize(ResizeDialog),
}

impl ToolDialog {
    /// The dialog `tool` opens for `files`; `None` for a tool with no
    /// dialog, or files it doesn't take.
    pub fn new(tool: Tool, files: Vec<File>) -> Option<ToolDialog> {
        match tool {
            Tool::Smaller => SmallerDialog::new(files).map(ToolDialog::Smaller),
            Tool::Resize => ResizeDialog::new(files).map(ToolDialog::Resize),
            _ => None,
        }
    }

    pub fn tool(&self) -> Tool {
        match self {
            ToolDialog::Smaller(_) => Tool::Smaller,
            ToolDialog::Resize(_) => Tool::Resize,
        }
    }

    pub fn files(&self) -> &[File] {
        match self {
            ToolDialog::Smaller(d) => &d.files,
            ToolDialog::Resize(d) => &d.files,
        }
    }

    pub fn paths(&self) -> Vec<PathBuf> {
        self.files().iter().map(|f| f.path.clone()).collect()
    }

    /// The title, which is also the accent button's word.
    pub fn title(&self) -> &'static str {
        match self {
            ToolDialog::Smaller(_) => "Make smaller",
            ToolDialog::Resize(_) => "Resize",
        }
    }

    pub fn sub_line(&self) -> String {
        match self {
            ToolDialog::Smaller(d) => d.sub_line(),
            ToolDialog::Resize(d) => d.sub_line(),
        }
    }

    /// The words over the choices: "Max size" as a heading, or Resize's
    /// sentence; and whether they are a heading.
    pub fn heading(&self) -> (&'static str, bool) {
        match self {
            ToolDialog::Smaller(_) => ("Max size", true),
            ToolDialog::Resize(_) => ("Longest side", false),
        }
    }

    /// The choices' labels, Custom last.
    pub fn labels(&self) -> Vec<&'static str> {
        match self {
            ToolDialog::Smaller(d) => d
                .shrink
                .presets()
                .iter()
                .map(|p| p.label)
                .chain(["Custom"])
                .collect(),
            ToolDialog::Resize(_) => vec!["3840 px", "2560 px", "1920 px", "1280 px", "Custom"],
        }
    }

    /// Choice `i`'s words after its label.
    pub fn tail(&self, i: usize) -> Option<&'static str> {
        match self {
            ToolDialog::Smaller(d) => d.tail(i),
            ToolDialog::Resize(d) => d.tail(i),
        }
    }

    /// Whether choice `i` is shown disabled.
    pub fn disabled(&self, i: usize) -> bool {
        let d: &[bool] = match self {
            ToolDialog::Smaller(d) => &d.disabled,
            ToolDialog::Resize(d) => &d.disabled,
        };
        d.get(i).copied().unwrap_or(false)
    }

    pub fn radios(&self) -> &Radios {
        match self {
            ToolDialog::Smaller(d) => &d.radios,
            ToolDialog::Resize(d) => &d.radios,
        }
    }

    pub fn radios_mut(&mut self) -> &mut Radios {
        match self {
            ToolDialog::Smaller(d) => &mut d.radios,
            ToolDialog::Resize(d) => &mut d.radios,
        }
    }

    /// Pick choice `i`, as a click or an arrow key does; a disabled one
    /// can't be picked. Whether it was.
    pub fn pick(&mut self, i: usize) -> bool {
        if i >= self.radios().len || self.disabled(i) {
            return false;
        }
        let r = self.radios_mut();
        r.selected = Some(i);
        r.focus = i;
        true
    }

    /// Move the pick `delta` along, past disabled choices; the ends hold.
    /// Whether it moved.
    pub fn step(&mut self, delta: isize) -> bool {
        let r = *self.radios();
        let mut i = r.focus as isize;
        loop {
            i += delta.signum();
            if i < 0 || i >= r.len as isize {
                return false;
            }
            if !self.disabled(i as usize) {
                return self.pick(i as usize);
            }
        }
    }

    /// The index of Custom.
    pub fn custom_index(&self) -> usize {
        self.radios().len - 1
    }

    pub fn custom_picked(&self) -> bool {
        self.radios().selected == Some(self.custom_index())
    }

    pub fn custom(&self) -> &TextBoxState {
        match self {
            ToolDialog::Smaller(d) => &d.custom,
            ToolDialog::Resize(d) => &d.custom,
        }
    }

    pub fn custom_mut(&mut self) -> &mut TextBoxState {
        match self {
            ToolDialog::Smaller(d) => &mut d.custom,
            ToolDialog::Resize(d) => &mut d.custom,
        }
    }

    /// Custom's unit after its box.
    pub fn unit(&self) -> &'static str {
        match self {
            ToolDialog::Smaller(_) => "MB",
            ToolDialog::Resize(_) => "px",
        }
    }

    /// Custom's box's name for Narrator.
    pub fn custom_name(&self) -> &'static str {
        match self {
            ToolDialog::Smaller(_) => "Custom size in MB",
            ToolDialog::Resize(_) => "Custom longest side in pixels",
        }
    }

    /// The lines under More options; none means no More options.
    pub fn more_lines(&self) -> &'static [MoreLine] {
        match self {
            ToolDialog::Smaller(d) => d.shrink.more_lines(),
            ToolDialog::Resize(_) => &[],
        }
    }

    pub fn tool_options(&self) -> Option<ToolOptions> {
        match self {
            ToolDialog::Smaller(d) => d.tool_options(),
            ToolDialog::Resize(d) => d.tool_options(),
        }
    }

    pub fn can_run(&self) -> bool {
        match self {
            ToolDialog::Smaller(d) => d.can_run(),
            ToolDialog::Resize(d) => d.can_run(),
        }
    }

    /// The estimate card's lines; Resize has none, and nor has a Custom
    /// size still to be typed ([`ToolDialog::hint`]).
    pub fn estimate_lines(&self, saving: Saving) -> Option<(String, String)> {
        match self {
            ToolDialog::Smaller(d) if d.hint().is_none() => Some(d.estimate_lines(saving)),
            _ => None,
        }
    }

    /// The plain line under the choices while Custom waits for a size.
    pub fn hint(&self) -> Option<&'static str> {
        self.smaller().and_then(SmallerDialog::hint)
    }

    pub fn smaller(&self) -> Option<&SmallerDialog> {
        match self {
            ToolDialog::Smaller(d) => Some(d),
            ToolDialog::Resize(_) => None,
        }
    }

    /// A file's picture size and frame rate came in after the dialog
    /// opened: what depends on them follows. A choice the person hasn't
    /// changed from its first pick moves to the one these facts would
    /// have given; one they made stays where it can. Whether anything
    /// changed.
    pub fn facts_arrived(
        &mut self,
        path: &std::path::Path,
        picture: Option<(u32, u32)>,
        fps: Option<f64>,
    ) -> bool {
        let files = match self {
            ToolDialog::Smaller(d) => &d.files,
            ToolDialog::Resize(d) => &d.files,
        };
        let Some(at) = files.iter().position(|f| f.path == path) else {
            return false;
        };
        let f = &files[at];
        if (f.picture, f.fps) == (picture, fps) || picture.is_none() && fps.is_none() {
            return false;
        }
        let mut files = files.clone();
        files[at].picture = picture.or(files[at].picture);
        files[at].fps = fps.or(files[at].fps);
        match self {
            ToolDialog::Smaller(d) => {
                let (Some(was), Some(mut now)) = (
                    SmallerDialog::new(d.files.clone()),
                    SmallerDialog::new(files),
                ) else {
                    return false;
                };
                // A ComboBox line on its first pick takes the new first
                // pick; otherwise its value, where the new choices hold it.
                let carry = |old: &ComboState,
                             first: &ComboState,
                             from: &[Option<u32>],
                             new: &ComboState,
                             to: &[Option<u32>]| {
                    if old.value == first.value {
                        return new.value;
                    }
                    to.iter().position(|v| *v == from[old.value]).unwrap_or(0)
                };
                let picture = carry(
                    &d.picture,
                    &was.picture,
                    &d.picture_choices,
                    &now.picture,
                    &now.picture_choices,
                );
                let resize = carry(
                    &d.resize,
                    &was.resize,
                    &d.resize_choices,
                    &now.resize,
                    &now.resize_choices,
                );
                now.picture = ComboState::new(now.picture_choices.len(), picture);
                now.resize = ComboState::new(now.resize_choices.len(), resize);
                now.radios = d.radios;
                now.custom = d.custom.clone();
                now.more = d.more;
                now.sound = d.sound;
                now.remove_location = d.remove_location;
                now.estimates = std::mem::take(&mut d.estimates);
                *d = now;
            }
            ToolDialog::Resize(d) => {
                let (Some(was), Some(mut now)) =
                    (ResizeDialog::new(d.files.clone()), ResizeDialog::new(files))
                else {
                    return false;
                };
                // The first pick takes the new first pick; a side picked
                // stays unless it would now do nothing.
                let kept = d
                    .radios
                    .selected
                    .filter(|_| d.radios.selected != was.radios.selected)
                    .filter(|&i| !now.disabled.get(i).copied().unwrap_or(false));
                if let Some(i) = kept {
                    now.radios = Radios::new(now.radios.len, Some(i));
                }
                now.custom = d.custom.clone();
                *d = now;
            }
        }
        true
    }

    pub fn smaller_mut(&mut self) -> Option<&mut SmallerDialog> {
        match self {
            ToolDialog::Smaller(d) => Some(d),
            ToolDialog::Resize(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::{Key, Press};

    fn file(name: &str, bytes: u64, picture: Option<(u32, u32)>) -> File {
        let ext = name.rsplit('.').next().unwrap().to_owned();
        File {
            path: PathBuf::from(format!("C:\\f\\{name}")),
            name: name.to_owned(),
            ext,
            bytes,
            picture,
            fps: None,
        }
    }

    fn video(bytes: u64) -> SmallerDialog {
        let mut f = file("clip.mp4", bytes, Some((1920, 800)));
        f.fps = Some(24.0);
        SmallerDialog::new(vec![f]).unwrap()
    }

    #[test]
    fn a_videos_size_arriving_late_picks_720p_unless_a_size_was_chosen() {
        let mut f = file("clip.mp4", 60_000_000, None);
        f.fps = Some(24.0);
        let path = f.path.clone();
        let mut t = ToolDialog::Smaller(SmallerDialog::new(vec![f.clone()]).unwrap());
        // Not known yet: every size, on Keep.
        assert_eq!(t.smaller().unwrap().picture_lines(), None);
        assert!(t.facts_arrived(&path, Some((1920, 1080)), Some(24.0)));
        let d = t.smaller().unwrap();
        assert_eq!(d.picture_lines(), Some(720));
        assert_eq!(d.picture_labels(), ["Keep", "720p", "480p"]);
        let (_, detail) = d.estimate_lines(Saving::NextToOriginal);
        assert!(
            detail.starts_with("Becomes 1280 × 720 at 24 frames a second."),
            "{detail}"
        );
        // The same facts again change nothing.
        assert!(!t.facts_arrived(&path, Some((1920, 1080)), Some(24.0)));
        // A size the person chose stays.
        let mut t = ToolDialog::Smaller(SmallerDialog::new(vec![f]).unwrap());
        let d = t.smaller_mut().unwrap();
        let at = d.picture_labels().iter().position(|l| l == "480p").unwrap();
        d.picture.value = at;
        t.facts_arrived(&path, Some((1920, 1080)), Some(24.0));
        assert_eq!(t.smaller().unwrap().picture_lines(), Some(480));
    }

    #[test]
    fn a_photos_size_arriving_late_fills_the_picture_line_and_the_resize_sides() {
        let f = file("p.jpg", 3_000_000, None);
        let path = f.path.clone();
        let mut t = ToolDialog::Smaller(SmallerDialog::new(vec![f.clone()]).unwrap());
        t.facts_arrived(&path, Some((1600, 1200)), None);
        let d = t.smaller().unwrap();
        assert_eq!(d.resize_labels(), ["Keep", "1280 px"]);
        let (_, detail) = d.estimate_lines(Saving::NextToOriginal);
        assert!(detail.starts_with("Keeps 1600 × 1200."), "{detail}");
        // Resize: the sides at or above it go, and it starts on 1280.
        let mut t = ToolDialog::new(Tool::Resize, vec![f]).unwrap();
        assert_eq!(t.tool_options().unwrap().long_side, Some(1920));
        t.facts_arrived(&path, Some((1600, 1200)), None);
        assert_eq!(t.tool_options().unwrap().long_side, Some(1280));
        assert_eq!(t.tail(2), Some(ALREADY));
        // Another file's facts change nothing.
        assert!(!t.facts_arrived(std::path::Path::new(r"C:\other.jpg"), Some((10, 10)), None));
    }

    #[test]
    fn presets_per_kind_follow_the_spec() {
        let labels = |s: Shrink| s.presets().map(|p| p.label);
        assert_eq!(labels(Shrink::Video), ["10 MB", "25 MB", "50 MB"]);
        assert_eq!(labels(Shrink::Photo), ["500 KB", "1 MB", "2 MB"]);
        assert_eq!(labels(Shrink::Pdf), ["1 MB", "5 MB", "10 MB"]);
        assert_eq!(
            Shrink::Video.presets().map(|p| p.note),
            [Some("Fits email"), Some("Fits most chat apps"), None]
        );
        for ext in ["mp4", "mov", "m4v", "mkv"] {
            assert_eq!(Shrink::of(ext), Some(Shrink::Video));
        }
        for ext in ["jpg", "png", "webp"] {
            assert_eq!(Shrink::of(ext), Some(Shrink::Photo));
        }
        assert_eq!(Shrink::of("pdf"), Some(Shrink::Pdf));
        // Kiln can't write these.
        assert_eq!(Shrink::of("heic"), None);
        assert_eq!(Shrink::of("mp3"), None);
    }

    #[test]
    fn the_second_preset_is_picked_at_first() {
        let d = video(60_000_000);
        assert_eq!(d.radios.selected, Some(1));
        assert_eq!(d.max_bytes(), Some(25_000_000));
        assert_eq!(d.disabled, [false, false, false]);
        let t = ToolDialog::Smaller(d);
        assert_eq!(t.labels(), ["10 MB", "25 MB", "50 MB", "Custom"]);
        assert_eq!(t.tail(0), Some("Fits email"));
        assert_eq!(t.tail(1), Some("Fits most chat apps"));
        assert_eq!(t.tail(2), None);
    }

    #[test]
    fn presets_at_or_above_the_files_size_are_disabled_and_say_already_smaller() {
        // A 20 MB video: 25 and 50 MB would do nothing.
        let d = video(20_000_000);
        assert_eq!(d.disabled, [false, true, true]);
        assert_eq!(d.radios.selected, Some(0));
        let t = ToolDialog::Smaller(d);
        assert_eq!(t.tail(0), Some("Fits email"));
        assert_eq!(t.tail(1), Some(ALREADY));
        assert_eq!(t.tail(2), Some(ALREADY));
        // Exactly the size is "already smaller" too.
        let d = video(25_000_000);
        assert_eq!(d.disabled, [false, true, true]);
        // Under every preset: Custom.
        let d = video(4_000_000);
        assert_eq!(d.disabled, [true, true, true]);
        assert_eq!(d.radios.selected, Some(3));
        assert_eq!(d.max_bytes(), None);
        assert!(!d.can_run());
        // A disabled preset can't be picked, by click or arrow.
        let mut t = ToolDialog::Smaller(video(20_000_000));
        assert!(!t.pick(1));
        assert_eq!(t.radios().selected, Some(0));
        assert!(t.step(1));
        assert_eq!(t.radios().selected, Some(3), "past the disabled ones");
        assert!(t.step(-1));
        assert_eq!(t.radios().selected, Some(0));
        assert!(!t.step(-1));
    }

    #[test]
    fn the_saving_line_says_where_the_result_really_goes() {
        // A folder chosen in Settings, or next to the original.
        assert_eq!(Saving::of(true), Saving::Folder);
        assert_eq!(Saving::of(false), Saving::NextToOriginal);
    }

    #[test]
    fn several_files_disable_a_preset_only_when_it_does_nothing_for_any() {
        let d = SmallerDialog::new(vec![
            file("a.jpg", 800_000, None),
            file("b.jpg", 3_000_000, None),
        ])
        .unwrap();
        assert_eq!(d.disabled, [false, false, false]);
        assert_eq!(d.sub_line(), "2 files. Keeps JPG.");
        let (bold, rest) = d.estimate_lines(Saving::NextToOriginal);
        assert_eq!(bold, "Up to 1 MB each");
        assert_eq!(rest, "Saved next to the originals.");
        // Nothing to ask a worker: the size applies to each.
        assert_eq!(d.wants_estimate(), None);
        assert!(d.can_run());
        // Mixed kinds aren't one dialog.
        assert!(SmallerDialog::new(vec![
            file("a.jpg", 800_000, None),
            file("b.mp4", 3_000_000, None)
        ])
        .is_none());
        let mixed = SmallerDialog::new(vec![
            file("a.jpg", 800_000, None),
            file("b.png", 3_000_000, None),
        ])
        .unwrap();
        assert_eq!(mixed.sub_line(), "2 files. Each keeps its format.");
    }

    #[test]
    fn custom_takes_a_size_in_mb_with_digits_and_a_point() {
        let mut d = video(31_000_000);
        d.radios.selected = Some(3);
        assert!(d.custom_picked());
        assert_eq!(d.max_bytes(), None);
        // A hint under the box, plain, and no estimate card.
        assert_eq!(d.hint(), Some("Type a size in MB."));
        let t = ToolDialog::Smaller(d.clone());
        assert_eq!(t.hint(), Some("Type a size in MB."));
        assert_eq!(t.estimate_lines(Saving::NextToOriginal), None);
        for c in "8.5x".chars() {
            d.custom.key(Press::key(Key::Char(c)));
        }
        assert_eq!(d.custom.text(), "8.5");
        assert_eq!(d.max_bytes(), Some(8_500_000));
        assert_eq!(d.hint(), None, "a size typed: the estimate speaks");
        assert!(ToolDialog::Smaller(d.clone())
            .estimate_lines(Saving::NextToOriginal)
            .is_some());
        assert_eq!(custom_bytes("0"), None);
        assert_eq!(custom_bytes("."), None);
        assert_eq!(custom_bytes(""), None);
        assert_eq!(custom_bytes("0.3"), Some(300_000));
        assert_eq!(custom_bytes("12"), Some(12_000_000));
        // At or above the file's size: nothing to do, and it says so.
        d.custom.set("40");
        assert!(!d.can_run());
        assert_eq!(
            d.estimate_lines(Saving::NextToOriginal).0,
            "This file is already smaller than 40 MB."
        );
        assert_eq!(d.wants_estimate(), None);
    }

    #[test]
    fn more_options_per_kind_and_their_defaults() {
        let d = video(31_000_000);
        assert_eq!(
            d.shrink.more_lines(),
            [MoreLine::PictureSize, MoreLine::Sound]
        );
        // 1920 × 800: 1080p isn't smaller than 800 lines.
        assert_eq!(d.picture_labels(), ["Keep", "720p", "480p"]);
        assert_eq!(d.line_value(MoreLine::PictureSize), "720p", "720p helps");
        assert!(d.line_on(MoreLine::Sound));
        let t = d.tool_options().unwrap();
        assert_eq!(t.picture_lines, Some(720));
        assert_eq!(t.keep_sound, None);
        assert_eq!(t.max_bytes, Some(25_000_000));
        // A 640 × 480 video: 720p wouldn't help, so Keep.
        let small = SmallerDialog::new(vec![file("s.mp4", 31_000_000, Some((640, 480)))]).unwrap();
        assert_eq!(small.picture_labels(), ["Keep"]);
        assert_eq!(small.picture_lines(), None);
        // Unknown size: every choice, Keep.
        let unknown = SmallerDialog::new(vec![file("u.mov", 31_000_000, None)]).unwrap();
        assert_eq!(unknown.picture_labels(), ["Keep", "1080p", "720p", "480p"]);
        assert_eq!(unknown.picture_lines(), None);
        // Sound off leaves it out.
        let mut off = video(31_000_000);
        off.toggle(MoreLine::Sound);
        assert_eq!(off.tool_options().unwrap().keep_sound, Some(false));

        let photo = SmallerDialog::new(vec![file("p.jpg", 3_200_000, Some((3000, 2000)))]).unwrap();
        assert_eq!(
            photo.shrink.more_lines(),
            [MoreLine::Resize, MoreLine::RemoveLocation]
        );
        assert_eq!(
            photo.resize_labels(),
            ["Keep", "2560 px", "1920 px", "1280 px"]
        );
        assert!(photo.line_on(MoreLine::RemoveLocation), "On by default");
        let t = photo.tool_options().unwrap();
        assert!(t.remove_location);
        assert_eq!(t.long_side, None);
        assert_eq!(t.max_bytes, Some(1_000_000));

        let pdf = SmallerDialog::new(vec![file("d.pdf", 12_000_000, None)]).unwrap();
        assert!(pdf.shrink.more_lines().is_empty());
        assert!(ToolDialog::Smaller(pdf).more_lines().is_empty());
    }

    #[test]
    fn the_estimate_line_works_out_then_says_about() {
        let mut d = video(31_000_000);
        assert_eq!(d.sub_line(), "clip.mp4 · 31 MB. Keeps MP4.");
        let (path, opts) = d.wants_estimate().unwrap();
        assert_eq!(path, PathBuf::from("C:\\f\\clip.mp4"));
        d.asked(opts.clone());
        assert_eq!(d.wants_estimate(), None, "asked once");
        let (bold, rest) = d.estimate_lines(Saving::NextToOriginal);
        assert_eq!(bold, "Working out the size…");
        assert_eq!(
            rest,
            "Becomes 1728 × 720 at 24 frames a second. Saved next to the original."
        );
        d.estimated(opts.clone(), Estimate::About(24_400_000));
        assert_eq!(d.estimate_lines(Saving::NextToOriginal).0, "About 24 MB");
        // Another preset asks again; going back finds the answer kept.
        assert!(ToolDialog::Smaller(d.clone()).pick(0));
        d.radios.selected = Some(0);
        assert!(d.wants_estimate().is_some());
        d.radios.selected = Some(1);
        assert_eq!(d.wants_estimate(), None);
        // An answer for options no longer asked is kept, not shown.
        d.estimated(
            ToolOptions {
                max_bytes: Some(1),
                ..ToolOptions::default()
            },
            Estimate::Failed,
        );
        assert_eq!(d.estimate_lines(Saving::NextToOriginal).0, "About 24 MB");
        assert_eq!(
            d.estimate_lines(Saving::NextToOriginal).1,
            "Becomes 1728 × 720 at 24 frames a second. Saved next to the original."
        );
        // Keep: the picture stays.
        d.picture.value = 0;
        let (_, rest) = d.estimate_lines(Saving::Folder);
        assert_eq!(
            rest,
            "Keeps 1920 × 800 at 24 frames a second. Saved in the folder chosen in Settings."
        );
    }

    #[test]
    fn out_of_reach_and_already_say_so_and_make_smaller_waits() {
        let mut d = SmallerDialog::new(vec![file("d.pdf", 12_000_000, None)]).unwrap();
        let (_, opts) = d.wants_estimate().unwrap();
        d.asked(opts.clone());
        assert!(d.can_run(), "it may still fit while it works out");
        d.estimated(
            opts.clone(),
            Estimate::OutOfReach {
                max_bytes: 5_000_000,
                smallest: 7_300_000,
            },
        );
        assert_eq!(
            d.estimate_lines(Saving::NextToOriginal),
            (
                "Can't get this under 5 MB.".to_owned(),
                "The smallest Kiln can make is about 7.3 MB.".to_owned()
            )
        );
        assert!(!d.can_run());
        d.estimated(
            opts,
            Estimate::Already("This file is already as small as Kiln can make it.".to_owned()),
        );
        assert_eq!(
            d.estimate_lines(Saving::NextToOriginal).0,
            "This file is already as small as Kiln can make it."
        );
        assert!(!d.can_run());
        // A photo's picture line follows Resize.
        let mut p = SmallerDialog::new(vec![file("p.jpg", 3_200_000, Some((4032, 3024)))]).unwrap();
        let (_, o) = p.wants_estimate().unwrap();
        p.estimated(o, Estimate::About(960_000));
        assert_eq!(
            p.estimate_lines(Saving::NextToOriginal),
            (
                "About 960 KB".to_owned(),
                "Keeps 4032 × 3024. Saved next to the original.".to_owned()
            )
        );
        p.resize.value = 3; // 1920 px
        assert_eq!(p.long_side(), Some(1920));
        assert_eq!(
            p.estimate_lines(Saving::NextToOriginal),
            (
                "Working out the size…".to_owned(),
                "Becomes 1920 × 1440. Saved next to the original.".to_owned()
            )
        );
    }

    #[test]
    fn resize_offers_long_sides_and_custom_and_says_heic_is_saved_as_jpg() {
        let d = ResizeDialog::new(vec![file("p.jpg", 3_200_000, Some((4032, 3024)))]).unwrap();
        let t = ToolDialog::Resize(d.clone());
        assert_eq!(t.title(), "Resize");
        assert_eq!(t.heading(), ("Longest side", false));
        assert_eq!(
            t.labels(),
            ["3840 px", "2560 px", "1920 px", "1280 px", "Custom"]
        );
        assert_eq!(d.radios.selected, Some(2), "1920 at first");
        assert_eq!(d.tool_options().unwrap().long_side, Some(1920));
        assert_eq!(d.sub_line(), "p.jpg · 4032 × 3024. Keeps JPG.");
        assert!(t.estimate_lines(Saving::NextToOriginal).is_none());
        assert!(t.more_lines().is_empty());

        let heic = ResizeDialog::new(vec![file("p.heic", 2_000_000, Some((4032, 3024)))]).unwrap();
        assert_eq!(heic.sub_line(), "p.heic · 4032 × 3024. Saved as JPG.");

        // A 2000 px photo: 3840 and 2560 would do nothing.
        let mut small = ToolDialog::Resize(
            ResizeDialog::new(vec![file("s.png", 900_000, Some((2000, 1500)))]).unwrap(),
        );
        assert_eq!(small.tail(0), Some(ALREADY));
        assert_eq!(small.tail(1), Some(ALREADY));
        assert_eq!(small.tail(2), None);
        assert_eq!(small.radios().selected, Some(2));
        // Custom: digits only, in pixels.
        assert!(small.pick(4));
        assert!(small.custom_picked());
        assert_eq!(small.unit(), "px");
        assert!(!small.can_run());
        small.custom_mut().key(Press::key(Key::Char('8')));
        small.custom_mut().key(Press::key(Key::Char('.')));
        small.custom_mut().key(Press::key(Key::Char('0')));
        small.custom_mut().key(Press::key(Key::Char('0')));
        assert_eq!(small.custom().text(), "800");
        assert_eq!(small.tool_options().unwrap().long_side, Some(800));
        assert!(small.can_run());

        // A tiny photo: every side does nothing, so Custom.
        let tiny = ResizeDialog::new(vec![file("t.png", 9_000, Some((640, 480)))]).unwrap();
        assert_eq!(tiny.radios.selected, Some(4));
        assert!(!tiny.can_run());
    }

    #[test]
    fn sizes_and_frame_rates_read_plainly() {
        assert_eq!(size_words(10_000_000), "10 MB");
        assert_eq!(size_words(1_000_000), "1 MB");
        assert_eq!(size_words(2_500_000), "2.5 MB");
        assert_eq!(size_words(500_000), "500 KB");
        assert_eq!(fps_words(24.0), "24");
        assert_eq!(fps_words(29.97), "29.97");
        assert_eq!(fps_words(23.976), "23.98");
        assert_eq!(with_lines((1920, 1080), 720), (1280, 720));
        assert_eq!(with_lines((1080, 1920), 720), (720, 1280));
        assert_eq!(with_lines((640, 480), 720), (640, 480));
        assert_eq!(with_long_side((4032, 3024), 1920), (1920, 1440));
        assert_eq!(with_long_side((800, 600), 1920), (800, 600));
    }
}
