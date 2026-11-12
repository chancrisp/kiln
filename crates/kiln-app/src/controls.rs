//! Phase 3's controls as pure models: what each does with a key, and the
//! colours each is drawn in for a state, from the Fluent tokens. Nothing
//! here draws or knows of a window; `win::render::controls` draws them and
//! `a11y` names them. The surfaces (Tasks 16 to 21) hold these states.
//!
//! The keyboard follows WinUI's controls: a ComboBox opens on Alt+Down,
//! F4, Enter or Space and steps its value with Up and Down even closed; a
//! ToggleSwitch flips on Space; RadioButtons move and pick with the arrows
//! and Tab leaves the group; an Expander opens and closes on Enter and
//! Space; a menu's Up and Down wrap round.

use crate::theme::fluent::{Edge, FluentPalette, Tint};
use crate::theme::Rgb;

/// A key the controls answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    Enter,
    Space,
    Esc,
    Tab,
    Backspace,
    Delete,
    F4,
    /// A typed character (`WM_CHAR`).
    Char(char),
}

/// A key with the modifiers held as it went down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Press {
    pub key: Key,
    pub alt: bool,
    pub ctrl: bool,
    pub shift: bool,
}

impl Press {
    pub const fn key(key: Key) -> Self {
        Press {
            key,
            alt: false,
            ctrl: false,
            shift: false,
        }
    }

    pub const fn alt(key: Key) -> Self {
        Press {
            alt: true,
            ..Press::key(key)
        }
    }

    pub const fn ctrl(key: Key) -> Self {
        Press {
            ctrl: true,
            ..Press::key(key)
        }
    }

    pub const fn shift(key: Key) -> Self {
        Press {
            shift: true,
            ..Press::key(key)
        }
    }

    /// No modifier at all.
    fn plain(self) -> bool {
        !self.alt && !self.ctrl && !self.shift
    }
}

// ---------------------------------------------------------------- Buttons

/// The four kinds of button the look spec names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ButtonKind {
    /// `control` fill and stroke, Body text.
    Standard,
    /// The accent fill and `on_accent` text: one per surface.
    Accent,
    /// Transparent until the pointer is over it: row actions, title-bar
    /// icons.
    Subtle,
    /// `accent_text`, underlined under the pointer only.
    Hyperlink,
}

/// How lit a control is: how far its hover and press have eased in, 0 to
/// 1, and whether it is disabled (which outranks both).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Lit {
    pub hover: f32,
    pub pressed: f32,
    pub disabled: bool,
}

impl Lit {
    pub const REST: Lit = Lit {
        hover: 0.0,
        pressed: 0.0,
        disabled: false,
    };
    pub const HOVER: Lit = Lit {
        hover: 1.0,
        pressed: 0.0,
        disabled: false,
    };
    pub const PRESSED: Lit = Lit {
        hover: 1.0,
        pressed: 1.0,
        disabled: false,
    };
    pub const DISABLED: Lit = Lit {
        hover: 0.0,
        pressed: 0.0,
        disabled: true,
    };

    /// Lit enough to show it: a contrast theme has no in-between, so it
    /// shows once more than half in.
    pub fn lit(self) -> bool {
        !self.disabled && (self.hover > 0.5 || self.pressed > 0.5)
    }
}

/// What a control is drawn in for one state: its fill, its 1-DIP stroke,
/// the stronger edge along one side (if it has one), its text and glyph
/// colour, and whether its text is underlined.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    pub fill: Tint,
    pub stroke: Tint,
    pub edge: Option<(Edge, Tint)>,
    pub ink: Rgb,
    pub underline: bool,
}

/// The accent button's own stroke over its fill, from the mock: white 8%
/// round it. Its stronger edge along the bottom is the palette's
/// `accent_edge`.
pub const ACCENT_STROKE: Tint = Tint::alpha(Rgb::hex(0xFFFFFF), 0.08);

/// `t` at `k` of its strength.
fn faded(t: Tint, k: f32) -> Tint {
    Tint::alpha(t.rgb, t.alpha * k.clamp(0.0, 1.0))
}

/// `a` moving toward `b`, `k` of the way: the fill a hover or press
/// eases through. Two tints of one colour (the dark theme's whites) blend
/// their alphas; two solid colours blend in linear light, so the middle
/// isn't darker than both ends; anything else takes the nearer.
fn toward(a: Tint, b: Tint, k: f32) -> Tint {
    let k = k.clamp(0.0, 1.0);
    if a.rgb == b.rgb {
        Tint::alpha(a.rgb, a.alpha + (b.alpha - a.alpha) * k)
    } else if a.alpha == 1.0 && b.alpha == 1.0 {
        Tint::solid(blend_linear(a.rgb, b.rgb, k))
    } else if k >= 0.5 {
        b
    } else {
        a
    }
}

/// `a` moving toward `b`, `k` of the way, in linear RGB.
fn blend_linear(a: Rgb, b: Rgb, k: f32) -> Rgb {
    fn to_linear(v: u8) -> f32 {
        let c = v as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }
    fn to_srgb(l: f32) -> u8 {
        let c = if l <= 0.003_130_8 {
            l * 12.92
        } else {
            1.055 * l.powf(1.0 / 2.4) - 0.055
        };
        (c * 255.0).round().clamp(0.0, 255.0) as u8
    }
    let mix = |x: u8, y: u8| {
        let (x, y) = (to_linear(x), to_linear(y));
        to_srgb(x + (y - x) * k)
    };
    Rgb(mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

/// A button of `kind` lit as `lit`, in `p`. Under a contrast theme a lit
/// button is Highlight with HighlightText, as Windows draws its own; a lit
/// accent button turns the other way round (HighlightText with a
/// Highlight edge), since at rest it is already Highlight.
pub fn button_look(kind: ButtonKind, lit: Lit, p: &FluentPalette) -> Look {
    if p.high_contrast {
        return contrast_button(kind, lit, p);
    }
    let (hover, pressed) = (lit.hover.clamp(0.0, 1.0), lit.pressed.clamp(0.0, 1.0));
    let clear = Tint::alpha(p.control_hover.rgb, 0.0);
    let plain = Look {
        fill: clear,
        stroke: Tint::alpha(p.control_stroke.rgb, 0.0),
        edge: None,
        ink: p.text,
        underline: false,
    };
    // A press dims the text, as the mock's `:active`.
    let ink = |rest: Rgb| {
        if pressed > 0.5 {
            p.text_secondary
        } else {
            rest
        }
    };
    match kind {
        ButtonKind::Standard | ButtonKind::Accent if lit.disabled => Look {
            fill: p.control,
            stroke: p.control_stroke,
            ink: p.text_disabled,
            ..plain
        },
        ButtonKind::Standard => Look {
            fill: toward(
                toward(p.control, p.control_hover, hover),
                p.control_pressed,
                pressed,
            ),
            stroke: p.control_stroke,
            // Pressed, the control sinks: its stronger edge goes.
            edge: Some((
                p.control_stroke_edge.0,
                faded(p.control_stroke_edge.1, 1.0 - pressed),
            )),
            ink: ink(p.control_text),
            underline: false,
        },
        ButtonKind::Accent => {
            let fill = crate::theme::mix(p.accent, p.accent_hover, hover * (1.0 - pressed));
            Look {
                // Pressed, it goes back to the accent and its edge sinks:
                // a lighter fill would take the light theme's text under
                // 4.5:1.
                fill: Tint::solid(fill),
                stroke: ACCENT_STROKE,
                edge: Some((Edge::Bottom, faded(p.accent_edge, 1.0 - pressed))),
                ink: p.on_accent,
                underline: false,
            }
        }
        ButtonKind::Subtle if lit.disabled => Look {
            ink: p.text_disabled,
            ..plain
        },
        ButtonKind::Subtle => Look {
            fill: toward(
                toward(clear, p.control_hover, hover),
                p.control_pressed,
                pressed,
            ),
            ink: ink(p.text),
            ..plain
        },
        ButtonKind::Hyperlink if lit.disabled => Look {
            ink: p.text_disabled,
            ..plain
        },
        ButtonKind::Hyperlink => Look {
            ink: if pressed > 0.5 {
                p.accent
            } else {
                p.accent_text
            },
            underline: hover > 0.5 || pressed > 0.5,
            ..plain
        },
    }
}

/// A button under a contrast theme, where nothing is in between: at rest
/// ButtonFace and ButtonText in a WindowText edge; under the pointer
/// Highlight with HighlightText in a Highlight edge (an accent button,
/// Highlight at rest, turns to HighlightText with Highlight text);
/// pressed Highlight in a WindowText edge; disabled in GrayText.
fn contrast_button(kind: ButtonKind, lit: Lit, p: &FluentPalette) -> Look {
    let clear = Tint::alpha(p.control.rgb, 0.0);
    let (hover, pressed) = (lit.hover > 0.5, lit.pressed > 0.5);
    let highlight = Tint::solid(p.accent);
    let look = |fill: Tint, stroke: Tint, ink: Rgb| Look {
        fill,
        stroke,
        edge: None,
        ink,
        underline: false,
    };
    match kind {
        ButtonKind::Hyperlink => Look {
            ink: if lit.disabled {
                p.text_disabled
            } else if pressed {
                p.accent
            } else {
                p.accent_text
            },
            underline: !lit.disabled && (hover || pressed),
            ..look(clear, clear, p.accent_text)
        },
        ButtonKind::Subtle if lit.disabled => look(clear, clear, p.text_disabled),
        _ if lit.disabled => look(p.control, Tint::solid(p.text_disabled), p.text_disabled),
        ButtonKind::Accent if pressed => look(highlight, p.control_hover_stroke, p.on_accent),
        ButtonKind::Accent if hover => {
            look(Tint::solid(p.on_accent), p.control_hover_stroke, p.accent)
        }
        ButtonKind::Accent => look(highlight, p.control_stroke, p.on_accent),
        _ if pressed => look(highlight, p.control_stroke, p.on_accent),
        _ if hover => look(highlight, p.control_hover_stroke, p.on_accent),
        ButtonKind::Subtle => look(clear, clear, p.text),
        ButtonKind::Standard => look(p.control, p.control_stroke, p.control_text),
    }
}

// ---------------------------------------------------------------- InfoBar

/// An InfoBar's kind: its fill, its glyph's colour and glyph, and the
/// word a screen reader says for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    Info,
    Success,
    Caution,
    Critical,
}

impl Severity {
    pub const ALL: [Severity; 4] = [
        Severity::Info,
        Severity::Success,
        Severity::Caution,
        Severity::Critical,
    ];

    pub fn glyph(self) -> char {
        use crate::theme::fluent::glyph;
        match self {
            Severity::Info => glyph::INFO,
            Severity::Success => glyph::CHECKMARK,
            Severity::Caution => glyph::WARNING,
            Severity::Critical => glyph::ERROR,
        }
    }

    /// What UI Automation gives as its status.
    pub fn word(self) -> &'static str {
        match self {
            Severity::Info => "Information",
            Severity::Success => "Success",
            Severity::Caution => "Warning",
            Severity::Critical => "Error",
        }
    }

    /// Its fill and its glyph's colour in `p`.
    pub fn colours(self, p: &FluentPalette) -> (Tint, Rgb) {
        match self {
            Severity::Info => (p.info_fill, p.info),
            Severity::Success => (Tint::solid(p.success_fill), p.success),
            Severity::Caution => (Tint::solid(p.caution_fill), p.caution),
            Severity::Critical => (Tint::solid(p.critical_fill), p.critical),
        }
    }
}

// ----------------------------------------------------------- ToggleSwitch

/// A ToggleSwitch: on or off. Space flips it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Toggle {
    pub on: bool,
}

impl Toggle {
    /// Answer `p`: true when the switch flipped.
    pub fn key(&mut self, p: Press) -> bool {
        if p.plain() && p.key == Key::Space {
            self.on = !self.on;
            return true;
        }
        false
    }

    /// The word shown beside the switch and said for it.
    pub fn word(self) -> &'static str {
        if self.on {
            "On"
        } else {
            "Off"
        }
    }
}

/// What a ToggleSwitch is drawn in: its track's fill and rim, its knob's
/// colour, and the word's colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToggleLook {
    pub track: Tint,
    pub rim: Tint,
    pub knob: Rgb,
    pub word: Rgb,
}

/// A switch `on` (the knob's side), lit `lit`.
pub fn toggle_look(on: bool, lit: Lit, p: &FluentPalette) -> ToggleLook {
    let (hover, pressed) = (lit.hover.clamp(0.0, 1.0), lit.pressed.clamp(0.0, 1.0));
    if lit.disabled {
        let gray = Tint::solid(p.text_disabled);
        return ToggleLook {
            track: if on { gray } else { p.control },
            rim: gray,
            knob: if on { p.base } else { p.text_disabled },
            word: p.text_disabled,
        };
    }
    if p.high_contrast {
        let rim = if lit.lit() {
            p.control_hover_stroke
        } else {
            p.control_stroke
        };
        return ToggleLook {
            track: if on { Tint::solid(p.accent) } else { p.control },
            rim,
            knob: match (on, lit.lit()) {
                (true, _) => p.on_accent,
                (false, true) => p.accent,
                (false, false) => p.control_text,
            },
            word: p.text,
        };
    }
    if on {
        let fill = crate::theme::mix(p.accent, p.accent_hover, hover * (1.0 - pressed));
        ToggleLook {
            track: Tint::solid(fill),
            rim: Tint::solid(fill),
            knob: p.on_accent,
            word: p.text,
        }
    } else {
        ToggleLook {
            track: toward(
                toward(p.control, p.control_hover, hover),
                p.control_pressed,
                pressed,
            ),
            rim: p.strong_stroke,
            knob: p.text_secondary,
            word: p.text,
        }
    }
}

/// The knob's diameter: 12 DIPs at rest, 14 under the pointer, as WinUI's.
pub fn knob_size(lit: Lit) -> f32 {
    if lit.disabled {
        return 12.0;
    }
    12.0 + 2.0 * lit.hover.max(lit.pressed).clamp(0.0, 1.0)
}

// ----------------------------------------------------------- RadioButtons

/// What a key did to a group of radio buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioEvent {
    Ignored,
    /// The focus moved to this one and picked it.
    Picked(usize),
    /// Tab (forward) or Shift+Tab (`back`) leaves the group.
    Leave {
        back: bool,
    },
}

/// A group of `len` radio buttons, `selected` the one picked (none yet,
/// at first), the focus on one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Radios {
    pub len: usize,
    pub selected: Option<usize>,
    pub focus: usize,
}

impl Radios {
    /// The focus comes in on the one picked, or the first.
    pub fn new(len: usize, selected: Option<usize>) -> Self {
        let selected = selected.filter(|&i| i < len);
        Radios {
            len,
            selected,
            focus: selected.unwrap_or(0),
        }
    }

    /// Up and Left go back, Down and Right forward, picking as they move;
    /// they stop at the ends. Space picks the focused one. Tab leaves.
    pub fn key(&mut self, p: Press) -> RadioEvent {
        if p.key == Key::Tab && !p.alt && !p.ctrl {
            return RadioEvent::Leave { back: p.shift };
        }
        if self.len == 0 || !p.plain() {
            return RadioEvent::Ignored;
        }
        let to = match p.key {
            Key::Up | Key::Left => self.focus.checked_sub(1),
            Key::Down | Key::Right => Some(self.focus + 1).filter(|&i| i < self.len),
            Key::Space => Some(self.focus),
            _ => None,
        };
        match to {
            Some(i) => {
                self.focus = i;
                self.selected = Some(i);
                RadioEvent::Picked(i)
            }
            None => RadioEvent::Ignored,
        }
    }
}

/// A radio button's circle: its fill, rim, the dot in it (none when not
/// picked) and the dot's diameter, and the label's colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RadioLook {
    pub fill: Tint,
    pub rim: Tint,
    pub dot: Option<Rgb>,
    pub dot_size: f32,
    pub label: Rgb,
}

/// A radio button `on` or not, lit `lit`.
pub fn radio_look(on: bool, lit: Lit, p: &FluentPalette) -> RadioLook {
    let (hover, pressed) = (lit.hover.clamp(0.0, 1.0), lit.pressed.clamp(0.0, 1.0));
    // The dot: 10 DIPs (the mock's), 12 under the pointer, 8 pressed.
    let dot_size = if on && !lit.disabled {
        10.0 + 2.0 * hover - 4.0 * pressed
    } else if on {
        10.0
    } else {
        0.0
    };
    if lit.disabled {
        let gray = Tint::solid(p.text_disabled);
        return RadioLook {
            fill: if on { gray } else { p.control },
            rim: gray,
            dot: on.then_some(p.base),
            dot_size,
            label: p.text_disabled,
        };
    }
    if p.high_contrast {
        let rim = if lit.lit() {
            p.control_hover_stroke
        } else {
            p.control_stroke
        };
        return RadioLook {
            fill: if on { Tint::solid(p.accent) } else { p.control },
            rim,
            dot: on.then_some(p.on_accent),
            dot_size,
            label: p.text,
        };
    }
    if on {
        let fill = Tint::solid(crate::theme::mix(
            p.accent,
            p.accent_hover,
            hover * (1.0 - pressed),
        ));
        RadioLook {
            fill,
            rim: fill,
            dot: Some(p.on_accent),
            dot_size,
            label: p.text,
        }
    } else {
        RadioLook {
            fill: toward(
                toward(p.control, p.control_hover, hover),
                p.control_pressed,
                pressed,
            ),
            rim: p.strong_stroke,
            dot: None,
            dot_size,
            label: p.text,
        }
    }
}

// --------------------------------------------------------------- Expander

/// An Expander: its header opens and closes its content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Expander {
    pub open: bool,
}

impl Expander {
    /// Enter and Space open and close it: true when it changed.
    pub fn key(&mut self, p: Press) -> bool {
        if p.plain() && matches!(p.key, Key::Enter | Key::Space) {
            self.open = !self.open;
            return true;
        }
        false
    }
}

// --------------------------------------------------------------- ComboBox

/// What a key did to a ComboBox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComboEvent {
    Ignored,
    Opened,
    /// Closed with the value it had: Esc, or Enter on the value already
    /// picked.
    Closed,
    /// The highlight moved in the open list.
    Moved(usize),
    /// The value changed to this choice (and the list, if open, closed).
    Changed(usize),
}

/// A ComboBox of `len` choices: the value picked, whether its list is
/// open, and the choice lit in the open list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComboState {
    pub len: usize,
    pub value: usize,
    pub open: bool,
    pub highlight: usize,
}

impl ComboState {
    pub fn new(len: usize, value: usize) -> Self {
        let value = value.min(len.saturating_sub(1));
        ComboState {
            len,
            value,
            open: false,
            highlight: value,
        }
    }

    /// Close the list, taking the highlight as the value.
    fn take(&mut self) -> ComboEvent {
        self.open = false;
        if self.highlight != self.value {
            self.value = self.highlight;
            ComboEvent::Changed(self.value)
        } else {
            ComboEvent::Closed
        }
    }

    fn open_list(&mut self) -> ComboEvent {
        self.open = true;
        self.highlight = self.value;
        ComboEvent::Opened
    }

    /// Close the list as it was.
    fn dismiss(&mut self) -> ComboEvent {
        self.open = false;
        self.highlight = self.value;
        ComboEvent::Closed
    }

    /// Closed: Alt+Down, Alt+Up, F4, Enter and Space open it; Up and Down
    /// step the value. Open: Up and Down move the highlight (stopping at
    /// the ends), Home and End go to the ends, Enter, Space, Alt+Up and
    /// Alt+Down pick the highlight and close, Esc and Tab close and keep
    /// the old value.
    pub fn key(&mut self, p: Press) -> ComboEvent {
        if self.len == 0 {
            return ComboEvent::Ignored;
        }
        let alt_arrow = p.alt && !p.ctrl && matches!(p.key, Key::Up | Key::Down);
        let opener = alt_arrow || (p.plain() && matches!(p.key, Key::F4 | Key::Enter | Key::Space));
        if !self.open {
            if opener {
                return self.open_list();
            }
            if !p.plain() {
                return ComboEvent::Ignored;
            }
            let to = match p.key {
                Key::Up => self.value.checked_sub(1),
                Key::Down => Some(self.value + 1).filter(|&i| i < self.len),
                _ => None,
            };
            return match to {
                Some(i) => {
                    self.value = i;
                    self.highlight = i;
                    ComboEvent::Changed(i)
                }
                None => ComboEvent::Ignored,
            };
        }
        if opener {
            return self.take();
        }
        if p.key == Key::Esc || (p.key == Key::Tab && !p.alt && !p.ctrl) {
            return self.dismiss();
        }
        if !p.plain() {
            return ComboEvent::Ignored;
        }
        let to = match p.key {
            Key::Up => self.highlight.checked_sub(1),
            Key::Down => Some(self.highlight + 1).filter(|&i| i < self.len),
            Key::Home => Some(0),
            Key::End => Some(self.len - 1),
            _ => None,
        };
        match to {
            Some(i) if i != self.highlight => {
                self.highlight = i;
                ComboEvent::Moved(i)
            }
            _ => ComboEvent::Ignored,
        }
    }

    /// A click on the box: opens the list, or closes it as it was.
    pub fn click(&mut self) -> ComboEvent {
        if self.len == 0 {
            ComboEvent::Ignored
        } else if self.open {
            self.dismiss()
        } else {
            self.open_list()
        }
    }

    /// A click on choice `i` of the open list.
    pub fn pick(&mut self, i: usize) -> ComboEvent {
        if !self.open || i >= self.len {
            return ComboEvent::Ignored;
        }
        self.highlight = i;
        self.take()
    }
}

// ---------------------------------------------------------------- TextBox

/// What a text box takes: at most `max_len` characters, digits only when
/// `digits` (and one decimal point with them when `point`), and never a
/// control character. UI Automation's Value pattern checks a value
/// against it as the box would (`TextBoxState::set`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextRule {
    pub max_len: usize,
    pub digits: bool,
    pub point: bool,
}

impl TextRule {
    /// Whether `s`, whole, is a text the box takes.
    pub fn allows(self, s: &str) -> bool {
        let number = |c: char| c.is_ascii_digit() || (self.point && c == '.');
        let points = s.chars().filter(|&c| c == '.').count();
        let digits_ok = !self.digits || (s.chars().all(number) && points <= 1);
        digits_ok && s.chars().count() <= self.max_len && !s.chars().any(char::is_control)
    }
}

/// A one-line text box: its text, the caret and the selection's other
/// end (both in characters), its longest, and whether it takes digits
/// only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextBoxState {
    text: String,
    caret: usize,
    anchor: usize,
    pub max_len: usize,
    pub digits: bool,
    /// With `digits`, one decimal point too: a size in MB.
    pub point: bool,
}

impl TextBoxState {
    /// A box holding `text` with the caret at its end.
    pub fn new(text: &str, max_len: usize, digits: bool) -> Self {
        let text: String = text.chars().take(max_len).collect();
        let end = text.chars().count();
        TextBoxState {
            text,
            caret: end,
            anchor: end,
            max_len,
            digits,
            point: false,
        }
    }

    /// A box for a size, "2.5": digits and one decimal point.
    pub fn size(text: &str, max_len: usize) -> Self {
        TextBoxState {
            point: true,
            ..TextBoxState::new(text, max_len, true)
        }
    }

    fn len(&self) -> usize {
        self.text.chars().count()
    }

    /// What it takes.
    pub fn rule(&self) -> TextRule {
        TextRule {
            max_len: self.max_len,
            digits: self.digits,
            point: self.point,
        }
    }

    /// The byte offset of character `i`.
    fn byte(&self, i: usize) -> usize {
        self.text
            .char_indices()
            .nth(i)
            .map_or(self.text.len(), |(b, _)| b)
    }

    /// Take out characters `r`, leaving the caret where they were.
    fn cut(&mut self, r: std::ops::Range<usize>) {
        let (a, b) = (self.byte(r.start), self.byte(r.end));
        self.text.replace_range(a..b, "");
        self.caret = r.start;
        self.anchor = r.start;
    }

    /// `s` in place of the selection, as much of it as fits; nothing at
    /// all if the box takes digits and `s` has anything else. True when
    /// the text changed.
    fn insert(&mut self, s: &str) -> bool {
        let before = self.text.clone();
        let sel = self.selection();
        if self.digits {
            // What the text would be with all of `s` in: a number the box
            // takes, length aside.
            let (a, b) = (self.byte(sel.start), self.byte(sel.end));
            let mut would = before.clone();
            would.replace_range(a..b, s);
            let rule = TextRule {
                max_len: usize::MAX,
                ..self.rule()
            };
            if !rule.allows(&would) {
                return false;
            }
        }
        let room = self.max_len.saturating_sub(self.len() - sel.len());
        let taken: String = s.chars().filter(|c| !c.is_control()).take(room).collect();
        if taken.is_empty() && sel.is_empty() {
            return false;
        }
        self.cut(sel);
        let at = self.byte(self.caret);
        self.text.insert_str(at, &taken);
        self.caret += taken.chars().count();
        self.anchor = self.caret;
        self.text != before
    }

    /// Move the caret to `to`; with `extend`, the selection's other end
    /// stays where it was.
    fn go(&mut self, to: usize, extend: bool) {
        self.caret = to.min(self.len());
        if !extend {
            self.anchor = self.caret;
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// The caret, in characters from the start.
    pub fn caret(&self) -> usize {
        self.caret
    }

    /// The selection, start to end in characters; empty when there is
    /// none.
    pub fn selection(&self) -> std::ops::Range<usize> {
        self.caret.min(self.anchor)..self.caret.max(self.anchor)
    }

    /// Answer a key: typing, Backspace, Delete, Left, Right, Home and End
    /// (with Shift, selecting), and Ctrl+A. True when the text changed.
    pub fn key(&mut self, p: Press) -> bool {
        let sel = self.selection();
        match p.key {
            Key::Char(c) if p.ctrl && !p.alt => {
                if c.eq_ignore_ascii_case(&'a') {
                    self.anchor = 0;
                    self.caret = self.len();
                }
                false
            }
            Key::Char(c) if c.is_control() => false,
            Key::Char(c) => self.insert(c.encode_utf8(&mut [0; 4])),
            Key::Backspace | Key::Delete if !sel.is_empty() => {
                self.cut(sel);
                true
            }
            Key::Backspace if self.caret > 0 => {
                self.cut(self.caret - 1..self.caret);
                true
            }
            Key::Delete if self.caret < self.len() => {
                self.cut(self.caret..self.caret + 1);
                true
            }
            Key::Left if !p.shift && !sel.is_empty() => {
                self.go(sel.start, false);
                false
            }
            Key::Right if !p.shift && !sel.is_empty() => {
                self.go(sel.end, false);
                false
            }
            Key::Left => {
                self.go(self.caret.saturating_sub(1), p.shift);
                false
            }
            Key::Right => {
                self.go(self.caret + 1, p.shift);
                false
            }
            Key::Home => {
                self.go(0, p.shift);
                false
            }
            Key::End => {
                self.go(self.len(), p.shift);
                false
            }
            _ => false,
        }
    }

    /// Paste `s` over the selection: refused whole when the box takes
    /// digits and `s` holds anything else, cut at the box's length
    /// otherwise. True when the text changed.
    pub fn paste(&mut self, s: &str) -> bool {
        self.insert(s)
    }

    /// Set the whole text, as UI Automation's Value pattern does: refused
    /// when the box takes digits and `s` has anything else, or when `s`
    /// is longer than the box. True when it was taken.
    pub fn set(&mut self, s: &str) -> bool {
        if !self.rule().allows(s) {
            return false;
        }
        self.text = s.to_owned();
        self.caret = self.len();
        self.anchor = self.caret;
        true
    }
}

// ------------------------------------------------------------ Menu flyout

/// What a key did to an open menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuEvent {
    Ignored,
    Moved(usize),
    /// Do item `i`; the menu closes.
    Act(usize),
    Close,
}

/// An open menu flyout: which of its items can be used, and which is lit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuState {
    enabled: Vec<bool>,
    pub highlight: Option<usize>,
}

impl MenuState {
    /// A menu opened from the keyboard lights its first usable item; one
    /// opened by the pointer lights nothing until a key or the pointer
    /// moves.
    pub fn new(enabled: Vec<bool>, by_keyboard: bool) -> Self {
        let first = enabled.iter().position(|&e| e);
        MenuState {
            enabled,
            highlight: if by_keyboard { first } else { None },
        }
    }

    /// Up and Down move through the usable items and wrap round; Home and
    /// End go to the first and last; Enter and Space do the lit item; Esc
    /// closes.
    pub fn key(&mut self, p: Press) -> MenuEvent {
        if p.key == Key::Esc {
            return MenuEvent::Close;
        }
        if !p.plain() {
            return MenuEvent::Ignored;
        }
        let usable: Vec<usize> = (0..self.enabled.len())
            .filter(|&i| self.enabled[i])
            .collect();
        let (Some(&first), Some(&last)) = (usable.first(), usable.last()) else {
            return MenuEvent::Ignored;
        };
        let at = self
            .highlight
            .and_then(|h| usable.iter().position(|&i| i == h));
        let n = usable.len();
        let to = match p.key {
            Key::Enter | Key::Space => {
                return self.highlight.map_or(MenuEvent::Ignored, MenuEvent::Act);
            }
            Key::Down => at.map_or(first, |k| usable[(k + 1) % n]),
            Key::Up => at.map_or(last, |k| usable[(k + n - 1) % n]),
            Key::Home => first,
            Key::End => last,
            _ => return MenuEvent::Ignored,
        };
        if self.highlight == Some(to) {
            return MenuEvent::Ignored;
        }
        self.highlight = Some(to);
        MenuEvent::Moved(to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::fluent::{contrast_palette, ALL_BUILT_IN, DARK, LIGHT};
    use crate::theme::SysColors;

    fn hc() -> FluentPalette {
        contrast_palette(SysColors {
            window: Rgb::hex(0x000000),
            window_text: Rgb::hex(0xFFFFFF),
            highlight: Rgb::hex(0x1AEBFF),
            highlight_text: Rgb::hex(0x000000),
            gray_text: Rgb::hex(0x3FF23F),
            hotlight: Rgb::hex(0xFFFF00),
            button_face: Rgb::hex(0x101010),
            button_text: Rgb::hex(0xF0F0F0),
        })
    }

    const K: fn(Key) -> Press = Press::key;

    // ---- ComboBox

    #[test]
    fn a_combo_opens_on_alt_down_f4_enter_and_space() {
        for p in [
            Press::alt(Key::Down),
            Press::alt(Key::Up),
            K(Key::F4),
            K(Key::Enter),
            K(Key::Space),
        ] {
            let mut c = ComboState::new(4, 2);
            assert_eq!(c.key(p), ComboEvent::Opened, "{p:?}");
            assert!(c.open);
            assert_eq!(c.highlight, 2, "the list opens on the value");
        }
    }

    #[test]
    fn up_and_down_step_a_closed_combo_and_stop_at_the_ends() {
        let mut c = ComboState::new(3, 1);
        assert_eq!(c.key(K(Key::Down)), ComboEvent::Changed(2));
        assert_eq!(c.value, 2);
        assert_eq!(c.key(K(Key::Down)), ComboEvent::Ignored);
        assert_eq!(c.key(K(Key::Up)), ComboEvent::Changed(1));
        assert_eq!(c.key(K(Key::Up)), ComboEvent::Changed(0));
        assert_eq!(c.key(K(Key::Up)), ComboEvent::Ignored);
        assert!(!c.open);
    }

    #[test]
    fn up_and_down_move_the_open_lists_highlight_and_enter_picks_it() {
        let mut c = ComboState::new(4, 1);
        c.key(K(Key::Enter));
        assert_eq!(c.key(K(Key::Down)), ComboEvent::Moved(2));
        assert_eq!(c.key(K(Key::Down)), ComboEvent::Moved(3));
        assert_eq!(c.key(K(Key::Down)), ComboEvent::Ignored);
        assert_eq!(c.value, 1, "moving in the list keeps the value");
        assert_eq!(c.key(K(Key::Home)), ComboEvent::Moved(0));
        assert_eq!(c.key(K(Key::End)), ComboEvent::Moved(3));
        assert_eq!(c.key(K(Key::Up)), ComboEvent::Moved(2));
        assert_eq!(c.key(K(Key::Enter)), ComboEvent::Changed(2));
        assert_eq!((c.value, c.open), (2, false));
        // Enter on the value already picked just closes.
        c.key(K(Key::Space));
        assert_eq!(c.key(K(Key::Enter)), ComboEvent::Closed);
        assert!(!c.open);
        // Alt+Up picks too.
        c.key(Press::alt(Key::Down));
        c.key(K(Key::Down));
        assert_eq!(c.key(Press::alt(Key::Up)), ComboEvent::Changed(3));
    }

    #[test]
    fn esc_and_tab_close_the_list_and_keep_the_old_value() {
        for p in [K(Key::Esc), K(Key::Tab), Press::shift(Key::Tab)] {
            let mut c = ComboState::new(4, 1);
            c.key(K(Key::Enter));
            c.key(K(Key::Down));
            c.key(K(Key::Down));
            assert_eq!(c.key(p), ComboEvent::Closed, "{p:?}");
            assert_eq!((c.value, c.open), (1, false));
        }
        // Closed, Esc and Tab are not the combo's.
        let mut c = ComboState::new(4, 1);
        assert_eq!(c.key(K(Key::Esc)), ComboEvent::Ignored);
        assert_eq!(c.key(K(Key::Tab)), ComboEvent::Ignored);
    }

    #[test]
    fn a_combo_answers_clicks() {
        let mut c = ComboState::new(3, 0);
        assert_eq!(c.click(), ComboEvent::Opened);
        assert_eq!(c.pick(2), ComboEvent::Changed(2));
        assert!(!c.open);
        c.click();
        assert_eq!(c.click(), ComboEvent::Closed);
        assert_eq!(c.value, 2);
        c.click();
        assert_eq!(c.pick(9), ComboEvent::Ignored, "no such choice");
        assert_eq!(c.pick(2), ComboEvent::Closed);
        // An empty combo does nothing, and a value past the end is held
        // to the last choice.
        let mut e = ComboState::new(0, 0);
        assert_eq!(e.key(K(Key::Enter)), ComboEvent::Ignored);
        assert_eq!(e.click(), ComboEvent::Ignored);
        assert_eq!(ComboState::new(3, 7).value, 2);
    }

    // ---- ToggleSwitch

    #[test]
    fn space_flips_a_toggle_switch_and_it_says_on_or_off() {
        let mut t = Toggle::default();
        assert_eq!(t.word(), "Off");
        assert!(t.key(K(Key::Space)));
        assert!(t.on);
        assert_eq!(t.word(), "On");
        for p in [
            K(Key::Enter),
            K(Key::Left),
            K(Key::Tab),
            Press::ctrl(Key::Space),
        ] {
            assert!(!t.key(p), "{p:?}");
        }
        assert!(t.key(K(Key::Space)));
        assert_eq!(t.word(), "Off");
    }

    #[test]
    fn a_toggle_is_the_accent_when_on_and_its_knob_grows_under_the_pointer() {
        for p in ALL_BUILT_IN {
            let off = toggle_look(false, Lit::REST, &p);
            let on = toggle_look(true, Lit::REST, &p);
            assert_eq!(on.track, Tint::solid(p.accent));
            assert_eq!(on.knob, p.on_accent);
            assert_eq!(off.rim, p.strong_stroke);
            assert_eq!(off.knob, p.text_secondary);
            let dis = toggle_look(true, Lit::DISABLED, &p);
            assert_eq!(dis.word, p.text_disabled);
        }
        assert_eq!(knob_size(Lit::REST), 12.0);
        assert_eq!(knob_size(Lit::HOVER), 14.0);
        assert_eq!(knob_size(Lit::DISABLED), 12.0);
    }

    // ---- RadioButtons

    #[test]
    fn arrows_move_and_pick_a_radio_and_tab_leaves() {
        let mut r = Radios::new(3, None);
        assert_eq!((r.focus, r.selected), (0, None));
        assert_eq!(r.key(K(Key::Down)), RadioEvent::Picked(1));
        assert_eq!(r.key(K(Key::Right)), RadioEvent::Picked(2));
        assert_eq!(r.key(K(Key::Down)), RadioEvent::Ignored, "no wrap");
        assert_eq!(r.key(K(Key::Up)), RadioEvent::Picked(1));
        assert_eq!(r.key(K(Key::Left)), RadioEvent::Picked(0));
        assert_eq!(r.key(K(Key::Left)), RadioEvent::Ignored);
        assert_eq!(r.selected, Some(0));
        assert_eq!(r.key(K(Key::Tab)), RadioEvent::Leave { back: false });
        assert_eq!(
            r.key(Press::shift(Key::Tab)),
            RadioEvent::Leave { back: true }
        );
        // Focus comes in on the one picked; Space picks the focused one.
        let mut r = Radios::new(3, Some(2));
        assert_eq!(r.focus, 2);
        let mut none = Radios::new(2, None);
        assert_eq!(none.key(K(Key::Space)), RadioEvent::Picked(0));
        assert_eq!(r.key(K(Key::Space)), RadioEvent::Picked(2));
        assert_eq!(Radios::new(0, None).key(K(Key::Down)), RadioEvent::Ignored);
    }

    #[test]
    fn a_picked_radio_is_an_accent_disc_with_a_dot() {
        for p in ALL_BUILT_IN {
            let on = radio_look(true, Lit::REST, &p);
            assert_eq!(on.fill, Tint::solid(p.accent));
            assert_eq!(on.dot, Some(p.on_accent));
            let off = radio_look(false, Lit::REST, &p);
            assert_eq!(off.dot, None);
            assert_eq!(off.rim, p.strong_stroke);
            assert!(radio_look(true, Lit::HOVER, &p).dot_size > on.dot_size);
            assert!(radio_look(true, Lit::PRESSED, &p).dot_size < on.dot_size);
            assert_eq!(radio_look(false, Lit::DISABLED, &p).label, p.text_disabled);
        }
    }

    // ---- Expander

    #[test]
    fn enter_and_space_open_and_close_an_expander() {
        let mut e = Expander::default();
        assert!(e.key(K(Key::Enter)));
        assert!(e.open);
        assert!(e.key(K(Key::Space)));
        assert!(!e.open);
        assert!(!e.key(K(Key::Down)));
        assert!(!e.key(Press::alt(Key::Enter)));
    }

    // ---- TextBox

    #[test]
    fn a_digit_box_takes_typing_and_refuses_letters() {
        let mut t = TextBoxState::new("", 4, true);
        for c in "1a2".chars() {
            t.key(K(Key::Char(c)));
        }
        assert_eq!(t.text(), "12");
        assert_eq!(t.caret(), 2);
        // A letter box takes anything.
        let mut any = TextBoxState::new("", 10, false);
        assert!(any.key(K(Key::Char('a'))));
        assert_eq!(any.text(), "a");
        // Control characters are never typed.
        assert!(!any.key(K(Key::Char('\u{8}'))));
    }

    #[test]
    fn backspace_delete_and_the_caret_keys_edit_where_the_caret_is() {
        let mut t = TextBoxState::new("1234", 8, true);
        assert!(t.key(K(Key::Backspace)));
        assert_eq!(t.text(), "123");
        t.key(K(Key::Home));
        assert_eq!(t.caret(), 0);
        assert!(!t.key(K(Key::Backspace)), "nothing before the caret");
        assert!(t.key(K(Key::Delete)));
        assert_eq!(t.text(), "23");
        t.key(K(Key::Right));
        assert_eq!(t.caret(), 1);
        t.key(K(Key::Char('9')));
        assert_eq!(t.text(), "293");
        t.key(K(Key::End));
        assert_eq!(t.caret(), 3);
        assert!(!t.key(K(Key::Delete)), "nothing after the caret");
        t.key(K(Key::Left));
        t.key(K(Key::Left));
        assert_eq!(t.caret(), 1);
        t.key(K(Key::Home));
        t.key(K(Key::Left));
        assert_eq!(t.caret(), 0, "stops at the start");
    }

    #[test]
    fn shift_selects_and_typing_or_deleting_replaces_the_selection() {
        let mut t = TextBoxState::new("12345", 8, true);
        t.key(Press::shift(Key::Left));
        t.key(Press::shift(Key::Left));
        assert_eq!(t.selection(), 3..5);
        assert!(t.key(K(Key::Char('7'))));
        assert_eq!(t.text(), "1237");
        t.key(Press::shift(Key::Home));
        assert_eq!(t.selection(), 0..4);
        assert!(t.key(K(Key::Delete)));
        assert_eq!(t.text(), "");
        // Left with a selection goes to its start and drops it.
        let mut t = TextBoxState::new("123", 8, true);
        t.key(Press::shift(Key::Left));
        t.key(K(Key::Left));
        assert_eq!((t.caret(), t.selection()), (2, 2..2));
    }

    #[test]
    fn ctrl_a_selects_everything() {
        let mut t = TextBoxState::new("2048", 8, true);
        t.key(K(Key::Home));
        assert!(!t.key(Press::ctrl(Key::Char('a'))));
        assert_eq!(t.selection(), 0..4);
        t.key(K(Key::Char('5')));
        assert_eq!(t.text(), "5");
        // Ctrl with another letter types nothing.
        assert!(!t.key(Press::ctrl(Key::Char('b'))));
        assert_eq!(t.text(), "5");
    }

    #[test]
    fn a_paste_of_digits_is_taken_and_one_with_a_letter_is_refused() {
        let mut t = TextBoxState::new("", 4, true);
        assert!(t.paste("15"));
        assert_eq!(t.text(), "15");
        assert!(!t.paste("1a5"));
        assert_eq!(t.text(), "15");
        // Cut at the box's length.
        assert!(t.paste("6789"));
        assert_eq!(t.text(), "1567");
        assert_eq!(t.caret(), 4);
        // A full box takes no more typing.
        assert!(!t.key(K(Key::Char('1'))));
        assert_eq!(t.text(), "1567");
    }

    #[test]
    fn a_size_box_takes_digits_and_one_decimal_point() {
        let mut t = TextBoxState::size("", 6);
        for c in "2.5".chars() {
            assert!(t.key(K(Key::Char(c))));
        }
        assert_eq!(t.text(), "2.5");
        // A second point, a letter or a comma is ignored.
        assert!(!t.key(K(Key::Char('.'))));
        assert!(!t.key(K(Key::Char('a'))));
        assert!(!t.key(K(Key::Char(','))));
        assert_eq!(t.text(), "2.5");
        assert!(!t.paste("1.2"));
        assert!(t.paste("0"));
        assert_eq!(t.text(), "2.50");
        // UI Automation's value follows the same rule.
        assert!(t.rule().allows("12.75"));
        assert!(!t.rule().allows("1.2.3"));
        assert!(!t.rule().allows("1e3"));
        assert!(t.set(".5"));
        assert!(!t.set("5.."));
        assert_eq!(t.text(), ".5");
        // A digits-only box still takes no point.
        let mut d = TextBoxState::new("", 4, true);
        assert!(!d.key(K(Key::Char('.'))));
        assert!(!d.rule().allows("1.5"));
    }

    #[test]
    fn setting_the_value_follows_the_paste_rules() {
        let mut t = TextBoxState::new("10", 3, true);
        assert!(t.set("250"));
        assert_eq!((t.text(), t.caret()), ("250", 3));
        assert!(!t.set("2x"));
        assert_eq!(t.text(), "250");
        assert!(!t.set("1234"), "longer than the box");
        assert!(t.set(""));
        assert_eq!(t.text(), "");
        // The box starts no longer than it may be.
        assert_eq!(TextBoxState::new("123456", 3, true).text(), "123");
    }

    // ---- Menu flyout

    #[test]
    fn a_menus_arrows_wrap_and_skip_what_cant_be_used() {
        let mut m = MenuState::new(vec![true, false, true, true], true);
        assert_eq!(m.highlight, Some(0));
        assert_eq!(m.key(K(Key::Down)), MenuEvent::Moved(2), "skips 1");
        assert_eq!(m.key(K(Key::Down)), MenuEvent::Moved(3));
        assert_eq!(m.key(K(Key::Down)), MenuEvent::Moved(0), "wraps");
        assert_eq!(m.key(K(Key::Up)), MenuEvent::Moved(3), "wraps back");
        assert_eq!(m.key(K(Key::Home)), MenuEvent::Moved(0));
        assert_eq!(m.key(K(Key::End)), MenuEvent::Moved(3));
        assert_eq!(m.key(K(Key::Enter)), MenuEvent::Act(3));
        assert_eq!(m.key(K(Key::Esc)), MenuEvent::Close);
    }

    #[test]
    fn a_menu_opened_by_the_pointer_lights_nothing_until_a_key() {
        let mut m = MenuState::new(vec![false, true], false);
        assert_eq!(m.highlight, None);
        assert_eq!(m.key(K(Key::Enter)), MenuEvent::Ignored);
        assert_eq!(m.key(K(Key::Down)), MenuEvent::Moved(1));
        assert_eq!(m.key(K(Key::Space)), MenuEvent::Act(1));
        let mut up = MenuState::new(vec![true, true, false], false);
        assert_eq!(
            up.key(K(Key::Up)),
            MenuEvent::Moved(1),
            "Up starts at the end"
        );
        // Nothing usable: arrows do nothing.
        let mut dead = MenuState::new(vec![false, false], true);
        assert_eq!(dead.highlight, None);
        assert_eq!(dead.key(K(Key::Down)), MenuEvent::Ignored);
        assert_eq!(dead.key(K(Key::Esc)), MenuEvent::Close);
    }

    // ---- Looks

    #[test]
    fn buttons_follow_the_spec_at_rest() {
        for p in ALL_BUILT_IN {
            let s = button_look(ButtonKind::Standard, Lit::REST, &p);
            assert_eq!(s.fill, p.control);
            assert_eq!(s.stroke, p.control_stroke);
            assert_eq!(s.edge, Some(p.control_stroke_edge));
            assert_eq!(s.ink, p.control_text);
            let a = button_look(ButtonKind::Accent, Lit::REST, &p);
            assert_eq!(a.fill, Tint::solid(p.accent));
            assert_eq!(a.ink, p.on_accent);
            let sub = button_look(ButtonKind::Subtle, Lit::REST, &p);
            assert_eq!(sub.fill.alpha, 0.0, "subtle is clear at rest");
            assert_eq!(sub.stroke.alpha, 0.0);
            let link = button_look(ButtonKind::Hyperlink, Lit::REST, &p);
            assert_eq!(link.ink, p.accent_text);
            assert!(!link.underline);
            assert!(button_look(ButtonKind::Hyperlink, Lit::HOVER, &p).underline);
        }
    }

    #[test]
    fn hover_and_press_change_every_button_in_every_theme() {
        for p in ALL_BUILT_IN.into_iter().chain([hc()]) {
            for kind in [
                ButtonKind::Standard,
                ButtonKind::Accent,
                ButtonKind::Subtle,
                ButtonKind::Hyperlink,
            ] {
                let rest = button_look(kind, Lit::REST, &p);
                let hover = button_look(kind, Lit::HOVER, &p);
                let pressed = button_look(kind, Lit::PRESSED, &p);
                assert_ne!(rest, hover, "{} {kind:?} hover", p.name);
                assert_ne!(hover, pressed, "{} {kind:?} press", p.name);
            }
        }
    }

    #[test]
    fn hover_eases_between_rest_and_lit() {
        let half = Lit {
            hover: 0.5,
            ..Lit::REST
        };
        let s = button_look(ButtonKind::Subtle, half, &DARK);
        assert!((s.fill.alpha - DARK.control_hover.alpha * 0.5).abs() < 1e-6);
        let s = button_look(ButtonKind::Standard, half, &DARK);
        let mid = (DARK.control.alpha + DARK.control_hover.alpha) / 2.0;
        assert!((s.fill.alpha - mid).abs() < 1e-6);
    }

    #[test]
    fn a_contrast_theme_lights_buttons_in_highlight_and_disables_in_gray_text() {
        let p = hc();
        let s = button_look(ButtonKind::Standard, Lit::HOVER, &p);
        assert_eq!(s.fill, Tint::solid(p.accent), "Highlight");
        assert_eq!(s.ink, p.on_accent, "HighlightText");
        let rest = button_look(ButtonKind::Standard, Lit::REST, &p);
        assert_eq!(rest.fill, p.control);
        assert_eq!(rest.ink, p.control_text);
        let a = button_look(ButtonKind::Accent, Lit::HOVER, &p);
        assert_eq!(a.fill, Tint::solid(p.on_accent));
        assert_eq!(a.ink, p.accent);
        assert_eq!(a.stroke, p.control_hover_stroke);
        for kind in [ButtonKind::Standard, ButtonKind::Accent, ButtonKind::Subtle] {
            let d = button_look(kind, Lit::DISABLED, &p);
            assert_eq!(d.ink, p.text_disabled, "{kind:?}");
        }
        // Nothing in the contrast theme is see-through, bar a clear subtle
        // button at rest.
        for kind in [ButtonKind::Standard, ButtonKind::Accent] {
            for lit in [Lit::REST, Lit::HOVER, Lit::PRESSED, Lit::DISABLED] {
                let l = button_look(kind, lit, &p);
                assert!(
                    l.fill.alpha == 1.0 && l.stroke.alpha == 1.0,
                    "{kind:?} {lit:?}"
                );
            }
        }
    }

    #[test]
    fn a_contrast_theme_shows_hover_on_toggles_and_radios() {
        let p = hc();
        for on in [false, true] {
            assert_ne!(
                toggle_look(on, Lit::REST, &p),
                toggle_look(on, Lit::HOVER, &p),
                "toggle {on}"
            );
            assert_ne!(
                radio_look(on, Lit::REST, &p),
                radio_look(on, Lit::HOVER, &p),
                "radio {on}"
            );
        }
        assert_eq!(
            toggle_look(false, Lit::HOVER, &p).rim,
            p.control_hover_stroke
        );
    }

    #[test]
    fn a_hover_between_two_solid_colours_blends_in_linear_light() {
        let (dark, light) = (Tint::solid(Rgb(0, 0, 0)), Tint::solid(Rgb(255, 255, 255)));
        assert_eq!(toward(dark, light, 0.0), dark);
        assert_eq!(toward(dark, light, 1.0), light);
        // Half way in linear light is 188 an sRGB channel, not 128.
        assert_eq!(toward(dark, light, 0.5), Tint::solid(Rgb(188, 188, 188)));
        // It runs smoothly, not as a jump at the middle.
        let mut last = 0;
        for i in 0..=10 {
            let v = toward(dark, light, i as f32 / 10.0).rgb.0;
            assert!(v >= last);
            last = v;
        }
        let (a, b) = (Rgb(0x2D, 0x2E, 0x27), Rgb(0x35, 0x36, 0x2E));
        let mid = toward(Tint::solid(a), Tint::solid(b), 0.5).rgb;
        assert!(a.0 < mid.0 && mid.0 < b.0, "{mid:?}");
        // One colour with two strengths blends its strength, as before; a
        // see-through tint against a solid still takes the nearer.
        let w = Rgb(255, 255, 255);
        assert_eq!(
            toward(Tint::alpha(w, 0.0), Tint::alpha(w, 0.1), 0.5),
            Tint::alpha(w, 0.05)
        );
        let clear = Tint::alpha(Rgb(0, 0, 0), 0.0);
        assert_eq!(toward(clear, light, 0.4), clear);
        assert_eq!(toward(clear, light, 0.6), light);
    }

    #[test]
    fn the_accent_button_shows_the_same_lip_as_the_buttons_beside_it() {
        // light's accent edge is 24% black, where 14% over jade
        // vanished beside a standard button's 16% lip; dark is as it was.
        assert_eq!(LIGHT.accent_edge, Tint::alpha(Rgb::hex(0x000000), 0.24));
        assert_eq!(DARK.accent_edge, Tint::alpha(Rgb::hex(0x000000), 0.14));
        for p in ALL_BUILT_IN {
            let look = button_look(ButtonKind::Accent, Lit::REST, &p);
            assert_eq!(look.edge, Some((Edge::Bottom, p.accent_edge)));
            // Pressed, the lip sinks as a standard button's does.
            let pressed = button_look(ButtonKind::Accent, Lit::PRESSED, &p);
            assert_eq!(pressed.edge.map(|(_, t)| t.alpha), Some(0.0));
        }
        // The lip over the light accent is as dark, against the face, as
        // a standard button's over its own.
        let lip = |fill: Rgb, t: Tint| crate::theme::contrast(t.over(fill), fill);
        let accent = lip(LIGHT.accent, LIGHT.accent_edge);
        let standard = lip(LIGHT.control.rgb, LIGHT.control_stroke_edge.1);
        assert!(
            accent >= 1.15 && standard >= 1.15,
            "accent {accent}, standard {standard}"
        );
    }

    #[test]
    fn a_text_boxs_sides_are_stronger_in_light_so_an_empty_one_is_a_field() {
        // 12% black on the sides in light; dark keeps its 7% white.
        assert_eq!(LIGHT.field_stroke, Tint::alpha(Rgb::hex(0x000000), 0.12));
        assert_eq!(DARK.field_stroke, DARK.control_stroke);
    }

    #[test]
    fn an_accent_buttons_text_holds_its_contrast_in_every_state() {
        for p in ALL_BUILT_IN {
            for bg in [p.base, p.layer.solid, p.card.solid, p.overlay] {
                for lit in [Lit::REST, Lit::HOVER, Lit::PRESSED] {
                    let l = button_look(ButtonKind::Accent, lit, &p);
                    let ratio = crate::theme::contrast(l.ink, l.fill.over(bg));
                    assert!(ratio >= 4.5, "{} {lit:?} on {bg:?}: {ratio}", p.name);
                }
            }
        }
    }

    #[test]
    fn disabled_controls_use_the_disabled_text() {
        for p in ALL_BUILT_IN {
            for kind in [ButtonKind::Standard, ButtonKind::Accent, ButtonKind::Subtle] {
                let d = button_look(kind, Lit::DISABLED, &p);
                assert_eq!(d.ink, p.text_disabled, "{} {kind:?}", p.name);
                // Disabled outranks hover.
                let both = Lit {
                    hover: 1.0,
                    ..Lit::DISABLED
                };
                assert_eq!(button_look(kind, both, &p), d);
            }
        }
    }
}
