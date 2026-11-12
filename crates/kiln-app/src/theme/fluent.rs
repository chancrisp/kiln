//! Kiln's Fluent tokens: colour roles, type, spacing, radii and motion, as
//! DESIGN.md describes them. Every surface draws with them. `tests/snapshots/fluent_tokens.snap` pins every value.

use super::{contrast, Rgb, SysColors, NON_TEXT, STROKE, TEXT};
use crate::settings::ThemeChoice;

/// One measured pair: what it is, foreground, background, the ratio and
/// what it needs (`None`: nothing).
pub type FluentPair = (String, Rgb, Rgb, f32, Option<f32>);

const WHITE: Rgb = Rgb::hex(0xFFFFFF);
const BLACK: Rgb = Rgb::hex(0x000000);

/// A colour and how much of it covers what lies under it: 1 is solid,
/// 0 draws nothing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tint {
    pub rgb: Rgb,
    pub alpha: f32,
}

impl Tint {
    pub const fn solid(rgb: Rgb) -> Self {
        Tint { rgb, alpha: 1.0 }
    }

    pub const fn alpha(rgb: Rgb, alpha: f32) -> Self {
        Tint { rgb, alpha }
    }

    /// Nothing at all: what a contrast theme draws for smoke.
    pub const NONE: Tint = Tint {
        rgb: BLACK,
        alpha: 0.0,
    };

    /// This tint composited over a solid `bg`, as Direct2D draws it.
    pub fn over(self, bg: Rgb) -> Rgb {
        super::over(self.rgb, self.alpha, bg)
    }
}

/// A surface with two forms: solid where the window has no Mica (older
/// Windows, transparency effects off) and a tint over the Mica backdrop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Surface {
    pub solid: Rgb,
    pub mica: Tint,
}

/// Which edge of a control carries its stronger stroke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
}

/// One drop shadow, as CSS writes it: `0 y blur colour`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shadow {
    pub y: f32,
    pub blur: f32,
    pub rgb: Rgb,
    pub alpha: f32,
}

/// The kinds a file's small cue is coloured by. Spreadsheets,
/// presentations, subtitles and e-books count as documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileKind {
    Image,
    Video,
    Audio,
    Document,
    Other,
}

impl FileKind {
    /// In the order `FluentPalette::kinds` is indexed.
    pub const ALL: [FileKind; 5] = [
        FileKind::Image,
        FileKind::Video,
        FileKind::Audio,
        FileKind::Document,
        FileKind::Other,
    ];

    pub fn of(kind: Option<kiln_core::Kind>) -> FileKind {
        use kiln_core::Kind;
        match kind {
            Some(Kind::Image) => FileKind::Image,
            Some(Kind::Video) => FileKind::Video,
            Some(Kind::Audio) => FileKind::Audio,
            Some(Kind::Document | Kind::Spreadsheet | Kind::Presentation | Kind::Subtitle) => {
                FileKind::Document
            }
            None => FileKind::Other,
        }
    }

    fn name(self) -> &'static str {
        match self {
            FileKind::Image => "image",
            FileKind::Video => "video",
            FileKind::Audio => "audio",
            FileKind::Document => "document",
            FileKind::Other => "other",
        }
    }
}

/// Every colour the Fluent surfaces draw with, for one theme.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FluentPalette {
    pub name: &'static str,
    pub high_contrast: bool,
    /// Under Mica the base is the backdrop itself; this is its solid form.
    pub base: Rgb,
    /// The content layer under the title bar.
    pub layer: Surface,
    /// The side pane, settings cards and the row hover card.
    pub card: Surface,
    pub card_stroke: Tint,
    pub control: Tint,
    pub control_hover: Tint,
    pub control_pressed: Tint,
    pub control_stroke: Tint,
    /// A control's stroke under the pointer: `control_stroke`, or
    /// Highlight under a contrast theme, where fills can't change shade.
    pub control_hover_stroke: Tint,
    /// The stronger stroke on one edge of a control, and which edge.
    pub control_stroke_edge: (Edge, Tint),
    /// A text box's stroke on the sides its stronger bottom edge isn't on:
    /// strong enough that an empty box reads as a field, not an underline
    pub field_stroke: Tint,
    /// Progress track, a toggle's off rim, radio rings (mock `--track`).
    pub strong_stroke: Tint,
    pub divider: Tint,
    /// Dialogs, the wheel, flyouts.
    pub overlay: Rgb,
    pub overlay_stroke: Tint,
    pub text: Rgb,
    pub text_secondary: Rgb,
    pub text_disabled: Rgb,
    /// Text on a control: `text`, or ButtonText under a contrast theme.
    pub control_text: Rgb,
    /// Primary button, lit wheel tile, toggle on, progress.
    pub accent: Rgb,
    pub accent_hover: Rgb,
    /// The accent button's stronger edge along its bottom: as visible as a
    /// standard button's lip, so the buttons beside it sit level.
    pub accent_edge: Tint,
    pub on_accent: Rgb,
    /// Links and the selected pill.
    pub accent_text: Rgb,
    /// A selected row's fill; the 3 × 16 pill is `accent`.
    pub selection: Tint,
    pub on_selection: Rgb,
    pub critical: Rgb,
    pub critical_fill: Rgb,
    pub success: Rgb,
    pub success_fill: Rgb,
    pub caution: Rgb,
    pub caution_fill: Rgb,
    /// An info InfoBar's icon and fill.
    pub info: Rgb,
    pub info_fill: Tint,
    /// Behind dialogs.
    pub smoke: Tint,
    /// The two-tone focus ring: 2-DIP outer, 1-DIP inner.
    pub focus_outer: Rgb,
    pub focus_inner: Rgb,
    /// The title bar's Close button under the pointer, and its glyph.
    pub close_hover: Rgb,
    pub on_close_hover: Rgb,
    /// Small cues only, in `FileKind::ALL` order: never a large fill.
    pub kinds: [Rgb; 5],
    /// Flyouts and the wheel.
    pub flyout_shadow: &'static [Shadow],
    pub dialog_shadow: &'static [Shadow],
}

impl FluentPalette {
    pub fn kind(&self, k: FileKind) -> Rgb {
        self.kinds[k as usize]
    }
}

const fn shadow(y: f32, blur: f32, alpha: f32) -> Shadow {
    Shadow {
        y,
        blur,
        rgb: BLACK,
        alpha,
    }
}

const CLOSE_HOVER: Rgb = Rgb::hex(0xC42B1C);
const ON_CLOSE_HOVER: Rgb = Rgb::hex(0xFAFAFA);

pub const DARK: FluentPalette = FluentPalette {
    name: "dark",
    high_contrast: false,
    base: Rgb::hex(0x202020),
    layer: Surface {
        solid: Rgb::hex(0x282828),
        mica: Tint::alpha(WHITE, 0.04),
    },
    card: Surface {
        solid: Rgb::hex(0x2B2B2B),
        mica: Tint::alpha(WHITE, 0.05),
    },
    card_stroke: Tint::alpha(WHITE, 0.08),
    control: Tint::alpha(WHITE, 0.06),
    control_hover: Tint::alpha(WHITE, 0.08),
    control_pressed: Tint::alpha(WHITE, 0.03),
    control_stroke: Tint::alpha(WHITE, 0.07),
    control_hover_stroke: Tint::alpha(WHITE, 0.07),
    control_stroke_edge: (Edge::Top, Tint::alpha(WHITE, 0.09)),
    field_stroke: Tint::alpha(WHITE, 0.07),
    strong_stroke: Tint::alpha(WHITE, 0.54),
    divider: Tint::alpha(WHITE, 0.08),
    overlay: Rgb::hex(0x2C2C2C),
    overlay_stroke: Tint::alpha(WHITE, 0.08),
    text: Rgb::hex(0xF5F5F5),
    text_secondary: Rgb::hex(0xC5C5C5),
    text_disabled: Rgb::hex(0x7A7A7A),
    control_text: Rgb::hex(0xF5F5F5),
    accent: Rgb::hex(0x6CCBA8),
    accent_hover: Rgb::hex(0x63BC9B),
    accent_edge: Tint::alpha(BLACK, 0.14),
    on_accent: Rgb::hex(0x0B1F18),
    accent_text: Rgb::hex(0x7ED8B6),
    selection: Tint::alpha(Rgb::hex(0x6CCBA8), 0.10),
    on_selection: Rgb::hex(0xF5F5F5),
    critical: Rgb::hex(0xFF99A4),
    critical_fill: Rgb::hex(0x442726),
    success: Rgb::hex(0x6CCB5F),
    success_fill: Rgb::hex(0x393D1B),
    caution: Rgb::hex(0xFCE100),
    caution_fill: Rgb::hex(0x433519),
    info: Rgb::hex(0x7ED8B6),
    info_fill: Tint::alpha(WHITE, 0.05),
    smoke: Tint::alpha(BLACK, 0.30),
    focus_outer: Rgb::hex(0xF5F5F5),
    focus_inner: Rgb::hex(0x1B1B1B),
    close_hover: CLOSE_HOVER,
    on_close_hover: ON_CLOSE_HOVER,
    kinds: [
        Rgb::hex(0x4FA3F7),
        Rgb::hex(0xA884F3),
        Rgb::hex(0xE67AA6),
        Rgb::hex(0xC9B458),
        Rgb::hex(0x9A9A9A),
    ],
    flyout_shadow: &[shadow(8.0, 16.0, 0.26)],
    dialog_shadow: &[shadow(32.0, 64.0, 0.37), shadow(2.0, 21.0, 0.37)],
};

pub const LIGHT: FluentPalette = FluentPalette {
    name: "light",
    high_contrast: false,
    base: Rgb::hex(0xF3F3F3),
    layer: Surface {
        solid: Rgb::hex(0xF9F9F9),
        mica: Tint::alpha(WHITE, 0.50),
    },
    card: Surface {
        solid: Rgb::hex(0xFBFBFB),
        mica: Tint::alpha(WHITE, 0.70),
    },
    card_stroke: Tint::alpha(BLACK, 0.07),
    control: Tint::solid(Rgb::hex(0xFBFBFB)),
    control_hover: Tint::solid(Rgb::hex(0xF6F6F6)),
    control_pressed: Tint::solid(Rgb::hex(0xF5F5F5)),
    control_stroke: Tint::alpha(BLACK, 0.06),
    control_hover_stroke: Tint::alpha(BLACK, 0.06),
    control_stroke_edge: (Edge::Bottom, Tint::alpha(BLACK, 0.16)),
    field_stroke: Tint::alpha(BLACK, 0.12),
    strong_stroke: Tint::alpha(BLACK, 0.45),
    divider: Tint::alpha(BLACK, 0.08),
    overlay: Rgb::hex(0xF9F9F9),
    overlay_stroke: Tint::alpha(BLACK, 0.06),
    text: Rgb::hex(0x1B1B1B),
    text_secondary: Rgb::hex(0x5D5D5D),
    text_disabled: Rgb::hex(0x9E9E9E),
    control_text: Rgb::hex(0x1B1B1B),
    accent: Rgb::hex(0x1F7A5C),
    // The mock's #2A8566 leaves on_accent at 4.33:1; half its step
    // lighter keeps the direction and 4.70:1.
    accent_hover: Rgb::hex(0x247F60),
    accent_edge: Tint::alpha(BLACK, 0.24),
    on_accent: Rgb::hex(0xFAFAFA),
    accent_text: Rgb::hex(0x17664C),
    selection: Tint::alpha(Rgb::hex(0x1F7A5C), 0.10),
    on_selection: Rgb::hex(0x1B1B1B),
    critical: Rgb::hex(0xC42B1C),
    critical_fill: Rgb::hex(0xFDE7E9),
    success: Rgb::hex(0x0F7B0F),
    success_fill: Rgb::hex(0xDFF6DD),
    caution: Rgb::hex(0x9D5D00),
    caution_fill: Rgb::hex(0xFFF4CE),
    info: Rgb::hex(0x17664C),
    info_fill: Tint::alpha(Rgb::hex(0xF6F6F6), 0.50),
    smoke: Tint::alpha(BLACK, 0.30),
    focus_outer: Rgb::hex(0x1B1B1B),
    focus_inner: Rgb::hex(0xF5F5F5),
    close_hover: CLOSE_HOVER,
    on_close_hover: ON_CLOSE_HOVER,
    kinds: [
        Rgb::hex(0x0F6CBD),
        Rgb::hex(0x6B47C9),
        Rgb::hex(0xB4366E),
        Rgb::hex(0x7A6A1C),
        Rgb::hex(0x616161),
    ],
    flyout_shadow: &[shadow(8.0, 16.0, 0.14)],
    dialog_shadow: &[shadow(32.0, 64.0, 0.28), shadow(2.0, 21.0, 0.28)],
};

/// Dracula: its purple-grey bases and its green as the accent.
pub const DRACULA: FluentPalette = FluentPalette {
    name: "dracula",
    high_contrast: false,
    base: Rgb::hex(0x21222C),
    layer: Surface {
        solid: Rgb::hex(0x282A36),
        mica: Tint::solid(Rgb::hex(0x282A36)),
    },
    card: Surface {
        solid: Rgb::hex(0x2F3140),
        mica: Tint::solid(Rgb::hex(0x2F3140)),
    },
    card_stroke: Tint::alpha(Rgb::hex(0xF8F8F2), 0.08),
    control: Tint::alpha(Rgb::hex(0xF8F8F2), 0.06),
    control_hover: Tint::alpha(Rgb::hex(0xF8F8F2), 0.08),
    control_pressed: Tint::alpha(Rgb::hex(0xF8F8F2), 0.03),
    control_stroke: Tint::alpha(Rgb::hex(0xF8F8F2), 0.07),
    control_hover_stroke: Tint::alpha(Rgb::hex(0xF8F8F2), 0.07),
    control_stroke_edge: (Edge::Top, Tint::alpha(Rgb::hex(0xF8F8F2), 0.09)),
    field_stroke: Tint::alpha(Rgb::hex(0xF8F8F2), 0.07),
    strong_stroke: Tint::alpha(Rgb::hex(0xF8F8F2), 0.54),
    divider: Tint::alpha(Rgb::hex(0xF8F8F2), 0.08),
    overlay: Rgb::hex(0x343746),
    overlay_stroke: Tint::alpha(Rgb::hex(0xF8F8F2), 0.08),
    text: Rgb::hex(0xF8F8F2),
    text_secondary: Rgb::hex(0xA4ADCA),
    text_disabled: Rgb::hex(0x6272A4),
    control_text: Rgb::hex(0xF8F8F2),
    accent: Rgb::hex(0x50FA7B),
    accent_hover: Rgb::hex(0x4AE872),
    accent_edge: Tint::alpha(BLACK, 0.14),
    on_accent: Rgb::hex(0x21222C),
    accent_text: Rgb::hex(0x69FF94),
    selection: Tint::alpha(Rgb::hex(0x50FA7B), 0.10),
    on_selection: Rgb::hex(0xF8F8F2),
    critical: Rgb::hex(0xFF6B6B),
    critical_fill: Rgb::hex(0x3E2E39),
    success: Rgb::hex(0x50FA7B),
    success_fill: Rgb::hex(0x2E4B41),
    caution: Rgb::hex(0xF1FA8C),
    caution_fill: Rgb::hex(0x484B44),
    info: Rgb::hex(0x50FA7B),
    info_fill: Tint::alpha(Rgb::hex(0xF8F8F2), 0.05),
    smoke: Tint::alpha(BLACK, 0.30),
    focus_outer: Rgb::hex(0xF8F8F2),
    focus_inner: Rgb::hex(0x21222C),
    close_hover: CLOSE_HOVER,
    on_close_hover: ON_CLOSE_HOVER,
    kinds: [
        Rgb::hex(0x8BE9FD),
        Rgb::hex(0xBD93F9),
        Rgb::hex(0xFF79C6),
        Rgb::hex(0xF1FA8C),
        Rgb::hex(0x94A0C1),
    ],
    flyout_shadow: &[shadow(8.0, 16.0, 0.26)],
    dialog_shadow: &[shadow(32.0, 64.0, 0.37), shadow(2.0, 21.0, 0.37)],
};

/// Gruvbox, the dark one, with its aqua-green as the accent.
pub const GRUVBOX: FluentPalette = FluentPalette {
    name: "gruvbox",
    high_contrast: false,
    base: Rgb::hex(0x1D2021),
    layer: Surface {
        solid: Rgb::hex(0x282828),
        mica: Tint::solid(Rgb::hex(0x282828)),
    },
    card: Surface {
        solid: Rgb::hex(0x32302F),
        mica: Tint::solid(Rgb::hex(0x32302F)),
    },
    card_stroke: Tint::alpha(Rgb::hex(0xEBDBB2), 0.08),
    control: Tint::alpha(Rgb::hex(0xEBDBB2), 0.06),
    control_hover: Tint::alpha(Rgb::hex(0xEBDBB2), 0.08),
    control_pressed: Tint::alpha(Rgb::hex(0xEBDBB2), 0.03),
    control_stroke: Tint::alpha(Rgb::hex(0xEBDBB2), 0.07),
    control_hover_stroke: Tint::alpha(Rgb::hex(0xEBDBB2), 0.07),
    control_stroke_edge: (Edge::Top, Tint::alpha(Rgb::hex(0xEBDBB2), 0.09)),
    field_stroke: Tint::alpha(Rgb::hex(0xEBDBB2), 0.07),
    strong_stroke: Tint::alpha(Rgb::hex(0xEBDBB2), 0.54),
    divider: Tint::alpha(Rgb::hex(0xEBDBB2), 0.08),
    overlay: Rgb::hex(0x3C3836),
    overlay_stroke: Tint::alpha(Rgb::hex(0xEBDBB2), 0.08),
    text: Rgb::hex(0xEBDBB2),
    text_secondary: Rgb::hex(0xBDAE93),
    text_disabled: Rgb::hex(0x7C6F64),
    control_text: Rgb::hex(0xEBDBB2),
    accent: Rgb::hex(0x8EC07C),
    accent_hover: Rgb::hex(0x84B373),
    accent_edge: Tint::alpha(BLACK, 0.14),
    on_accent: Rgb::hex(0x1D2021),
    accent_text: Rgb::hex(0xA0D08E),
    selection: Tint::alpha(Rgb::hex(0x8EC07C), 0.10),
    on_selection: Rgb::hex(0xEBDBB2),
    critical: Rgb::hex(0xFB6654),
    critical_fill: Rgb::hex(0x3D2B29),
    success: Rgb::hex(0xB8BB26),
    success_fill: Rgb::hex(0x3F4028),
    caution: Rgb::hex(0xFABD2F),
    caution_fill: Rgb::hex(0x4A4029),
    info: Rgb::hex(0x8EC07C),
    info_fill: Tint::alpha(Rgb::hex(0xEBDBB2), 0.05),
    smoke: Tint::alpha(BLACK, 0.30),
    focus_outer: Rgb::hex(0xEBDBB2),
    focus_inner: Rgb::hex(0x1D2021),
    close_hover: CLOSE_HOVER,
    on_close_hover: ON_CLOSE_HOVER,
    kinds: [
        Rgb::hex(0x89A99D),
        Rgb::hex(0xD68FA2),
        Rgb::hex(0xFE8019),
        Rgb::hex(0xFABD2F),
        Rgb::hex(0xABA094),
    ],
    flyout_shadow: &[shadow(8.0, 16.0, 0.26)],
    dialog_shadow: &[shadow(32.0, 64.0, 0.37), shadow(2.0, 21.0, 0.37)],
};

/// Monokai, the classic, with its lime as the accent.
pub const MONOKAI: FluentPalette = FluentPalette {
    name: "monokai",
    high_contrast: false,
    base: Rgb::hex(0x1E1F1C),
    layer: Surface {
        solid: Rgb::hex(0x272822),
        mica: Tint::solid(Rgb::hex(0x272822)),
    },
    card: Surface {
        solid: Rgb::hex(0x2D2E27),
        mica: Tint::solid(Rgb::hex(0x2D2E27)),
    },
    card_stroke: Tint::alpha(Rgb::hex(0xF8F8F2), 0.08),
    control: Tint::alpha(Rgb::hex(0xF8F8F2), 0.06),
    control_hover: Tint::alpha(Rgb::hex(0xF8F8F2), 0.08),
    control_pressed: Tint::alpha(Rgb::hex(0xF8F8F2), 0.03),
    control_stroke: Tint::alpha(Rgb::hex(0xF8F8F2), 0.07),
    control_hover_stroke: Tint::alpha(Rgb::hex(0xF8F8F2), 0.07),
    control_stroke_edge: (Edge::Top, Tint::alpha(Rgb::hex(0xF8F8F2), 0.09)),
    field_stroke: Tint::alpha(Rgb::hex(0xF8F8F2), 0.07),
    strong_stroke: Tint::alpha(Rgb::hex(0xF8F8F2), 0.54),
    divider: Tint::alpha(Rgb::hex(0xF8F8F2), 0.08),
    overlay: Rgb::hex(0x32332B),
    overlay_stroke: Tint::alpha(Rgb::hex(0xF8F8F2), 0.08),
    text: Rgb::hex(0xF8F8F2),
    text_secondary: Rgb::hex(0xAAA997),
    text_disabled: Rgb::hex(0x75715E),
    control_text: Rgb::hex(0xF8F8F2),
    accent: Rgb::hex(0xA6E22E),
    accent_hover: Rgb::hex(0x9AD22B),
    accent_edge: Tint::alpha(BLACK, 0.14),
    on_accent: Rgb::hex(0x1E1F1C),
    accent_text: Rgb::hex(0xB6EC4A),
    selection: Tint::alpha(Rgb::hex(0xA6E22E), 0.10),
    on_selection: Rgb::hex(0xF8F8F2),
    critical: Rgb::hex(0xFA5A93),
    critical_fill: Rgb::hex(0x3C282A),
    success: Rgb::hex(0xA6E22E),
    success_fill: Rgb::hex(0x3B4624),
    caution: Rgb::hex(0xE6DB74),
    caution_fill: Rgb::hex(0x46452F),
    info: Rgb::hex(0xA6E22E),
    info_fill: Tint::alpha(Rgb::hex(0xF8F8F2), 0.05),
    smoke: Tint::alpha(BLACK, 0.30),
    focus_outer: Rgb::hex(0xF8F8F2),
    focus_inner: Rgb::hex(0x1E1F1C),
    close_hover: CLOSE_HOVER,
    on_close_hover: ON_CLOSE_HOVER,
    kinds: [
        Rgb::hex(0x66D9EF),
        Rgb::hex(0xAF83FF),
        Rgb::hex(0xFA669C),
        Rgb::hex(0xE6DB74),
        Rgb::hex(0x9F9A87),
    ],
    flyout_shadow: &[shadow(8.0, 16.0, 0.26)],
    dialog_shadow: &[shadow(32.0, 64.0, 0.37), shadow(2.0, 21.0, 0.37)],
};

/// Nord, with its frost green as the accent.
pub const NORD: FluentPalette = FluentPalette {
    name: "nord",
    high_contrast: false,
    base: Rgb::hex(0x2B303B),
    layer: Surface {
        solid: Rgb::hex(0x2E3440),
        mica: Tint::solid(Rgb::hex(0x2E3440)),
    },
    card: Surface {
        solid: Rgb::hex(0x3B4252),
        mica: Tint::solid(Rgb::hex(0x3B4252)),
    },
    card_stroke: Tint::alpha(Rgb::hex(0xECEFF4), 0.08),
    control: Tint::alpha(Rgb::hex(0xECEFF4), 0.06),
    control_hover: Tint::alpha(Rgb::hex(0xECEFF4), 0.08),
    control_pressed: Tint::alpha(Rgb::hex(0xECEFF4), 0.03),
    control_stroke: Tint::alpha(Rgb::hex(0xECEFF4), 0.07),
    control_hover_stroke: Tint::alpha(Rgb::hex(0xECEFF4), 0.07),
    control_stroke_edge: (Edge::Top, Tint::alpha(Rgb::hex(0xECEFF4), 0.09)),
    field_stroke: Tint::alpha(Rgb::hex(0xECEFF4), 0.07),
    strong_stroke: Tint::alpha(Rgb::hex(0xECEFF4), 0.54),
    divider: Tint::alpha(Rgb::hex(0xECEFF4), 0.08),
    overlay: Rgb::hex(0x3B4252),
    overlay_stroke: Tint::alpha(Rgb::hex(0xECEFF4), 0.08),
    text: Rgb::hex(0xECEFF4),
    text_secondary: Rgb::hex(0xC2C9D6),
    text_disabled: Rgb::hex(0x616E88),
    control_text: Rgb::hex(0xECEFF4),
    accent: Rgb::hex(0x8FBCBB),
    accent_hover: Rgb::hex(0x85AFAE),
    accent_edge: Tint::alpha(BLACK, 0.14),
    on_accent: Rgb::hex(0x2E3440),
    accent_text: Rgb::hex(0x9EC5C4),
    selection: Tint::alpha(Rgb::hex(0x8FBCBB), 0.10),
    on_selection: Rgb::hex(0xECEFF4),
    critical: Rgb::hex(0xD39398),
    critical_fill: Rgb::hex(0x3C3844),
    success: Rgb::hex(0xA3BE8C),
    success_fill: Rgb::hex(0x3E474B),
    caution: Rgb::hex(0xEBCB8B),
    caution_fill: Rgb::hex(0x4C4C4C),
    info: Rgb::hex(0x9EC5C4),
    info_fill: Tint::alpha(Rgb::hex(0xECEFF4), 0.05),
    smoke: Tint::alpha(BLACK, 0.30),
    focus_outer: Rgb::hex(0xECEFF4),
    focus_inner: Rgb::hex(0x2B303B),
    close_hover: CLOSE_HOVER,
    on_close_hover: ON_CLOSE_HOVER,
    kinds: [
        Rgb::hex(0x88C0D0),
        Rgb::hex(0xC4A5BE),
        Rgb::hex(0xD9A1A6),
        Rgb::hex(0xEBCB8B),
        Rgb::hex(0xA6AFC0),
    ],
    flyout_shadow: &[shadow(8.0, 16.0, 0.26)],
    dialog_shadow: &[shadow(32.0, 64.0, 0.37), shadow(2.0, 21.0, 0.37)],
};

/// Solarized Dark, with a teal-green accent.
pub const SOLARIZED_DARK: FluentPalette = FluentPalette {
    name: "solarized-dark",
    high_contrast: false,
    base: Rgb::hex(0x00212B),
    layer: Surface {
        solid: Rgb::hex(0x002B36),
        mica: Tint::solid(Rgb::hex(0x002B36)),
    },
    card: Surface {
        solid: Rgb::hex(0x073642),
        mica: Tint::solid(Rgb::hex(0x073642)),
    },
    card_stroke: Tint::alpha(Rgb::hex(0xEEE8D5), 0.08),
    control: Tint::alpha(Rgb::hex(0xEEE8D5), 0.06),
    control_hover: Tint::alpha(Rgb::hex(0xEEE8D5), 0.08),
    control_pressed: Tint::alpha(Rgb::hex(0xEEE8D5), 0.03),
    control_stroke: Tint::alpha(Rgb::hex(0xEEE8D5), 0.07),
    control_hover_stroke: Tint::alpha(Rgb::hex(0xEEE8D5), 0.07),
    control_stroke_edge: (Edge::Top, Tint::alpha(Rgb::hex(0xEEE8D5), 0.09)),
    field_stroke: Tint::alpha(Rgb::hex(0xEEE8D5), 0.07),
    strong_stroke: Tint::alpha(Rgb::hex(0xEEE8D5), 0.54),
    divider: Tint::alpha(Rgb::hex(0xEEE8D5), 0.08),
    overlay: Rgb::hex(0x073642),
    overlay_stroke: Tint::alpha(Rgb::hex(0xEEE8D5), 0.08),
    text: Rgb::hex(0xEEE8D5),
    text_secondary: Rgb::hex(0x9AA7A7),
    text_disabled: Rgb::hex(0x586E75),
    control_text: Rgb::hex(0xEEE8D5),
    accent: Rgb::hex(0x2AA198),
    accent_hover: Rgb::hex(0x27968D),
    accent_edge: Tint::alpha(BLACK, 0.14),
    on_accent: Rgb::hex(0x00212B),
    accent_text: Rgb::hex(0x30B8AD),
    selection: Tint::alpha(Rgb::hex(0x2AA198), 0.10),
    on_selection: Rgb::hex(0xEEE8D5),
    critical: Rgb::hex(0xE66967),
    critical_fill: Rgb::hex(0x1A2C35),
    success: Rgb::hex(0x8CA100),
    success_fill: Rgb::hex(0x0D3631),
    caution: Rgb::hex(0xBD8F00),
    caution_fill: Rgb::hex(0x123431),
    info: Rgb::hex(0x30B8AD),
    info_fill: Tint::alpha(Rgb::hex(0xEEE8D5), 0.05),
    smoke: Tint::alpha(BLACK, 0.30),
    focus_outer: Rgb::hex(0xEEE8D5),
    focus_inner: Rgb::hex(0x00212B),
    close_hover: CLOSE_HOVER,
    on_close_hover: ON_CLOSE_HOVER,
    kinds: [
        Rgb::hex(0x47A0DE),
        Rgb::hex(0x9194D3),
        Rgb::hex(0xE177AB),
        Rgb::hex(0xC19200),
        Rgb::hex(0x8C9C9D),
    ],
    flyout_shadow: &[shadow(8.0, 16.0, 0.26)],
    dialog_shadow: &[shadow(32.0, 64.0, 0.37), shadow(2.0, 21.0, 0.37)],
};

/// Solarized Light, with a deep teal-green accent.
pub const SOLARIZED_LIGHT: FluentPalette = FluentPalette {
    name: "solarized-light",
    high_contrast: false,
    base: Rgb::hex(0xEEE8D5),
    layer: Surface {
        solid: Rgb::hex(0xFDF6E3),
        mica: Tint::solid(Rgb::hex(0xFDF6E3)),
    },
    card: Surface {
        solid: Rgb::hex(0xFFFBF0),
        mica: Tint::solid(Rgb::hex(0xFFFBF0)),
    },
    card_stroke: Tint::alpha(Rgb::hex(0x073642), 0.10),
    control: Tint::solid(Rgb::hex(0xFFFBF0)),
    control_hover: Tint::solid(Rgb::hex(0xFAF7EC)),
    control_pressed: Tint::solid(Rgb::hex(0xF8F6EB)),
    control_stroke: Tint::alpha(Rgb::hex(0x073642), 0.10),
    control_hover_stroke: Tint::alpha(Rgb::hex(0x073642), 0.10),
    control_stroke_edge: (Edge::Bottom, Tint::alpha(Rgb::hex(0x073642), 0.16)),
    field_stroke: Tint::alpha(Rgb::hex(0x073642), 0.12),
    strong_stroke: Tint::alpha(Rgb::hex(0x073642), 0.54),
    divider: Tint::alpha(Rgb::hex(0x073642), 0.10),
    overlay: Rgb::hex(0xFDF6E3),
    overlay_stroke: Tint::alpha(Rgb::hex(0x073642), 0.10),
    text: Rgb::hex(0x073642),
    text_secondary: Rgb::hex(0x566C73),
    text_disabled: Rgb::hex(0x93A1A1),
    control_text: Rgb::hex(0x073642),
    accent: Rgb::hex(0x217D76),
    accent_hover: Rgb::hex(0x1F766F),
    accent_edge: Tint::alpha(BLACK, 0.24),
    on_accent: Rgb::hex(0xFDF6E3),
    accent_text: Rgb::hex(0x1E736D),
    selection: Tint::alpha(Rgb::hex(0x217D76), 0.10),
    on_selection: Rgb::hex(0x073642),
    critical: Rgb::hex(0xC72521),
    critical_fill: Rgb::hex(0xFAE2D1),
    success: Rgb::hex(0x627000),
    success_fill: Rgb::hex(0xEFEBC8),
    caution: Rgb::hex(0x846400),
    caution_fill: Rgb::hex(0xF4E9C8),
    info: Rgb::hex(0x1E736D),
    info_fill: Tint::alpha(Rgb::hex(0xFAF7EC), 0.50),
    smoke: Tint::alpha(BLACK, 0.30),
    focus_outer: Rgb::hex(0x073642),
    focus_inner: Rgb::hex(0xEEE8D5),
    close_hover: CLOSE_HOVER,
    on_close_hover: ON_CLOSE_HOVER,
    kinds: [
        Rgb::hex(0x2075B1),
        Rgb::hex(0x6267C0),
        Rgb::hex(0xCE2D7B),
        Rgb::hex(0x8C6A00),
        Rgb::hex(0x5F737B),
    ],
    flyout_shadow: &[shadow(8.0, 16.0, 0.14)],
    dialog_shadow: &[shadow(32.0, 64.0, 0.28), shadow(2.0, 21.0, 0.28)],
};

/// The palette under a Windows contrast theme: every token is one of the
/// person's eight system colours, solid. Controls take ButtonFace and
/// ButtonText; cards, controls and overlays get a WindowText edge;
/// selection, the lit wheel tile and Close under the pointer use Highlight
/// and HighlightText; links use Hotlight. No smoke, no shadow.
pub fn contrast_palette(c: SysColors) -> FluentPalette {
    let window = Surface {
        solid: c.window,
        mica: Tint::solid(c.window),
    };
    let edge = Tint::solid(c.window_text);
    FluentPalette {
        name: "high-contrast",
        high_contrast: true,
        base: c.window,
        layer: window,
        card: window,
        card_stroke: edge,
        control: Tint::solid(c.button_face),
        control_hover: Tint::solid(c.button_face),
        control_pressed: Tint::solid(c.button_face),
        control_stroke: edge,
        control_hover_stroke: Tint::solid(c.highlight),
        control_stroke_edge: (Edge::Top, edge),
        field_stroke: edge,
        strong_stroke: edge,
        divider: edge,
        overlay: c.window,
        overlay_stroke: edge,
        text: c.window_text,
        text_secondary: c.window_text,
        text_disabled: c.gray_text,
        control_text: c.button_text,
        accent: c.highlight,
        accent_hover: c.highlight,
        accent_edge: Tint::NONE,
        on_accent: c.highlight_text,
        accent_text: c.hotlight,
        selection: Tint::solid(c.highlight),
        on_selection: c.highlight_text,
        critical: c.window_text,
        critical_fill: c.window,
        success: c.window_text,
        success_fill: c.window,
        caution: c.window_text,
        caution_fill: c.window,
        info: c.window_text,
        info_fill: Tint::solid(c.window),
        smoke: Tint::NONE,
        focus_outer: c.window_text,
        focus_inner: c.window,
        close_hover: c.highlight,
        on_close_hover: c.highlight_text,
        kinds: [c.window_text; 5],
        flyout_shadow: &[],
        dialog_shadow: &[],
    }
}

/// Every palette Kiln brings itself (a contrast theme's comes from
/// Windows). Tests and captures loop over this, so a theme is covered by
/// being listed here.
pub const ALL_BUILT_IN: [FluentPalette; 8] = [
    DARK,
    LIGHT,
    DRACULA,
    GRUVBOX,
    MONOKAI,
    NORD,
    SOLARIZED_DARK,
    SOLARIZED_LIGHT,
];

/// The palette to draw with, in one place for the window and the drag-wheel:
/// a Windows contrast theme's while one is on, whatever `choice` says;
/// otherwise the theme `choice` names, with System following the Windows
/// app mode (`system_light`).
pub fn palette(
    contrast: Option<SysColors>,
    choice: ThemeChoice,
    system_light: bool,
) -> FluentPalette {
    if let Some(c) = contrast {
        return contrast_palette(c);
    }
    match choice {
        ThemeChoice::System if system_light => LIGHT,
        ThemeChoice::System | ThemeChoice::Dark => DARK,
        ThemeChoice::Light => LIGHT,
        ThemeChoice::Dracula => DRACULA,
        ThemeChoice::Gruvbox => GRUVBOX,
        ThemeChoice::Monokai => MONOKAI,
        ThemeChoice::Nord => NORD,
        ThemeChoice::SolarizedDark => SOLARIZED_DARK,
        ThemeChoice::SolarizedLight => SOLARIZED_LIGHT,
    }
}

/// The type ramp. Sizes and line heights are DIPs at 100% Text size;
/// Windows' Text size scales every style.
pub mod type_ramp {
    pub const REGULAR: u16 = 400;
    pub const SEMIBOLD: u16 = 600;

    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct TextStyle {
        pub name: &'static str,
        pub size: f32,
        pub line: f32,
        pub weight: u16,
        /// Tabular figures, so changing numbers don't shift.
        pub tabular: bool,
    }

    const fn style(name: &'static str, size: f32, line: f32, weight: u16) -> TextStyle {
        TextStyle {
            name,
            size,
            line,
            weight,
            tabular: false,
        }
    }

    /// Row facts, badges, the title-bar title, flyout headers.
    pub const CAPTION: TextStyle = style("caption", 12.0, 16.0, REGULAR);
    /// Row names, controls, dialog body.
    pub const BODY: TextStyle = style("body", 14.0, 20.0, REGULAR);
    /// Selected row name, settings section headers, the wheel's pair.
    pub const BODY_STRONG: TextStyle = style("body_strong", 14.0, 20.0, SEMIBOLD);
    /// Side-pane file name, dialog titles, the empty-state headline.
    pub const SUBTITLE: TextStyle = style("subtitle", 20.0, 28.0, SEMIBOLD);
    /// The side pane's percentage.
    pub const TITLE: TextStyle = TextStyle {
        tabular: true,
        ..style("title", 28.0, 36.0, SEMIBOLD)
    };
    pub const ALL: [TextStyle; 5] = [CAPTION, BODY, BODY_STRONG, SUBTITLE, TITLE];

    /// Format codes: Semibold, uppercase, tracked out by 2% of the size.
    pub const CODE_WEIGHT: u16 = SEMIBOLD;
    pub const CODE_TRACKING_EM: f32 = 0.02;

    pub const SMALL_FAMILY: &str = "Segoe UI Variable Small";
    pub const TEXT_FAMILY: &str = "Segoe UI Variable Text";
    pub const DISPLAY_FAMILY: &str = "Segoe UI Variable Display";
    /// Where Segoe UI Variable isn't installed (Windows 10).
    pub const UI_FALLBACK: &str = "Segoe UI";
    /// Segoe Fluent Icons, or Segoe MDL2 Assets on Windows 10.
    pub const ICON_FAMILIES: [&str; 2] = ["Segoe Fluent Icons", "Segoe MDL2 Assets"];
    /// Only in the Details log.
    pub const MONO: [&str; 2] = ["Cascadia Mono", "Consolas"];
    pub const ICON_SIZE: f32 = 16.0;
    pub const ICON_SIZE_TITLE_BAR: f32 = 20.0;

    /// Segoe UI Variable's optical size for text of `size` DIPs: Small up
    /// to 12, Text to 19, Display from 20.
    pub fn optical_family(size: f32) -> &'static str {
        if size <= 12.0 {
            SMALL_FAMILY
        } else if size < 20.0 {
            TEXT_FAMILY
        } else {
            DISPLAY_FAMILY
        }
    }
}

/// Spacing and sizes in DIPs, on a 4-DIP grid.
pub mod space {
    pub const GRID: [f32; 7] = [4.0, 8.0, 12.0, 16.0, 24.0, 32.0, 48.0];
    /// Window edge to the content layer, and 0 below `NARROW_BELOW` wide.
    pub const WINDOW_EDGE: f32 = 16.0;
    pub const WINDOW_EDGE_NARROW: f32 = 0.0;
    pub const NARROW_BELOW: f32 = 600.0;
    pub const CARD_PADDING: f32 = 16.0;
    pub const DIALOG_PADDING: f32 = 24.0;
    pub const ROW_HEIGHT: f32 = 56.0;
    pub const THUMB: f32 = 40.0;
    /// Between a row's thumbnail and its text.
    pub const THUMB_GAP: f32 = 12.0;
    /// The side pane widens from the first to the second as the window
    /// grows, and folds below `FOLD_BELOW`.
    pub const SIDE_PANE: (f32, f32) = (320.0, 400.0);
    pub const FOLD_BELOW: f32 = 760.0;
    pub const TITLE_BAR: f32 = 48.0;
    /// The K in the title bar: its size, and its x.
    pub const TITLE_MARK: f32 = 16.0;
    pub const TITLE_MARK_X: f32 = 16.0;
    /// Help and Settings in the title bar.
    pub const TITLE_BUTTON: (f32, f32) = (40.0, 32.0);
    /// Minimise, Maximise and Close.
    pub const CAPTION_BUTTON: (f32, f32) = (46.0, 48.0);
    pub const BUTTON_HEIGHT: f32 = 32.0;
    pub const TOGGLE: (f32, f32) = (40.0, 20.0);
    pub const PROGRESS_HEIGHT: f32 = 3.0;
    /// The accent pill at a selected row's left.
    pub const SELECTION_PILL: (f32, f32) = (3.0, 16.0);
    pub const KIND_DOT: f32 = 6.0;
    /// The kind's arc round the wheel's outside.
    pub const KIND_ARC: f32 = 2.0;
    /// A kind badge's fill: the kind colour at this opacity.
    pub const KIND_BADGE_FILL: f32 = 0.12;
    pub const FOCUS_OUTER: f32 = 2.0;
    pub const FOCUS_INNER: f32 = 1.0;
    /// Between a control and its focus ring.
    pub const FOCUS_GAP: f32 = 2.0;
    pub const SCROLL_THUMB_REST: f32 = 2.0;
    pub const SCROLL_THUMB_HOVER: f32 = 6.0;
    /// The K on the empty window.
    pub const EMPTY_MARK: f32 = 64.0;
    /// Between the wheel's segments.
    pub const WHEEL_GAP: f32 = 2.0;
    /// A hairline, and a stroke inside a control.
    pub const STROKE: f32 = 1.0;
    /// The layout's base step.
    pub const GRID_UNIT: f32 = 4.0;
    /// The list panel's padding, round it.
    pub const PANEL_PADDING: f32 = 16.0;
    /// The mark's grid, and its line in the app icon.
    pub const ICON_GRID: f32 = 24.0;
    pub const ICON_STROKE: f32 = 1.5;

    /// The window edge for a window `width` DIPs wide.
    pub fn window_edge(width: f32) -> f32 {
        if width < NARROW_BELOW {
            WINDOW_EDGE_NARROW
        } else {
            WINDOW_EDGE
        }
    }
}

/// Corner radii in DIPs.
pub mod radius {
    /// Controls, rows, thumbnails, badges.
    pub const CONTROL: f32 = 4.0;
    /// The content layer, cards, dialogs, flyouts.
    pub const LAYER: f32 = 8.0;
    pub const PROGRESS: f32 = 1.5;
    /// The wheel segments' inner corners.
    pub const WHEEL_INNER: f32 = 4.0;
}

/// How high a surface sits, and what that gives it. Only overlays cast a
/// shadow (`FluentPalette::flyout_shadow` and `dialog_shadow`).
pub mod elevation {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    pub enum Level {
        /// Mica, or the solid base.
        Base = 0,
        /// The content layer: a fill step only.
        Layer = 1,
        /// Cards and the side pane: fill and `card_stroke`.
        Card = 2,
        /// Dialogs, the wheel, flyouts, tooltips, InfoBar toasts:
        /// `overlay`, `overlay_stroke` and a shadow.
        Overlay = 3,
    }

    impl Level {
        pub const ALL: [Level; 4] = [Level::Base, Level::Layer, Level::Card, Level::Overlay];

        pub fn stroked(self) -> bool {
            self >= Level::Card
        }

        pub fn shadowed(self) -> bool {
            self == Level::Overlay
        }
    }
}

/// Times and easing. With Windows' Animation effects off every duration
/// is 0, so nothing moves; motion never carries meaning alone.
pub mod motion {
    /// A CSS `cubic-bezier(x1, y1, x2, y2)` timing curve from (0, 0) to
    /// (1, 1).
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct CubicBezier {
        pub x1: f32,
        pub y1: f32,
        pub x2: f32,
        pub y2: f32,
    }

    impl CubicBezier {
        /// The curve's point at parameter `s`, 0 to 1.
        pub fn point(self, s: f32) -> (f32, f32) {
            let b = |p1: f32, p2: f32| {
                let u = 1.0 - s;
                3.0 * u * u * s * p1 + 3.0 * u * s * s * p2 + s * s * s
            };
            (b(self.x1, self.x2), b(self.y1, self.y2))
        }

        /// How far along, 0 to 1, the motion is when `t` of its time has
        /// passed. x rises with s for every curve here, so a bisection
        /// finds the s whose x is `t`.
        pub fn ease(self, t: f32) -> f32 {
            if t <= 0.0 {
                return 0.0;
            }
            if t >= 1.0 {
                return 1.0;
            }
            let (mut lo, mut hi) = (0.0f32, 1.0f32);
            for _ in 0..40 {
                let mid = 0.5 * (lo + hi);
                if self.point(mid).0 < t {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            self.point(0.5 * (lo + hi)).1.clamp(0.0, 1.0)
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum Easing {
        Linear,
        Curve(CubicBezier),
    }

    impl Easing {
        pub fn ease(self, t: f32) -> f32 {
            match self {
                Easing::Linear => t.clamp(0.0, 1.0),
                Easing::Curve(c) => c.ease(t),
            }
        }
    }

    /// Things arriving: fast start, gentle stop.
    pub const DECELERATE: CubicBezier = CubicBezier {
        x1: 0.0,
        y1: 0.0,
        x2: 0.0,
        y2: 1.0,
    };
    /// Things leaving: slow start, quick finish.
    pub const ACCELERATE: CubicBezier = CubicBezier {
        x1: 0.7,
        y1: 0.0,
        x2: 1.0,
        y2: 0.5,
    };

    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct Transition {
        pub ms: u32,
        pub easing: Easing,
    }

    /// Hover and press.
    pub const HOVER: Transition = Transition {
        ms: 83,
        easing: Easing::Linear,
    };
    /// A small state change, a flyout or the wheel coming in, a toggle.
    pub const SMALL: Transition = Transition {
        ms: 167,
        easing: Easing::Curve(DECELERATE),
    };
    /// A dialog, a pane, a row being added.
    pub const PANE: Transition = Transition {
        ms: 250,
        easing: Easing::Curve(DECELERATE),
    };
    /// Anything leaving.
    pub const EXIT: Transition = Transition {
        ms: 167,
        easing: Easing::Curve(ACCELERATE),
    };
    /// The Expander's chevron, which turns `EXPANDER_TURN_DEG`.
    pub const EXPANDER: Transition = SMALL;
    pub const EXPANDER_TURN_DEG: f32 = 180.0;
    /// How far an added row rises as it comes in, in DIPs.
    pub const ROW_RISE: f32 = 10.0;
    /// The wheel opens with a fade and a scale from this to 1, from its
    /// centre.
    pub const WHEEL_SCALE_FROM: f32 = 0.92;

    /// How long a motion of `ms` lasts: all of it with animations on,
    /// none with them off.
    pub fn duration(ms: u32, animations: bool) -> u32 {
        if animations {
            ms
        } else {
            0
        }
    }

    /// A short change with no curve of its own: a row fading, a drag's
    /// outline.
    pub const QUICK_MS: u32 = 120;

    static ANIMATE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

    /// Follow Windows' Animation effects setting
    /// (`SPI_GETCLIENTAREAANIMATION`). Every window in the process shares
    /// it, as Windows does.
    pub fn set_animations(on: bool) {
        ANIMATE.store(on, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn animations() -> bool {
        #[cfg(test)]
        if let Some(on) = test_override::get() {
            return on;
        }
        ANIMATE.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// For a test: Animation effects set for the calling thread only, as
    /// they were before when the guard drops. Tests run in parallel, one
    /// thread each, and a window's messages are handled on the thread that
    /// made it, so this reaches the windows a test makes and no other
    /// test's.
    #[cfg(test)]
    pub struct AnimationsFor(Option<bool>);

    #[cfg(test)]
    impl AnimationsFor {
        pub fn new(on: bool) -> Self {
            AnimationsFor(test_override::set(Some(on)))
        }
    }

    #[cfg(test)]
    impl Drop for AnimationsFor {
        fn drop(&mut self) {
            test_override::set(self.0);
        }
    }

    /// How far, 0 to 1 and eased, a motion of `ms` is after `elapsed_ms`.
    /// One of no length is already done.
    pub fn progress(elapsed_ms: f32, ms: u32) -> f32 {
        if ms == 0 {
            return 1.0;
        }
        ease_out(elapsed_ms / ms as f32)
    }

    /// Cubic ease-out: fast start, gentle stop, never past 1.
    pub fn ease_out(t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        1.0 - (1.0 - t).powi(3)
    }

    #[cfg(test)]
    mod test_override {
        use std::cell::Cell;
        thread_local! {
            static OVERRIDE: Cell<Option<bool>> = const { Cell::new(None) };
        }
        pub fn get() -> Option<bool> {
            OVERRIDE.with(Cell::get)
        }
        /// Set it; what it was.
        pub fn set(v: Option<bool>) -> Option<bool> {
            OVERRIDE.with(|c| c.replace(v))
        }
    }
}

/// Segoe Fluent Icons code points (the same in Segoe MDL2 Assets).
pub mod glyph {
    pub const SETTINGS: char = '\u{E713}';
    pub const HELP: char = '\u{E897}';
    pub const BACK: char = '\u{E72B}';
    pub const ADD_FILES: char = '\u{E710}';
    pub const SHOW_IN_FOLDER: char = '\u{E838}';
    pub const MORE: char = '\u{E712}';
    pub const DISMISS: char = '\u{E711}';
    pub const CHEVRON_DOWN: char = '\u{E70D}';
    pub const CHEVRON_UP: char = '\u{E70E}';
    pub const CHEVRON_RIGHT: char = '\u{E76C}';
    pub const CHECKMARK: char = '\u{E73E}';
    pub const ERROR: char = '\u{EA39}';
    pub const INFO: char = '\u{E946}';
    pub const WARNING: char = '\u{E7BA}';
    pub const MAKE_SMALLER: char = '\u{E73F}';
    pub const COPY_PATH: char = '\u{E8C8}';
    pub const REMOVE: char = '\u{E74D}';
    /// Convert to…, in the right-click menu.
    pub const CONVERT: char = '\u{E8AB}';
    pub const RETRY: char = '\u{E72C}';
    pub const MINIMISE: char = '\u{E921}';
    pub const MAXIMISE: char = '\u{E922}';
    pub const RESTORE: char = '\u{E923}';
    pub const CLOSE: char = '\u{E8BB}';

    pub const ALL: [(&str, char); 23] = [
        ("settings", SETTINGS),
        ("help", HELP),
        ("back", BACK),
        ("add_files", ADD_FILES),
        ("show_in_folder", SHOW_IN_FOLDER),
        ("more", MORE),
        ("dismiss", DISMISS),
        ("chevron_down", CHEVRON_DOWN),
        ("chevron_up", CHEVRON_UP),
        ("chevron_right", CHEVRON_RIGHT),
        ("checkmark", CHECKMARK),
        ("error", ERROR),
        ("info", INFO),
        ("warning", WARNING),
        ("make_smaller", MAKE_SMALLER),
        ("copy_path", COPY_PATH),
        ("remove", REMOVE),
        ("convert", CONVERT),
        ("retry", RETRY),
        ("minimise", MINIMISE),
        ("maximise", MAXIMISE),
        ("restore", RESTORE),
        ("close", CLOSE),
    ];
}

/// Every pair the Fluent surfaces draw, measured from `p` on solid
/// backgrounds (tints composited first): text and secondary text need
/// 4.5:1, so does a glyph on a coloured fill; the accent fill, the strong
/// stroke, the focus ring and the kind cues need 3:1. Disabled text needs
/// nothing and isn't listed.
pub fn contrast_report(p: &FluentPalette) -> Vec<FluentPair> {
    let (base, layer, card, overlay) = (p.base, p.layer.solid, p.card.solid, p.overlay);
    let sel_layer = p.selection.over(layer);
    let sel_card = p.selection.over(card);
    let control = p.control.over(card);
    let control_hover = p.control_hover.over(card);
    let info_fill = p.info_fill.over(card);
    let surfaces: [(&str, Rgb); 6] = [
        ("base", base),
        ("layer", layer),
        ("card", card),
        ("overlay", overlay),
        ("selection on layer", sel_layer),
        ("selection on card", sel_card),
    ];
    let mut out: Vec<(String, Rgb, Rgb, Option<f32>)> = vec![];
    let mut add = |name: String, fg: Rgb, bg: Rgb, need: Option<f32>| {
        out.push((name, fg, bg, need));
    };
    for (sname, s) in surfaces {
        add(format!("text on {sname}"), p.text, s, TEXT);
        add(
            format!("secondary text on {sname}"),
            p.text_secondary,
            s,
            TEXT,
        );
        add(format!("accent text on {sname}"), p.accent_text, s, TEXT);
        for k in FileKind::ALL {
            add(
                format!("{} kind cue on {sname}", k.name()),
                p.kind(k),
                s,
                NON_TEXT,
            );
        }
    }
    // A kind colour on the flyout's overlay is held to text contrast, as it
    // was when More coloured its format codes by kind.
    for k in FileKind::ALL {
        add(
            format!("{} format code on overlay", k.name()),
            p.kind(k),
            overlay,
            TEXT,
        );
    }
    for (sname, s) in surfaces.iter().take(4) {
        add(format!("accent fill on {sname}"), p.accent, *s, NON_TEXT);
        add(
            format!("strong stroke on {sname}"),
            p.strong_stroke.over(*s),
            *s,
            NON_TEXT,
        );
        add(
            format!("focus ring on {sname}"),
            p.focus_outer,
            *s,
            NON_TEXT,
        );
    }
    add(
        "selected text on selection".into(),
        p.on_selection,
        sel_layer,
        TEXT,
    );
    add("text on accent".into(), p.on_accent, p.accent, TEXT);
    add(
        "text on accent hover".into(),
        p.on_accent,
        p.accent_hover,
        TEXT,
    );
    add(
        "control text on control".into(),
        p.control_text,
        control,
        TEXT,
    );
    add(
        "control text on hovered control".into(),
        p.control_text,
        control_hover,
        TEXT,
    );
    add(
        "focus inner ring on focus outer".into(),
        p.focus_inner,
        p.focus_outer,
        NON_TEXT,
    );
    add(
        "close glyph on close hover".into(),
        p.on_close_hover,
        p.close_hover,
        TEXT,
    );
    for (name, fg, fill) in [
        ("critical", p.critical, p.critical_fill),
        ("success", p.success, p.success_fill),
        ("caution", p.caution, p.caution_fill),
        ("info", p.info, info_fill),
    ] {
        add(format!("{name} text on its InfoBar"), fg, fill, TEXT);
        add(format!("{name} text on layer"), fg, layer, TEXT);
        add(format!("text on {name} InfoBar"), p.text, fill, TEXT);
    }
    // A card's outline shows against the card, so cards and the rows laid
    // on them keep a shape in every theme.
    add(
        "card stroke on card".into(),
        p.card_stroke.over(card),
        card,
        STROKE,
    );
    out.into_iter()
        .map(|(name, fg, bg, need)| (name, fg, bg, contrast(fg, bg), need))
        .collect()
}

fn tint(t: Tint) -> String {
    if t.alpha == 1.0 {
        t.rgb.to_hex()
    } else if t.alpha == 0.0 {
        "none".into()
    } else {
        format!("{} {}%", t.rgb.to_hex(), (t.alpha * 100.0).round())
    }
}

fn shadows(s: &[Shadow]) -> String {
    if s.is_empty() {
        return "none".into();
    }
    s.iter()
        .map(|s| format!("0 {} {} {}", s.y, s.blur, tint(Tint::alpha(s.rgb, s.alpha))))
        .collect::<Vec<_>>()
        .join(", ")
}

fn palette_lines(p: &FluentPalette, name: &dyn Fn(Rgb) -> String) -> String {
    let t = |x: Tint| {
        if x.alpha == 1.0 {
            name(x.rgb)
        } else {
            tint(x)
        }
    };
    let surface = |s: Surface| format!("{} | {}", name(s.solid), t(s.mica));
    let edge = match p.control_stroke_edge.0 {
        Edge::Top => "top",
        Edge::Bottom => "bottom",
    };
    let mut rows: Vec<(String, String)> = vec![
        ("base".into(), name(p.base)),
        ("layer".into(), surface(p.layer)),
        ("card".into(), surface(p.card)),
        ("card_stroke".into(), t(p.card_stroke)),
        ("control".into(), t(p.control)),
        ("control_hover".into(), t(p.control_hover)),
        ("control_pressed".into(), t(p.control_pressed)),
        ("control_stroke".into(), t(p.control_stroke)),
        ("control_hover_stroke".into(), t(p.control_hover_stroke)),
        (
            "control_stroke_edge".into(),
            format!("{edge} {}", t(p.control_stroke_edge.1)),
        ),
        ("field_stroke".into(), t(p.field_stroke)),
        ("strong_stroke".into(), t(p.strong_stroke)),
        ("divider".into(), t(p.divider)),
        ("overlay".into(), name(p.overlay)),
        ("overlay_stroke".into(), t(p.overlay_stroke)),
        ("text".into(), name(p.text)),
        ("text_secondary".into(), name(p.text_secondary)),
        ("text_disabled".into(), name(p.text_disabled)),
        ("control_text".into(), name(p.control_text)),
        ("accent".into(), name(p.accent)),
        ("accent_hover".into(), name(p.accent_hover)),
        ("accent_edge".into(), t(p.accent_edge)),
        ("on_accent".into(), name(p.on_accent)),
        ("accent_text".into(), name(p.accent_text)),
        ("selection".into(), t(p.selection)),
        ("on_selection".into(), name(p.on_selection)),
        ("critical".into(), name(p.critical)),
        ("critical_fill".into(), name(p.critical_fill)),
        ("success".into(), name(p.success)),
        ("success_fill".into(), name(p.success_fill)),
        ("caution".into(), name(p.caution)),
        ("caution_fill".into(), name(p.caution_fill)),
        ("info".into(), name(p.info)),
        ("info_fill".into(), t(p.info_fill)),
        ("smoke".into(), t(p.smoke)),
        ("focus_outer".into(), name(p.focus_outer)),
        ("focus_inner".into(), name(p.focus_inner)),
        ("close_hover".into(), name(p.close_hover)),
        ("on_close_hover".into(), name(p.on_close_hover)),
    ];
    for k in FileKind::ALL {
        rows.push((format!("kind_{}", k.name()), name(p.kind(k))));
    }
    rows.push(("flyout_shadow".into(), shadows(p.flyout_shadow)));
    rows.push(("dialog_shadow".into(), shadows(p.dialog_shadow)));
    rows.iter().map(|(k, v)| format!("{k} = {v}\n")).collect()
}

/// Eight colours no theme uses, one per system colour, so the contrast
/// palette can be printed by role.
const MARKERS: SysColors = SysColors {
    window: Rgb(1, 0, 0),
    window_text: Rgb(2, 0, 0),
    highlight: Rgb(3, 0, 0),
    highlight_text: Rgb(4, 0, 0),
    gray_text: Rgb(5, 0, 0),
    hotlight: Rgb(6, 0, 0),
    button_face: Rgb(7, 0, 0),
    button_text: Rgb(8, 0, 0),
};

fn system_name(c: Rgb) -> String {
    match c.0 {
        1 => "window",
        2 => "window_text",
        3 => "highlight",
        4 => "highlight_text",
        5 => "gray_text",
        6 => "hotlight",
        7 => "button_face",
        8 => "button_text",
        _ => return format!("not a system colour: {}", c.to_hex()),
    }
    .into()
}

/// Every Fluent token as text, for `tests/snapshots/fluent_tokens.snap`.
pub fn describe() -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let hex = |c: Rgb| c.to_hex();
    for p in ALL_BUILT_IN {
        let _ = writeln!(s, "[fluent.{}]", p.name);
        s.push_str(&palette_lines(&p, &hex));
        s.push('\n');
    }
    s.push_str("[fluent.high-contrast]\n");
    s.push_str(&palette_lines(&contrast_palette(MARKERS), &system_name));

    use type_ramp as ty;
    s.push_str("\n[fluent.type]\n");
    for st in ty::ALL {
        let _ = writeln!(
            s,
            "{} = {}/{} {}{}",
            st.name,
            st.size,
            st.line,
            st.weight,
            if st.tabular { " tabular" } else { "" }
        );
    }
    let _ = writeln!(
        s,
        "code = {} uppercase tracking {}",
        ty::CODE_WEIGHT,
        ty::CODE_TRACKING_EM
    );
    let _ = writeln!(
        s,
        "optical = {} <= 12, {} 13-19, {} >= 20",
        ty::optical_family(12.0),
        ty::optical_family(13.0),
        ty::optical_family(20.0)
    );
    let _ = writeln!(s, "ui_fallback = {}", ty::UI_FALLBACK);
    let _ = writeln!(s, "icons = {}", ty::ICON_FAMILIES.join(", "));
    let _ = writeln!(s, "icon_size = {}", ty::ICON_SIZE);
    let _ = writeln!(s, "icon_size_title_bar = {}", ty::ICON_SIZE_TITLE_BAR);
    let _ = writeln!(s, "mono = {}", ty::MONO.join(", "));

    use space as sp;
    let pair = |(a, b): (f32, f32)| format!("{a}x{b}");
    s.push_str("\n[fluent.space]\n");
    let grid: Vec<String> = sp::GRID.iter().map(|g| g.to_string()).collect();
    let _ = writeln!(s, "grid = {}", grid.join(", "));
    for (k, v) in [
        ("window_edge", sp::WINDOW_EDGE),
        ("window_edge_narrow", sp::WINDOW_EDGE_NARROW),
        ("narrow_below", sp::NARROW_BELOW),
        ("card_padding", sp::CARD_PADDING),
        ("dialog_padding", sp::DIALOG_PADDING),
        ("row_height", sp::ROW_HEIGHT),
        ("thumb", sp::THUMB),
        ("thumb_gap", sp::THUMB_GAP),
    ] {
        let _ = writeln!(s, "{k} = {v}");
    }
    let _ = writeln!(s, "side_pane = {}-{}", sp::SIDE_PANE.0, sp::SIDE_PANE.1);
    let _ = writeln!(s, "fold_below = {}", sp::FOLD_BELOW);
    let _ = writeln!(s, "title_bar = {}", sp::TITLE_BAR);
    let _ = writeln!(s, "title_mark = {} at {}", sp::TITLE_MARK, sp::TITLE_MARK_X);
    let _ = writeln!(s, "title_button = {}", pair(sp::TITLE_BUTTON));
    let _ = writeln!(s, "caption_button = {}", pair(sp::CAPTION_BUTTON));
    let _ = writeln!(s, "button_height = {}", sp::BUTTON_HEIGHT);
    let _ = writeln!(s, "toggle = {}", pair(sp::TOGGLE));
    let _ = writeln!(s, "progress_height = {}", sp::PROGRESS_HEIGHT);
    let _ = writeln!(s, "selection_pill = {}", pair(sp::SELECTION_PILL));
    let _ = writeln!(s, "kind_dot = {}", sp::KIND_DOT);
    let _ = writeln!(s, "kind_arc = {}", sp::KIND_ARC);
    let _ = writeln!(
        s,
        "kind_badge_fill = {}%",
        (sp::KIND_BADGE_FILL * 100.0).round()
    );
    let _ = writeln!(
        s,
        "focus = {} outer, {} inner, {} gap",
        sp::FOCUS_OUTER,
        sp::FOCUS_INNER,
        sp::FOCUS_GAP
    );
    let _ = writeln!(
        s,
        "scroll_thumb = {} rest, {} hover",
        sp::SCROLL_THUMB_REST,
        sp::SCROLL_THUMB_HOVER
    );
    let _ = writeln!(s, "empty_mark = {}", sp::EMPTY_MARK);
    let _ = writeln!(s, "wheel_gap = {}", sp::WHEEL_GAP);
    let _ = writeln!(s, "stroke = {}", sp::STROKE);
    let _ = writeln!(s, "grid_unit = {}", sp::GRID_UNIT);
    let _ = writeln!(s, "panel_padding = {}", sp::PANEL_PADDING);
    let _ = writeln!(s, "icon_grid = {}", sp::ICON_GRID);
    let _ = writeln!(s, "icon_stroke = {}", sp::ICON_STROKE);

    s.push_str("\n[fluent.radius]\n");
    let _ = writeln!(s, "control = {}", radius::CONTROL);
    let _ = writeln!(s, "layer = {}", radius::LAYER);
    let _ = writeln!(s, "progress = {}", radius::PROGRESS);
    let _ = writeln!(s, "wheel_inner = {}", radius::WHEEL_INNER);

    s.push_str("\n[fluent.elevation]\n");
    for level in elevation::Level::ALL {
        use elevation::Level;
        let what = match level {
            Level::Base => "base = mica",
            Level::Layer => "layer = fill",
            Level::Card => "card = fill",
            Level::Overlay => "overlay = overlay",
        };
        let mut line = format!("{} {what}", level as u8);
        if level.stroked() {
            line.push_str(if level == Level::Overlay {
                ", overlay_stroke"
            } else {
                ", card_stroke"
            });
        }
        if level.shadowed() {
            line.push_str(", shadow");
        }
        let _ = writeln!(s, "{line}");
    }

    s.push_str("\n[fluent.motion]\n");
    let easing = |e: motion::Easing| match e {
        motion::Easing::Linear => "linear".to_string(),
        motion::Easing::Curve(c) => {
            format!("cubic-bezier({}, {}, {}, {})", c.x1, c.y1, c.x2, c.y2)
        }
    };
    for (k, t) in [
        ("hover", motion::HOVER),
        ("small", motion::SMALL),
        ("pane", motion::PANE),
        ("exit", motion::EXIT),
    ] {
        let _ = writeln!(s, "{k} = {} {}", t.ms, easing(t.easing));
    }
    let _ = writeln!(
        s,
        "expander = {} {} turn {}",
        motion::EXPANDER.ms,
        easing(motion::EXPANDER.easing),
        motion::EXPANDER_TURN_DEG
    );
    let _ = writeln!(s, "row_rise = {}", motion::ROW_RISE);
    let _ = writeln!(s, "wheel_scale_from = {}", motion::WHEEL_SCALE_FROM);
    let _ = writeln!(s, "quick = {}", motion::QUICK_MS);

    s.push_str("\n[fluent.glyphs]\n");
    for (k, g) in glyph::ALL {
        let _ = writeln!(s, "{k} = {:04X}", g as u32);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// High Contrast Black, with ButtonFace and ButtonText.
    const HC_BLACK: SysColors = SysColors {
        window: Rgb(0, 0, 0),
        window_text: Rgb(255, 255, 255),
        highlight: Rgb(0x1A, 0xEB, 0xFF),
        highlight_text: Rgb(0, 0, 0),
        gray_text: Rgb(0x3F, 0xF2, 0x3F),
        hotlight: Rgb(0xFF, 0xFF, 0),
        button_face: Rgb(0, 0, 0),
        button_text: Rgb(255, 255, 255),
    };

    #[test]
    fn every_pair_meets_its_contrast_in_light_and_dark() {
        for p in ALL_BUILT_IN {
            let report = contrast_report(&p);
            assert!(report.len() > 70, "{} pairs", report.len());
            let failing: Vec<String> = report
                .iter()
                .filter(|(_, _, _, r, need)| need.is_some_and(|n| *r < n))
                .map(|(name, fg, bg, r, need)| {
                    format!(
                        "{}: {name} {} on {} is {r:.2}, needs {need:?}",
                        p.name,
                        fg.to_hex(),
                        bg.to_hex()
                    )
                })
                .collect();
            assert!(failing.is_empty(), "{}", failing.join("\n"));
        }
    }

    #[test]
    fn a_card_stroke_shows_on_its_card_in_every_palette() {
        // the dark card stroke was black at 10% over a #2B2B2B
        // card and measured 1.05:1, so cards and rows had no outline. The
        // floor is in the report, for every palette, now and later.
        let name = "card stroke on card";
        for p in ALL_BUILT_IN.into_iter().chain([contrast_palette(HC_BLACK)]) {
            let report = contrast_report(&p);
            let pair = report
                .iter()
                .find(|(n, ..)| *n == name)
                .unwrap_or_else(|| panic!("{}: no pair {name}", p.name));
            assert_eq!(pair.4, STROKE, "{}", p.name);
            assert!(pair.3 >= 1.15, "{}: {:.3}", p.name, pair.3);
        }
        // The old dark stroke fails the floor, so a palette can't lose it.
        let old = FluentPalette {
            card_stroke: Tint::alpha(BLACK, 0.10),
            ..DARK
        };
        let pair = contrast_report(&old)
            .into_iter()
            .find(|(n, ..)| *n == name)
            .unwrap();
        assert!(pair.3 < 1.15, "{:.3}", pair.3);
        assert!(pair.4.is_some_and(|need| pair.3 < need));
        // The dark card stroke is white at 8%, as the overlay's.
        assert_eq!(DARK.card_stroke, DARK.overlay_stroke);
    }

    #[test]
    fn kind_coloured_format_codes_are_held_to_text_contrast_on_the_overlay() {
        // More's flyout once coloured its format codes by kind; they are
        // plain text now, and the kind colours stay held to the 4.5:1 of
        // text on the flyout's overlay, not the 3:1 of a cue.
        for p in ALL_BUILT_IN {
            let report = contrast_report(&p);
            for k in FileKind::ALL {
                let name = format!("{} format code on overlay", k.name());
                let pair = report
                    .iter()
                    .find(|(n, ..)| *n == name)
                    .unwrap_or_else(|| panic!("{}: no pair {name}", p.name));
                assert_eq!((pair.1, pair.2), (p.kind(k), p.overlay), "{name}");
                assert_eq!(pair.4, TEXT, "{name}");
            }
        }
    }

    #[test]
    fn a_surface_over_mica_lands_on_its_solid_in_every_palette() {
        // Mica takes the base's colour, so a Mica tint over the base must
        // come to the solid value (within 1 per channel, for rounding):
        // the code uses the tint where it has Mica and the solid where it
        // has none, and the two must look the same.
        for p in ALL_BUILT_IN {
            for (what, s) in [("layer", p.layer), ("card", p.card)] {
                let got = s.mica.over(p.base);
                let near = |a: u8, b: u8| a.abs_diff(b) <= 1;
                assert!(
                    near(got.0, s.solid.0) && near(got.1, s.solid.1) && near(got.2, s.solid.2),
                    "{} {what}: mica over base is {}, solid is {}",
                    p.name,
                    got.to_hex(),
                    s.solid.to_hex()
                );
            }
        }
    }

    #[test]
    fn every_theme_choice_draws_its_own_palette() {
        let named = [
            (ThemeChoice::Dracula, "dracula"),
            (ThemeChoice::Gruvbox, "gruvbox"),
            (ThemeChoice::Monokai, "monokai"),
            (ThemeChoice::Nord, "nord"),
            (ThemeChoice::SolarizedDark, "solarized-dark"),
            (ThemeChoice::SolarizedLight, "solarized-light"),
        ];
        for (choice, name) in named {
            for system_light in [false, true] {
                let p = palette(None, choice, system_light);
                assert_eq!(p.name, name);
                assert!(ALL_BUILT_IN.contains(&p), "{name} is listed");
            }
        }
        // The names are those settings.toml uses, and each is listed once.
        let mut names: Vec<&str> = ALL_BUILT_IN.iter().map(|p| p.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ALL_BUILT_IN.len());
        for t in ThemeChoice::ALL {
            if t.is_named() {
                assert!(names.contains(&t.name()), "{t:?}");
            }
        }
    }

    #[test]
    fn every_built_in_theme_has_one_accent_and_it_is_a_green() {
        // Hue in degrees, from the largest and smallest channels.
        fn hue(c: Rgb) -> f32 {
            let (r, g, b) = (c.0 as f32, c.1 as f32, c.2 as f32);
            let (hi, lo) = (r.max(g).max(b), r.min(g).min(b));
            let d = hi - lo;
            let h = if hi == r {
                ((g - b) / d).rem_euclid(6.0)
            } else if hi == g {
                (b - r) / d + 2.0
            } else {
                (r - g) / d + 4.0
            };
            h * 60.0
        }
        for p in ALL_BUILT_IN {
            for (what, c) in [("accent", p.accent), ("accent_text", p.accent_text)] {
                let h = hue(c);
                assert!(
                    (75.0..=190.0).contains(&h),
                    "{} {what} {} is hue {h:.0}, not a green",
                    p.name,
                    c.to_hex()
                );
            }
            // The selection, the info bar and the focus-free accents use it.
            assert_eq!(p.selection.rgb, p.accent, "{}", p.name);
        }
    }

    #[test]
    fn a_contrast_theme_wins_over_every_choice() {
        for t in ThemeChoice::ALL {
            for light in [false, true] {
                assert_eq!(
                    palette(Some(HC_BLACK), t, light),
                    contrast_palette(HC_BLACK),
                    "{t:?}"
                );
            }
        }
    }

    #[test]
    fn the_accent_pairs_are_as_the_spec_measures_them() {
        // Look spec: on_accent 8.8:1 dark. It gives 5.2:1 light, but
        // #FAFAFA on #1F7A5C measures 5.03:1 by WCAG 2.x; still over 4.5.
        assert!((contrast(DARK.on_accent, DARK.accent) - 8.8).abs() < 0.05);
        assert!((contrast(LIGHT.on_accent, LIGHT.accent) - 5.03).abs() < 0.01);
    }

    #[test]
    fn text_and_secondary_text_reach_four_and_a_half_on_every_surface() {
        for p in ALL_BUILT_IN {
            for bg in [
                p.base,
                p.layer.solid,
                p.card.solid,
                p.overlay,
                p.selection.over(p.layer.solid),
                p.selection.over(p.card.solid),
            ] {
                for fg in [p.text, p.text_secondary] {
                    assert!(
                        contrast(fg, bg) >= 4.5,
                        "{} {} on {}",
                        p.name,
                        fg.to_hex(),
                        bg.to_hex()
                    );
                }
            }
        }
    }

    #[test]
    fn the_contrast_palette_uses_only_the_eight_system_colours_solid() {
        let p = contrast_palette(MARKERS);
        assert!(p.high_contrast);
        let lines = palette_lines(&p, &system_name);
        assert!(!lines.contains("not a system colour"), "{lines}");
        assert!(
            !lines.contains('%'),
            "alpha in the contrast palette:\n{lines}"
        );
        assert!(!lines.contains('#'), "{lines}");
        // Smoke is drawn not at all, rather than as a see-through wash.
        assert_eq!(p.smoke.alpha, 0.0);
        let tints = [
            p.layer.mica,
            p.card.mica,
            p.card_stroke,
            p.control,
            p.control_hover,
            p.control_pressed,
            p.control_stroke,
            p.control_stroke_edge.1,
            p.strong_stroke,
            p.divider,
            p.overlay_stroke,
            p.selection,
            p.info_fill,
        ];
        assert!(tints.iter().all(|t| t.alpha == 1.0));
        assert!(p.flyout_shadow.is_empty() && p.dialog_shadow.is_empty());
        // All eight are used somewhere.
        for role in [
            "= window\n",
            "= window_text\n",
            "= highlight\n",
            "= highlight_text\n",
            "= gray_text\n",
            "= hotlight\n",
            "= button_face\n",
            "= button_text\n",
        ] {
            assert!(lines.contains(role), "{role:?} unused");
        }
    }

    #[test]
    fn controls_take_the_button_colours_under_contrast() {
        let p = contrast_palette(HC_BLACK);
        assert_eq!(p.control.rgb, HC_BLACK.button_face);
        assert_eq!(p.control_text, HC_BLACK.button_text);
        assert_eq!(p.card_stroke.rgb, HC_BLACK.window_text);
        assert_eq!(
            (p.selection.rgb, p.on_selection),
            (HC_BLACK.highlight, HC_BLACK.highlight_text)
        );
        assert_eq!(palette(Some(HC_BLACK), ThemeChoice::Light, true), p);
        assert_eq!(palette(None, ThemeChoice::System, true), LIGHT);
        assert_eq!(palette(None, ThemeChoice::System, false), DARK);
        assert_eq!(palette(None, ThemeChoice::Light, false), LIGHT);
        assert_eq!(palette(None, ThemeChoice::Dark, true), DARK);
    }

    #[test]
    fn no_pure_white_or_black_in_text_or_solid_fills() {
        for p in ALL_BUILT_IN {
            for c in [
                p.base,
                p.layer.solid,
                p.card.solid,
                p.overlay,
                p.text,
                p.text_secondary,
                p.text_disabled,
                p.control_text,
                p.accent,
                p.on_accent,
                p.accent_text,
                p.on_selection,
                p.focus_outer,
                p.focus_inner,
                p.on_close_hover,
            ] {
                assert!(c != WHITE && c != BLACK, "{} {}", p.name, c.to_hex());
            }
        }
    }

    #[test]
    fn the_light_controls_are_their_tints_over_the_base() {
        // The mock draws light controls as white 70% and #F9F9F9 at 50%
        // and 30%; over the base those are the spec's solid values.
        let b = LIGHT.base;
        assert_eq!(Tint::alpha(WHITE, 0.70).over(b), LIGHT.control.rgb);
        assert_eq!(
            Tint::alpha(Rgb::hex(0xF9F9F9), 0.50).over(b),
            LIGHT.control_hover.rgb
        );
        assert_eq!(
            Tint::alpha(Rgb::hex(0xF9F9F9), 0.30).over(b),
            LIGHT.control_pressed.rgb
        );
    }

    #[test]
    fn both_easing_curves_run_through_their_control_points() {
        use motion::{CubicBezier, ACCELERATE, DECELERATE};
        for c in [DECELERATE, ACCELERATE] {
            assert_eq!(c.ease(0.0), 0.0);
            assert!((c.ease(1.0) - 1.0).abs() < 1e-5);
            // De Casteljau's midpoint of P0, P1, P2, P3.
            let lerp = |a: (f32, f32), b: (f32, f32)| (0.5 * (a.0 + b.0), 0.5 * (a.1 + b.1));
            let (p0, p1, p2, p3) = ((0.0, 0.0), (c.x1, c.y1), (c.x2, c.y2), (1.0, 1.0));
            let (a, b, d) = (lerp(p0, p1), lerp(p1, p2), lerp(p2, p3));
            let (e, f) = (lerp(a, b), lerp(b, d));
            let mid = lerp(e, f);
            assert!((c.ease(mid.0) - mid.1).abs() < 1e-4, "{c:?} at {mid:?}");
            // It leaves P0 toward the first control point that isn't P0,
            // and reaches P3 from the last that isn't P3.
            let start = if (c.x1, c.y1) != p0 { p1 } else { p2 };
            let end = if (c.x2, c.y2) != p3 { p2 } else { p1 };
            let s = c.point(1e-3);
            let cross = s.0 * start.1 - s.1 * start.0;
            assert!(cross.abs() < 1e-3, "{c:?} leaves off its tangent");
            let t = c.point(1.0 - 1e-3);
            let (u, v) = ((t.0 - 1.0, t.1 - 1.0), (end.0 - 1.0, end.1 - 1.0));
            assert!(
                (u.0 * v.1 - u.1 * v.0).abs() < 1e-3,
                "{c:?} arrives off its tangent"
            );
            // Never backwards.
            let mut last = 0.0;
            for i in 0..=100 {
                let y = c.ease(i as f32 / 100.0);
                assert!(y + 1e-5 >= last, "{c:?} goes back at {i}");
                last = y;
            }
            let _: CubicBezier = c;
        }
        // Decelerate is past halfway at a third of its time; accelerate
        // isn't.
        assert!(DECELERATE.ease(1.0 / 3.0) > 0.5);
        assert!(ACCELERATE.ease(1.0 / 3.0) < 0.2);
        assert_eq!(motion::HOVER.easing.ease(0.25), 0.25);
    }

    #[test]
    fn nothing_moves_with_animations_off() {
        for t in [
            motion::HOVER,
            motion::SMALL,
            motion::PANE,
            motion::EXIT,
            motion::EXPANDER,
        ] {
            assert_eq!(motion::duration(t.ms, false), 0);
            assert_eq!(motion::duration(t.ms, true), t.ms);
        }
    }

    #[test]
    fn the_optical_size_follows_the_point_size() {
        use type_ramp::*;
        assert_eq!(optical_family(CAPTION.size), SMALL_FAMILY);
        assert_eq!(optical_family(BODY.size), TEXT_FAMILY);
        assert_eq!(optical_family(19.0), TEXT_FAMILY);
        assert_eq!(optical_family(SUBTITLE.size), DISPLAY_FAMILY);
        assert_eq!(optical_family(TITLE.size), DISPLAY_FAMILY);
        assert!(TITLE.tabular && ALL.iter().filter(|s| s.tabular).count() == 1);
    }

    #[test]
    fn the_window_edge_closes_up_when_narrow() {
        assert_eq!(space::window_edge(599.0), 0.0);
        assert_eq!(space::window_edge(600.0), 16.0);
    }

    #[test]
    fn spreadsheets_and_subtitles_are_documents_and_unknown_is_other() {
        use kiln_core::Kind;
        assert_eq!(FileKind::of(Some(Kind::Spreadsheet)), FileKind::Document);
        assert_eq!(FileKind::of(Some(Kind::Subtitle)), FileKind::Document);
        assert_eq!(FileKind::of(None), FileKind::Other);
        assert_eq!(DARK.kind(FileKind::Video), Rgb::hex(0xA884F3));
        assert_eq!(LIGHT.kind(FileKind::Other), Rgb::hex(0x616161));
    }

    #[test]
    fn the_glyph_table_has_no_repeats() {
        let mut seen = std::collections::HashSet::new();
        for (name, g) in glyph::ALL {
            assert!(seen.insert(g), "{name} repeats {:04X}", g as u32);
            assert!(('\u{E700}'..='\u{F8FF}').contains(&g), "{name}");
        }
    }

    #[test]
    fn animation_off_makes_every_motion_instant() {
        use super::motion::*;
        for ms in [QUICK_MS, 160, 300] {
            assert_eq!(duration(ms, true), ms);
            assert_eq!(duration(ms, false), 0);
        }
        // A motion of no length is done at once, even at time zero.
        assert_eq!(progress(0.0, 0), 1.0);
        assert_eq!(progress(0.0, duration(QUICK_MS, false)), 1.0);
        assert_eq!(progress(0.0, QUICK_MS), 0.0);
        assert_eq!(progress(QUICK_MS as f32, QUICK_MS), 1.0);
    }

    #[test]
    fn motion_is_short_and_never_overshoots() {
        assert!((120..=180).contains(&motion::QUICK_MS));
        let mut last = 0.0;
        for i in 0..=100 {
            let v = motion::ease_out(i as f32 / 100.0);
            assert!(v >= last && v <= 1.0);
            last = v;
        }
        assert_eq!(motion::ease_out(1.0), 1.0);
    }

    #[test]
    fn spacing_sits_on_the_grid() {
        for v in [space::PANEL_PADDING, space::ICON_GRID] {
            assert_eq!(v % space::GRID_UNIT, 0.0, "{v}");
        }
        // Every step of the grid is a whole number of base steps.
        for v in space::GRID {
            assert_eq!(v % space::GRID_UNIT, 0.0, "{v}");
        }
    }
}
