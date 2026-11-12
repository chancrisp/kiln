//! DirectWrite: the type ramp's fonts as this Windows has them, the icon
//! font, and the text formats and layouts the app draws with.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use windows::core::{Interface, Result, BOOL, HSTRING};
use windows::Win32::Graphics::DirectWrite::*;

use super::render::cache::{self, Laid, LayoutKey, Layouts};
use crate::theme::fluent::type_ramp::{self, TextStyle};

pub struct Text {
    pub factory: IDWriteFactory,
    /// Wrapping monospace for engine output in the details dialog, the
    /// only text not in Segoe UI Variable (look spec, Type).
    pub mono_block: IDWriteTextFormat,
    /// Figures of one width each, so numbers that change don't jiggle.
    pub tabular: IDWriteTypography,
    /// The Fluent type ramp's and glyphs' fonts and formats (`format`,
    /// `styled`, `glyph`).
    pub(in crate::win) fluent: Fluent,
    /// The layouts made so far, by words, style and box: texts are drawn
    /// and measured again each frame.
    layouts: RefCell<Layouts>,
}

thread_local! {
    /// The text formats `shared` gave out on this thread, with the factory
    /// they were made with.
    static SHARED: RefCell<Option<(IDWriteFactory, Rc<Text>)>> = const { RefCell::new(None) };
}

impl Text {
    pub fn new() -> Result<Self> {
        let factory: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
        Self::with_factory(factory)
    }

    /// The text formats made with `factory`, made once per thread and
    /// factory, so a surface rebuilt often doesn't make them again.
    pub fn shared(factory: &IDWriteFactory) -> Result<Rc<Self>> {
        SHARED.with(|cell| {
            if let Some((f, text)) = &*cell.borrow() {
                if f == factory {
                    return Ok(text.clone());
                }
            }
            let text = Rc::new(Self::with_factory(factory.clone())?);
            *cell.borrow_mut() = Some((factory.clone(), text.clone()));
            Ok(text)
        })
    }

    /// The formats, made with `factory`.
    pub fn with_factory(factory: IDWriteFactory) -> Result<Self> {
        let mono_block = mono_block(&factory)?;
        // SAFETY: plain calls on this thread's factory and the typography
        // just made.
        let tabular = unsafe {
            let t = factory.CreateTypography()?;
            t.AddFontFeature(DWRITE_FONT_FEATURE {
                nameTag: DWRITE_FONT_FEATURE_TAG_TABULAR_FIGURES,
                parameter: 1,
            })?;
            t
        };
        Ok(Self {
            mono_block,
            tabular,
            layouts: RefCell::new(cache::layouts()),
            fluent: Fluent::new(&factory),
            factory,
        })
    }

    /// How many layouts are kept.
    #[cfg(all(test, debug_assertions))]
    pub fn cached_layouts(&self) -> usize {
        self.layouts.borrow().len()
    }

    /// How many glyph formats are kept.
    #[cfg(all(test, debug_assertions))]
    pub fn cached_glyph_formats(&self) -> usize {
        self.fluent.glyphs.borrow().len()
    }

    /// The sizes glyph formats are kept for, smallest first.
    #[cfg(test)]
    pub fn glyph_sizes(&self) -> Vec<f32> {
        let mut sizes: Vec<f32> = self
            .fluent
            .glyphs
            .borrow()
            .keys()
            .map(|&k| f32::from_bits(k))
            .collect();
        sizes.sort_by(f32::total_cmp);
        sizes
    }

    /// Forget the layouts made so far.
    pub fn clear_caches(&self) {
        self.layouts.borrow_mut().clear();
        self.fluent.hearts.borrow_mut().clear();
    }
}

/// The details dialog's engine output: the first of the look's mono
/// families Windows has, at `MONO_SIZE`, wrapping from the top of its box.
fn mono_block(factory: &IDWriteFactory) -> Result<IDWriteTextFormat> {
    let mut collection = None;
    // SAFETY: a plain read of the system font collection.
    let _ = unsafe { factory.GetSystemFontCollection(&mut collection, false) };
    let family = type_ramp::MONO
        .iter()
        .copied()
        .find(|f| has_family(collection.as_ref(), f))
        .unwrap_or(type_ramp::MONO[type_ramp::MONO.len() - 1]);
    // SAFETY: plain calls on this thread's factory and the format just
    // made.
    unsafe {
        let f = factory.CreateTextFormat(
            &HSTRING::from(family),
            None,
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            MONO_SIZE,
            &HSTRING::from("en-us"),
        )?;
        f.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
        f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR)?;
        Ok(f)
    }
}

/// The details log's size in DIPs.
const MONO_SIZE: f32 = 13.0;

impl Text {
    /// Height of `s` wrapped to `width` in `format`, for scrolling it.
    pub fn block_height(&self, s: &str, format: &IDWriteTextFormat, width: f32) -> f32 {
        let wide: Vec<u16> = s.encode_utf16().collect();
        // SAFETY: plain calls on this thread's factory and the layout just
        // made; the metrics are a local.
        unsafe {
            let Ok(layout) = self
                .factory
                .CreateTextLayout(&wide, format, width.max(1.0), 1e6)
            else {
                return 0.0;
            };
            let mut m = DWRITE_TEXT_METRICS::default();
            if layout.GetMetrics(&mut m).is_err() {
                return 0.0;
            }
            m.height
        }
    }
}

/// Segoe UI Variable in its three optical sizes, or Segoe UI where Windows
/// doesn't have them (Windows 10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UiFamily {
    Variable,
    SegoeUi,
}

impl UiFamily {
    /// The family to set text of `size` DIPs on screen in.
    pub fn family(self, size: f32) -> &'static str {
        match self {
            UiFamily::Variable => type_ramp::optical_family(size),
            UiFamily::SegoeUi => type_ramp::UI_FALLBACK,
        }
    }
}

/// Segoe UI Variable if `available` says Windows has all three of its
/// optical sizes, else Segoe UI.
pub fn pick_ui_family(available: impl Fn(&str) -> bool) -> UiFamily {
    let all = [
        type_ramp::SMALL_FAMILY,
        type_ramp::TEXT_FAMILY,
        type_ramp::DISPLAY_FAMILY,
    ]
    .iter()
    .all(|f| available(f));
    if all {
        UiFamily::Variable
    } else {
        UiFamily::SegoeUi
    }
}

/// Segoe Fluent Icons (Windows 11), or Segoe MDL2 Assets (Windows 10). Both
/// have every glyph Kiln draws at the same code point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IconFamily {
    Fluent,
    Mdl2,
}

impl IconFamily {
    pub fn name(self) -> &'static str {
        match self {
            IconFamily::Fluent => type_ramp::ICON_FAMILIES[0],
            IconFamily::Mdl2 => type_ramp::ICON_FAMILIES[1],
        }
    }
}

/// Segoe Fluent Icons if `available` says Windows has it, else Segoe MDL2
/// Assets.
pub fn pick_icon_family(available: impl Fn(&str) -> bool) -> IconFamily {
    if available(IconFamily::Fluent.name()) {
        IconFamily::Fluent
    } else {
        IconFamily::Mdl2
    }
}

/// A step of the look spec's type ramp. Until the surfaces are set in it
/// (Tasks 15 to 21), only the debug type page uses every step, so a
/// release build would call some unused.
#[cfg_attr(not(debug_assertions), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Ramp {
    Caption,
    Body,
    BodyStrong,
    Subtitle,
    Title,
}

impl Ramp {
    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    pub const ALL: [Ramp; 5] = [
        Ramp::Caption,
        Ramp::Body,
        Ramp::BodyStrong,
        Ramp::Subtitle,
        Ramp::Title,
    ];

    pub fn style(self) -> TextStyle {
        match self {
            Ramp::Caption => type_ramp::CAPTION,
            Ramp::Body => type_ramp::BODY,
            Ramp::BodyStrong => type_ramp::BODY_STRONG,
            Ramp::Subtitle => type_ramp::SUBTITLE,
            Ramp::Title => type_ramp::TITLE,
        }
    }
}

/// Where a line sits across its box (unused in release, as [`Ramp`]).
#[cfg_attr(not(debug_assertions), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Align {
    Leading,
    Center,
    Trailing,
}

/// A ramp step as it is set: across its box, on one line (trimmed with an
/// ellipsis and centred on the box's height) or wrapping from the top, and
/// as plain text or a format code (Semibold, uppercase, tracked).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Style {
    pub ramp: Ramp,
    pub align: Align,
    pub wrap: bool,
    pub code: bool,
}

impl Style {
    pub const fn new(ramp: Ramp) -> Self {
        Style {
            ramp,
            align: Align::Leading,
            wrap: false,
            code: false,
        }
    }

    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    pub const fn centered(self) -> Self {
        Style {
            align: Align::Center,
            ..self
        }
    }

    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    pub const fn trailing(self) -> Self {
        Style {
            align: Align::Trailing,
            ..self
        }
    }

    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    pub const fn wrapping(self) -> Self {
        Style { wrap: true, ..self }
    }

    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    pub const fn code(self) -> Self {
        Style { code: true, ..self }
    }
}

/// A style as it comes out at a Text size: its size, line height and
/// tracking in DIPs on screen, and the family, weight and figures it is
/// set in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    pub size: f32,
    pub line: f32,
    pub family: &'static str,
    pub weight: u16,
    pub tabular: bool,
    pub tracking: f32,
}

/// `style` at Text size zoom `zoom` (the layout's, `layout::text_zoom`)
/// with `ui`'s families. Every size grows with the zoom, and the optical
/// size is the one for the size on screen: a 12-DIP caption at 200% is set
/// in Display. Formats are made in layout units, which the zoom scales, so
/// a format's own size is `size / zoom`.
pub fn metrics(style: Style, ui: UiFamily, zoom: f32) -> Metrics {
    let zoom = if zoom.is_finite() { zoom.max(1.0) } else { 1.0 };
    let s = style.ramp.style();
    let size = s.size * zoom;
    Metrics {
        size,
        line: s.line * zoom,
        family: ui.family(size),
        weight: if style.code {
            type_ramp::CODE_WEIGHT
        } else {
            s.weight
        },
        tabular: s.tabular,
        tracking: if style.code {
            type_ramp::CODE_TRACKING_EM * size
        } else {
            0.0
        },
    }
}

/// Where the baseline sits from a line's top, for a font of `size` whose
/// ascent and descent are `em` (in ems) set in lines `line` tall: the
/// font's height centred in the line, as XAML sets it.
pub fn baseline(line: f32, size: f32, em: (f32, f32)) -> f32 {
    let (ascent, descent) = (em.0 * size, em.1 * size);
    (line - ascent - descent) / 2.0 + ascent
}

/// The ascent and descent, in ems, of `family`'s regular face in
/// `collection`.
fn em_metrics(collection: &IDWriteFontCollection, family: &str) -> Option<(f32, f32)> {
    // SAFETY: plain reads of this thread's system font collection; the
    // metrics are a local.
    unsafe {
        let mut index = 0;
        let mut exists = BOOL(0);
        collection
            .FindFamilyName(&HSTRING::from(family), &mut index, &mut exists)
            .ok()?;
        if !exists.as_bool() {
            return None;
        }
        let font = collection
            .GetFontFamily(index)
            .ok()?
            .GetFirstMatchingFont(
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
            )
            .ok()?;
        let mut m = DWRITE_FONT_METRICS::default();
        font.GetMetrics(&mut m);
        let em = f32::from(m.designUnitsPerEm.max(1));
        Some((f32::from(m.ascent) / em, f32::from(m.descent) / em))
    }
}

/// Whether Windows has `family`, by its system font collection.
fn has_family(collection: Option<&IDWriteFontCollection>, family: &str) -> bool {
    let Some(c) = collection else {
        return false;
    };
    let mut index = 0;
    let mut exists = BOOL(0);
    // SAFETY: a plain lookup in this thread's system font collection.
    let found = unsafe { c.FindFamilyName(&HSTRING::from(family), &mut index, &mut exists) };
    found.is_ok() && exists.as_bool()
}

/// The colour emoji font.
pub const EMOJI_FAMILY: &str = "Segoe UI Emoji";

/// Segoe UI's ascent and descent in ems, for when a family's can't be read.
const SEGOE_EM: (f32, f32) = (2210.0 / 2048.0, 514.0 / 2048.0);

/// How many glyph sizes keep their formats at once. Glyphs are asked for
/// at their tokens only (10 for the caption buttons, 12 for chevrons and
/// Dismiss, 16, 20); the title bar's, which don't grow with Text size, are
/// laid out at their token and scaled by a transform. The bound is a
/// guard should a size that isn't a token ever be asked for.
pub(in crate::win) const GLYPH_SIZES: usize = 6;

/// The fonts the type ramp and the glyphs are set in, as this Windows has
/// them, with the formats made from them so far.
pub(in crate::win) struct Fluent {
    pub ui: UiFamily,
    pub icons: IconFamily,
    /// Each family's ascent and descent in ems, by name.
    em: HashMap<&'static str, (f32, f32)>,
    /// The formats made so far by style and family: a few dozen at most,
    /// each kept for the life of the `Text`, so a layout keyed by its
    /// format's address can never meet another format at that address.
    formats: RefCell<HashMap<(Style, &'static str), IDWriteTextFormat>>,
    /// Glyph formats by size, kept the same way, for at most
    /// [`GLYPH_SIZES`] sizes: past that they start again, and the layouts
    /// with them.
    glyphs: RefCell<HashMap<u32, IDWriteTextFormat>>,
    /// Whether Windows has Segoe UI Emoji, the colour emoji font the
    /// credit's heart is drawn in.
    pub emoji: bool,
    /// The credit's heart laid out in it, by Text size zoom.
    hearts: RefCell<HashMap<u32, Laid>>,
}

impl Fluent {
    fn new(factory: &IDWriteFactory) -> Self {
        let mut collection = None;
        // SAFETY: a plain read of the system font collection.
        let _ = unsafe { factory.GetSystemFontCollection(&mut collection, false) };
        let has = |f: &str| has_family(collection.as_ref(), f);
        let ui = pick_ui_family(has);
        let icons = pick_icon_family(has);
        let emoji = has(EMOJI_FAMILY);
        let mut em = HashMap::new();
        let families = [
            type_ramp::SMALL_FAMILY,
            type_ramp::TEXT_FAMILY,
            type_ramp::DISPLAY_FAMILY,
            type_ramp::UI_FALLBACK,
            icons.name(),
        ];
        if let Some(c) = &collection {
            for f in families {
                if let Some(m) = em_metrics(c, f) {
                    em.insert(f, m);
                }
            }
        }
        Fluent {
            ui,
            icons,
            em,
            formats: RefCell::new(HashMap::new()),
            glyphs: RefCell::new(HashMap::new()),
            emoji,
            hearts: RefCell::new(HashMap::new()),
        }
    }

    fn em(&self, family: &str) -> (f32, f32) {
        self.em.get(family).copied().unwrap_or(SEGOE_EM)
    }
}

/// A format with `family` at `size` DIPs (layout units) and `weight`, one
/// line high `line`, its baseline centring the font in the line.
fn ramp_format(
    factory: &IDWriteFactory,
    family: &str,
    weight: u16,
    size: f32,
    line: f32,
    em: (f32, f32),
) -> Result<IDWriteTextFormat> {
    // SAFETY: plain calls on this thread's factory and the format just
    // made.
    unsafe {
        let f = factory.CreateTextFormat(
            &HSTRING::from(family),
            None,
            DWRITE_FONT_WEIGHT(i32::from(weight)),
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            &HSTRING::from("en-us"),
        )?;
        f.SetLineSpacing(
            DWRITE_LINE_SPACING_METHOD_UNIFORM,
            line,
            baseline(line, size, em),
        )?;
        Ok(f)
    }
}

impl Text {
    /// The UI family this Windows has.
    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    pub fn ui_family(&self) -> UiFamily {
        self.fluent.ui
    }

    /// The icon font this Windows has.
    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    pub fn icon_family(&self) -> IconFamily {
        self.fluent.icons
    }

    /// The format `style` is set in at Text size zoom `zoom`: made once
    /// for each style and optical size.
    pub fn format(&self, style: Style, zoom: f32) -> Result<IDWriteTextFormat> {
        let m = metrics(style, self.fluent.ui, zoom);
        let key = (style, m.family);
        if let Some(f) = self.fluent.formats.borrow().get(&key) {
            return Ok(f.clone());
        }
        let s = style.ramp.style();
        let em = self.fluent.em(m.family);
        let f = ramp_format(&self.factory, m.family, m.weight, s.size, s.line, em)?;
        // SAFETY: plain setters on the format just made.
        unsafe {
            f.SetTextAlignment(match style.align {
                Align::Leading => DWRITE_TEXT_ALIGNMENT_LEADING,
                Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                Align::Trailing => DWRITE_TEXT_ALIGNMENT_TRAILING,
            })?;
            if style.wrap {
                f.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
                f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR)?;
            } else {
                f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
                f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
                let sign = self.factory.CreateEllipsisTrimmingSign(&f)?;
                let trimming = DWRITE_TRIMMING {
                    granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                    delimiter: 0,
                    delimiterCount: 0,
                };
                f.SetTrimming(&trimming, &sign)?;
            }
        }
        self.fluent.formats.borrow_mut().insert(key, f.clone());
        Ok(f)
    }

    /// `s` in `style` at Text size zoom `zoom`, in a box `w` x `h` (layout
    /// units), and how wide it runs: a code in capitals and tracked, a
    /// Title with figures of one width. Made once for the same words,
    /// style, optical size and box.
    pub fn styled(&self, s: &str, style: Style, zoom: f32, w: f32, h: f32) -> Result<Laid> {
        self.styled_runs(s, style, zoom, w, h, &[])
    }

    /// `styled`, with the byte ranges `strong` of `s` set in Body
    /// Strong's weight (Semibold), as a line names a button: made once
    /// for the same words, style, box and runs.
    pub fn styled_runs(
        &self,
        s: &str,
        style: Style,
        zoom: f32,
        w: f32,
        h: f32,
        strong: &[std::ops::Range<usize>],
    ) -> Result<Laid> {
        let format = self.format(style, zoom)?;
        let (w, h) = (w.max(0.0), h.max(0.0));
        // In UTF-16, as DirectWrite counts; a range that isn't on `s`'s
        // characters is left out.
        let wide_at = |at: usize| s.get(..at).map(|p| p.encode_utf16().count() as u32);
        let runs: Vec<(u32, u32)> = strong
            .iter()
            .filter_map(|r| {
                let (a, b) = (wide_at(r.start)?, wide_at(r.end)?);
                (b > a).then_some((a, b - a))
            })
            .collect();
        let key = LayoutKey::styled_runs(s, &format, w, h, style, &runs);
        if let Some(laid) = self.layouts.borrow_mut().get(&key) {
            return Ok(laid.clone());
        }
        let words = if style.code {
            s.to_uppercase()
        } else {
            s.to_owned()
        };
        let wide: Vec<u16> = words.encode_utf16().collect();
        let range = DWRITE_TEXT_RANGE {
            startPosition: 0,
            length: wide.len() as u32,
        };
        let size = style.ramp.style().size;
        // SAFETY: plain calls on this thread's factory and the layout just
        // made; the metrics are a local.
        let laid = unsafe {
            let layout = self.factory.CreateTextLayout(&wide, &format, w, h)?;
            if style.ramp.style().tabular {
                layout.SetTypography(&self.tabular, range)?;
            }
            if style.code {
                let tracking = type_ramp::CODE_TRACKING_EM * size;
                if let Ok(l1) = layout.cast::<IDWriteTextLayout1>() {
                    l1.SetCharacterSpacing(0.0, tracking, 0.0, range)?;
                }
            }
            for &(start, length) in &runs {
                let run = DWRITE_TEXT_RANGE {
                    startPosition: start,
                    length,
                };
                layout.SetFontWeight(DWRITE_FONT_WEIGHT_SEMI_BOLD, run)?;
            }
            let mut m = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut m)?;
            Laid {
                layout,
                width: m.widthIncludingTrailingWhitespace,
            }
        };
        self.layouts.borrow_mut().insert(key, laid.clone());
        Ok(laid)
    }

    /// How tall `s` runs in `style`, wrapped to `w`: one line of the
    /// style's height when it can't be laid out.
    pub fn styled_height(&self, s: &str, style: Style, zoom: f32, w: f32) -> f32 {
        self.styled_height_runs(s, style, zoom, w, &[])
    }

    /// `styled_height`, with `strong` set as `styled_runs` sets it.
    pub fn styled_height_runs(
        &self,
        s: &str,
        style: Style,
        zoom: f32,
        w: f32,
        strong: &[std::ops::Range<usize>],
    ) -> f32 {
        let line = style.ramp.style().line;
        let Ok(laid) = self.styled_runs(s, style.wrapping(), zoom, w, 100_000.0, strong) else {
            return line;
        };
        let mut m = DWRITE_TEXT_METRICS::default();
        // SAFETY: a plain query of a layout this thread made, into a local.
        match unsafe { laid.layout.GetMetrics(&mut m) } {
            Ok(()) => m.height.max(line),
            Err(_) => line,
        }
    }

    /// How many lines `s` takes in `style` wrapped to `w`: one when it
    /// can't be laid out.
    pub fn line_count(&self, s: &str, style: Style, zoom: f32, w: f32) -> usize {
        let Ok(laid) = self.styled(s, style.wrapping(), zoom, w, 100_000.0) else {
            return 1;
        };
        let mut m = DWRITE_TEXT_METRICS::default();
        // SAFETY: a plain query of a layout this thread made, into a local.
        match unsafe { laid.layout.GetMetrics(&mut m) } {
            Ok(()) => (m.lineCount as usize).max(1),
            Err(_) => 1,
        }
    }

    /// `s` as it is drawn in at most `lines` lines of `style` wrapped to
    /// `w`: whole when it fits, otherwise cut as late as it can be with an
    /// ellipsis (U+2026) ending the last line. DirectWrite's own trimming
    /// only cuts a line that doesn't wrap, so the cut is made here.
    pub fn fit_lines(&self, s: &str, style: Style, zoom: f32, w: f32, lines: usize) -> String {
        if self.line_count(s, style, zoom, w) <= lines {
            return s.to_owned();
        }
        let chars: Vec<(usize, char)> = s.char_indices().collect();
        let cut = |n: usize| {
            let end = chars.get(n).map_or(s.len(), |&(at, _)| at);
            format!("{}\u{2026}", s[..end].trim_end())
        };
        // The most characters that still fit with the ellipsis after them.
        let (mut lo, mut hi) = (0, chars.len());
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if self.line_count(&cut(mid), style, zoom, w) <= lines {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        cut(lo)
    }

    /// The credit's heart in Caption's size and line, set in Segoe UI
    /// Emoji to be drawn in colour, at Text size zoom `zoom`. `None` where
    /// Windows has no Segoe UI Emoji: the plain heart is drawn instead.
    pub fn heart(&self, zoom: f32) -> Result<Option<Laid>> {
        if !self.fluent.emoji {
            return Ok(None);
        }
        let key = zoom.to_bits();
        if let Some(l) = self.fluent.hearts.borrow().get(&key) {
            return Ok(Some(l.clone()));
        }
        let style = Style::new(Ramp::Caption);
        let format = self.format(style, zoom)?;
        let wide: Vec<u16> = crate::credit::HEART.encode_utf16().collect();
        let line = style.ramp.style().line;
        // SAFETY: plain calls on this thread's factory and the layout just
        // made; the metrics are a local.
        let laid = unsafe {
            let layout = self
                .factory
                .CreateTextLayout(&wide, &format, 4096.0, line)?;
            let all = DWRITE_TEXT_RANGE {
                startPosition: 0,
                length: wide.len() as u32,
            };
            layout.SetFontFamilyName(&HSTRING::from(EMOJI_FAMILY), all)?;
            let mut m = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut m)?;
            Laid {
                layout,
                width: m.widthIncludingTrailingWhitespace,
            }
        };
        self.fluent.hearts.borrow_mut().insert(key, laid.clone());
        Ok(Some(laid))
    }

    /// The icon font at `size` DIPs, its em square one line, centred in a
    /// box of that size: made once for each size, of the last
    /// [`GLYPH_SIZES`] or so.
    pub fn glyph_format(&self, size: f32) -> Result<IDWriteTextFormat> {
        let key = size.to_bits();
        if let Some(f) = self.fluent.glyphs.borrow().get(&key) {
            return Ok(f.clone());
        }
        let family = self.fluent.icons.name();
        let f = ramp_format(
            &self.factory,
            family,
            type_ramp::REGULAR,
            size,
            size,
            self.fluent.em(family),
        )?;
        // SAFETY: plain setters on the format just made.
        unsafe {
            f.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        }
        let mut glyphs = self.fluent.glyphs.borrow_mut();
        if glyphs.len() >= GLYPH_SIZES {
            // Too many sizes: start again. The layouts go with the formats,
            // as they are keyed by a format's address, which a new format
            // may take.
            glyphs.clear();
            self.layouts.borrow_mut().clear();
        }
        glyphs.insert(key, f.clone());
        Ok(f)
    }

    /// Glyph `code` of the icon font, `size` DIPs, laid out in a box that
    /// size: made once for each glyph and size.
    pub fn glyph(&self, code: char, size: f32) -> Result<Laid> {
        let format = self.glyph_format(size)?;
        let mut buf = [0u16; 2];
        let wide = code.encode_utf16(&mut buf);
        let s = code.to_string();
        let key = LayoutKey::glyph(&s, &format, size, size);
        if let Some(laid) = self.layouts.borrow_mut().get(&key) {
            return Ok(laid.clone());
        }
        // SAFETY: plain calls on this thread's factory and the layout just
        // made; the metrics are a local.
        let laid = unsafe {
            let layout = self.factory.CreateTextLayout(wide, &format, size, size)?;
            let mut m = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut m)?;
            Laid {
                layout,
                width: m.widthIncludingTrailingWhitespace,
            }
        };
        self.layouts.borrow_mut().insert(key, laid.clone());
        Ok(laid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::fluent::type_ramp;

    fn only<'a>(names: &'a [&'a str]) -> impl Fn(&str) -> bool + 'a {
        move |f| names.contains(&f)
    }

    #[test]
    fn segoe_ui_variable_where_windows_has_it_else_segoe_ui() {
        let win11 = [
            "Segoe UI",
            "Segoe UI Variable Small",
            "Segoe UI Variable Text",
            "Segoe UI Variable Display",
        ];
        assert_eq!(pick_ui_family(only(&win11)), UiFamily::Variable);
        assert_eq!(pick_ui_family(only(&["Segoe UI"])), UiFamily::SegoeUi);
        // One optical size missing is no Variable: all three or none.
        let partial = ["Segoe UI", "Segoe UI Variable Text"];
        assert_eq!(pick_ui_family(only(&partial)), UiFamily::SegoeUi);
        assert_eq!(pick_ui_family(only(&[])), UiFamily::SegoeUi);

        assert_eq!(UiFamily::Variable.family(12.0), "Segoe UI Variable Small");
        assert_eq!(UiFamily::Variable.family(14.0), "Segoe UI Variable Text");
        assert_eq!(UiFamily::Variable.family(28.0), "Segoe UI Variable Display");
        for size in [12.0, 14.0, 20.0, 28.0] {
            assert_eq!(UiFamily::SegoeUi.family(size), "Segoe UI");
        }
    }

    /// Whether the system font `family` has a glyph for `c`; `None` where
    /// this Windows doesn't have the family.
    fn family_has(family: &str, c: char) -> Option<bool> {
        let factory: IDWriteFactory =
            unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).unwrap() };
        let mut collection = None;
        // SAFETY: a plain read of the system font collection.
        unsafe {
            factory
                .GetSystemFontCollection(&mut collection, false)
                .ok()?
        };
        let collection = collection?;
        let mut index = 0;
        let mut exists = BOOL(0);
        // SAFETY: plain lookups in the collection just read.
        unsafe {
            collection
                .FindFamilyName(&HSTRING::from(family), &mut index, &mut exists)
                .ok()?;
            if !exists.as_bool() {
                return None;
            }
            let font = collection
                .GetFontFamily(index)
                .ok()?
                .GetFirstMatchingFont(
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                )
                .ok()?;
            Some(font.HasCharacter(c as u32).ok()?.as_bool())
        }
    }

    #[test]
    fn every_glyph_settings_draws_is_in_both_icon_fonts() {
        // the cards' own glyphs must exist in Segoe Fluent Icons and,
        // for Windows 10, at the same code point in Segoe MDL2 Assets.
        for family in type_ramp::ICON_FAMILIES {
            for (name, c) in crate::settings_page::glyph::ALL {
                if let Some(has) = family_has(family, c) {
                    assert!(has, "{name} {:04X} is not in {family}", c as u32);
                }
            }
        }
    }

    #[test]
    fn fluent_icons_where_windows_has_them_else_mdl2() {
        let win11 = ["Segoe Fluent Icons", "Segoe MDL2 Assets"];
        assert_eq!(pick_icon_family(only(&win11)), IconFamily::Fluent);
        assert_eq!(
            pick_icon_family(only(&["Segoe MDL2 Assets"])),
            IconFamily::Mdl2
        );
        // Neither: MDL2 is named anyway, and DirectWrite falls back.
        assert_eq!(pick_icon_family(only(&[])), IconFamily::Mdl2);
        assert_eq!(IconFamily::Fluent.name(), "Segoe Fluent Icons");
        assert_eq!(IconFamily::Mdl2.name(), "Segoe MDL2 Assets");
    }

    #[test]
    fn each_style_scales_with_text_size() {
        for ramp in Ramp::ALL {
            let s = ramp.style();
            for zoom in [1.0, 1.25, 1.5, 2.0, 2.25] {
                let m = metrics(Style::new(ramp), UiFamily::Variable, zoom);
                assert_eq!(m.size, s.size * zoom, "{} size at {zoom}", s.name);
                assert_eq!(m.line, s.line * zoom, "{} line at {zoom}", s.name);
                // The optical size follows the size on screen.
                assert_eq!(m.family, type_ramp::optical_family(s.size * zoom));
                assert_eq!(m.weight, s.weight);
            }
        }
        // Caption is Small at 100% and Display at 200% Text size.
        let caption = Style::new(Ramp::Caption);
        assert_eq!(
            metrics(caption, UiFamily::Variable, 1.0).family,
            "Segoe UI Variable Small"
        );
        assert_eq!(
            metrics(caption, UiFamily::Variable, 1.25).family,
            "Segoe UI Variable Text"
        );
        assert_eq!(
            metrics(caption, UiFamily::Variable, 2.0).family,
            "Segoe UI Variable Display"
        );
        assert_eq!(metrics(caption, UiFamily::SegoeUi, 2.0).family, "Segoe UI");
        // Below 100% (no such Text size) it stays at 100%.
        assert_eq!(metrics(caption, UiFamily::Variable, 0.5).size, 12.0);
        assert_eq!(metrics(caption, UiFamily::Variable, f32::NAN).size, 12.0);
    }

    #[test]
    fn the_ramp_is_the_look_specs() {
        let sizes: Vec<_> = Ramp::ALL
            .iter()
            .map(|r| (r.style().size, r.style().line))
            .collect();
        assert_eq!(
            sizes,
            [
                (12.0, 16.0),
                (14.0, 20.0),
                (14.0, 20.0),
                (20.0, 28.0),
                (28.0, 36.0)
            ]
        );
        assert_eq!(Ramp::BodyStrong.style().weight, type_ramp::SEMIBOLD);
    }

    #[test]
    fn tabular_figures_are_on_for_title_only() {
        for ramp in Ramp::ALL {
            let m = metrics(Style::new(ramp), UiFamily::Variable, 1.0);
            assert_eq!(m.tabular, ramp == Ramp::Title, "{ramp:?}");
        }
        // And a Title layout carries the feature.
        let text = Text::new().unwrap();
        let laid = text
            .styled("12%", Style::new(Ramp::Title), 1.0, 200.0, 40.0)
            .unwrap();
        assert!(has_tabular(&laid.layout));
        let body = text
            .styled("12%", Style::new(Ramp::Body), 1.0, 200.0, 40.0)
            .unwrap();
        assert!(!has_tabular(&body.layout));
    }

    fn has_tabular(layout: &IDWriteTextLayout) -> bool {
        // SAFETY: plain reads of a layout made on this thread.
        unsafe {
            let mut t = None;
            if layout.GetTypography(0, &mut t, None).is_err() {
                return false;
            }
            let Some(t) = t else {
                return false;
            };
            (0..t.GetFontFeatureCount()).any(|i| {
                t.GetFontFeature(i)
                    .is_ok_and(|f| f.nameTag == DWRITE_FONT_FEATURE_TAG_TABULAR_FIGURES)
            })
        }
    }

    #[test]
    fn a_code_is_semibold_uppercase_and_tracked_by_its_size() {
        for ramp in Ramp::ALL {
            let m = metrics(Style::new(ramp).code(), UiFamily::Variable, 1.5);
            assert_eq!(m.weight, type_ramp::SEMIBOLD);
            assert!((m.tracking - 0.02 * ramp.style().size * 1.5).abs() < 1e-5);
            let plain = metrics(Style::new(ramp), UiFamily::Variable, 1.5);
            assert_eq!(plain.tracking, 0.0);
        }
        let text = Text::new().unwrap();
        let code = text
            .styled("png", Style::new(Ramp::Caption).code(), 1.0, 200.0, 20.0)
            .unwrap();
        let plain = text
            .styled("PNG", Style::new(Ramp::Caption), 1.0, 200.0, 20.0)
            .unwrap();
        // Uppercase, semibold and tracked: wider than the plain capitals.
        assert!(
            code.width > plain.width,
            "{} vs {}",
            code.width,
            plain.width
        );
    }

    #[test]
    fn a_line_is_as_tall_as_its_style_says() {
        let text = Text::new().unwrap();
        for ramp in Ramp::ALL {
            let s = ramp.style();
            let laid = text
                .styled("Ag\nAg", Style::new(ramp).wrapping(), 1.0, 400.0, 400.0)
                .unwrap();
            let mut m = DWRITE_TEXT_METRICS::default();
            // SAFETY: a plain read of a layout made on this thread.
            unsafe { laid.layout.GetMetrics(&mut m).unwrap() };
            assert!(
                (m.height - 2.0 * s.line).abs() < 0.01,
                "{}: {}",
                s.name,
                m.height
            );
        }
    }

    #[test]
    fn the_baseline_centres_the_font_in_its_line() {
        // Ascent 1.0 and descent 0.25 em at 16 DIPs is 20 DIPs of font: in
        // a 24-DIP line, 2 above and 2 below.
        assert_eq!(baseline(24.0, 16.0, (1.0, 0.25)), 18.0);
        // A line shorter than the font splits the shortfall the same way.
        assert_eq!(baseline(16.0, 16.0, (1.0, 0.25)), 14.0);
    }

    #[test]
    fn this_windows_has_the_families_it_says() {
        let text = Text::new().unwrap();
        // Whatever this PC has, the formats are made with what was picked.
        let caption = text.format(Style::new(Ramp::Caption), 1.0).unwrap();
        assert_eq!(format_family(&caption), text.ui_family().family(12.0));
        let glyph = text.glyph_format(16.0).unwrap();
        assert_eq!(format_family(&glyph), text.icon_family().name());
    }

    fn format_family(f: &IDWriteTextFormat) -> String {
        // SAFETY: plain reads of a format made on this thread.
        unsafe {
            let len = f.GetFontFamilyNameLength() as usize;
            let mut buf = vec![0u16; len + 1];
            f.GetFontFamilyName(&mut buf).unwrap();
            String::from_utf16_lossy(&buf[..len])
        }
    }

    #[test]
    fn a_style_and_a_glyph_are_each_laid_out_once() {
        let text = Text::new().unwrap();
        let style = Style::new(Ramp::Body);
        let a = text.styled("Settings", style, 1.0, 200.0, 20.0).unwrap();
        let b = text.styled("Settings", style, 1.0, 200.0, 20.0).unwrap();
        assert!(a.layout == b.layout);
        let strong = text
            .styled("Settings", Style::new(Ramp::BodyStrong), 1.0, 200.0, 20.0)
            .unwrap();
        assert!(strong.layout != a.layout);
        // At another Text size with the same optical size, the same format.
        let f1 = text.format(style, 1.0).unwrap();
        let f2 = text.format(style, 1.25).unwrap();
        assert!(f1 == f2);
        let g1 = text.glyph('\u{E713}', 16.0).unwrap();
        let g2 = text.glyph('\u{E713}', 16.0).unwrap();
        assert!(g1.layout == g2.layout);
        let g3 = text.glyph('\u{E713}', 20.0).unwrap();
        assert!(g3.layout != g1.layout);
    }

    #[test]
    fn strong_runs_are_semibold_and_laid_out_apart_from_plain_text() {
        let text = Text::new().unwrap();
        let style = Style::new(Ramp::Body).wrapping();
        let s = "Drop files here, or choose Add files.";
        let at = s.find("Add files").unwrap();
        // The words "Add files", as `ways_in::strong_runs` finds them.
        let runs: Vec<_> = std::iter::once(at..at + "Add files".len()).collect();
        let plain = text.styled(s, style, 1.0, 400.0, 100.0).unwrap();
        let strong = text
            .styled_runs(s, style, 1.0, 400.0, 100.0, &runs)
            .unwrap();
        // Not the plain layout from the cache, and wider for it.
        assert!(strong.layout != plain.layout);
        assert!(
            strong.width > plain.width,
            "{} {}",
            strong.width,
            plain.width
        );
        // Made once for the same runs.
        let again = text
            .styled_runs(s, style, 1.0, 400.0, 100.0, &runs)
            .unwrap();
        assert!(again.layout == strong.layout);
        // No runs is the plain text.
        let none = text.styled_runs(s, style, 1.0, 400.0, 100.0, &[]).unwrap();
        assert!(none.layout == plain.layout);
        // Body Strong's weight inside the run, Body's outside.
        let weight = |pos: usize| {
            let mut w = DWRITE_FONT_WEIGHT::default();
            // SAFETY: a plain query of a layout this thread made, into a
            // local.
            unsafe { strong.layout.GetFontWeight(pos as u32, &mut w, None) }.unwrap();
            w
        };
        assert_eq!(weight(at), DWRITE_FONT_WEIGHT_SEMI_BOLD);
        assert_eq!(weight(at + 8), DWRITE_FONT_WEIGHT_SEMI_BOLD);
        assert_eq!(weight(0), DWRITE_FONT_WEIGHT_NORMAL);
        assert_eq!(weight(at + 9), DWRITE_FONT_WEIGHT_NORMAL);
        // Measured as drawn: at least as tall as the plain text.
        let h = text.styled_height_runs(s, Style::new(Ramp::Body), 1.0, 120.0, &runs);
        assert!(h >= text.styled_height(s, Style::new(Ramp::Body), 1.0, 120.0));
    }

    #[test]
    fn glyph_formats_are_kept_for_a_few_sizes_only() {
        // The title bar asks for its glyphs at a token over the zoom, which
        // follows the window's size: a resize must not grow the cache.
        let text = Text::new().unwrap();
        for i in 0..100 {
            text.glyph('\u{E713}', 10.0 + i as f32 * 0.01).unwrap();
            assert!(text.cached_glyph_formats() <= GLYPH_SIZES);
        }
        // The few sizes in use are each made once and kept.
        let text = Text::new().unwrap();
        let f16 = text.glyph_format(16.0).unwrap();
        for size in [10.0, 20.0, 12.8, 16.0, 10.0] {
            text.glyph_format(size).unwrap();
        }
        assert!(text.glyph_format(16.0).unwrap() == f16);
        assert_eq!(text.cached_glyph_formats(), 4);
    }

    #[test]
    fn a_name_too_long_for_its_lines_ends_in_an_ellipsis_on_the_last() {
        let text = Text::new().unwrap();
        let style = Style::new(Ramp::Subtitle).wrapping();
        let lines = |s: &str| text.line_count(s, style, 1.0, 240.0);
        // A name that fits is left alone.
        assert_eq!(
            text.fit_lines("holiday.jpg", style, 1.0, 240.0, 2),
            "holiday.jpg"
        );
        let two = "a-fairly-long-name-that-wraps.jpg";
        assert_eq!(lines(two), 2, "{two}");
        assert_eq!(text.fit_lines(two, style, 1.0, 240.0, 2), two);
        // One that needs a third line is cut so it takes two, the second
        // ending in an ellipsis.
        let long = "a-file-name-that-keeps-going-and-going-and-going-until-it-has-run-out-of-room-for-even-two-lines-of-the-title.jpg";
        assert!(lines(long) > 2);
        let fit = text.fit_lines(long, style, 1.0, 240.0, 2);
        assert!(fit.ends_with('\u{2026}'), "{fit}");
        assert_eq!(lines(&fit), 2, "{fit}");
        assert!(long.starts_with(fit.trim_end_matches('\u{2026}')), "{fit}");
        // The cut keeps as much as fits: one more letter would not.
        let kept = fit.trim_end_matches('\u{2026}').chars().count();
        let more: String = long.chars().take(kept + 1).collect();
        assert!(lines(&format!("{more}\u{2026}")) > 2, "{fit}");
    }
}
