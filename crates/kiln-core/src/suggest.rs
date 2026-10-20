//! Suggestions: what Kiln offers for the files at hand, and which of it
//! to put forward.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::facts::{Bars, Facts, Look};
use crate::registry::Kind;
use crate::tools::Tool;

/// The conversions and tools Kiln can run on a set of files with the
/// engines that are installed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Offered {
    /// Target extensions every file can reach, in registry order.
    pub targets: Vec<String>,
    pub tools: Vec<Tool>,
}

/// What a suggestion puts forward: a conversion to a target extension,
/// or a tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Suggested {
    Convert(String),
    Tool(Tool),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    pub action: Suggested,
    /// One calm sentence, shown as is.
    pub reason: String,
    /// The likely size of the result in bytes, when it can be estimated.
    pub estimate: Option<u64>,
}

/// The convert wheel shows `convert`, the Tools wheel shows `tool`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Suggestions {
    pub convert: Option<Suggestion>,
    pub tool: Option<Suggestion>,
}

/// Picks per source type: from extension to (target extension to count).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Habits(BTreeMap<String, BTreeMap<String, u32>>);

impl Habits {
    /// The most source types kept.
    pub const MAX_SOURCES: usize = 64;
    /// Picks a target needs before it wins as a habit.
    const WINS_AT: u32 = 3;

    /// Count one pick of `to` for files of type `from`. Before a new
    /// source type would make 65, the one with the fewest picks goes
    /// (the first alphabetically on a tie).
    pub fn record(&mut self, from: &str, to: &str) {
        if !self.0.contains_key(from) && self.0.len() >= Self::MAX_SOURCES {
            let fewest = self
                .0
                .iter()
                .min_by_key(|(_, picks)| picks.values().map(|&n| u64::from(n)).sum::<u64>())
                .map(|(source, _)| source.clone());
            if let Some(source) = fewest {
                self.0.remove(&source);
            }
        }
        let count = self
            .0
            .entry(from.to_string())
            .or_default()
            .entry(to.to_string())
            .or_insert(0);
        *count = count.saturating_add(1);
    }

    /// The target picked for `from` at least 3 times and strictly more
    /// often than any other.
    pub fn favourite(&self, from: &str) -> Option<&str> {
        let picks = self.0.get(from)?;
        let mut best: Option<(&str, u32)> = None;
        let mut tied = false;
        for (to, &n) in picks {
            match best {
                Some((_, m)) if n < m => {}
                Some((_, m)) if n == m => tied = true,
                _ => {
                    best = Some((to.as_str(), n));
                    tied = false;
                }
            }
        }
        match best {
            Some((to, n)) if n >= Self::WINS_AT && !tied => Some(to),
            _ => None,
        }
    }

    /// Every source type's favourite, as `favourite` gives it.
    pub fn favourites(&self) -> BTreeMap<&str, &str> {
        self.0
            .keys()
            .filter_map(|from| Some((from.as_str(), self.favourite(from)?)))
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }
}

/// Camera RAW types.
const RAW: &[&str] = &[
    "dng", "cr2", "cr3", "nef", "arw", "orf", "rw2", "raf", "pef", "srw",
];
/// Image types where a photo is better kept as JPG.
const PHOTO: &[&str] = &[
    "png", "bmp", "tiff", "webp", "avif", "jxl", "tga", "qoi", "ppm",
];
/// Audio types better kept as MP3.
const AUDIO: &[&str] = &["wav", "aiff", "wma", "ac3", "ogg", "opus", "aac", "wv"];
/// Word-processor and markup types better kept as DOCX.
const WORD: &[&str] = &[
    "doc", "rtf", "odt", "pages", "wpd", "md", "rst", "org", "adoc", "tex", "typ", "wiki",
    "textile", "dbk",
];
const SHEET: &[&str] = &["xls", "ods", "numbers", "csv"];
const SLIDES: &[&str] = &["ppt", "odp", "key"];
const EBOOK: &[&str] = &["mobi", "azw3", "fb2"];
const SUBTITLE: &[&str] = &["ass", "vtt"];

/// Video above this average bit rate is large for its length.
const LARGE_BITRATE: f64 = 8_000_000.0;
/// Video over this many bytes is large whatever its length.
const LARGE_BYTES: u64 = 500_000_000;

/// Why a suggestion is made, before any number is put in.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Reason {
    Fixed(&'static str),
    /// A fixed sentence that promises a smaller file.
    Saving(&'static str),
    Photo,
    Habit(String),
    Compress,
}

impl Reason {
    /// The sentence, with `estimate` in it when the reason has a number.
    fn render(&self, estimate: Option<u64>) -> String {
        match (self, estimate) {
            (Reason::Fixed(text) | Reason::Saving(text), _) => (*text).to_string(),
            (Reason::Photo, Some(bytes)) => {
                format!("A photo. JPG keeps the look at about {}.", about(bytes))
            }
            (Reason::Photo, None) => "A photo. JPG keeps the look.".to_string(),
            (Reason::Habit(ext), _) => format!("You usually pick {}.", ext.to_ascii_uppercase()),
            (Reason::Compress, Some(bytes)) => {
                format!("Large for its length. About {} after.", about(bytes))
            }
            (Reason::Compress, None) => "Large for its length.".to_string(),
        }
    }

    /// Whether the sentence sells the conversion on a smaller file.
    fn claims_saving(&self) -> bool {
        matches!(self, Reason::Photo | Reason::Saving(_))
    }
}

/// Every convert rule `f` matches, in the spec's order, as target and reason.
fn convert_rules(f: &Facts) -> Vec<(&'static str, Reason)> {
    let ext = f.ext.as_str();
    let pic = f.picture.as_ref();
    let mut out = Vec::new();
    if ext == "gif" && pic.is_some_and(|p| p.animated) {
        out.push((
            "mp4",
            Reason::Saving("Same motion at a fraction of the size."),
        ));
    }
    if ext == "heic" || RAW.contains(&ext) {
        out.push((
            "jpg",
            Reason::Fixed("Opens anywhere. The original stays as it is."),
        ));
    }
    if PHOTO.contains(&ext) && pic.is_some_and(|p| p.look == Look::Photo && !p.alpha && !p.animated)
    {
        out.push(("jpg", Reason::Photo));
    }
    if ext != "png"
        && f.kind == Kind::Image
        && pic.is_some_and(|p| !p.animated && (p.look == Look::Graphic || p.alpha))
    {
        out.push((
            "png",
            Reason::Fixed("Sharp edges and transparency, and it opens anywhere."),
        ));
    }
    if f.kind == Kind::Video && ext != "mp4" {
        out.push(("mp4", Reason::Fixed("Plays on any phone, PC or browser.")));
    }
    if AUDIO.contains(&ext) {
        out.push((
            "mp3",
            Reason::Saving("Plays anywhere at a fraction of the size."),
        ));
    }
    let lists: [(&[&str], &'static str, &'static str); 5] = [
        (WORD, "docx", "Opens in Word and Google Docs."),
        (SHEET, "xlsx", "The spreadsheet format everyone can open."),
        (SLIDES, "pptx", "The slide format everyone can open."),
        (EBOOK, "epub", "Opens in most e-book readers."),
        (SUBTITLE, "srt", "Works with almost every player."),
    ];
    for (exts, to, reason) in lists {
        if exts.contains(&ext) {
            out.push((to, Reason::Fixed(reason)));
        }
    }
    out
}

/// The conversion for one file: its habit when that target is offered,
/// otherwise the first matching rule whose target is offered. A rule sold on
/// size is skipped when its estimate shows no clear saving.
fn convert_for(f: &Facts, offered: &Offered, habits: &Habits) -> Option<(String, Reason)> {
    let offers = |ext: &str| offered.targets.iter().any(|t| t == ext);
    if let Some(fav) = habits.favourite(&f.ext) {
        if offers(fav) {
            return Some((fav.to_string(), Reason::Habit(fav.to_string())));
        }
    }
    convert_rules(f)
        .into_iter()
        .find(|(to, reason)| {
            offers(to)
                && !(reason.claims_saving()
                    && f.estimates
                        .get(*to)
                        .is_some_and(|&after| !clearly_smaller(after, f.bytes)))
        })
        .map(|(to, reason)| (to.to_string(), reason))
}

fn is_large_video(f: &Facts) -> bool {
    if f.kind != Kind::Video {
        return false;
    }
    let rate = f
        .media
        .as_ref()
        .and_then(|m| m.duration)
        .filter(|&d| d > 0.0)
        .map(|d| f.bytes as f64 * 8.0 / d);
    rate.is_some_and(|r| r > LARGE_BITRATE) || f.bytes > LARGE_BYTES
}

/// Whether `after` is under 80% of `before`: a saving too small to see
/// isn't worth suggesting.
fn clearly_smaller(after: u64, before: u64) -> bool {
    u128::from(after) * 10 < u128::from(before) * 8
}

/// A large video whose compressed size, when known, is clearly smaller.
fn worth_compressing(f: &Facts) -> bool {
    is_large_video(f) && f.compressed.is_none_or(|c| clearly_smaller(c, f.bytes))
}

/// The sum of `values` when every one is known.
fn sum_all(mut values: impl Iterator<Item = Option<u64>>) -> Option<u64> {
    values.try_fold(0u64, |acc, v| Some(acc.saturating_add(v?)))
}

fn convert_suggestion(files: &[Facts], offered: &Offered, habits: &Habits) -> Option<Suggestion> {
    let mut picks = files.iter().map(|f| convert_for(f, offered, habits));
    let (to, reason) = picks.next()??;
    if !picks.all(|p| p.is_some_and(|(t, _)| t == to)) {
        return None;
    }
    let estimate = sum_all(files.iter().map(|f| f.estimates.get(&to).copied()));
    // A conversion shared by several files uses the numberless reason.
    let shown = if files.len() == 1 { estimate } else { None };
    Some(Suggestion {
        reason: reason.render(shown),
        action: Suggested::Convert(to),
        estimate,
    })
}

/// The Crop reason for `files`, by the bars they have. Files whose bars
/// differ share the numberless form.
fn crop_reason(files: &[Facts]) -> &'static str {
    let first = files.first().and_then(|f| f.bars);
    if !files.iter().all(|f| f.bars == first) {
        return "Black bars to crop.";
    }
    match first {
        Some(Bars::Sides) => "Black bars on the sides.",
        Some(Bars::AboveBelow) => "Black bars above and below.",
        Some(Bars::Around) => "Black bars around the picture.",
        // Crop doesn't apply then.
        _ => "Black bars to crop.",
    }
}

fn tool_suggestion(files: &[Facts], offered: &Offered) -> Option<Suggestion> {
    let every = |test: fn(&Facts) -> bool| files.iter().all(test);
    let several = files.len() >= 2;
    let rules: [(Tool, bool, Reason); 4] = [
        (Tool::Compress, every(worth_compressing), Reason::Compress),
        (
            Tool::Merge,
            several && every(|f| f.ext == "pdf"),
            Reason::Fixed("One PDF, in the order shown."),
        ),
        (
            Tool::Merge,
            several && every(|f| f.kind == Kind::Image),
            Reason::Fixed("One PDF, a page each, in the order shown."),
        ),
        (
            Tool::Crop,
            every(|f| f.kind == Kind::Video && f.bars.is_some_and(|b| b != Bars::None)),
            Reason::Fixed(crop_reason(files)),
        ),
    ];
    let (tool, _, reason) = rules
        .into_iter()
        .find(|(tool, applies, _)| *applies && offered.tools.contains(tool))?;
    let estimate = match tool {
        Tool::Compress => sum_all(files.iter().map(|f| f.compressed)),
        _ => None,
    };
    Some(Suggestion {
        action: Suggested::Tool(tool),
        reason: reason.render(estimate),
        estimate,
    })
}

/// What to put forward for `files`: at most one conversion and one tool,
/// each from `offered`. Pure: it reads nothing but its arguments.
pub fn suggest(files: &[Facts], offered: &Offered, habits: &Habits) -> Suggestions {
    // A scanned PDF gets no suggestion in this sub-project.
    let scanned = files
        .iter()
        .any(|f| f.pdf.as_ref().is_some_and(|p| p.has_text == Some(false)));
    if files.is_empty() || scanned {
        return Suggestions::default();
    }
    Suggestions {
        convert: convert_suggestion(files, offered, habits),
        tool: tool_suggestion(files, offered),
    }
}

/// Two significant figures, decimal units: "2 MB", "2.1 MB", "180 MB",
/// "950 KB". Anything under 1 KB reads "1 KB".
pub fn about(bytes: u64) -> String {
    let digits = bytes.max(1).ilog10() + 1;
    let rounded = if digits > 2 {
        let factor = 10u64.pow(digits - 2);
        (bytes / factor + u64::from(bytes % factor >= factor / 2)) * factor
    } else {
        bytes
    };
    let (unit, name) = match rounded {
        n if n >= 1_000_000_000 => (1_000_000_000, "GB"),
        n if n >= 1_000_000 => (1_000_000, "MB"),
        _ => (1_000, "KB"),
    };
    if rounded < unit {
        return "1 KB".to_string();
    }
    let tenths = rounded / (unit / 10);
    if tenths % 10 == 0 {
        format!("{} {name}", tenths / 10)
    } else {
        format!("{}.{} {name}", tenths / 10, tenths % 10)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{PdfFacts, Picture};
    use crate::quality::MediaInfo;
    use crate::registry::Registry;

    fn facts(ext: &str, kind: Kind) -> Facts {
        Facts {
            ext: ext.to_string(),
            kind,
            bytes: 1_000_000,
            picture: None,
            media: None,
            pdf: None,
            bars: None,
            estimates: BTreeMap::new(),
            compressed: None,
        }
    }

    fn with_picture(mut f: Facts, look: Look, alpha: bool, animated: bool) -> Facts {
        f.picture = Some(Picture {
            width: 1024,
            height: 768,
            alpha,
            animated,
            look,
        });
        f
    }

    fn with_media(mut f: Facts, bytes: u64, seconds: f64) -> Facts {
        f.bytes = bytes;
        f.media = Some(MediaInfo {
            duration: Some(seconds),
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            has_audio: true,
        });
        f
    }

    fn with_pdf(mut f: Facts, has_text: Option<bool>) -> Facts {
        f.pdf = Some(PdfFacts { pages: 3, has_text });
        f
    }

    fn with_bars(mut f: Facts, bars: Bars) -> Facts {
        f.bars = Some(bars);
        f
    }

    fn photo(ext: &str) -> Facts {
        with_picture(facts(ext, Kind::Image), Look::Photo, false, false)
    }

    fn graphic(ext: &str) -> Facts {
        with_picture(facts(ext, Kind::Image), Look::Graphic, false, false)
    }

    fn all() -> Offered {
        Offered {
            targets: [
                "mp4", "jpg", "png", "webp", "mp3", "docx", "xlsx", "pptx", "epub", "srt",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            tools: Tool::ALL.to_vec(),
        }
    }

    fn convert(files: &[Facts]) -> Option<Suggestion> {
        suggest(files, &all(), &Habits::default()).convert
    }

    fn tool(files: &[Facts]) -> Option<Suggestion> {
        suggest(files, &all(), &Habits::default()).tool
    }

    fn conv(to: &str, reason: &str, estimate: Option<u64>) -> Option<Suggestion> {
        Some(Suggestion {
            action: Suggested::Convert(to.to_string()),
            reason: reason.to_string(),
            estimate,
        })
    }

    fn tl(t: Tool, reason: &str, estimate: Option<u64>) -> Option<Suggestion> {
        Some(Suggestion {
            action: Suggested::Tool(t),
            reason: reason.to_string(),
            estimate,
        })
    }

    #[test]
    fn each_rule_suggests_its_target() {
        let mut estimated = photo("png");
        estimated.bytes = 6_000_000;
        estimated.estimates.insert("jpg".into(), 2_140_000);
        let cases: Vec<(Facts, Option<Suggestion>)> = vec![
            (
                with_picture(facts("gif", Kind::Image), Look::Graphic, false, true),
                conv("mp4", "Same motion at a fraction of the size.", None),
            ),
            (
                facts("heic", Kind::Image),
                conv("jpg", "Opens anywhere. The original stays as it is.", None),
            ),
            (
                facts("cr2", Kind::Image),
                conv("jpg", "Opens anywhere. The original stays as it is.", None),
            ),
            (
                estimated,
                conv(
                    "jpg",
                    "A photo. JPG keeps the look at about 2.1 MB.",
                    Some(2_140_000),
                ),
            ),
            (
                photo("png"),
                conv("jpg", "A photo. JPG keeps the look.", None),
            ),
            (
                graphic("bmp"),
                conv(
                    "png",
                    "Sharp edges and transparency, and it opens anywhere.",
                    None,
                ),
            ),
            (
                with_picture(facts("webp", Kind::Image), Look::Photo, true, false),
                conv(
                    "png",
                    "Sharp edges and transparency, and it opens anywhere.",
                    None,
                ),
            ),
            (
                facts("mov", Kind::Video),
                conv("mp4", "Plays on any phone, PC or browser.", None),
            ),
            (
                facts("wav", Kind::Audio),
                conv("mp3", "Plays anywhere at a fraction of the size.", None),
            ),
            (
                facts("md", Kind::Document),
                conv("docx", "Opens in Word and Google Docs.", None),
            ),
            (
                facts("doc", Kind::Document),
                conv("docx", "Opens in Word and Google Docs.", None),
            ),
            (
                facts("csv", Kind::Spreadsheet),
                conv("xlsx", "The spreadsheet format everyone can open.", None),
            ),
            (
                facts("key", Kind::Presentation),
                conv("pptx", "The slide format everyone can open.", None),
            ),
            (
                facts("mobi", Kind::Document),
                conv("epub", "Opens in most e-book readers.", None),
            ),
            (
                facts("vtt", Kind::Subtitle),
                conv("srt", "Works with almost every player.", None),
            ),
        ];
        for (f, want) in cases {
            let ext = f.ext.clone();
            assert_eq!(convert(&[f]), want, "{ext}");
        }
    }

    #[test]
    fn a_conversion_sold_on_size_needs_a_real_saving() {
        let with_estimate = |mut f: Facts, to: &str, bytes: u64, after: u64| {
            f.bytes = bytes;
            f.estimates.insert(to.into(), after);
            f
        };
        let gif = || with_picture(facts("gif", Kind::Image), Look::Graphic, false, true);
        // A tiny PNG photo whose JPG would be three times bigger.
        assert_eq!(
            convert(&[with_estimate(photo("png"), "jpg", 396, 1_200)]),
            None
        );
        // At 80% of the file or more, the saving is too small to suggest.
        assert_eq!(
            convert(&[with_estimate(photo("png"), "jpg", 1_000, 800)]),
            None
        );
        assert_eq!(
            convert(&[with_estimate(photo("png"), "jpg", 1_000, 799)]),
            conv(
                "jpg",
                "A photo. JPG keeps the look at about 1 KB.",
                Some(799)
            )
        );
        assert_eq!(
            convert(&[with_estimate(facts("wav", Kind::Audio), "mp3", 1_000, 900)]),
            None
        );
        assert_eq!(convert(&[with_estimate(gif(), "mp4", 1_000, 2_000)]), None);
        // Reasons that aren't about size still stand when the result is bigger.
        assert_eq!(
            convert(&[with_estimate(
                facts("heic", Kind::Image),
                "jpg",
                1_000,
                3_000
            )]),
            conv(
                "jpg",
                "Opens anywhere. The original stays as it is.",
                Some(3_000)
            )
        );
        // One file that wouldn't shrink keeps a shared JPG off the whole set.
        let big = with_estimate(photo("png"), "jpg", 6_000_000, 2_000_000);
        let tiny = with_estimate(photo("png"), "jpg", 396, 1_200);
        assert_eq!(convert(&[big, tiny]), None);
        // A habit is the person's own choice and isn't second-guessed.
        let mut habits = Habits::default();
        for _ in 0..3 {
            habits.record("png", "jpg");
        }
        let tiny = with_estimate(photo("png"), "jpg", 396, 1_200);
        assert_eq!(
            suggest(&[tiny], &all(), &habits).convert,
            conv("jpg", "You usually pick JPG.", Some(1_200))
        );
    }

    #[test]
    fn a_gif_without_picture_facts_is_not_animated() {
        assert_eq!(convert(&[facts("gif", Kind::Image)]), None);
    }

    #[test]
    fn each_tool_rule() {
        let mut big = with_media(facts("mp4", Kind::Video), 600_000_000, 60.0);
        big.compressed = Some(183_400_000);
        assert_eq!(
            tool(&[big]),
            tl(
                Tool::Compress,
                "Large for its length. About 180 MB after.",
                Some(183_400_000)
            )
        );

        let fast = with_media(facts("mp4", Kind::Video), 9_000_000 * 10 / 8, 10.0);
        assert_eq!(
            tool(&[fast]),
            tl(Tool::Compress, "Large for its length.", None)
        );

        let calm = with_media(facts("mp4", Kind::Video), 50_000_000, 100.0);
        assert_eq!(tool(&[calm]), None);

        // Compress only when the result would be under 80% of the file.
        for (after, suggested) in [
            (479_999_999, true),
            (480_000_000, false),
            (660_000_000, false),
        ] {
            let mut f = with_media(facts("mp4", Kind::Video), 600_000_000, 60.0);
            f.compressed = Some(after);
            assert_eq!(tool(&[f]).is_some(), suggested, "{after}");
        }
        // In a set, every file has to be worth it.
        let mut worth = with_media(facts("mp4", Kind::Video), 600_000_000, 60.0);
        worth.compressed = Some(100_000_000);
        let mut not = worth.clone();
        not.compressed = Some(590_000_000);
        assert_eq!(tool(&[worth.clone(), not]), None);
        assert!(tool(&[worth.clone(), worth]).is_some());

        let pdfs = [
            with_pdf(facts("pdf", Kind::Document), Some(true)),
            with_pdf(facts("pdf", Kind::Document), Some(true)),
        ];
        assert_eq!(
            tool(&pdfs),
            tl(Tool::Merge, "One PDF, in the order shown.", None)
        );

        assert_eq!(
            tool(&[photo("jpg"), graphic("png")]),
            tl(
                Tool::Merge,
                "One PDF, a page each, in the order shown.",
                None
            )
        );

        let barred = |b| with_bars(facts("mp4", Kind::Video), b);
        for (bars, reason) in [
            (Bars::Sides, "Black bars on the sides."),
            (Bars::AboveBelow, "Black bars above and below."),
            (Bars::Around, "Black bars around the picture."),
        ] {
            assert_eq!(tool(&[barred(bars)]), tl(Tool::Crop, reason, None));
            // A set whose bars agree keeps the worded reason.
            assert_eq!(
                tool(&[barred(bars), barred(bars)]),
                tl(Tool::Crop, reason, None)
            );
        }
        // A set whose bars differ shares the numberless form.
        assert_eq!(
            tool(&[barred(Bars::Sides), barred(Bars::AboveBelow)]),
            tl(Tool::Crop, "Black bars to crop.", None)
        );
        // No bars worth cropping, or not known, is no Crop.
        assert_eq!(tool(&[barred(Bars::None)]), None);
        assert_eq!(tool(&[facts("mp4", Kind::Video)]), None);
        assert_eq!(tool(&[barred(Bars::Sides), barred(Bars::None)]), None);
    }

    #[test]
    fn already_in_place() {
        assert_eq!(convert(&[photo("jpg")]), None);
        assert_eq!(convert(&[facts("mp4", Kind::Video)]), None);
        assert_eq!(convert(&[facts("mp3", Kind::Audio)]), None);
    }

    #[test]
    fn scanned_pdf_gets_nothing() {
        let scan = with_pdf(facts("pdf", Kind::Document), Some(false));
        assert_eq!(
            suggest(&[scan], &all(), &Habits::default()),
            Suggestions::default()
        );
    }

    #[test]
    fn nothing_matches() {
        assert_eq!(
            suggest(&[facts("txt", Kind::Document)], &all(), &Habits::default()),
            Suggestions::default()
        );
    }

    #[test]
    fn no_files_no_suggestions() {
        assert_eq!(
            suggest(&[], &all(), &Habits::default()),
            Suggestions::default()
        );
    }

    #[test]
    fn missing_target_is_never_suggested() {
        let offered = Offered {
            targets: vec!["avi".into()],
            ..all()
        };
        let got = suggest(&[facts("mov", Kind::Video)], &offered, &Habits::default());
        assert_eq!(got.convert, None);
    }

    #[test]
    fn habits_win_at_three_not_two() {
        let heic = [facts("heic", Kind::Image)];
        let mut habits = Habits::default();
        habits.record("heic", "png");
        habits.record("heic", "png");
        let got = suggest(&heic, &all(), &habits).convert.unwrap();
        assert_eq!(got.action, Suggested::Convert("jpg".into()));

        habits.record("heic", "png");
        assert_eq!(
            suggest(&heic, &all(), &habits).convert,
            conv("png", "You usually pick PNG.", None)
        );
    }

    #[test]
    fn a_tie_is_no_habit() {
        let mut habits = Habits::default();
        for _ in 0..3 {
            habits.record("heic", "png");
            habits.record("heic", "webp");
        }
        assert_eq!(habits.favourite("heic"), None);
        let got = suggest(&[facts("heic", Kind::Image)], &all(), &habits)
            .convert
            .unwrap();
        assert_eq!(got.action, Suggested::Convert("jpg".into()));
    }

    #[test]
    fn several_files_must_agree() {
        let mut a = photo("png");
        a.bytes = 5_000_000;
        a.estimates.insert("jpg".into(), 1_000_000);
        let mut b = photo("png");
        b.bytes = 8_000_000;
        b.estimates.insert("jpg".into(), 2_000_000);
        assert_eq!(
            convert(&[a, b]),
            conv("jpg", "A photo. JPG keeps the look.", Some(3_000_000))
        );
        assert_eq!(convert(&[photo("png"), graphic("png")]), None);
    }

    #[test]
    fn habits_keep_64_sources() {
        let mut habits = Habits::default();
        habits.record("s00", "jpg");
        for i in 1..65 {
            habits.record(&format!("s{i:02}"), "jpg");
            habits.record(&format!("s{i:02}"), "jpg");
        }
        assert_eq!(habits.0.len(), Habits::MAX_SOURCES);
        assert!(!habits.0.contains_key("s00"));
        assert!(habits.0.contains_key("s64"));
        assert!(!habits.is_empty());
        habits.clear();
        assert!(habits.is_empty());
    }

    #[test]
    fn favourites_lists_each_source_that_has_one() {
        let mut habits = Habits::default();
        for _ in 0..3 {
            habits.record("heic", "jpg");
        }
        habits.record("png", "jpg");
        for _ in 0..3 {
            habits.record("mov", "mp4");
            habits.record("mov", "mkv");
        }
        let fav: Vec<(&str, &str)> = habits.favourites().into_iter().collect();
        assert_eq!(fav, [("heic", "jpg")]);
        // One more pick that doesn't change a winner leaves them equal.
        let before: Vec<(String, String)> = owned(&habits);
        habits.record("png", "webp");
        assert_eq!(owned(&habits), before);
        habits.record("mov", "mp4");
        assert_ne!(owned(&habits), before);
    }

    fn owned(h: &Habits) -> Vec<(String, String)> {
        h.favourites()
            .into_iter()
            .map(|(a, b)| (a.to_owned(), b.to_owned()))
            .collect()
    }

    #[test]
    fn about_rounds_to_two_figures() {
        assert_eq!(about(2_040_000), "2 MB");
        assert_eq!(about(2_140_000), "2.1 MB");
        assert_eq!(about(183_400_000), "180 MB");
        assert_eq!(about(950_400), "950 KB");
        assert_eq!(about(999_600), "1 MB");
        assert_eq!(about(9_960_000), "10 MB");
        assert_eq!(about(500), "1 KB");
    }

    #[test]
    fn rule_extensions_are_real_formats() {
        let registry = Registry::builtin();
        let lists = [RAW, PHOTO, AUDIO, WORD, SHEET, SLIDES, EBOOK, SUBTITLE];
        let named = [
            "gif", "heic", "png", "mp4", "jpg", "mp3", "docx", "xlsx", "pptx", "epub", "srt", "pdf",
        ];
        for ext in lists.iter().flat_map(|l| l.iter()).chain(named.iter()) {
            let format = registry.format(ext);
            assert_eq!(format.map(|f| f.ext.as_str()), Some(*ext), "{ext}");
        }
    }
}
