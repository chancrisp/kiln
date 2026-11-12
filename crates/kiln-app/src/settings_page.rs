//! Settings as one page of cards (mock screen 6): its sections and cards in
//! the mock's order, what each card says, and the control at its right.
//! Pure, so the words and the order can be tested without a window;
//! `layout::settings` places it, `win::surfaces::settings` drives it and
//! `win::render::settings` draws it.

use kiln_core::{Limit, Preset};

use crate::model::{Prompt, PromptButton};
use crate::quality::{DefaultSetting, Value};
use crate::settings::{self, Engine, EngineStatus, OfficeStatus, Settings, ThemeChoice};
use crate::theme::Rgb;
use crate::update::{UpdateState, CURRENT};

/// The page's sections, top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Appearance,
    Saving,
    Quality,
    DragWheel,
    HelperApps,
    About,
}

impl Section {
    pub const ALL: [Section; 6] = [
        Section::Appearance,
        Section::Saving,
        Section::Quality,
        Section::DragWheel,
        Section::HelperApps,
        Section::About,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Section::Appearance => "Appearance",
            Section::Saving => "Saving",
            Section::Quality => "Quality",
            Section::DragWheel => "Drag-wheel",
            Section::HelperApps => kiln_core::plain::HELPER_APPS_TAB,
            Section::About => "About",
        }
    }
}

/// The helper apps, each an Expander with its status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Helper {
    Ffmpeg,
    Office,
    LibreOffice,
    Pandoc,
    Calibre,
}

impl Helper {
    pub const ALL: [Helper; 5] = [
        Helper::Ffmpeg,
        Helper::Office,
        Helper::LibreOffice,
        Helper::Pandoc,
        Helper::Calibre,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    /// The engine Kiln runs for it; Microsoft Office isn't one Kiln is
    /// pointed at by path.
    pub fn engine(self) -> Option<Engine> {
        match self {
            Helper::Ffmpeg => Some(Engine::Ffmpeg),
            Helper::Office => None,
            Helper::LibreOffice => Some(Engine::LibreOffice),
            Helper::Pandoc => Some(Engine::Pandoc),
            Helper::Calibre => Some(Engine::Calibre),
        }
    }

    pub fn label(self) -> &'static str {
        match self.engine() {
            Some(e) => e.label(),
            None => settings::OFFICE_LABEL,
        }
    }
}

/// One card of the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Card {
    Theme,
    SaveWhere,
    Video,
    VideoSize,
    Photos,
    PhotoSize,
    Sound,
    PageDpi,
    DragWheel,
    Helper(Helper),
    Version,
    Diagnostics,
    Donate,
    Licences,
}

/// Something on the page that can be pressed, picked or turned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Control {
    /// The Theme ComboBox.
    Theme,
    /// "Where new files go": Next to the original, or A folder I choose….
    SaveWhere,
    /// One radio button of Video's or Photos' Small, High and Best.
    Preset(DefaultSetting, Preset),
    /// A quality ComboBox: Largest video size, Longest side, Sound,
    /// PDF pages as pictures.
    Choice(DefaultSetting),
    /// The drag-wheel's ToggleSwitch.
    DragWheel,
    /// A helper app's Expander header: it opens and closes.
    Helper(Helper),
    /// "Choose…", or FFmpeg's "Use another copy…": a program picker.
    Choose(Engine),
    /// Back to finding the helper by itself, when one was chosen.
    Reset(Engine),
    /// A missing helper's Get it: its official download page.
    Get(Engine),
    /// A missing helper's Install with winget, when winget is there.
    Winget(Engine),
    CheckUpdates,
    /// Diagnostics' Copy diagnostics.
    CopyDiagnostics,
    /// Diagnostics' Open log folder.
    OpenLogFolder,
    /// Support Kiln's Donate.
    Donate,
    /// Licences and third-party notices' Open.
    Licences,
}

/// What a control was asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Set {
    /// A button, or an Expander's header, was pressed.
    Press,
    /// A ComboBox or radio button's choice `n` was picked.
    Pick(usize),
    /// A ToggleSwitch was turned on (true) or off.
    Turn(bool),
}

/// A helper's status at the right of its header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Badge {
    pub text: &'static str,
    /// Found or included, drawn in the success colours; otherwise in the
    /// control's.
    pub found: bool,
}

pub const INCLUDED: &str = "Included";
pub const FOUND: &str = "Found";
pub const NOT_FOUND: &str = "Not found";

/// A helper app's Expander.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpanderSpec {
    pub helper: Helper,
    /// `None` until Kiln has looked.
    pub badge: Option<Badge>,
    pub open: bool,
    /// Its content: a line (with a Caption label over it when it is a place
    /// on disk), a caption under it, and its buttons.
    pub line_label: Option<&'static str>,
    pub line: String,
    pub line_note: Option<String>,
    pub buttons: Vec<(Control, String)>,
}

/// Whether picking choice `i` of `c`'s list asks for something first (A
/// folder I choose… opens the folder picker): an arrow on the closed list
/// only shows it, and Enter or a click chooses it.
pub fn asks_first(c: Control, i: usize) -> bool {
    c == Control::SaveWhere && i == 1
}

/// What sits at a card's right.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    Combo {
        control: Control,
        choices: Vec<String>,
        value: usize,
        /// For the Theme list: each choice's base, text and accent colours,
        /// shown as three dots at the right of its item. Decorative: the
        /// choice's name says what it is. Empty for any other list.
        swatches: Vec<[Rgb; 3]>,
    },
    Toggle {
        control: Control,
        on: bool,
    },
    /// Small, High and Best.
    Radios {
        setting: DefaultSetting,
        value: Option<Preset>,
    },
    Buttons(Vec<(Control, String)>),
    Expander(ExpanderSpec),
}

/// One card: its glyph (`None` for the K mark), its label, the caption
/// under it, and its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardSpec {
    pub card: Card,
    pub glyph: Option<char>,
    pub label: String,
    pub note: Option<String>,
    /// The note says something went wrong, in the critical ink.
    pub alert: bool,
    pub body: Body,
}

/// The whole page, section by section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageSpec {
    pub sections: Vec<(Section, Vec<CardSpec>)>,
}

impl PageSpec {
    pub fn cards(&self) -> impl Iterator<Item = &CardSpec> {
        self.sections.iter().flat_map(|(_, c)| c.iter())
    }

    pub fn card(&self, card: Card) -> Option<&CardSpec> {
        self.cards().find(|c| c.card == card)
    }
}

/// What Kiln has found out that the page shows: where each helper is,
/// Microsoft Office, the update check, and a card's note replaced by
/// something that went wrong (a page that didn't open).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facts {
    /// In `Engine::ALL` order; `None` until the engines thread answers.
    pub engines: Option<[EngineStatus; 4]>,
    /// `None` until Office has been looked for.
    pub office: Option<OfficeStatus>,
    pub update: UpdateState,
    pub alert: Option<(Card, String)>,
    /// The folder chosen for new files isn't there now.
    pub save_to_missing: bool,
    /// Whether Windows' app mode is light, which System in the Theme list
    /// follows.
    pub system_light: bool,
    /// A Windows contrast theme is on, so it, not the Theme setting, makes
    /// the colours.
    pub contrast: bool,
    /// `winget.exe` was found, so a missing helper can be installed with it.
    pub winget: bool,
}

/// The cards' glyphs (mock screen 6), Segoe Fluent Icons (the same code
/// points in Segoe MDL2 Assets): one for each card, so no two rows of the
/// page share one.
pub mod glyph {
    /// Color (a palette).
    pub const THEME: char = '\u{E790}';
    /// OpenFolderHorizontal.
    pub const FOLDER: char = '\u{E8B7}';
    /// Video (a camera).
    pub const VIDEO: char = '\u{E714}';
    /// TVMonitor: how big a video may be.
    pub const VIDEO_SIZE: char = '\u{E7F4}';
    /// Photo2 (a picture).
    pub const PHOTO: char = '\u{EB9F}';
    /// A frame's corners: how big a photo may be.
    pub const PHOTO_SIZE: char = '\u{E9A6}';
    /// Audio (a note).
    pub const SOUND: char = '\u{E8D6}';
    /// Page: a PDF's pages.
    pub const PDF_PAGES: char = '\u{E7C3}';
    /// TouchPointer.
    pub const DRAG_WHEEL: char = '\u{E7C9}';
    /// A film frame: FFmpeg.
    pub const FFMPEG: char = '\u{E8B2}';
    /// A page of text with a heading: LibreOffice.
    pub const LIBREOFFICE: char = '\u{E8A1}';
    /// Microsoft Office, when its card shows.
    pub const OFFICE: char = '\u{E8A5}';
    /// A document with an arrow: Pandoc converts documents.
    pub const PANDOC: char = '\u{E8E5}';
    /// An open book: Calibre.
    pub const BOOK: char = '\u{E736}';
    /// A certificate: the licences.
    pub const LICENCES: char = '\u{EB95}';
    /// A bug: the diagnostics for a bug report.
    pub const DIAGNOSTICS: char = '\u{EBE8}';
    /// A heart: supporting Kiln.
    pub const DONATE: char = '\u{EB51}';

    pub const ALL: [(&str, char); 17] = [
        ("theme", THEME),
        ("folder", FOLDER),
        ("video", VIDEO),
        ("video-size", VIDEO_SIZE),
        ("photo", PHOTO),
        ("photo-size", PHOTO_SIZE),
        ("sound", SOUND),
        ("pdf-pages", PDF_PAGES),
        ("drag-wheel", DRAG_WHEEL),
        ("ffmpeg", FFMPEG),
        ("libreoffice", LIBREOFFICE),
        ("office", OFFICE),
        ("pandoc", PANDOC),
        ("book", BOOK),
        ("licences", LICENCES),
        ("diagnostics", DIAGNOSTICS),
        ("donate", DONATE),
    ];
}

/// The Caption label over a helper's place on disk.
pub const LOCATION: &str = "Location";

// The words of the page. The mock's, but for R7 and what the report's
// copy list names.
pub const TITLE: &str = "Settings";
pub const THEME_LABEL: &str = "Theme";
pub const THEME_NOTE: &str = "System follows Windows' app mode.";
pub const THEME_CONTRAST_NOTE: &str =
    "A Windows contrast theme is on, so Kiln is using its colours.";
pub const SAVE_WHERE_LABEL: &str = "Where new files go";
pub const NEXT_TO_ORIGINAL: &str = "Next to the original";
pub const A_FOLDER_I_CHOOSE: &str = "A folder I choose\u{2026}";
pub const VIDEO_LABEL: &str = "Video";
pub const VIDEO_SIZE_LABEL: &str = "Largest video size";
pub const VIDEO_SIZE_NOTE: &str = "Bigger videos are made smaller to fit.";
pub const PHOTOS_LABEL: &str = "Photos";
pub const PHOTO_SIZE_LABEL: &str = "Longest side";
pub const PHOTO_SIZE_NOTE: &str = "In pixels.";
pub const SOUND_LABEL: &str = "Sound";
pub const PAGE_DPI_LABEL: &str = "PDF pages as pictures";
pub const FFMPEG_LINE: &str = "Comes with Kiln, as an LGPL build.";
pub const FFMPEG_LINE_NOTE: &str = "Its licence and where to get its source are in About.";
pub const USE_ANOTHER_COPY: &str = "Use another copy\u{2026}";
pub const CHOOSE: &str = "Choose\u{2026}";
pub const RESET: &str = "Reset";
pub const FINDS_IT_AGAIN: &str = "Kiln finds it again as soon as it's installed.";
pub const LICENCES_LABEL: &str = "Licences and third-party notices";
pub const LICENCES_NOTE: &str =
    "Kiln is free software under the GPL 3.0 or later, with no warranty.";
pub const OPEN: &str = "Open";
/// Said in the Licences card when its file didn't open.
pub const LICENCES_DIDNT_OPEN: &str = "Couldn't open the notices.";
pub const DIAGNOSTICS_LABEL: &str = "Diagnostics";
pub const DIAGNOSTICS_NOTE: &str =
    "Kiln's version, Windows' version and the end of its log, for a bug report.";
pub const COPY_DIAGNOSTICS: &str = "Copy diagnostics";
pub const OPEN_LOG_FOLDER: &str = "Open log folder";
/// Said in the Diagnostics card when the log folder didn't open.
pub const LOG_FOLDER_DIDNT_OPEN: &str = "Couldn't open the log folder.";
pub const DONATE_LABEL: &str = "Support Kiln";
pub const DONATE_NOTE: &str = "Donations pay for code signing and upkeep.";
pub const DONATE_BUTTON: &str = "Donate";
/// Said in the Support Kiln card when its page didn't open.
pub const DONATE_DIDNT_OPEN: &str = "Couldn't open the page.";
/// Where Donate goes.
pub const DONATE_PAGE: &str = "https://github.com/chancrisp/kiln/blob/main/DONATE.md";
pub const INSTALL_WITH_WINGET: &str = "Install with winget";

/// The Theme ComboBox's choices, in the mock's order.
pub const THEMES: [ThemeChoice; 9] = ThemeChoice::ALL;

/// The values a quality ComboBox offers (mock screen 6), with `current`
/// among them in its place when a hand-edited or older file holds another.
pub fn choices(s: DefaultSetting, current: Value) -> Vec<Value> {
    let mut list: Vec<Value> = match s {
        DefaultSetting::Resolution => std::iter::once(Limit::Original)
            .chain([2160, 1080, 720, 480].map(Limit::Max))
            .map(Value::Limit)
            .collect(),
        DefaultSetting::ImageSize => std::iter::once(Limit::Original)
            .chain([3840, 2560, 1920, 1280].map(Limit::Max))
            .map(Value::Limit)
            .collect(),
        DefaultSetting::Audio => std::iter::once(None)
            .chain([128, 192, 256, 320].map(Some))
            .map(Value::Kbps)
            .collect(),
        DefaultSetting::PageDpi => [96, 150, 300].map(Value::Dpi).to_vec(),
        DefaultSetting::Video | DefaultSetting::Images => {
            Preset::ALL.into_iter().map(Value::Preset).collect()
        }
    };
    if !list.contains(&current) {
        let at = list
            .iter()
            .position(|v| rank(*v) > rank(current))
            .unwrap_or(list.len());
        list.insert(at, current);
    }
    list
}

/// Where a value sorts in its list: Original (or Automatic) first, then
/// sizes from the largest down, bitrates and DPI from the smallest up.
fn rank(v: Value) -> (u8, i64) {
    match v {
        Value::Limit(Limit::Original) => (0, 0),
        Value::Limit(Limit::Auto) => (1, 0),
        Value::Limit(Limit::Max(n)) => (2, -i64::from(n)),
        Value::Kbps(None) => (0, 0),
        Value::Kbps(Some(k)) => (2, i64::from(k)),
        Value::Dpi(d) => (2, i64::from(d)),
        Value::Preset(p) => (2, p as i64),
    }
}

/// How a quality ComboBox's value reads (mock screen 6).
pub fn choice_label(s: DefaultSetting, v: Value) -> String {
    match (s, v) {
        (_, Value::Kbps(None)) => "Automatic".to_owned(),
        (DefaultSetting::ImageSize, Value::Limit(Limit::Max(n))) => n.to_string(),
        _ => s.label(v),
    }
}

/// The base, text and accent colours `choice` draws with, as dots.
fn swatch(choice: ThemeChoice, system_light: bool) -> [Rgb; 3] {
    let p = crate::theme::fluent::palette(None, choice, system_light);
    [p.base, p.text, p.accent]
}

/// The Settings page as `s` and `facts` have it, with the helper apps'
/// Expanders open as `open` says (in `Helper::ALL` order).
pub fn page(s: &Settings, facts: &Facts, open: [bool; 5], hooks_refused: bool) -> PageSpec {
    let card = |card, glyph, label: &str, note: Option<&str>, body| CardSpec {
        card,
        glyph: Some(glyph),
        label: label.to_owned(),
        note: note.map(str::to_owned),
        alert: false,
        body,
    };
    let q = &s.quality;
    let combo = |setting: DefaultSetting| {
        let current = q.get(setting);
        let list = choices(setting, current);
        Body::Combo {
            control: Control::Choice(setting),
            value: list.iter().position(|v| *v == current).unwrap_or(0),
            choices: list.iter().map(|v| choice_label(setting, *v)).collect(),
            swatches: vec![],
        }
    };
    let radios = |setting: DefaultSetting| Body::Radios {
        setting,
        value: match q.get(setting) {
            Value::Preset(p) => Some(p),
            _ => None,
        },
    };

    let appearance = vec![card(
        Card::Theme,
        glyph::THEME,
        THEME_LABEL,
        Some(if facts.contrast {
            THEME_CONTRAST_NOTE
        } else {
            THEME_NOTE
        }),
        Body::Combo {
            control: Control::Theme,
            choices: THEMES.iter().map(|t| t.label().to_owned()).collect(),
            value: THEMES.iter().position(|t| *t == s.theme).unwrap_or(0),
            // The three dots at the right of each item, so a theme can be
            // picked without knowing its name.
            swatches: THEMES
                .iter()
                .map(|t| swatch(*t, facts.system_light))
                .collect(),
        },
    )];

    let folder = s.save_to.as_ref().map(|p| match facts.save_to_missing {
        true => format!("Not found at {}", p.display()),
        false => p.display().to_string(),
    });
    let mut save_where = card(
        Card::SaveWhere,
        glyph::FOLDER,
        SAVE_WHERE_LABEL,
        folder.as_deref(),
        Body::Combo {
            control: Control::SaveWhere,
            choices: vec![NEXT_TO_ORIGINAL.to_owned(), A_FOLDER_I_CHOOSE.to_owned()],
            value: usize::from(s.save_to.is_some()),
            swatches: vec![],
        },
    );
    save_where.alert = s.save_to.is_some() && facts.save_to_missing;
    let saving = vec![save_where];

    let quality = vec![
        card(
            Card::Video,
            glyph::VIDEO,
            VIDEO_LABEL,
            None,
            radios(DefaultSetting::Video),
        ),
        card(
            Card::VideoSize,
            glyph::VIDEO_SIZE,
            VIDEO_SIZE_LABEL,
            Some(VIDEO_SIZE_NOTE),
            combo(DefaultSetting::Resolution),
        ),
        card(
            Card::Photos,
            glyph::PHOTO,
            PHOTOS_LABEL,
            None,
            radios(DefaultSetting::Images),
        ),
        card(
            Card::PhotoSize,
            glyph::PHOTO_SIZE,
            PHOTO_SIZE_LABEL,
            Some(PHOTO_SIZE_NOTE),
            combo(DefaultSetting::ImageSize),
        ),
        card(
            Card::Sound,
            glyph::SOUND,
            SOUND_LABEL,
            None,
            combo(DefaultSetting::Audio),
        ),
        card(
            Card::PageDpi,
            glyph::PDF_PAGES,
            PAGE_DPI_LABEL,
            None,
            combo(DefaultSetting::PageDpi),
        ),
    ];

    let drag_wheel = vec![CardSpec {
        alert: s.drag_wheel && hooks_refused,
        ..card(
            Card::DragWheel,
            glyph::DRAG_WHEEL,
            settings::DRAG_WHEEL_LABEL,
            Some(if s.drag_wheel && hooks_refused {
                settings::DRAG_WHEEL_REFUSED
            } else {
                settings::DRAG_WHEEL_NOTE
            }),
            Body::Toggle {
                control: Control::DragWheel,
                on: s.drag_wheel,
            },
        )
    }];

    // Microsoft Office's card only while Office to PDF is switched on: Kiln doesn't use Office otherwise.
    let helpers = Helper::ALL
        .into_iter()
        .filter(|&h| h != Helper::Office || kiln_core::office::OFFICE_TO_PDF)
        .map(|h| helper_card(h, facts, open[h.index()]))
        .collect();

    let (update_note, _) = facts.update.note();
    let about = vec![
        CardSpec {
            card: Card::Version,
            glyph: None,
            label: format!("Kiln {CURRENT}"),
            note: (facts.update != UpdateState::Idle).then_some(update_note),
            alert: matches!(facts.update, UpdateState::Failed(_)),
            body: Body::Buttons(vec![(
                Control::CheckUpdates,
                facts.update.button().to_owned(),
            )]),
        },
        card(
            Card::Diagnostics,
            glyph::DIAGNOSTICS,
            DIAGNOSTICS_LABEL,
            Some(DIAGNOSTICS_NOTE),
            Body::Buttons(vec![
                (Control::CopyDiagnostics, COPY_DIAGNOSTICS.to_owned()),
                (Control::OpenLogFolder, OPEN_LOG_FOLDER.to_owned()),
            ]),
        ),
        card(
            Card::Donate,
            glyph::DONATE,
            DONATE_LABEL,
            Some(DONATE_NOTE),
            Body::Buttons(vec![(Control::Donate, DONATE_BUTTON.to_owned())]),
        ),
        card(
            Card::Licences,
            glyph::LICENCES,
            LICENCES_LABEL,
            Some(LICENCES_NOTE),
            Body::Buttons(vec![(Control::Licences, OPEN.to_owned())]),
        ),
    ];

    let mut page = PageSpec {
        sections: vec![
            (Section::Appearance, appearance),
            (Section::Saving, saving),
            (Section::Quality, quality),
            (Section::DragWheel, drag_wheel),
            (Section::HelperApps, helpers),
            (Section::About, about),
        ],
    };
    // Something that went wrong takes its card's note.
    if let Some((at, text)) = &facts.alert {
        for (_, cards) in &mut page.sections {
            for c in cards.iter_mut().filter(|c| c.card == *at) {
                c.note = Some(text.clone());
                c.alert = true;
            }
        }
    }
    page
}

/// What Install with winget asks first, for `e`: it says what will run.
/// `None` for an engine winget has no package for.
pub fn winget_prompt(e: Engine) -> Option<Prompt> {
    let id = e.winget_id()?;
    Some(Prompt {
        heading: Some(format!("Install {} with winget?", e.label())),
        title: format!(
            "Kiln opens a console window that runs winget install -e --id {id}. Windows may ask for permission."
        ),
        buttons: vec![PromptButton::Install, PromptButton::Cancel],
    })
}

/// A helper app's card: its purpose under its name, its status, and what
/// opening it shows.
fn helper_card(h: Helper, facts: &Facts, open: bool) -> CardSpec {
    let glyph = match h {
        Helper::Ffmpeg => glyph::FFMPEG,
        Helper::Office => glyph::OFFICE,
        Helper::LibreOffice => glyph::LIBREOFFICE,
        Helper::Pandoc => glyph::PANDOC,
        Helper::Calibre => glyph::BOOK,
    };
    let found = |text| Badge { text, found: true };
    let missing = Badge {
        text: NOT_FOUND,
        found: false,
    };
    let spec = match h.engine() {
        // Microsoft Office: only what was found. Office to PDF isn't
        // offered as working yet, so its card makes no promise of it.
        None => match &facts.office {
            None => ExpanderSpec {
                helper: h,
                badge: None,
                open,
                line_label: None,
                line: String::new(),
                line_note: None,
                buttons: vec![],
            },
            Some(o) => ExpanderSpec {
                helper: h,
                badge: Some(if o.apps.is_some() {
                    found(FOUND)
                } else {
                    missing
                }),
                open,
                line_label: None,
                line: o.apps.clone().unwrap_or_else(|| FINDS_IT_AGAIN.to_owned()),
                line_note: o
                    .folder
                    .as_ref()
                    .filter(|_| o.apps.is_some())
                    .map(|f| f.display().to_string()),
                buttons: vec![],
            },
        },
        Some(e) => {
            let status = facts.engines.as_ref().map(|s| &s[engine_index(e)]);
            let choose = (
                Control::Choose(e),
                if e == Engine::Ffmpeg {
                    USE_ANOTHER_COPY
                } else {
                    CHOOSE
                }
                .to_owned(),
            );
            let reset = (Control::Reset(e), RESET.to_owned());
            let get = e
                .download_page()
                .map(|_| (Control::Get(e), format!("Get {}", e.label())));
            // Beside it, when winget is there to do the installing: only
            // for a helper with a package, so never FFmpeg.
            let winget = (facts.winget && e.winget_id().is_some())
                .then(|| (Control::Winget(e), INSTALL_WITH_WINGET.to_owned()));
            // A bare path says what it is: "Location" over it.
            let (badge, label, line, line_note, buttons) = match status {
                None => (None, None, String::new(), None, vec![choose]),
                Some(EngineStatus::Included(_)) => (
                    Some(found(INCLUDED)),
                    None,
                    FFMPEG_LINE.to_owned(),
                    Some(FFMPEG_LINE_NOTE.to_owned()),
                    vec![choose],
                ),
                Some(st @ EngineStatus::Chosen { exists, .. }) => (
                    Some(if *exists { found(FOUND) } else { missing }),
                    exists.then_some(LOCATION),
                    st.describe(),
                    None,
                    vec![choose, reset],
                ),
                Some(EngineStatus::OnPath(p) | EngineStatus::Installed(p)) => (
                    Some(found(FOUND)),
                    Some(LOCATION),
                    p.display().to_string(),
                    None,
                    vec![choose],
                ),
                Some(EngineStatus::Missing) => (
                    Some(missing),
                    None,
                    FINDS_IT_AGAIN.to_owned(),
                    None,
                    get.into_iter().chain(winget).chain([choose]).collect(),
                ),
            };
            ExpanderSpec {
                helper: h,
                badge,
                open,
                line_label: label,
                line,
                line_note,
                buttons,
            }
        }
    };
    CardSpec {
        card: Card::Helper(h),
        glyph: Some(glyph),
        label: h.label().to_owned(),
        note: h.engine().map(|e| e.purpose().to_owned()),
        alert: false,
        body: Body::Expander(spec),
    }
}

fn engine_index(e: Engine) -> usize {
    Engine::ALL.iter().position(|x| *x == e).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn page_of(s: &Settings, facts: &Facts) -> PageSpec {
        page(s, facts, [false; 5], false)
    }

    fn default_page() -> PageSpec {
        page_of(&Settings::default(), &Facts::default())
    }

    #[test]
    fn sections_and_cards_are_in_the_mocks_order() {
        let p = default_page();
        let sections: Vec<Section> = p.sections.iter().map(|(s, _)| *s).collect();
        assert_eq!(sections, Section::ALL);
        let labels: Vec<&str> = Section::ALL.iter().map(|s| s.label()).collect();
        assert_eq!(
            labels,
            [
                "Appearance",
                "Saving",
                "Quality",
                "Drag-wheel",
                "Helper apps",
                "About"
            ]
        );
        let cards: Vec<Card> = p.cards().map(|c| c.card).collect();
        assert_eq!(
            cards,
            [
                Card::Theme,
                Card::SaveWhere,
                Card::Video,
                Card::VideoSize,
                Card::Photos,
                Card::PhotoSize,
                Card::Sound,
                Card::PageDpi,
                Card::DragWheel,
                Card::Helper(Helper::Ffmpeg),
                // no Office card while Office to PDF is switched
                // off (`kiln_core::office::OFFICE_TO_PDF`).
                Card::Helper(Helper::LibreOffice),
                Card::Helper(Helper::Pandoc),
                Card::Helper(Helper::Calibre),
                Card::Version,
                Card::Diagnostics,
                Card::Donate,
                Card::Licences,
            ]
        );
    }

    #[test]
    fn cards_say_the_mocks_words() {
        let p = default_page();
        let said: Vec<(&str, Option<&str>)> = p
            .cards()
            .map(|c| (c.label.as_str(), c.note.as_deref()))
            .collect();
        assert_eq!(
            &said[..9],
            [
                ("Theme", Some("System follows Windows' app mode.")),
                ("Where new files go", None),
                ("Video", None),
                (
                    "Largest video size",
                    Some("Bigger videos are made smaller to fit.")
                ),
                ("Photos", None),
                ("Longest side", Some("In pixels.")),
                ("Sound", None),
                ("PDF pages as pictures", None),
                (
                    "Pick a format while dragging anywhere in Windows",
                    Some(
                        "Tap Shift while dragging files to open the wheel at the pointer. \
                         Kiln then starts with Windows."
                    )
                ),
            ]
        );
        // Tap, not Hold.
        assert!(!settings::DRAG_WHEEL_NOTE.contains("Hold"));
        let helpers: Vec<(&str, Option<&str>)> = said[9..13].to_vec();
        assert_eq!(
            helpers,
            [
                (
                    "FFmpeg",
                    Some("Other video formats, GIFs, and making videos smaller.")
                ),
                // No Microsoft Office card while Office to PDF is off.
                ("LibreOffice", Some("Office files and documents to PDF.")),
                ("Pandoc", Some("Documents to EPUB, HTML and Markdown.")),
                ("Calibre", Some("E-books: EPUB, MOBI and AZW3.")),
            ]
        );
        assert_eq!(said[13], (format!("Kiln {CURRENT}").as_str(), None));
        assert_eq!(
            said[14],
            (
                "Diagnostics",
                Some("Kiln's version, Windows' version and the end of its log, for a bug report.")
            )
        );
        assert_eq!(
            said[15],
            (
                "Support Kiln",
                Some("Donations pay for code signing and upkeep.")
            )
        );
        assert_eq!(
            said[16],
            (
                "Licences and third-party notices",
                Some("Kiln is free software under the GPL 3.0 or later, with no warranty.")
            )
        );
    }

    #[test]
    fn the_theme_list_has_each_themes_base_text_and_accent_as_dots() {
        let swatches = |facts: &Facts| match &page_of(&Settings::default(), facts)
            .card(Card::Theme)
            .unwrap()
            .body
        {
            Body::Combo { swatches, .. } => swatches.clone(),
            b => panic!("{b:?}"),
        };
        let dots = swatches(&Facts::default());
        assert_eq!(dots.len(), THEMES.len());
        for (t, d) in THEMES.iter().zip(&dots) {
            if *t == ThemeChoice::System {
                continue;
            }
            let p = crate::theme::fluent::palette(None, *t, false);
            assert_eq!(*d, [p.base, p.text, p.accent], "{t:?}");
        }
        // System shows what it follows now, light or dark.
        let light = swatches(&Facts {
            system_light: true,
            ..Facts::default()
        });
        assert_eq!(light[0][0], crate::theme::fluent::LIGHT.base);
        assert_eq!(dots[0][0], crate::theme::fluent::DARK.base);
        // Only the Theme list has dots.
        for c in default_page().cards() {
            if let Body::Combo {
                control, swatches, ..
            } = &c.body
            {
                assert_eq!(
                    swatches.is_empty(),
                    *control != Control::Theme,
                    "{control:?}"
                );
            }
        }
    }

    #[test]
    fn a_contrast_theme_changes_the_theme_note() {
        let note = |contrast: bool| {
            page_of(
                &Settings::default(),
                &Facts {
                    contrast,
                    ..Facts::default()
                },
            )
            .card(Card::Theme)
            .unwrap()
            .note
            .clone()
        };
        assert_eq!(
            note(false).as_deref(),
            Some("System follows Windows' app mode.")
        );
        assert_eq!(
            note(true).as_deref(),
            Some("A Windows contrast theme is on, so Kiln is using its colours.")
        );
    }

    #[test]
    fn each_card_has_the_mocks_control() {
        let p = default_page();
        let combo = |c: Card| match &p.card(c).unwrap().body {
            Body::Combo { choices, value, .. } => (choices.clone(), *value),
            b => panic!("{c:?} is {b:?}"),
        };
        assert_eq!(
            combo(Card::Theme),
            (
                [
                    "System",
                    "Light",
                    "Dark",
                    "Dracula",
                    "Gruvbox",
                    "Monokai",
                    "Nord",
                    "Solarized Dark",
                    "Solarized Light"
                ]
                .map(String::from)
                .to_vec(),
                0
            )
        );
        assert_eq!(
            combo(Card::SaveWhere),
            (
                vec![
                    "Next to the original".into(),
                    "A folder I choose\u{2026}".into()
                ],
                0
            )
        );
        assert_eq!(
            combo(Card::VideoSize),
            (
                ["Original", "2160p", "1080p", "720p", "480p"]
                    .map(String::from)
                    .to_vec(),
                0
            )
        );
        assert_eq!(
            combo(Card::PhotoSize),
            (
                ["Original", "3840", "2560", "1920", "1280"]
                    .map(String::from)
                    .to_vec(),
                0
            )
        );
        assert_eq!(
            combo(Card::Sound),
            (
                ["Automatic", "128 kbps", "192 kbps", "256 kbps", "320 kbps"]
                    .map(String::from)
                    .to_vec(),
                0
            )
        );
        assert_eq!(
            combo(Card::PageDpi),
            (
                ["96 dpi", "150 dpi", "300 dpi"].map(String::from).to_vec(),
                1
            )
        );
        for c in [Card::Video, Card::Photos] {
            assert!(
                matches!(
                    p.card(c).unwrap().body,
                    Body::Radios {
                        value: Some(Preset::High),
                        ..
                    }
                ),
                "{c:?}"
            );
        }

        assert_eq!(
            p.card(Card::DragWheel).unwrap().body,
            Body::Toggle {
                control: Control::DragWheel,
                on: false
            }
        );
        assert_eq!(
            p.card(Card::Version).unwrap().body,
            Body::Buttons(vec![(Control::CheckUpdates, "Check for updates".into())])
        );
        assert_eq!(
            p.card(Card::Diagnostics).unwrap().body,
            Body::Buttons(vec![
                (Control::CopyDiagnostics, "Copy diagnostics".into()),
                (Control::OpenLogFolder, "Open log folder".into())
            ])
        );
        assert_eq!(
            p.card(Card::Donate).unwrap().body,
            Body::Buttons(vec![(Control::Donate, "Donate".into())])
        );
        assert_eq!(
            p.card(Card::Licences).unwrap().body,
            Body::Buttons(vec![(Control::Licences, "Open".into())])
        );
    }

    #[test]
    fn install_with_winget_is_beside_get_only_for_a_missing_helper_and_only_with_winget() {
        use EngineStatus::*;
        let mut status: [EngineStatus; 4] = std::array::from_fn(|_| Missing);
        status[1] = OnPath(PathBuf::from(r"C:\Tools\pandoc.exe"));
        let facts = |winget| Facts {
            engines: Some(status.clone()),
            winget,
            ..Facts::default()
        };
        let controls = |p: &PageSpec, h: Helper| -> Vec<Control> {
            expander(p, h).buttons.iter().map(|(c, _)| *c).collect()
        };
        let with = page_of(&Settings::default(), &facts(true));
        // LibreOffice and Calibre are missing: Get, Install, Choose.
        for (h, e) in [
            (Helper::LibreOffice, Engine::LibreOffice),
            (Helper::Calibre, Engine::Calibre),
        ] {
            assert_eq!(
                controls(&with, h),
                [Control::Get(e), Control::Winget(e), Control::Choose(e)]
            );
            let label = &expander(&with, h).buttons[1].1;
            assert_eq!(label, "Install with winget");
        }
        // Pandoc is found: no Install. FFmpeg never has one.
        assert_eq!(
            controls(&with, Helper::Pandoc),
            [Control::Choose(Engine::Pandoc)]
        );
        let mut all_missing = status.clone();
        all_missing[1] = Missing;
        let missing = Facts {
            engines: Some(all_missing),
            winget: true,
            ..Facts::default()
        };
        let p = page_of(&Settings::default(), &missing);
        assert_eq!(
            controls(&p, Helper::Pandoc),
            [
                Control::Get(Engine::Pandoc),
                Control::Winget(Engine::Pandoc),
                Control::Choose(Engine::Pandoc)
            ]
        );
        assert_eq!(
            controls(&p, Helper::Ffmpeg),
            [Control::Choose(Engine::Ffmpeg)]
        );
        for c in p.cards() {
            if let Body::Expander(e) = &c.body {
                if matches!(e.helper, Helper::Ffmpeg | Helper::Office) {
                    assert!(e
                        .buttons
                        .iter()
                        .all(|(c, _)| !matches!(c, Control::Winget(_))));
                }
            }
        }
        // Office's card, were it shown, never has one.
        let office = Facts {
            winget: true,
            ..Facts::default()
        };
        assert!(office_expander(&office)
            .buttons
            .iter()
            .all(|(c, _)| !matches!(c, Control::Winget(_))));
        // Without winget, none has one.
        let without = page_of(&Settings::default(), &facts(false));
        assert!(without.cards().all(|c| match &c.body {
            Body::Expander(e) => e
                .buttons
                .iter()
                .all(|(c, _)| !matches!(c, Control::Winget(_))),
            _ => true,
        }));
        // Before Kiln has looked, none is offered either.
        let unknown = Facts {
            winget: true,
            ..Facts::default()
        };
        let p = page_of(&Settings::default(), &unknown);
        assert_eq!(
            controls(&p, Helper::Calibre),
            [Control::Choose(Engine::Calibre)]
        );
    }

    #[test]
    fn install_with_winget_asks_first_and_says_what_runs() {
        let p = winget_prompt(Engine::Pandoc).unwrap();
        assert_eq!(p.dialog_title(), "Install Pandoc with winget?");
        assert_eq!(
            p.dialog_text(),
            Some(
                "Kiln opens a console window that runs winget install -e --id JohnMacFarlane.Pandoc. Windows may ask for permission."
            )
        );
        assert_eq!(p.buttons, [PromptButton::Install, PromptButton::Cancel]);
        assert_eq!(p.label(PromptButton::Install), "Install");
        assert_eq!(p.label(PromptButton::Cancel), "Cancel");
        // Cancel is the one the keyboard starts on.
        assert_eq!(p.accent(), Some(1));
        assert!(winget_prompt(Engine::LibreOffice)
            .unwrap()
            .title
            .contains("TheDocumentFoundation.LibreOffice"));
        assert!(winget_prompt(Engine::Calibre)
            .unwrap()
            .dialog_text()
            .unwrap()
            .contains("--id calibre.calibre."));
        assert_eq!(winget_prompt(Engine::Ffmpeg), None);
    }

    #[test]
    fn support_kiln_goes_to_the_donate_page() {
        assert_eq!(
            DONATE_PAGE,
            "https://github.com/chancrisp/kiln/blob/main/DONATE.md"
        );
    }

    #[test]
    fn the_diagnostics_card_can_say_the_log_folder_didnt_open() {
        let facts = Facts {
            alert: Some((Card::Diagnostics, LOG_FOLDER_DIDNT_OPEN.into())),
            ..Facts::default()
        };
        let c = page_of(&Settings::default(), &facts)
            .card(Card::Diagnostics)
            .cloned()
            .unwrap();
        assert_eq!(c.note.as_deref(), Some("Couldn't open the log folder."));
        assert!(c.alert);
    }

    #[test]
    fn a_value_off_the_list_is_shown_in_its_place() {
        let at = |s, v| choices(s, v).iter().position(|x| *x == v).unwrap();
        assert_eq!(
            at(DefaultSetting::Resolution, Value::Limit(Limit::Max(1440))),
            2
        );
        assert_eq!(
            at(DefaultSetting::Resolution, Value::Limit(Limit::Max(360))),
            5
        );
        assert_eq!(
            at(DefaultSetting::ImageSize, Value::Limit(Limit::Max(4096))),
            1
        );
        assert_eq!(at(DefaultSetting::Audio, Value::Kbps(Some(64))), 1);
        assert_eq!(at(DefaultSetting::Audio, Value::Kbps(Some(640))), 5);
        assert_eq!(at(DefaultSetting::PageDpi, Value::Dpi(72)), 0);
        assert_eq!(at(DefaultSetting::PageDpi, Value::Dpi(600)), 3);
        // A value on the list is there once.
        assert_eq!(choices(DefaultSetting::PageDpi, Value::Dpi(150)).len(), 3);
        let mut s = Settings::default();
        s.quality.image_size = Limit::Max(4096);
        let p = page_of(&s, &Facts::default());
        let Body::Combo { choices, value, .. } = &p.card(Card::PhotoSize).unwrap().body else {
            panic!()
        };
        assert_eq!(choices[*value], "4096");
    }

    #[test]
    fn a_folder_chosen_for_new_files_is_named_under_the_card() {
        let s = Settings {
            save_to: Some(PathBuf::from(r"C:\Converted")),
            ..Settings::default()
        };
        let p = page_of(&s, &Facts::default());
        let c = p.card(Card::SaveWhere).unwrap();
        assert_eq!(c.note.as_deref(), Some(r"C:\Converted"));
        assert!(matches!(c.body, Body::Combo { value: 1, .. }));

        assert!(!c.alert);
        // Gone since: the card says so, in the critical ink.
        let gone = Facts {
            save_to_missing: true,
            ..Facts::default()
        };
        let c = page_of(&s, &gone).card(Card::SaveWhere).cloned().unwrap();
        assert_eq!(c.note.as_deref(), Some(r"Not found at C:\Converted"));
        assert!(c.alert);
        // Next to the original: nothing to find.
        let c = page_of(&Settings::default(), &gone)
            .card(Card::SaveWhere)
            .cloned()
            .unwrap();
        assert_eq!(c.note, None);
        assert!(!c.alert);
    }

    #[test]
    fn the_drag_wheel_card_says_when_windows_refused_it() {
        let s = Settings {
            drag_wheel: true,
            ..Settings::default()
        };
        let c = |refused| {
            page(&s, &Facts::default(), [false; 5], refused)
                .card(Card::DragWheel)
                .cloned()
                .unwrap()
        };
        assert_eq!(c(false).note.as_deref(), Some(settings::DRAG_WHEEL_NOTE));
        assert!(!c(false).alert);
        assert_eq!(c(true).note.as_deref(), Some(settings::DRAG_WHEEL_REFUSED));
        assert!(c(true).alert);
    }

    #[test]
    fn every_card_has_a_glyph_of_its_own() {
        // the diagonal arrows were both sizes' and the page glyph was
        // PDF pages', LibreOffice's, Pandoc's and the licences'.
        let all: Vec<char> = glyph::ALL.iter().map(|(_, c)| *c).collect();
        let mut seen = std::collections::HashSet::new();
        for (name, c) in glyph::ALL {
            assert!(seen.insert(c), "{name} repeats {:04X}", c as u32);
            assert!(('\u{E700}'..='\u{F8FF}').contains(&c), "{name}");
        }
        // The page, with the Office card shown as well.
        let p = default_page();
        let on_page: Vec<char> = p.cards().filter_map(|c| c.glyph).collect();
        let mut distinct = on_page.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(distinct.len(), on_page.len(), "{on_page:?}");
        assert!(on_page.iter().all(|c| all.contains(c)));
        let g = |card| p.card(card).and_then(|c| c.glyph);
        assert_ne!(g(Card::VideoSize), g(Card::PhotoSize));
        assert_ne!(g(Card::PageDpi), g(Card::Licences));
        assert_ne!(
            g(Card::Helper(Helper::LibreOffice)),
            g(Card::Helper(Helper::Pandoc))
        );
        // The Office card, were it shown, has its own too.
        let office = helper_card(Helper::Office, &Facts::default(), false);
        assert!(office.glyph.is_some_and(|c| !on_page.contains(&c)));
    }

    #[test]
    fn a_helper_path_has_a_location_label_over_it() {
        use EngineStatus::*;
        let mut status: [EngineStatus; 4] = std::array::from_fn(|_| Missing);
        status[0] = Included(PathBuf::from(r"C:\Kiln\ffmpeg\bin\ffmpeg.exe"));
        status[1] = OnPath(PathBuf::from(r"C:\Tools\pandoc.exe"));
        status[2] = Chosen {
            path: PathBuf::from(r"D:\office\soffice.exe"),
            exists: true,
        };
        status[3] = Chosen {
            path: PathBuf::from(r"D:\calibre\ebook-convert.exe"),
            exists: false,
        };
        let facts = Facts {
            engines: Some(status),
            ..Facts::default()
        };
        let p = page_of(&Settings::default(), &facts);
        // A path says what it is; a sentence doesn't need telling.
        assert_eq!(expander(&p, Helper::Pandoc).line_label, Some("Location"));
        assert_eq!(
            expander(&p, Helper::LibreOffice).line_label,
            Some("Location")
        );
        assert_eq!(expander(&p, Helper::Ffmpeg).line_label, None);
        assert_eq!(expander(&p, Helper::Calibre).line_label, None);
        let missing = Facts {
            engines: Some(std::array::from_fn(|_| Missing)),
            ..Facts::default()
        };
        let p = page_of(&Settings::default(), &missing);
        assert_eq!(expander(&p, Helper::Pandoc).line_label, None);
    }

    #[test]
    fn the_drag_wheel_note_is_one_short_sentence_pair() {
        // it ran to two lines at 1100 DIPs.
        assert_eq!(
            settings::DRAG_WHEEL_NOTE,
            "Tap Shift while dragging files to open the wheel at the pointer. Kiln then starts with Windows."
        );
    }

    fn expander(p: &PageSpec, h: Helper) -> ExpanderSpec {
        match &p.card(Card::Helper(h)).unwrap().body {
            Body::Expander(e) => e.clone(),
            b => panic!("{b:?}"),
        }
    }

    #[test]
    fn ffmpeg_shows_included_with_use_another_copy() {
        use EngineStatus::*;
        let mut status: [EngineStatus; 4] = std::array::from_fn(|_| Missing);
        status[0] = Included(PathBuf::from(r"C:\Kiln\ffmpeg\bin\ffmpeg.exe"));
        let facts = Facts {
            engines: Some(status),
            ..Facts::default()
        };
        let e = expander(&page_of(&Settings::default(), &facts), Helper::Ffmpeg);
        assert_eq!(
            e.badge,
            Some(Badge {
                text: "Included",
                found: true
            })
        );
        assert_eq!(e.line, "Comes with Kiln, as an LGPL build.");
        assert_eq!(
            e.line_note.as_deref(),
            Some("Its licence and where to get its source are in About.")
        );
        assert_eq!(
            e.buttons,
            [(
                Control::Choose(Engine::Ffmpeg),
                "Use another copy\u{2026}".into()
            )]
        );
        // No Get it for FFmpeg, ever: Kiln never sends anyone to
        // ffmpeg.org.
        let missing = Facts {
            engines: Some(std::array::from_fn(|_| Missing)),
            ..Facts::default()
        };
        let e = expander(&page_of(&Settings::default(), &missing), Helper::Ffmpeg);
        assert!(e.buttons.iter().all(|(c, _)| !matches!(c, Control::Get(_))));
    }

    #[test]
    fn a_missing_helper_offers_get_it_and_choose_and_a_found_one_its_place() {
        use EngineStatus::*;
        let mut status: [EngineStatus; 4] = std::array::from_fn(|_| Missing);
        status[1] = OnPath(PathBuf::from(r"C:\Tools\pandoc.exe"));
        status[3] = Chosen {
            path: PathBuf::from(r"D:\calibre\ebook-convert.exe"),
            exists: false,
        };
        let facts = Facts {
            engines: Some(status),
            ..Facts::default()
        };
        let p = page_of(&Settings::default(), &facts);
        let lo = expander(&p, Helper::LibreOffice);
        assert_eq!(lo.badge.unwrap().text, "Not found");
        assert!(!lo.badge.unwrap().found);
        assert_eq!(lo.line, "Kiln finds it again as soon as it's installed.");
        assert_eq!(
            lo.buttons,
            [
                (Control::Get(Engine::LibreOffice), "Get LibreOffice".into()),
                (
                    Control::Choose(Engine::LibreOffice),
                    "Choose\u{2026}".into()
                )
            ]
        );
        let pandoc = expander(&p, Helper::Pandoc);
        assert_eq!(pandoc.badge.unwrap().text, "Found");
        assert_eq!(pandoc.line, r"C:\Tools\pandoc.exe");
        assert_eq!(
            pandoc.buttons,
            [(Control::Choose(Engine::Pandoc), "Choose\u{2026}".into())]
        );
        let calibre = expander(&p, Helper::Calibre);
        assert_eq!(calibre.badge.unwrap().text, "Not found");
        assert_eq!(calibre.line, r"Not found at D:\calibre\ebook-convert.exe");
        assert_eq!(
            calibre.buttons.iter().map(|(c, _)| *c).collect::<Vec<_>>(),
            [
                Control::Choose(Engine::Calibre),
                Control::Reset(Engine::Calibre)
            ]
        );
        // Before Kiln has looked, no status is claimed.
        let p = default_page();
        assert_eq!(expander(&p, Helper::Pandoc).badge, None);
        assert_eq!(
            office_expander(&Facts::default()).badge,
            None,
            "the Office card, were it shown"
        );
    }

    /// The Office card as it would be drawn, though the page leaves it
    /// out while Office to PDF is off.
    fn office_expander(facts: &Facts) -> ExpanderSpec {
        match helper_card(Helper::Office, facts, false).body {
            Body::Expander(e) => e,
            b => panic!("{b:?}"),
        }
    }

    #[test]
    fn no_office_card_while_office_to_pdf_is_off() {
        // Kiln doesn't use Office while the switch is off, so the
        // page says nothing of it, found or not.
        let found = Facts {
            office: Some(OfficeStatus {
                badge: "Found",
                apps: Some("Word".into()),
                folder: None,
            }),
            ..Facts::default()
        };
        for facts in [Facts::default(), found] {
            let p = page_of(&Settings::default(), &facts);
            assert_eq!(p.card(Card::Helper(Helper::Office)), None);
            assert!(!p.cards().any(|c| c.label.contains("Microsoft Office")));
        }
    }

    #[test]
    fn office_reports_only_what_was_found() {
        let found = OfficeStatus {
            badge: "Found",
            apps: Some("Word, Excel and PowerPoint".into()),
            folder: Some(PathBuf::from(
                r"C:\Program Files\Microsoft Office\root\Office16",
            )),
        };
        let facts = Facts {
            office: Some(found),
            ..Facts::default()
        };
        let e = office_expander(&facts);
        assert_eq!(e.badge.unwrap().text, "Found");
        assert_eq!(e.line, "Word, Excel and PowerPoint");
        assert_eq!(
            e.line_note.as_deref(),
            Some(r"C:\Program Files\Microsoft Office\root\Office16")
        );
        assert!(e.buttons.is_empty());
        let none = Facts {
            office: Some(OfficeStatus {
                badge: "Not found",
                apps: None,
                folder: None,
            }),
            ..Facts::default()
        };
        let e = office_expander(&none);
        assert_eq!(e.badge.unwrap().text, "Not found");
        assert_eq!(e.line, FINDS_IT_AGAIN);
        assert_eq!(e.line_note, None);
        // Nothing on the card promises a conversion.
        let c = helper_card(Helper::Office, &facts, false);
        assert!(!format!("{c:?}").contains("PDF"));
    }

    #[test]
    fn the_update_check_speaks_in_the_version_card() {
        let facts = Facts {
            update: UpdateState::Failed("Couldn't reach GitHub.".into()),
            ..Facts::default()
        };
        let c = page_of(&Settings::default(), &facts)
            .card(Card::Version)
            .cloned()
            .unwrap();
        assert_eq!(c.note.as_deref(), Some("Couldn't reach GitHub."));
        assert!(c.alert);
        let facts = Facts {
            update: UpdateState::Available("9.0.0".into()),
            ..Facts::default()
        };
        let c = page_of(&Settings::default(), &facts)
            .card(Card::Version)
            .cloned()
            .unwrap();
        assert_eq!(c.note.as_deref(), Some("Kiln 9.0.0 is available."));
        assert!(!c.alert);
        assert_eq!(
            c.body,
            Body::Buttons(vec![(Control::CheckUpdates, "Get the update".into())])
        );
    }

    #[test]
    fn something_that_went_wrong_takes_its_cards_note() {
        let facts = Facts {
            alert: Some((Card::Licences, LICENCES_DIDNT_OPEN.into())),
            ..Facts::default()
        };
        let c = page_of(&Settings::default(), &facts)
            .card(Card::Licences)
            .cloned()
            .unwrap();
        assert_eq!(c.note.as_deref(), Some("Couldn't open the notices."));
        assert!(c.alert);
    }

    #[test]
    fn an_open_expander_says_so() {
        let mut open = [false; 5];
        open[Helper::Pandoc.index()] = true;
        let p = page(&Settings::default(), &Facts::default(), open, false);
        assert!(expander(&p, Helper::Pandoc).open);
        assert!(!expander(&p, Helper::Calibre).open);
    }
}
