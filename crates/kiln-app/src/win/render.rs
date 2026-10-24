//! Direct2D drawing. Everything is laid out in DIPs; the render target's
//! DPI maps them to pixels, so 100%, 150% and 200% scaling share one path.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::f32::consts::{PI, TAU};
use std::path::PathBuf;
use std::sync::Arc;

use windows::core::Result;
use windows::Win32::Foundation::{D2DERR_RECREATE_TARGET, HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows_numerics::{Matrix3x2, Vector2};

use kiln_core::{Kind, Preset};

use crate::anim;
use crate::help::{self, HelpSection, Placed};
use crate::icons::{self, Cmd, Icon, Style};
use crate::inspector::{self as insp, Inspector, InspectorControl, Kept};
use crate::layout::{
    self, Control, DialogButton, InspectorLayout, Layout, Rect, SettingsLayout, SettingsTab,
    SheetControl, SheetLayout, Wrapped, CHIP_HEIGHT, CHIP_MIN_WIDTH, PROGRESS_HEIGHT, ROW_HEIGHT,
};
use crate::model::{self, Entry, FocusButton, JobState, Tone};
use crate::previews::{cover_source, Pixels, PreviewSize};
use crate::quality::{QualityDefaults, Value};
use crate::settings::{KeyMap, ThemeChoice};
use crate::theme::{self, depth, effects, shape, Glaze, Palette, Rgb};
use crate::ways_in::{self, EmptyLines};
use crate::wheel::{self, Action, Spot, Wheel, R_INNER, R_MORE_INNER, R_MORE_OUTER, R_OUTER};

use super::motion::ButtonId;
use super::text::Text;

/// How the wheel looks this frame.
pub struct WheelView<'a> {
    pub wheel: &'a Wheel,
    /// Its name and the key that opens it: "Convert · Shift".
    pub title: String,
    pub hover: Option<Spot>,
    /// 0 to 1: how far More's outer ring has poured open.
    pub more: f32,
    /// How long ago it opened, in milliseconds: the disc blooms
    /// (`anim::bloom`) and its segments follow (`anim::segment`).
    pub open_ms: f32,
    /// Windows' Animation effects: with them off it is open at once.
    pub animations: bool,
    /// 1 to 0 while closing.
    pub fade: f32,
    /// The item pulsing after a drop, and how far through the pulse.
    pub pulse: Option<(Spot, f32)>,
    /// Where the suggestion's dots go: its segment, or More and its slot.
    pub marks: Vec<Spot>,
    /// 0 to 1: the dots' fade in, when the suggestion arrived while the
    /// wheel was open.
    pub dot: f32,
    /// While the lit target has options: "Options… (Tab)" takes the key's
    /// place in the centre, brighter (`true`) while the pointer is on it.
    pub options: Option<bool>,
}

/// The suggestion's dot: a radius of 2 DIPs (4 across), 6 DIPs above the
/// icon or code it marks.
const DOT_R: f32 = 2.0;
const DOT_GAP: f32 = 6.0;

/// The key the footer's Convert names after its word, as the idle hint
/// does.
const CONVERT_KEY: &str = "Enter";

/// A keycap: the gap after the word, the padding either side of the key,
/// its height and corners, and its outline's opacity.
const KEYCAP_GAP: f32 = 6.0;
const KEYCAP_PAD: f32 = 4.0;
const KEYCAP_HEIGHT: f32 = 18.0;
const KEYCAP_RADIUS: f32 = 3.0;
const KEYCAP_OUTLINE: f32 = 0.45;

/// A row's glaze tile draws its kind's icon at this scale.
const ROW_ICON_SCALE: f32 = 0.75;

/// The empty window's mark is drawn with a line this many DIPs wide.
const MARK_LINE: f32 = 2.0;

/// The empty window's glaze tiles: each tile's corners, and the scale of
/// its icon, drawn with the mark's line.
const TILE_RADIUS: f32 = shape::RADIUS_PANEL;
const TILE_ICON_SCALE: f32 = 1.5;

/// The empty window's glazes: each tile's glaze, the kind whose icon it
/// shows, and the word under it.
const GLAZE_TILES: [(Glaze, Kind, &str); 4] = [
    (Glaze::Image, Kind::Image, "Images"),
    (Glaze::Video, Kind::Video, "Video"),
    (Glaze::Audio, Kind::Audio, "Audio"),
    (Glaze::Document, Kind::Document, "Documents"),
];

/// The inspector's picture, with corners this round.
const HERO_RADIUS: f32 = 8.0;
/// The picture's glaze tile draws its kind's icon at this scale: 48 DIPs.
const HERO_ICON_SCALE: f32 = 2.0;
/// Nothing's mark, in the picture's place: this many DIPs across.
const NOTHING_MARK: f32 = 48.0;
/// The selection's glaze washes the inspector's top at this opacity, and
/// fades out this far down it.
const INSPECTOR_WASH: f32 = 0.07;
const INSPECTOR_WASH_REACH: f32 = 0.4;
/// The suggested format's celadon edge and wash.
const SUGGESTED_EDGE: f32 = 0.55;
const SUGGESTED_WASH: f32 = 0.09;
/// Under a contrast theme, how far inside the suggested format's outline
/// the hover ring is.
const SUGGESTED_HOVER_INSET: f32 = 3.0;
/// A format's row: its chip starts this far in, leaving the suggestion's
/// dot room before it, and its estimate ends this far from its right.
const TARGET_INSET: f32 = 16.0;
const TARGET_END: f32 = 10.0;
/// A fact's value starts past the widest of its labels.
const FACT_LABEL: f32 = 72.0;
/// A converting file's heat over its picture, at its strongest, and how
/// much the breath takes from it: 1 down to 0.65.
const HERO_HEAT: f32 = 0.5;
const HEAT_BREATH: f32 = 0.35;
/// Several files' cards: each this far right and down from the one
/// behind it.
const CARD_STEP: (f32, f32) = (12.0, 8.0);
/// A finished file's check on its picture: the disc's radius and how far
/// it sits in from the picture's corner.
const CHECK_R: f32 = 14.0;
const CHECK_INSET: f32 = 10.0;

/// The inspector this frame, drawn where the layout puts it as far as the
/// fold has slid it in: what it says now and, while that is new, what it
/// said before.
pub struct InspectorView<'a> {
    pub now: InspectorPage<'a>,
    /// What it said before, fading out as `now` rises in.
    pub before: Option<InspectorPage<'a>>,
}

/// One content of the inspector, as it was laid out, and how it shows in
/// this frame.
pub struct InspectorPage<'a> {
    /// Where it was laid out: the inspector's whole place. The fold slides
    /// it across from there to where the inspector is drawn.
    pub laid: Rect,
    pub model: &'a Inspector,
    pub layout: &'a InspectorLayout,
    /// Its controls that are placed, in reading order, with where each is.
    pub controls: &'a [(InspectorControl, Rect)],
    /// Each format's glaze, or each kind's, in the order its rows show
    /// them.
    pub glazes: &'a [Glaze],
    /// What Nothing says under its title: the key hint, then the privacy
    /// line.
    pub nothing: [&'a str; 2],
    /// How far it has faded in, and how far it still has to rise, in DIPs.
    pub alpha: f32,
    pub rise: f32,
    /// Whether its controls light under the pointer: what it said before
    /// takes no pointer.
    pub live: bool,
}

/// A row the inspector offers: a format, or a kind of file to narrow the
/// selection to.
struct TargetRow<'a> {
    code: &'a str,
    glaze: Glaze,
    /// What it is for, or how many files a kind holds.
    why: Option<&'a str>,
    estimate: Option<&'a str>,
    suggested: bool,
}

/// The options sheet this frame.
pub struct SheetView<'a> {
    pub layout: &'a SheetLayout,
    pub title: &'a str,
    /// What the target format is: "MOV: QuickTime video from Apple".
    pub target: &'a str,
    /// What the source is.
    pub context: &'a str,
    /// The result's size and an estimate.
    pub result: &'a str,
    /// Each knob's name and its value as it reads.
    pub rows: &'a [(String, String)],
    pub preset: Preset,
    /// Indexes into `layout.controls`: the segments and steppers light by
    /// them.
    pub hover: Option<usize>,
    pub pressed: Option<usize>,
    pub focus: Option<usize>,
    /// Cancel's and Convert's hover and press, as they ease
    /// (`ButtonId::sheet`).
    pub button: &'a dyn Fn(ButtonId) -> (f32, f32),
    /// 0 to 1 as it fades in, how far it still has to rise in DIPs
    /// (`anim::rise`), and 1 to 0 as it fades out.
    pub open: f32,
    pub rise: f32,
    pub fade: f32,
}

/// Per-row animation state.
#[derive(Clone, Copy)]
pub struct RowAnim {
    /// The pour: how far the row has faded in since it was added, and how
    /// far it still has to rise into place, in DIPs.
    pub appear: f32,
    pub rise: f32,
    /// 0 to 1 after a job ended: a finished result cooling into place, or
    /// a failure's sentence turning to the error colour.
    pub settle: f32,
    /// The glaze wash a just-finished row flashes with, as it cools.
    pub cool: f32,
    /// How far a just-failed row's content is shaken sideways.
    pub shake: f32,
    /// 0 to 1 after facts arrived: the second line crossing over to what
    /// Kiln found.
    pub facts: f32,
}

impl RowAnim {
    /// A row with nothing moving, its facts `facts` of the way in.
    pub fn at_rest(facts: f32) -> Self {
        RowAnim {
            appear: 1.0,
            rise: 0.0,
            settle: 1.0,
            cool: 0.0,
            shake: 0.0,
            facts,
        }
    }
}

/// Everything the renderer needs to know about the app this frame.
pub struct View<'a> {
    pub palette: &'a Palette,
    pub entries: &'a [Entry],
    /// Lines that stand in for a row's own second line, by the row's
    /// file: a failed export, or a held result that's gone.
    pub notes: &'a HashMap<PathBuf, (String, Tone)>,
    /// Rows, by file, whose results are being exported: they read
    /// "Saving…" and have no link until it's done.
    pub saving: &'a HashSet<PathBuf>,
    /// The footer's "Export all (n)" label, while results are held.
    pub export_all: Option<&'a str>,
    /// The selected rows; each draws the selected wash.
    pub selected: &'a BTreeSet<usize>,
    /// The glaze of row `i`'s kind of file.
    pub glaze: &'a dyn Fn(usize) -> Glaze,
    /// Row `i`'s kind of file, whose icon its glaze tile shows. `None` for
    /// a type Kiln has no kind for.
    pub kind: &'a dyn Fn(usize) -> Option<Kind>,
    /// Row `i`'s picture at a size, once Windows has made one: its pixels,
    /// the id of its key (the renderer keeps a bitmap by it) and how far
    /// it has faded in, 0 to 1. `None` draws the glaze tile.
    pub preview: &'a dyn Fn(usize, PreviewSize) -> Option<(Arc<Pixels>, u64, f32)>,
    /// Whether rows draw their own link. False while the inspector shows,
    /// which carries the actions.
    pub row_links: bool,
    /// The row the keyboard is on; it keeps a ring even with nothing
    /// selected.
    pub focus: Option<usize>,
    /// The header or footer button Tab moved the keyboard to, which takes
    /// the ring instead.
    pub button_ring: Option<Rect>,
    /// How much of the inspector shows, 0 to 1: it folds away as the
    /// window narrows (`layout::layout_with`).
    pub shown: f32,
    /// Where the one selected row's wash and edge are, in DIPs from the
    /// top of the list before scrolling, as it glides from row to row.
    /// `None` while several rows are selected: each draws its own.
    pub selection_y: Option<f32>,
    /// A button's hover and press, each 0 to 1, as they ease: the header's,
    /// the footer's and the inspector's, and those on a sheet or dialog.
    pub button: &'a dyn Fn(ButtonId) -> (f32, f32),
    pub hover_row: Option<usize>,
    pub hover_cancel: bool,
    pub hover_link: bool,
    pub hover_retry: bool,
    pub scroll: f32,
    /// 0 to 1: how far the drag-over highlight has faded in.
    pub drag: f32,
    /// 0 to 1: how hot running progress lines burn this frame.
    pub heat: f32,
    pub row_anim: &'a dyn Fn(usize) -> RowAnim,
    pub status: &'a str,
    /// The empty window's lines; its key hint names the current wheel
    /// keys.
    pub empty: &'a EmptyLines,
    /// What the status says while the list is empty, shown under the Add
    /// files button. Empty when there's nothing to say.
    pub empty_status: &'a str,
    /// The footer's Convert button; disabled, it is drawn quiet and takes
    /// no hover.
    pub convert_enabled: bool,
    /// What the footer's button says: Convert, or Cancel while the
    /// selection holds a file that is converting.
    pub convert_label: &'a str,
    /// Whether the footer's button shows its key, Enter, in a keycap after
    /// its word: while it reads Convert, where the footer has room.
    pub convert_key: bool,
    /// Rows were just removed: the footer offers Undo.
    pub undo: bool,
    pub wheel: Option<WheelView<'a>>,
    /// The details dialog, while it's open.
    pub details: Option<DetailsView<'a>>,
    pub settings: Option<SettingsView<'a>>,
    pub sheet: Option<SheetView<'a>>,
    /// The help overlay, while it's open.
    pub help: Option<HelpView<'a>>,
    /// The inspector, while it shows beside the list.
    pub inspector: Option<InspectorView<'a>>,
}

/// The help overlay this frame.
pub struct HelpView<'a> {
    pub sections: &'a [HelpSection],
    pub scroll: f32,
    /// Close's hover and press, as they ease (`ButtonId::Help`). It is
    /// the only control, and has the keyboard focus.
    pub button: &'a dyn Fn(ButtonId) -> (f32, f32),
    /// 0 to 1 as it fades in, and how far it still has to rise in DIPs
    /// (`anim::rise`).
    pub open: f32,
    pub rise: f32,
}

/// The details dialog this frame: what a failed job's engine said.
pub struct DetailsView<'a> {
    /// "What FFmpeg said".
    pub title: &'a str,
    pub text: &'a str,
    pub scroll: f32,
    /// "Copy", or "Copied" for a moment after a copy.
    pub copy_label: &'static str,
    pub focus: DialogButton,
    /// Each button's hover and press, as they ease (`ButtonId::Details`).
    pub button: &'a dyn Fn(ButtonId) -> (f32, f32),
}

/// The settings panel this frame.
pub struct SettingsView<'a> {
    pub layout: &'a SettingsLayout,
    /// 0 to 1 as it fades in, and how far it still has to rise in DIPs
    /// (`anim::rise`).
    pub open: f32,
    pub rise: f32,
    /// Where the tab marker sits, in tabs from the left.
    pub tab_marker: f32,
    /// 0 to 1 after the page changed: the new page fades in.
    pub page: f32,
    pub quality: &'a QualityDefaults,
    /// The output folder line, and whether it is the default (muted).
    pub output: (&'a str, bool),
    pub theme: ThemeChoice,
    /// Whether suggestions are on.
    pub suggestions: bool,
    /// Whether results are saved next to the original as they are ready.
    pub autosave: bool,
    /// Whether any picks are counted; Forget picks is disabled without.
    pub has_habits: bool,
    pub keys: KeyMap,
    /// Each engine's line, and whether it is muted.
    pub engines: [(&'a str, bool); 4],
    /// Indexes into `layout.controls`: the tabs, segments and steppers
    /// light by them.
    pub hover: Option<usize>,
    pub pressed: Option<usize>,
    /// Each button's hover and press, as they ease
    /// (`ButtonId::settings`).
    pub button: &'a dyn Fn(ButtonId) -> (f32, f32),
    /// The update check's note, whether it is muted, and the button label.
    pub update: (&'a str, bool),
    pub update_button: &'a str,
    pub focus: Option<usize>,
}

pub struct Renderer {
    factory: ID2D1Factory,
    text: Text,
    round: ID2D1StrokeStyle,
    icons: Vec<(&'static str, ID2D1PathGeometry)>,
    target: Option<Target>,
    hwnd: HWND,
    dpi: f32,
    /// The soft shadow under a raised surface, worked out once: each
    /// layer's growth and its share of the shadow's alpha.
    shadow: Vec<(f32, f32)>,
    /// Frames drawn so far, to tell which previews' bitmaps have gone
    /// undrawn long enough to drop.
    frame: u64,
    /// The narrowest chip (`chip_floor`), measured once.
    chip_floor: f32,
    /// How wide "More formats…" is, measured once as the chip floor is.
    more_w: f32,
    /// The inspector's wrapped texts' heights, by text, how it wraps and
    /// width, so the same words are measured once. DirectWrite measures in
    /// DIPs, which the Text size's zoom doesn't change.
    measured: RefCell<HashMap<(String, Wrapped, u32), f32>>,
    /// Numbers set with figures of one width, laid out once for each text
    /// and box, with their widths.
    tabulars: Tabulars,
}

/// Laid-out numbers by whether they are the large figure and their box's
/// width and height, then by their text: looked up without making a key.
type Tabulars = RefCell<HashMap<(bool, u32, u32), HashMap<String, (IDWriteTextLayout, f32)>>>;

/// The most measured or laid-out texts kept before starting over: far
/// more than one frame uses.
const TEXT_CACHE: usize = 64;

/// Device-dependent resources, rebuilt if the device is lost.
struct Target {
    rt: ID2D1HwndRenderTarget,
    brush: ID2D1SolidColorBrush,
    grain: ID2D1BitmapBrush,
    /// The faint celadon glow at the top centre of the ground. It is made
    /// with the grain; each frame places it for the window's size.
    glow: ID2D1RadialGradientBrush,
    /// Each glaze from clear to full, in `Glaze::ALL` order, and the text
    /// colour the same way: a converting row's wash and its heat line's
    /// glow. Each use places them and sets their opacity, so a frame makes
    /// no brush of its own.
    glaze_ramps: Vec<ID2D1LinearGradientBrush>,
    text_ramp: ID2D1LinearGradientBrush,
    /// Previews made into brushes, by their key's id, and the frame each
    /// was last drawn in. They go with the device.
    pictures: RefCell<HashMap<u64, Picture>>,
    /// The palette the grain was made for.
    palette: Palette,
}

/// A preview's bitmap, ready to fill a shape with.
struct Picture {
    brush: ID2D1BitmapBrush,
    drawn: u64,
}

/// A preview's bitmap is dropped once it has gone this many frames
/// without being drawn.
const PICTURE_FRAMES: u64 = 120;

fn color(c: Rgb, a: f32) -> D2D1_COLOR_F {
    let [r, g, b] = c.unit();
    D2D1_COLOR_F { r, g, b, a }
}

/// `a` mixed toward `b`. A contrast theme has no in-between colours, so
/// there it snaps to the nearer end.
fn blend(p: &Palette, a: Rgb, b: Rgb, t: f32) -> Rgb {
    if !p.high_contrast {
        theme::mix(a, b, t)
    } else if t >= 0.5 {
        b
    } else {
        a
    }
}

/// How opaque a translucent cue is drawn: as designed, or solid under a
/// contrast theme.
fn solid(p: &Palette, a: f32) -> f32 {
    if p.high_contrast {
        1.0
    } else {
        a
    }
}

/// Two gradient stops: `c` at alpha `from` at the start, and at `to` at
/// the end.
fn ramp_stops(c: Rgb, from: f32, to: f32) -> [D2D1_GRADIENT_STOP; 2] {
    [
        D2D1_GRADIENT_STOP {
            position: 0.0,
            color: color(c, from),
        },
        D2D1_GRADIENT_STOP {
            position: 1.0,
            color: color(c, to),
        },
    ]
}

/// A gradient of `c` from clear at its start point to full at its end.
/// Each use places it and sets its opacity.
fn ramp(rt: &ID2D1HwndRenderTarget, c: Rgb) -> Result<ID2D1LinearGradientBrush> {
    unsafe {
        let coll = rt.CreateGradientStopCollection(
            &ramp_stops(c, 0.0, 1.0),
            D2D1_GAMMA_2_2,
            D2D1_EXTEND_MODE_CLAMP,
        )?;
        rt.CreateLinearGradientBrush(
            &D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                startPoint: pt(0.0, 0.0),
                endPoint: pt(1.0, 0.0),
            },
            None,
            &coll,
        )
    }
}

fn rectf(r: &Rect) -> D2D_RECT_F {
    D2D_RECT_F {
        left: r.x,
        top: r.y,
        right: r.right(),
        bottom: r.bottom(),
    }
}

fn rounded(r: &Rect, radius: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT {
        rect: rectf(r),
        radiusX: radius,
        radiusY: radius,
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn pt(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

fn scale_at(s: f32, x: f32, y: f32) -> Matrix3x2 {
    Matrix3x2 {
        M11: s,
        M12: 0.0,
        M21: 0.0,
        M22: s,
        M31: x,
        M32: y,
    }
}

/// Everything moved by `(dx, dy)`.
fn moved(dx: f32, dy: f32) -> Matrix3x2 {
    scale_at(1.0, dx, dy)
}

/// Turned `deg` degrees clockwise about the origin, scaled by `s`, then
/// moved to `(x, y)`.
fn turn_scale_at(deg: f32, s: f32, x: f32, y: f32) -> Matrix3x2 {
    let (sin, cos) = deg.to_radians().sin_cos();
    Matrix3x2 {
        M11: s * cos,
        M12: s * sin,
        M21: -s * sin,
        M22: s * cos,
        M31: x,
        M32: y,
    }
}

/// Scaled by `s` about `(x, y)`, which stays where it is.
fn scale_about(s: f32, x: f32, y: f32) -> Matrix3x2 {
    scale_at(s, x * (1.0 - s), y * (1.0 - s))
}

/// `inner` applied first, then `outer`.
fn compose(inner: &Matrix3x2, outer: &Matrix3x2) -> Matrix3x2 {
    Matrix3x2 {
        M11: inner.M11 * outer.M11 + inner.M12 * outer.M21,
        M12: inner.M11 * outer.M12 + inner.M12 * outer.M22,
        M21: inner.M21 * outer.M11 + inner.M22 * outer.M21,
        M22: inner.M21 * outer.M12 + inner.M22 * outer.M22,
        M31: inner.M31 * outer.M11 + inner.M32 * outer.M21 + outer.M31,
        M32: inner.M31 * outer.M12 + inner.M32 * outer.M22 + outer.M32,
    }
}

/// `r` under a transform that only scales and moves, as `m` is when it
/// has no turn in it.
fn mapped(r: &Rect, m: &Matrix3x2) -> Rect {
    Rect::new(
        r.x * m.M11 + m.M31,
        r.y * m.M22 + m.M32,
        r.w * m.M11,
        r.h * m.M22,
    )
}

/// A state that is on or off, as an amount from 0 to 1, for controls
/// that don't ease.
fn amount(on: bool) -> f32 {
    if on {
        1.0
    } else {
        0.0
    }
}

/// A missing helper app's link in Settings > Helper apps.
const GET_IT: &str = "Get it";

/// The code on a row's chip.
fn chip_code(e: &Entry) -> &str {
    if e.code.is_empty() {
        "—"
    } else {
        &e.code
    }
}

/// The code a chip is always at least as wide as: four letters.
const CHIP_FLOOR_CODE: &str = "DOCX";

/// How wide the narrowest chip is: a four-letter code with its padding, so
/// the name and line after every chip of up to four letters start at one
/// x. DirectWrite measures in DIPs whatever the scale, and the chip's font
/// never changes, so this is measured once.
fn chip_floor(text: &Text) -> f32 {
    let w = text
        .code_layout(CHIP_FLOOR_CODE, 120.0, CHIP_HEIGHT)
        .map_or(0.0, |(_, w)| w);
    (w + 16.0).max(CHIP_MIN_WIDTH).ceil()
}

/// How wide a row's chip is: its code with padding, and at least `floor`
/// (`chip_floor`). A longer code widens only its own chip.
fn chip_width(text: &Text, e: &Entry, floor: f32) -> Result<f32> {
    code_chip_width(text, chip_code(e), floor)
}

/// How wide the chip for `code` is, as `chip_width` has it. Each code's
/// width is measured once.
fn code_chip_width(text: &Text, code: &str, floor: f32) -> Result<f32> {
    let text_w = text.code_width(code)?;
    Ok((text_w + 16.0).ceil().max(floor))
}

/// The row whose file the inspector shows, or the first of several.
fn first_row(model: &Inspector) -> Option<usize> {
    match model {
        Inspector::Nothing => None,
        Inspector::Waiting { row, .. }
        | Inspector::Converting { row, .. }
        | Inspector::Done { row, .. }
        | Inspector::Failed { row, .. } => Some(*row),
        Inspector::Several { rows, .. } => rows.first().copied(),
    }
}

/// Where a row's name and second line start: right of its chip, which
/// starts after the preview, with the same gap again.
fn row_text_x(r: &Rect, chip_w: f32) -> f32 {
    layout::row_text_left(r) + CHIP_MIN_WIDTH.max(chip_w) + layout::PREVIEW_GAP
}

/// Where a row's name and line run, from `x` to `right`, short of
/// Cancel, Retry or the link, and the link if there is one. `chip_w` is
/// the chip's measured width, `saving` says the row is being exported, and
/// `links` that rows show their links and Cancel (not while the inspector
/// does).
fn row_text_span(
    text: &Text,
    r: &Rect,
    e: &Entry,
    chip_w: f32,
    saving: bool,
    links: bool,
) -> (f32, f32, Option<&'static str>) {
    let x = row_text_x(r, chip_w);
    let link = e
        .link(line_cut_with(text, r, e, chip_w))
        .filter(|_| links && !saving);
    let right = if links && matches!(e.job, JobState::Running { .. }) {
        layout::row_cancel(r).x - 8.0
    } else if e.retryable() && !saving {
        layout::row_retry(r, link.is_some()).x - 8.0
    } else if link.is_some() {
        layout::row_link(r).x - 8.0
    } else {
        r.right() - 16.0
    };
    (x, right, link)
}

/// Whether a failed row's line runs past the room it has before the row's
/// right padding, so the row cuts it short.
fn line_cut(text: &Text, r: &Rect, e: &Entry, floor: f32) -> bool {
    if !matches!(e.job, JobState::Failed { .. }) {
        return false;
    }
    let Ok(chip_w) = chip_width(text, e, floor) else {
        return false;
    };
    line_cut_with(text, r, e, chip_w)
}

/// `line_cut` for a row whose chip is already measured, `chip_w` wide.
fn line_cut_with(text: &Text, r: &Rect, e: &Entry, chip_w: f32) -> bool {
    if !matches!(e.job, JobState::Failed { .. }) {
        return false;
    }
    let mut room = r.right() - 16.0 - row_text_x(r, chip_w);
    if e.retryable() {
        room -= layout::RETRY_WIDTH + 8.0;
    }
    text.measure(&e.line().0, &text.caption) > room
}

impl Renderer {
    pub fn new(hwnd: HWND, dpi: f32) -> Result<Self> {
        let factory: ID2D1Factory =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)? };
        let props = D2D1_STROKE_STYLE_PROPERTIES {
            startCap: D2D1_CAP_STYLE_ROUND,
            endCap: D2D1_CAP_STYLE_ROUND,
            dashCap: D2D1_CAP_STYLE_ROUND,
            lineJoin: D2D1_LINE_JOIN_ROUND,
            miterLimit: 10.0,
            dashStyle: D2D1_DASH_STYLE_SOLID,
            dashOffset: 0.0,
        };
        let round = unsafe { factory.CreateStrokeStyle(&props, None)? };
        let icons = icons::ALL
            .iter()
            .map(|i| Ok((i.name, geometry(&factory, i)?)))
            .collect::<Result<Vec<_>>>()?;
        let text = Text::new()?;
        let chip_floor = chip_floor(&text);
        let more_w = text.measure(insp::MORE_FORMATS, &text.caption);
        Ok(Self {
            factory,
            text,
            round,
            icons,
            target: None,
            hwnd,
            dpi,
            shadow: depth::shadow_layers(),
            frame: 0,
            chip_floor,
            more_w,
            measured: RefCell::new(HashMap::new()),
            tabulars: RefCell::new(HashMap::new()),
        })
    }

    /// How tall an inspector text runs at `width`, wrapped as `how` says
    /// and cut short at its most lines. The same words are measured once.
    pub fn inspector_text_height(&self, s: &str, how: Wrapped, width: f32) -> f32 {
        let key = (s.to_owned(), how, width.to_bits());
        if let Some(h) = self.measured.borrow().get(&key) {
            return *h;
        }
        let (format, lines) = match how {
            Wrapped::Title(n) => (&self.text.inspector_title_block, n),
            Wrapped::Body(n) => (&self.text.caption_block, n),
        };
        let h = self.text.lines_height(s, format, width, lines);
        let mut measured = self.measured.borrow_mut();
        if measured.len() >= TEXT_CACHE {
            measured.clear();
        }
        measured.insert(key, h);
        h
    }

    /// How wide the inspector's "More formats…" link is, in the caption
    /// size it is drawn in.
    pub fn more_width(&self) -> f32 {
        self.more_w
    }

    /// How tall the details dialog's text runs when wrapped to `width`.
    pub fn details_height(&self, text: &str, width: f32) -> f32 {
        self.text.block_height(text, &self.text.mono_block, width)
    }

    /// How tall one line of help's text runs wrapped to `width`.
    pub fn help_line_height(&self, text: &str, width: f32) -> f32 {
        self.text.block_height(text, &self.text.body_block, width)
    }

    /// How tall help's text runs in a body `width` wide, for scrolling it.
    pub fn help_height(&self, sections: &[HelpSection], width: f32) -> f32 {
        help::place(sections, width, |s, w| {
            self.text.block_height(s, &self.text.body_block, w)
        })
        .1
    }

    /// Row `r`'s tooltip area, from its left edge to where its name and
    /// line end (short of the cancel cross and the link, which have tools
    /// of their own), and whether `line`, as the row shows it, is cut.
    /// `links` says rows show their links.
    pub fn row_tip(
        &self,
        r: &Rect,
        e: &Entry,
        line: &str,
        saving: bool,
        links: bool,
    ) -> (Rect, bool) {
        let chip_w = chip_width(&self.text, e, self.chip_floor).unwrap_or(self.chip_floor);
        let (x, right, _) = row_text_span(&self.text, r, e, chip_w, saving, links);
        let cut = self.text.measure(line, &self.text.caption) > (right - x).max(0.0);
        (Rect::new(r.x, r.y, (right - r.x).max(0.0), r.h), cut)
    }

    /// Whether a failed row's line is too long for row `r`, so it's cut
    /// short and Show details offers the whole sentence.
    pub fn line_cut(&self, r: &Rect, e: &Entry) -> bool {
        line_cut(&self.text, r, e, self.chip_floor)
    }

    pub fn display_font_bundled(&self) -> bool {
        self.text.display_bundled
    }

    pub fn set_dpi(&mut self, dpi: f32) {
        if (dpi - self.dpi).abs() < 1e-3 {
            return;
        }
        self.dpi = dpi;
        // The grain bitmap is per-pixel, so rebuild everything at the new
        // scale.
        self.target = None;
    }

    pub fn resize(&mut self) {
        if let Some(t) = &self.target {
            let size = client_size(self.hwnd);
            unsafe {
                let _ = t.rt.Resize(&size);
            }
        }
    }

    fn ensure_target(&mut self, palette: &Palette) -> Result<()> {
        if let Some(t) = &self.target {
            if t.palette == *palette {
                return Ok(());
            }
        }
        let size = client_size(self.hwnd);
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: self.dpi,
            dpiY: self.dpi,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
            hwnd: self.hwnd,
            pixelSize: size,
            presentOptions: D2D1_PRESENT_OPTIONS_NONE,
        };
        let rt = unsafe { self.factory.CreateHwndRenderTarget(&props, &hwnd_props)? };
        unsafe {
            rt.SetDpi(self.dpi, self.dpi);
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        }
        let brush = unsafe { rt.CreateSolidColorBrush(&color(palette.text, 1.0), None)? };

        // The grain: one pre-rendered tile, one texel per physical pixel.
        let side = effects::GRAIN_TILE;
        let pixels = theme::grain_tile(palette);
        let bitmap = unsafe {
            rt.CreateBitmap(
                D2D_SIZE_U {
                    width: side,
                    height: side,
                },
                Some(pixels.as_ptr().cast()),
                side * 4,
                &D2D1_BITMAP_PROPERTIES {
                    pixelFormat: D2D1_PIXEL_FORMAT {
                        format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                    },
                    dpiX: self.dpi,
                    dpiY: self.dpi,
                },
            )?
        };
        let grain = unsafe {
            rt.CreateBitmapBrush(
                &bitmap,
                Some(&D2D1_BITMAP_BRUSH_PROPERTIES {
                    extendModeX: D2D1_EXTEND_MODE_WRAP,
                    extendModeY: D2D1_EXTEND_MODE_WRAP,
                    interpolationMode: D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                }),
                None,
            )?
        };
        // The glow: celadon at its faint peak to clear. Its centre and radii
        // are set for the window's size each frame.
        let glow = unsafe {
            let stops = ramp_stops(palette.accent, effects::GROUND_GLOW, 0.0);
            let coll =
                rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)?;
            rt.CreateRadialGradientBrush(
                &D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                    center: pt(0.0, 0.0),
                    gradientOriginOffset: pt(0.0, 0.0),
                    radiusX: 1.0,
                    radiusY: 1.0,
                },
                None,
                &coll,
            )?
        };
        let glaze_ramps = Glaze::ALL
            .iter()
            .map(|&g| ramp(&rt, palette.glaze(g)))
            .collect::<Result<Vec<_>>>()?;
        let text_ramp = ramp(&rt, palette.text)?;
        self.target = Some(Target {
            rt,
            brush,
            grain,
            glow,
            glaze_ramps,
            text_ramp,
            pictures: RefCell::new(HashMap::new()),
            palette: *palette,
        });
        Ok(())
    }

    pub fn draw(&mut self, v: &View) -> Result<()> {
        self.ensure_target(v.palette)?;
        let t = self.target.as_ref().expect("target exists");
        let (w, h) = self.size_dips();
        let l = layout::layout_with(w, h, v.entries.len(), v.shown);
        let p = v.palette;
        let rt = &t.rt;

        self.frame += 1;
        unsafe {
            rt.BeginDraw();
            rt.SetTransform(&Matrix3x2::identity());
            rt.Clear(Some(&color(p.ground, 1.0)));
            // A contrast theme's ground is flat. Kiln's own has a faint
            // celadon glow at the top centre, under the grain.
            if !p.high_contrast {
                let ground = rectf(&Rect::new(0.0, 0.0, w, h));
                t.glow.SetCenter(pt(w / 2.0, -0.1 * h));
                t.glow.SetRadiusX(1.2 * w);
                t.glow.SetRadiusY(0.7 * h);
                rt.FillRectangle(&ground, &t.glow);
                rt.FillRectangle(&ground, &t.grain);
            }
        }

        let d = Draw {
            factory: &self.factory,
            rt,
            brush: &t.brush,
            text: &self.text,
            round: &self.round,
            icons: &self.icons,
            base: Cell::new(Matrix3x2::identity()),
            shadow: &self.shadow,
            glaze_ramps: &t.glaze_ramps,
            text_ramp: &t.text_ramp,
            pictures: &t.pictures,
            frame: self.frame,
            chip_floor: self.chip_floor,
            tabulars: &self.tabulars,
        };
        let mut result = d.header(&l, v);
        if result.is_ok() {
            result = if v.entries.is_empty() {
                d.empty_state(&l, v)
            } else {
                d.list(&l, v).and_then(|_| d.footer(&l, v))
            };
        }
        if let (Ok(()), Some(r)) = (&result, &v.button_ring) {
            d.focus_ring(r, p);
        }
        if let (Ok(()), Some(wv)) = (&result, &v.wheel) {
            result = d.wheel(wv, p);
        }
        if let (Ok(()), Some(dv)) = (&result, &v.details) {
            result = d.dialog(w, h, dv, p);
        }
        if let (Ok(()), Some(sv)) = (&result, &v.settings) {
            result = d.settings(w, h, sv, p);
        }
        if let (Ok(()), Some(sv)) = (&result, &v.sheet) {
            result = d.sheet(w, h, sv, p);
        }
        if let (Ok(()), Some(hv)) = (&result, &v.help) {
            result = d.help(w, h, hv, p);
        }

        let end = unsafe { rt.EndDraw(None, None) };
        // Pictures not drawn for a while go; they're made again if their
        // rows come back into view.
        let frame = self.frame;
        t.pictures
            .borrow_mut()
            .retain(|_, pic| frame - pic.drawn < PICTURE_FRAMES);
        result?;
        match end {
            Err(e) if e.code() == D2DERR_RECREATE_TARGET => {
                self.target = None;
                Ok(())
            }
            other => other,
        }
    }

    pub fn size_dips(&self) -> (f32, f32) {
        let s = client_size(self.hwnd);
        let k = 96.0 / self.dpi;
        (s.width as f32 * k, s.height as f32 * k)
    }
}

struct Draw<'a> {
    factory: &'a ID2D1Factory,
    rt: &'a ID2D1HwndRenderTarget,
    brush: &'a ID2D1SolidColorBrush,
    text: &'a Text,
    round: &'a ID2D1StrokeStyle,
    icons: &'a [(&'static str, ID2D1PathGeometry)],
    /// The transform everything is currently drawn under.
    base: Cell<Matrix3x2>,
    /// The shadow's layers: growth and share of its alpha.
    shadow: &'a [(f32, f32)],
    glaze_ramps: &'a [ID2D1LinearGradientBrush],
    text_ramp: &'a ID2D1LinearGradientBrush,
    pictures: &'a RefCell<HashMap<u64, Picture>>,
    /// This frame's number, stamped on each picture drawn.
    frame: u64,
    chip_floor: f32,
    tabulars: &'a Tabulars,
}

impl Draw<'_> {
    fn set_base(&self, m: Matrix3x2) {
        self.base.set(m);
        unsafe { self.rt.SetTransform(&m) };
    }

    /// Draw `f` at `opacity` as one layer, so overlapping parts don't
    /// show through each other.
    fn with_opacity<T>(&self, opacity: f32, f: impl FnOnce() -> Result<T>) -> Result<T> {
        if opacity >= 0.999 {
            return f();
        }
        let layer = unsafe { self.rt.CreateLayer(None)? };
        let params = D2D1_LAYER_PARAMETERS {
            contentBounds: D2D_RECT_F {
                left: -1e6,
                top: -1e6,
                right: 1e6,
                bottom: 1e6,
            },
            geometricMask: std::mem::ManuallyDrop::new(None),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: Matrix3x2::identity(),
            opacity: opacity.max(0.0),
            opacityBrush: std::mem::ManuallyDrop::new(None),
            layerOptions: D2D1_LAYER_OPTIONS_NONE,
        };
        unsafe { self.rt.PushLayer(&params, &layer) };
        let result = f();
        unsafe { self.rt.PopLayer() };
        result
    }

    fn paint(&self, c: Rgb, a: f32) -> &ID2D1SolidColorBrush {
        unsafe { self.brush.SetColor(&color(c, a)) };
        self.brush
    }

    fn fill(&self, r: &Rect, radius: f32, c: Rgb, a: f32) {
        unsafe {
            self.rt
                .FillRoundedRectangle(&rounded(r, radius), self.paint(c, a))
        };
    }

    /// A border drawn inside `r`, so edges stay crisp and borders never
    /// double up with neighbours.
    fn border(&self, r: &Rect, radius: f32, c: Rgb, a: f32, width: f32) {
        let inner = r.inset(width / 2.0);
        unsafe {
            self.rt.DrawRoundedRectangle(
                &rounded(&inner, (radius - width / 2.0).max(0.0)),
                self.paint(c, a),
                width,
                None,
            )
        };
    }

    /// A translucent wash over `r`. Under a contrast theme washes go, and
    /// an outline in the same colour stands in for them.
    fn wash(&self, r: &Rect, radius: f32, c: Rgb, a: f32, p: &Palette) {
        if !p.high_contrast {
            self.fill(r, radius, c, a);
        } else if a > 0.0 {
            self.border(r, radius, c, 1.0, 2.0);
        }
    }

    /// The hover background of a small control, `hover` of the way in: the
    /// line colour, or under a contrast theme, which has no in-between, an
    /// outline in the text colour once it is more than half in.
    fn hover_back(&self, r: &Rect, radius: f32, hover: f32, p: &Palette) {
        if p.high_contrast {
            if hover > 0.5 {
                self.border(r, radius, p.text, 1.0, shape::BORDER);
            }
        } else if hover > 0.0 {
            self.fill(r, radius, p.line, hover);
        }
    }

    /// A thin scroll thumb.
    fn thumb(&self, r: &Rect, p: &Palette) {
        self.fill(r, 1.0, p.text_muted, solid(p, effects::THUMB));
    }

    /// A scrim over the whole window, at `a` of its full strength.
    fn scrim(&self, w: f32, h: f32, a: f32, p: &Palette) {
        self.fill(&Rect::new(0.0, 0.0, w, h), 0.0, p.ground, solid(p, 0.7) * a);
    }

    /// A row's underlined link text, right-aligned in `area`: muted, a
    /// step brighter on a wash, and full text colour while hot.
    #[allow(clippy::too_many_arguments)]
    fn row_link_label(
        &self,
        text: &str,
        area: &Rect,
        hot: bool,
        on_wash: bool,
        p: &Palette,
        a: f32,
    ) {
        let c = if hot {
            p.text
        } else if on_wash && !p.high_contrast {
            // A contrast theme has no wash, so its link keeps the
            // hot-light colour.
            p.muted_on_wash
        } else {
            p.link
        };
        let tw = self.text.measure(text, &self.text.caption);
        let lx = area.right() - tw;
        self.label(
            text,
            &self.text.caption,
            &Rect::new(lx, area.y, tw + 1.0, area.h),
            c,
            a,
        );
        self.hline(
            lx,
            lx + tw,
            area.y + 15.0,
            c,
            a * solid(p, effects::UNDERLINE),
        );
    }

    fn hline(&self, x0: f32, x1: f32, y: f32, c: Rgb, a: f32) {
        let y = y + 0.5;
        unsafe {
            self.rt
                .DrawLine(pt(x0, y), pt(x1, y), self.paint(c, a), shape::BORDER, None)
        };
    }

    fn label(&self, s: &str, format: &IDWriteTextFormat, r: &Rect, c: Rgb, a: f32) {
        unsafe {
            self.rt.DrawText(
                &wide(s),
                format,
                &rectf(r),
                self.paint(c, a),
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            )
        };
    }

    /// A format code: monospace, uppercase, tracked, centred in `r`.
    fn code(&self, code: &str, r: &Rect, c: Rgb, a: f32) -> Result<f32> {
        let (layout, w) = self.text.code_layout(code, r.w.max(1.0), r.h)?;
        unsafe {
            self.rt.DrawTextLayout(
                pt(r.x, r.y),
                &layout,
                self.paint(c, a),
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
            );
        }
        Ok(w)
    }

    fn icon(&self, name: &str, x: f32, y: f32, scale: f32, c: Rgb, a: f32) {
        self.icon_lined(name, x, y, scale, shape::ICON_STROKE * scale, c, a);
    }

    /// An icon at `scale` whose line is `line` DIPs wide whatever the
    /// scale, for the empty window's larger drawings.
    #[allow(clippy::too_many_arguments)]
    fn icon_lined(&self, name: &str, x: f32, y: f32, scale: f32, line: f32, c: Rgb, a: f32) {
        let Some((_, geom)) = self.icons.iter().find(|(n, _)| *n == name) else {
            return;
        };
        let solid = icons::ALL
            .iter()
            .any(|i| i.name == name && i.style == Style::Fill);
        let base = self.base.get();
        unsafe {
            self.rt
                .SetTransform(&compose(&scale_at(scale, x, y), &base));
            if solid {
                self.rt.FillGeometry(geom, self.paint(c, a), None);
            } else {
                // The stroke is drawn under the scale, so it's given in the
                // icon's own units.
                self.rt
                    .DrawGeometry(geom, self.paint(c, a), line / scale, self.round);
            }
            self.rt.SetTransform(&base);
        }
    }

    /// The soft shadow under a raised surface `r` with corners of `radius`:
    /// the shadow's layers, each the surface grown by its amount, offset
    /// down, in the theme's shadow colour at its share of the alpha. A
    /// layer pulled in so far it has no area is skipped. A contrast theme
    /// has no shadows.
    fn shadow(&self, r: &Rect, radius: f32, p: &Palette) {
        if p.high_contrast {
            return;
        }
        for &(grow, share) in self.shadow {
            let a = p.shadow_alpha * share;
            let layer = Rect::new(
                r.x - grow,
                r.y + depth::OFFSET_Y - grow,
                r.w + 2.0 * grow,
                r.h + 2.0 * grow,
            );
            if layer.w <= 0.0 || layer.h <= 0.0 || a <= 0.0 {
                continue;
            }
            let corner = (radius + grow).clamp(0.0, layer.w.min(layer.h) / 2.0);
            self.fill(&layer, corner, p.shadow, a);
        }
    }

    /// The 1 px light along the top of a raised surface `r`, `inset` below
    /// its top edge (inside its border, if it has one), between its rounded
    /// corners. A contrast theme has none.
    fn highlight(&self, r: &Rect, radius: f32, inset: f32, p: &Palette) {
        self.highlight_at(r, radius, inset, 1.0, p);
    }

    /// `highlight` at `k` of its strength: a press sinks it out of sight.
    fn highlight_at(&self, r: &Rect, radius: f32, inset: f32, k: f32, p: &Palette) {
        if p.high_contrast || p.highlight_alpha <= 0.0 || k <= 0.0 {
            return;
        }
        let (x0, x1) = (r.x + radius, r.right() - radius);
        if x1 > x0 {
            self.hline(x0, x1, r.y + inset, p.highlight, p.highlight_alpha * k);
        }
    }

    /// A raised panel: its shadow, the surface, its edge and its top
    /// highlight. Sheets and dialogs are drawn on one.
    fn raised_panel(&self, r: &Rect, p: &Palette) {
        self.shadow(r, shape::RADIUS_PANEL, p);
        self.fill(r, shape::RADIUS_PANEL, p.surface, 1.0);
        self.border(r, shape::RADIUS_PANEL, p.edge, 1.0, shape::BORDER);
        self.highlight(r, shape::RADIUS_PANEL, shape::BORDER, p);
    }

    /// The gradient of glaze `g`, from clear to full.
    fn glaze_ramp(&self, g: Glaze) -> &ID2D1LinearGradientBrush {
        let i = Glaze::ALL.iter().position(|&x| x == g).unwrap_or(0);
        &self.glaze_ramps[i]
    }

    /// A format chip: an opaque fill of glaze `g`, a 1 px outline of it,
    /// and the code in the glaze's ink. A contrast theme keeps its own
    /// colours: an outline in its line colour and the code in its text.
    fn chip(&self, code: &str, r: &Rect, g: Glaze, a: f32, p: &Palette) -> Result<()> {
        if p.high_contrast {
            self.border(r, shape::RADIUS_CONTROL, p.line, a, shape::BORDER);
            self.code(code, r, p.text, a)?;
        } else {
            self.fill(r, shape::RADIUS_CONTROL, p.chip_fill(g), a);
            let outline = effects::CHIP_OUTLINE * a;
            self.border(r, shape::RADIUS_CONTROL, p.glaze(g), outline, shape::BORDER);
            self.code(code, r, p.glaze_ink(g), a)?;
        }
        Ok(())
    }

    /// A glaze tile in `r`: the chip fill of glaze `g`, with the icon of
    /// `kind` centred in the glaze at `scale`, its line `line` DIPs wide,
    /// at `icon_a`. Under a contrast theme the tile is the surface with an
    /// outline, and the icon is in its text colour.
    #[allow(clippy::too_many_arguments)]
    fn glaze_tile(
        &self,
        r: &Rect,
        radius: f32,
        g: Glaze,
        kind: Option<Kind>,
        (scale, line): (f32, f32),
        icon_a: f32,
        a: f32,
        p: &Palette,
    ) {
        self.fill(r, radius, p.chip_fill(g), a);
        if p.high_contrast {
            self.border(r, radius, p.line, a, shape::BORDER);
        }
        if let (Some(kind), true) = (kind, icon_a > 0.0) {
            let side = shape::ICON_GRID * scale;
            let (x, y) = (r.x + (r.w - side) / 2.0, r.y + (r.h - side) / 2.0);
            let c = p.glaze(g);
            self.icon_lined(wheel::icon_for_kind(kind), x, y, scale, line, c, a * icon_a);
        }
    }

    /// A file's preview in `dst`, with corners of `radius`: its glaze tile,
    /// with its kind's icon at `icon` (scale and line), and over it, once
    /// Windows has made one, its picture cropped to fill, fading in as the
    /// tile's icon fades out.
    #[allow(clippy::too_many_arguments)]
    fn preview(
        &self,
        dst: &Rect,
        radius: f32,
        picture: Option<(Arc<Pixels>, u64, f32)>,
        g: Glaze,
        kind: Option<Kind>,
        icon: (f32, f32),
        a: f32,
        p: &Palette,
    ) {
        let shown = picture.and_then(|(px, id, fade)| {
            let brush = self.picture(&px, id)?;
            Some((brush, px, fade.clamp(0.0, 1.0)))
        });
        let fade = shown.as_ref().map_or(0.0, |(_, _, f)| *f);
        self.glaze_tile(dst, radius, g, kind, icon, 1.0 - fade, a, p);
        let Some((brush, px, fade)) = shown else {
            return;
        };
        // The brush maps the crop's pixels onto `dst`; the bitmap is at 96
        // DPI, so its DIPs are its pixels.
        let (sx, sy, sw, sh) = cover_source((px.width, px.height), (dst.w, dst.h));
        let (kx, ky) = (dst.w / sw, dst.h / sh);
        unsafe {
            brush.SetTransform(&Matrix3x2 {
                M11: kx,
                M12: 0.0,
                M21: 0.0,
                M22: ky,
                M31: dst.x - sx * kx,
                M32: dst.y - sy * ky,
            });
            brush.SetOpacity(a * fade);
            self.rt.FillRoundedRectangle(&rounded(dst, radius), &brush);
        }
    }

    /// The brush for preview `id`, made from `px` the first time it's
    /// drawn, and marked as drawn this frame. `None` when the pixels don't
    /// make a bitmap.
    fn picture(&self, px: &Pixels, id: u64) -> Option<ID2D1BitmapBrush> {
        let mut pictures = self.pictures.borrow_mut();
        if let Some(pic) = pictures.get_mut(&id) {
            pic.drawn = self.frame;
            return Some(pic.brush.clone());
        }
        let stride = px.width.checked_mul(4)?;
        let len = (stride as usize).checked_mul(px.height as usize)?;
        if px.width == 0 || px.height == 0 || px.bgra.len() < len {
            return None;
        }
        let brush = unsafe {
            let bitmap = self
                .rt
                .CreateBitmap(
                    D2D_SIZE_U {
                        width: px.width,
                        height: px.height,
                    },
                    Some(px.bgra.as_ptr().cast()),
                    stride,
                    &D2D1_BITMAP_PROPERTIES {
                        pixelFormat: D2D1_PIXEL_FORMAT {
                            format: DXGI_FORMAT_B8G8R8A8_UNORM,
                            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                        },
                        dpiX: 96.0,
                        dpiY: 96.0,
                    },
                )
                .ok()?;
            self.rt
                .CreateBitmapBrush(
                    &bitmap,
                    Some(&D2D1_BITMAP_BRUSH_PROPERTIES {
                        extendModeX: D2D1_EXTEND_MODE_CLAMP,
                        extendModeY: D2D1_EXTEND_MODE_CLAMP,
                        interpolationMode: D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                    }),
                    None,
                )
                .ok()?
        };
        pictures.insert(
            id,
            Picture {
                brush: brush.clone(),
                drawn: self.frame,
            },
        );
        Some(brush)
    }

    fn header(&self, l: &Layout, v: &View) -> Result<()> {
        let p = v.palette;
        // The app icon: a celadon mark on the soot ground.
        self.icon("mark-solid", l.icon.x, l.icon.y, 1.0, p.accent, 1.0);
        self.label("Kiln", &self.text.display, &l.wordmark, p.text, 1.0);
        // Help's question mark, then Settings' dial, at the right. Hover
        // lifts each into the line colour, as it eases.
        for (b, icon, which) in [
            (l.help_button, "help", FocusButton::Help),
            (l.settings_button, "dial", FocusButton::Settings),
        ] {
            let (hover, _) = (v.button)(ButtonId::Window(which));
            self.hover_back(&b, shape::RADIUS_CONTROL, hover, p);
            let c = blend(p, p.text_muted, p.text, hover);
            self.icon(icon, b.x + 4.0, b.y + 4.0, 1.0, c, 1.0);
        }
        self.hline(0.0, l.header.w, l.header.bottom() - 1.0, p.edge, 1.0);
        Ok(())
    }

    fn empty_state(&self, l: &Layout, v: &View) -> Result<()> {
        let p = v.palette;
        let panel = l.panel;
        self.shadow(&panel, shape::RADIUS_PANEL, p);
        self.fill(&panel, shape::RADIUS_PANEL, p.surface, 1.0);
        if v.drag > 0.0 && !p.high_contrast {
            self.fill(
                &panel,
                shape::RADIUS_PANEL,
                p.accent,
                effects::DROP_WASH * v.drag,
            );
        }
        let edge = blend(p, p.edge, p.accent_strong, v.drag);
        self.border(
            &panel,
            shape::RADIUS_PANEL,
            edge,
            1.0,
            shape::BORDER + 0.5 * v.drag,
        );
        self.highlight(&panel, shape::RADIUS_PANEL, shape::BORDER, p);

        // The centred group: the mark, the headline, how else to add
        // files, the button, what the status says, the keys to hold, the
        // privacy note and, where there's room, the four glazes.
        let e = layout::empty_state(&panel);
        let lines = v.empty;
        // Celadon line work, filling its 72 DIPs. A contrast theme draws
        // it in text, lit by a drag like the panel's edge.
        let mark = if p.high_contrast {
            blend(p, p.text_muted, p.accent, v.drag)
        } else {
            p.accent_strong
        };
        let k = e.mark.w / shape::ICON_GRID;
        self.icon_lined("mark", e.mark.x, e.mark.y, k, MARK_LINE, mark, 1.0);
        self.label(
            lines.headline,
            &self.text.lead_center,
            &e.headline,
            p.text,
            1.0,
        );
        self.label(
            lines.instruction,
            &self.text.body_center,
            &e.instruction,
            p.text,
            1.0,
        );
        let (hover, pressed) = (v.button)(ButtonId::Window(FocusButton::AddFiles));
        self.primary(lines.button, &e.button, hover, pressed, p);
        self.label(
            v.empty_status,
            &self.text.caption_center,
            &e.status,
            p.text_muted,
            1.0,
        );
        for (text, r) in [(lines.keys.as_str(), &e.keys), (lines.privacy, &e.privacy)] {
            self.label(text, &self.text.caption_center, r, p.text_muted, 1.0);
        }
        // The kinds Kiln reads, each a tile of its glaze and its icon, with
        // its name under it in the glaze's ink.
        for (tile, (g, kind, word)) in e.glazes.iter().flatten().zip(GLAZE_TILES) {
            let icon = (TILE_ICON_SCALE, MARK_LINE);
            self.glaze_tile(tile, TILE_RADIUS, g, Some(kind), icon, 1.0, 1.0, p);
            let under = Rect::new(
                tile.x,
                tile.bottom() + layout::GLAZE_WORD_GAP,
                tile.w,
                layout::GLAZE_WORD,
            );
            // 12 px and centred, as the wheel's group names are.
            let small = &self.text.ring_heading;
            self.label(word, small, &under, p.glaze_ink(g), 1.0);
        }
        Ok(())
    }

    fn list(&self, l: &Layout, v: &View) -> Result<()> {
        let p = v.palette;
        let panel = l.panel;
        self.shadow(&panel, shape::RADIUS_PANEL, p);
        self.fill(&panel, shape::RADIUS_PANEL, p.surface, 1.0);

        // Clip rows to the panel's rounded shape so selection fills
        // follow its corners.
        let mask = unsafe {
            self.factory
                .CreateRoundedRectangleGeometry(&rounded(&panel, shape::RADIUS_PANEL))?
        };
        let layer = unsafe { self.rt.CreateLayer(None)? };
        let params = D2D1_LAYER_PARAMETERS {
            contentBounds: rectf(&panel),
            geometricMask: std::mem::ManuallyDrop::new(Some(mask.into())),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: Matrix3x2::identity(),
            opacity: 1.0,
            opacityBrush: std::mem::ManuallyDrop::new(None),
            layerOptions: D2D1_LAYER_OPTIONS_NONE,
        };
        unsafe { self.rt.PushLayer(&params, &layer) };
        let first = (v.scroll / ROW_HEIGHT).floor().max(0.0) as usize;
        let visible = (panel.h / ROW_HEIGHT).ceil() as usize + 1;
        // The one selected row's wash and edge, under the rows, where it has
        // glided to; it pours in with its row.
        if let (Some(y), Some(&i)) = (v.selection_y, v.selected.first()) {
            let anim = (v.row_anim)(i);
            let at = l.list.y + y - v.scroll + anim.rise;
            let r = Rect::new(l.list.x, at, l.list.w, ROW_HEIGHT);
            self.selection_wash(&r, anim.appear, p);
        }
        let mut result = Ok(());
        for i in first..(first + visible).min(v.entries.len()) {
            let anim = (v.row_anim)(i);
            let mut r = l.rows[i];
            // Scrolled, and still rising as it pours in.
            r.y += anim.rise - v.scroll;
            result = self.row(&r, &v.entries[i], i, anim, v);
            if result.is_err() {
                break;
            }
        }
        unsafe { self.rt.PopLayer() };
        // PushLayer took its own reference; release ours.
        drop(std::mem::ManuallyDrop::into_inner(params.geometricMask));
        result?;
        // The inspector beside the rows, under the panel's edge.
        if let Some(iv) = &v.inspector {
            self.inspector(l, iv, v)?;
        }

        if v.drag > 0.0 && !p.high_contrast {
            self.fill(
                &panel,
                shape::RADIUS_PANEL,
                p.accent,
                effects::DROP_WASH * v.drag,
            );
        }
        let edge = blend(p, p.edge, p.accent_strong, v.drag);
        self.border(
            &panel,
            shape::RADIUS_PANEL,
            edge,
            1.0,
            shape::BORDER + 0.5 * v.drag,
        );
        self.highlight(&panel, shape::RADIUS_PANEL, shape::BORDER, p);

        // A thin scroll thumb when the list overflows, at the list's right
        // edge, short of the inspector.
        let max = layout::max_scroll(v.entries.len(), panel.h);
        if max > 0.0 {
            let content = v.entries.len() as f32 * ROW_HEIGHT;
            let track = panel.h - 16.0;
            let thumb_h = (track * panel.h / content).max(24.0);
            let thumb_y = panel.y + 8.0 + (track - thumb_h) * (v.scroll / max);
            let thumb = Rect::new(l.list.right() - 6.0, thumb_y, 2.0, thumb_h);
            self.thumb(&thumb, p);
        }
        Ok(())
    }

    /// A selected row's wash on `r`, at `a`, with a fine celadon edge on
    /// its left, so selection never rests on the tint alone.
    fn selection_wash(&self, r: &Rect, a: f32, p: &Palette) {
        self.wash(r, 0.0, p.accent, effects::HOVER_FILL * a, p);
        if !p.high_contrast {
            let edge = Rect::new(r.x + shape::BORDER, r.y, 1.0, r.h);
            self.fill(&edge, 0.0, p.accent_strong, a);
        }
    }

    fn row(&self, r: &Rect, e: &Entry, i: usize, anim: RowAnim, v: &View) -> Result<()> {
        let p = v.palette;
        let a = anim.appear;
        let glaze = (v.glaze)(i);
        let hovered = v.hover_row == Some(i);
        let selected = v.selected.contains(&i);
        if selected {
            // One selected row's wash glides, and is drawn under the rows.
            if v.selection_y.is_none() {
                self.selection_wash(r, a, p);
            }
        } else if hovered {
            if p.high_contrast {
                self.border(r, 0.0, p.line, a, shape::BORDER);
            } else {
                self.fill(r, 0.0, p.line, effects::ROW_HOVER * a);
            }
        }
        // A converting row glows in its glaze from the left, clear by 60%
        // of the way across. A contrast theme has no washes.
        if matches!(e.job, JobState::Running { .. }) && !p.high_contrast {
            let ramp = self.glaze_ramp(glaze);
            unsafe {
                ramp.SetStartPoint(pt(r.x + 0.6 * r.w, r.y));
                ramp.SetEndPoint(pt(r.x, r.y));
                ramp.SetOpacity(effects::ROW_WASH * a);
                self.rt.FillRectangle(&rectf(r), ramp);
            }
        }
        // A row that just finished flashes in its glaze and cools to its
        // resting colour. A contrast theme has no washes: "Done" says it.
        if anim.cool > 0.0 && !p.high_contrast {
            self.fill(r, 0.0, p.glaze(glaze), anim.cool * a);
        }
        if i > 0 {
            self.hline(r.x, r.right(), r.y - 0.5, p.edge, a);
        }
        // A row that just failed shakes its content, left then right, once.
        let shaken = anim.shake != 0.0;
        if shaken {
            self.set_base(moved(anim.shake, 0.0));
        }
        let drawn = self.row_content(r, e, i, anim, v);
        if shaken {
            self.set_base(Matrix3x2::identity());
        }
        drawn?;
        // The keyboard's row keeps its ring, selected or not.
        if v.focus == Some(i) {
            let ring = r.inset(2.0);
            self.border(
                &ring,
                shape::RADIUS_CONTROL,
                focus_colour(p),
                a,
                shape::ICON_STROKE,
            );
        }
        Ok(())
    }

    /// What row `i` shows on its wash: its preview, chip, name and line,
    /// its progress, and its links.
    fn row_content(&self, r: &Rect, e: &Entry, i: usize, anim: RowAnim, v: &View) -> Result<()> {
        let p = v.palette;
        let a = anim.appear;
        let glaze = (v.glaze)(i);
        let hovered = v.hover_row == Some(i);
        let selected = v.selected.contains(&i);
        // Muted text on a washed row is a step brighter (darker in light),
        // so it still reads.
        let muted = if selected || hovered {
            p.muted_on_wash
        } else {
            p.text_muted
        };

        // The preview, then the format chip after it: monospace,
        // uppercase, tracked, in the file's glaze.
        let picture = (v.preview)(i, PreviewSize::Row);
        let kind = (v.kind)(i);
        let preview = layout::row_preview(r);
        let icon = (ROW_ICON_SCALE, shape::ICON_STROKE * ROW_ICON_SCALE);
        self.preview(
            &preview,
            layout::PREVIEW_RADIUS,
            picture,
            glaze,
            kind,
            icon,
            a,
            p,
        );
        let chip_y = r.y + (ROW_HEIGHT - CHIP_HEIGHT) / 2.0;
        let chip_w = chip_width(self.text, e, self.chip_floor)?;
        let chip = Rect::new(layout::row_text_left(r), chip_y, chip_w, CHIP_HEIGHT);
        self.chip(chip_code(e), &chip, glaze, a, p)?;

        let saving = v.saving.contains(&e.path);
        let (x, right, link) = row_text_span(self.text, r, e, chip_w, saving, v.row_links);
        let w = (right - x).max(0.0);
        self.label(
            &e.name,
            &self.text.body,
            &Rect::new(x, r.y + 8.0, w, 20.0),
            p.text,
            a,
        );

        // A waiting file's own line ends with its suggestion, which is
        // drawn in the glaze's ink: its parts are built once, and the line
        // is made from them. Only a file Kiln can read gets a suggestion,
        // and its line is muted, as `Entry::line` has it.
        let note = v.notes.get(&e.path);
        let own = matches!(e.job, JobState::Idle) && !saving && note.is_none();
        let split = (own && e.suggestion.is_some())
            .then(|| e.detail_parts())
            .and_then(|(facts, suggestion)| {
                suggestion.map(|s| (format!("{facts}{}", model::DETAIL_SEPARATOR), s))
            });
        let (line, tone) = if saving {
            (model::SAVING.to_owned(), Tone::Muted)
        } else if let Some((head, suggestion)) = &split {
            (format!("{head}{suggestion}"), Tone::Muted)
        } else {
            note.cloned().unwrap_or_else(|| e.line())
        };
        let (c, la) = match tone {
            Tone::Muted => (muted, a),
            // The result cools into place over the settle fade.
            Tone::Result => (p.text, a * anim.settle),
            Tone::Crack => (p.crack, a * anim.settle),
        };
        let line_rect = Rect::new(x, r.y + 28.0, w, 20.0);
        // When facts arrive, the line they replace fades out in place as
        // the new one fades in, both muted.
        let mut la = la;
        if anim.facts < 1.0 && matches!(e.job, JobState::Idle) {
            let before = e.plain_detail();
            if before != line {
                self.label(
                    &before,
                    &self.text.caption,
                    &line_rect,
                    muted,
                    a * (1.0 - anim.facts),
                );
                la *= anim.facts;
            }
        }
        match &split {
            Some((head, suggestion)) => {
                self.label(head, &self.text.caption, &line_rect, c, la);
                let head_w = self.text.measure(head, &self.text.caption);
                if head_w < line_rect.w {
                    let rest = Rect::new(
                        line_rect.x + head_w,
                        line_rect.y,
                        line_rect.w - head_w,
                        line_rect.h,
                    );
                    let ink = p.glaze_ink(glaze);
                    self.label(suggestion, &self.text.caption, &rest, ink, la);
                }
            }
            None => self.label(&line, &self.text.caption, &line_rect, c, la),
        }
        // An exported row's "Saved to" opens its folder: underlined while
        // the pointer is on it, like a link, while rows have links.
        if v.row_links && hovered && v.hover_link && e.saved().is_some() {
            let tw = self.text.measure(&line, &self.text.caption).min(w);
            self.hline(
                x,
                x + tw,
                line_rect.y + 15.0,
                c,
                la * solid(p, effects::UNDERLINE),
            );
        }

        match &e.job {
            JobState::Running { fraction, .. } => {
                self.progress(r, *fraction, v.heat, glaze, p, a);
                // Cancel is always there to see, as a word, unless the
                // inspector carries it.
                if v.row_links {
                    let area = layout::row_cancel_text(r);
                    self.row_link_label(
                        "Cancel",
                        &area,
                        hovered && v.hover_cancel,
                        selected || hovered,
                        p,
                        a,
                    );
                }
            }
            JobState::Done { .. } => {
                // The finished line fades as the result settles.
                let fade = 1.0 - anim.settle;
                if fade > 0.0 {
                    self.progress(r, Some(1.0), 0.0, glaze, p, a * fade);
                }
            }
            _ => {}
        }
        // "Show details" on a failed row, "Export" on a held one.
        // The text sits on the second line; its target is taller
        // (`layout::row_link`).
        if let Some(text) = link {
            let area = layout::row_link_text(r);
            self.row_link_label(
                text,
                &area,
                hovered && v.hover_link,
                selected || hovered,
                p,
                a,
            );
        }
        // "Retry" on a cancelled or failed row, beside the link when there is
        // one.
        if e.retryable() && !saving {
            let area = layout::row_retry_text(r, link.is_some());
            self.row_link_label(
                "Retry",
                &area,
                hovered && v.hover_retry,
                selected || hovered,
                p,
                a,
            );
        }
        Ok(())
    }

    /// The temperature line along the bottom of a job row, in the file's
    /// glaze `g`. While running it burns a little brighter at its leading
    /// edge, like a glaze taking heat, and that glow slowly rises and falls
    /// with `heat`. A contrast theme draws it in its highlight colour, with
    /// no glow.
    fn progress(&self, r: &Rect, fraction: Option<f32>, heat: f32, g: Glaze, p: &Palette, a: f32) {
        let y = r.bottom() - PROGRESS_HEIGHT;
        let track = Rect::new(r.x, y, r.w, PROGRESS_HEIGHT);
        self.fill(&track, 0.0, p.line, a);
        let burn = if p.high_contrast {
            p.accent_strong
        } else {
            p.glaze(g)
        };
        let Some(f) = fraction else {
            // Unknown length: the whole line holds a heat that breathes,
            // solid so it reads against the row.
            let c = blend(p, burn, p.text, effects::BREATH_SWING * heat);
            self.fill(&track, 0.0, c, a);
            return;
        };
        let end = r.x + r.w * f.clamp(0.0, 1.0);
        self.fill(&Rect::new(r.x, y, end - r.x, PROGRESS_HEIGHT), 0.0, burn, a);
        if f < 1.0 && end > r.x && !p.high_contrast {
            // The glow: the glaze rising to full at the leading edge, and
            // over it the text colour by as much as the breath swings, so
            // the edge burns toward the text colour.
            let swing = effects::GLOW_MIN + effects::BREATH_SWING * heat;
            let glow_w = 40.0f32.min(end - r.x);
            let glow = rectf(&Rect::new(
                end - glow_w,
                y - 1.0,
                glow_w,
                PROGRESS_HEIGHT + 1.0,
            ));
            for (ramp, opacity) in [(self.glaze_ramp(g), a), (self.text_ramp, a * swing)] {
                unsafe {
                    ramp.SetStartPoint(pt(end - glow_w, y));
                    ramp.SetEndPoint(pt(end, y));
                    ramp.SetOpacity(opacity);
                    self.rt.FillRectangle(&glow, ramp);
                }
            }
        }
    }

    /// Draw `f` clipped to `r` with corners of `radius`. `r` is where the
    /// current transform puts it, which may only scale and move: the clip
    /// is made where `r` lands on the target, so it lines up however it
    /// was moved.
    fn clipped<T>(&self, r: &Rect, radius: f32, f: impl FnOnce() -> Result<T>) -> Result<T> {
        let base = self.base.get();
        let at = mapped(r, &base);
        let mask = unsafe {
            self.factory
                .CreateRoundedRectangleGeometry(&rounded(&at, radius * base.M11))?
        };
        let layer = unsafe { self.rt.CreateLayer(None)? };
        let params = D2D1_LAYER_PARAMETERS {
            contentBounds: rectf(&at),
            geometricMask: std::mem::ManuallyDrop::new(Some(mask.into())),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: Matrix3x2::identity(),
            opacity: 1.0,
            opacityBrush: std::mem::ManuallyDrop::new(None),
            layerOptions: D2D1_LAYER_OPTIONS_NONE,
        };
        unsafe {
            self.rt.SetTransform(&Matrix3x2::identity());
            self.rt.PushLayer(&params, &layer);
            self.rt.SetTransform(&base);
        }
        let result = f();
        unsafe { self.rt.PopLayer() };
        // PushLayer took its own reference; release ours.
        drop(std::mem::ManuallyDrop::into_inner(params.geometricMask));
        result
    }

    /// A number in `r` with figures of one width: a fact's value or an
    /// estimate against the box's right, or (`figure`) a converting file's
    /// percentage. Each text is laid out once for its box. Returns its
    /// width.
    fn tabular(&self, s: &str, figure: bool, r: &Rect, c: Rgb) -> Result<f32> {
        let boxed = (figure, r.w.to_bits(), r.h.to_bits());
        let cached = self
            .tabulars
            .borrow()
            .get(&boxed)
            .and_then(|texts| texts.get(s))
            .cloned();
        let (layout, w) = match cached {
            Some(laid) => laid,
            None => {
                let format = if figure {
                    &self.text.figure
                } else {
                    &self.text.caption_right
                };
                let layout = self.text.tabular_layout(s, format, r.w, r.h)?;
                let mut m = DWRITE_TEXT_METRICS::default();
                unsafe { layout.GetMetrics(&mut m)? };
                let laid = (layout, m.widthIncludingTrailingWhitespace);
                let mut all = self.tabulars.borrow_mut();
                if all.values().map(HashMap::len).sum::<usize>() >= TEXT_CACHE {
                    all.clear();
                }
                all.entry(boxed)
                    .or_default()
                    .insert(s.to_owned(), laid.clone());
                laid
            }
        };
        unsafe {
            self.rt.DrawTextLayout(
                pt(r.x, r.y),
                &layout,
                self.paint(c, 1.0),
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
            );
        }
        Ok(w)
    }

    /// The inspector beside the list: the rule before it, then, inside the
    /// panel's rounded corner, the selection's glaze washing its top and
    /// what it says for the selection, with its controls. It is drawn
    /// where the layout puts it, as far as the fold has slid it in: its
    /// content, laid out on its whole place, slides in from the panel's
    /// right edge, and the panel's corner clips it. Folded all the way,
    /// there is nothing of it to draw.
    fn inspector(&self, l: &Layout, iv: &InspectorView, v: &View) -> Result<()> {
        let p = v.palette;
        let Some(r) = l.inspector else {
            return Ok(());
        };
        let rule = Rect::new(
            r.x - layout::INSPECTOR_RULE,
            l.panel.y + shape::BORDER,
            layout::INSPECTOR_RULE,
            (l.panel.h - 2.0 * shape::BORDER).max(0.0),
        );
        self.fill(&rule, 0.0, p.edge, 1.0);
        self.clipped(&l.panel, shape::RADIUS_PANEL, || {
            // What it said before fades out under what it says now, which
            // rises in.
            for page in iv.before.iter().chain(std::iter::once(&iv.now)) {
                self.inspector_page(&r, page, v)?;
            }
            Ok(())
        })
    }

    /// One content of the inspector, slid across from where it was laid
    /// out to `r`, where the inspector is drawn, at its alpha and rise:
    /// the selection's glaze washing its top (which fades but doesn't
    /// rise), what it says and its controls.
    fn inspector_page(&self, r: &Rect, page: &InspectorPage, v: &View) -> Result<()> {
        let p = v.palette;
        if page.alpha <= 0.0 {
            return Ok(());
        }
        let dx = r.x - page.laid.x;
        let laid = page.laid;
        self.with_opacity(page.alpha, || {
            // A contrast theme has no washes.
            if let (Some(i), false) = (first_row(page.model), p.high_contrast) {
                self.set_base(moved(dx, 0.0));
                let reach = laid.h * INSPECTOR_WASH_REACH;
                let ramp = self.glaze_ramp((v.glaze)(i));
                unsafe {
                    ramp.SetStartPoint(pt(laid.x, laid.y + reach));
                    ramp.SetEndPoint(pt(laid.x, laid.y));
                    ramp.SetOpacity(INSPECTOR_WASH);
                    let top = Rect::new(laid.x, laid.y, laid.w, reach);
                    self.rt.FillRectangle(&rectf(&top), ramp);
                }
            }
            self.set_base(moved(dx, page.rise));
            let drawn = self
                .inspector_says(page, v)
                .and_then(|_| self.inspector_controls(page, v));
            self.set_base(Matrix3x2::identity());
            drawn
        })
    }

    /// What the inspector says for the selection: its picture, title,
    /// body, facts and heading.
    fn inspector_says(&self, iv: &InspectorPage, v: &View) -> Result<()> {
        let p = v.palette;
        let l = iv.layout;
        let muted = p.text_muted;
        let heading = |text: &str| {
            if let Some(h) = l.label {
                self.label(text, &self.text.small, &h, muted, 1.0);
            }
        };
        // Each text under the title where the layout put it, while it has
        // room: see `InspectorLayout::lines`. The tree reads the same.
        let say = |k: usize, text: &str, format: &IDWriteTextFormat, c: Rgb| {
            if let Some(at) = l.line(k) {
                self.label(text, format, &at, c, 1.0);
            }
        };
        match iv.model {
            Inspector::Nothing => {
                if l.hero.h > 0.0 {
                    let (h, k) = (&l.hero, NOTHING_MARK / shape::ICON_GRID);
                    let x = h.x + (h.w - NOTHING_MARK) / 2.0;
                    let y = h.y + (h.h - NOTHING_MARK) / 2.0;
                    let c = if p.high_contrast {
                        p.text_muted
                    } else {
                        p.accent_strong
                    };
                    self.icon_lined("mark", x, y, k, MARK_LINE, c, 1.0);
                }
                let title = &self.text.inspector_title_block;
                self.label(insp::NOTHING_TITLE, title, &l.title, p.text, 1.0);
                let [hint, privacy] = iv.nothing;
                say(0, hint, &self.text.caption_block_center, muted);
                say(1, privacy, &self.text.caption_block_center, muted);
            }
            Inspector::Waiting {
                row, what, facts, ..
            } => {
                self.one_file(*row, l, v)?;
                if let Some(what) = what {
                    say(0, what, &self.text.caption_block, muted);
                }
                for ((label, value), f) in facts.iter().zip(&l.facts) {
                    self.label(label, &self.text.caption, f, muted, 1.0);
                    let at = Rect::new(f.x + FACT_LABEL, f.y, (f.w - FACT_LABEL).max(0.0), f.h);
                    self.tabular(value, false, &at, p.text)?;
                }
                heading(insp::TURN_INTO);
            }
            Inspector::Converting { row, percent, time } => {
                self.one_file(*row, l, v)?;
                if l.hero.h > 0.0 {
                    let fraction = percent.map(|pc| pc as f32 / 100.0);
                    self.hero_heat(&l.hero, fraction, (v.glaze)(*row), v.heat, p)?;
                }
                if let (Some(pc), Some(figure)) = (percent, l.line(0)) {
                    self.tabular(&format!("{pc}%"), true, &figure, p.text)?;
                }
                // How long is left or, until that's known, what it does.
                say(1, time, &self.text.caption, muted);
            }
            Inspector::Done {
                row,
                result,
                change,
                kept,
            } => {
                self.one_file(*row, l, v)?;
                if l.hero.h > 0.0 {
                    // The check fades in as the result settles.
                    let settle = (v.row_anim)(*row).settle;
                    self.check(&l.hero, settle, p);
                }
                // The result in the caption size, in full text colour.
                say(0, result, &self.text.caption, p.text);
                if let Some(change) = change {
                    say(1, change, &self.text.caption, muted);
                }
                if *kept == Kept::Held {
                    say(2, model::DRAG_HINT, &self.text.caption_block, muted);
                }
            }
            Inspector::Failed {
                row,
                sentence,
                tone,
                ..
            } => {
                self.one_file(*row, l, v)?;
                // In the tone the row says it in.
                let c = match tone {
                    Tone::Crack => p.crack,
                    Tone::Result => p.text,
                    Tone::Muted => muted,
                };
                say(0, sentence, &self.text.caption_block, c);
            }
            Inspector::Several {
                rows,
                kinds,
                targets,
                none_shared,
                ..
            } => {
                self.stack(&l.hero, rows, v);
                self.label(kinds, &self.text.inspector_title, &l.title, p.text, 1.0);
                if let Some(none) = none_shared {
                    say(0, none, &self.text.caption_block, muted);
                }
                heading(if targets.is_empty() {
                    insp::ONE_KIND
                } else {
                    insp::TURN_INTO
                });
            }
        }
        Ok(())
    }

    /// One file's picture and its title: the chip and the file's name.
    fn one_file(&self, i: usize, l: &InspectorLayout, v: &View) -> Result<()> {
        let p = v.palette;
        let Some(e) = v.entries.get(i) else {
            return Ok(());
        };
        let g = (v.glaze)(i);
        if l.hero.h > 0.0 {
            let picture = (v.preview)(i, PreviewSize::Hero);
            let icon = (HERO_ICON_SCALE, MARK_LINE);
            self.preview(&l.hero, HERO_RADIUS, picture, g, (v.kind)(i), icon, 1.0, p);
        }
        let t = l.title;
        if t.h > 0.0 {
            let chip_w = chip_width(self.text, e, self.chip_floor)?;
            let chip = Rect::new(t.x, t.y + (t.h - CHIP_HEIGHT) / 2.0, chip_w, CHIP_HEIGHT);
            self.chip(chip_code(e), &chip, g, 1.0, p)?;
            let x = chip.right() + 8.0;
            let name = Rect::new(x, t.y, (t.right() - x).max(0.0), t.h);
            self.label(&e.name, &self.text.inspector_title, &name, p.text, 1.0);
        }
        Ok(())
    }

    /// Several files' pictures, stacked in the picture's place: up to
    /// three cards, each a step right and down from the one behind, the
    /// first selected file's at the front.
    fn stack(&self, hero: &Rect, rows: &[usize], v: &View) {
        let p = v.palette;
        let n = rows.len().min(3);
        if hero.h <= 0.0 || n == 0 {
            return;
        }
        let (dx, dy) = CARD_STEP;
        let (w, h) = (hero.w - 2.0 * dx, hero.h - 2.0 * dy);
        let steps = (n - 1) as f32;
        let x0 = hero.x + (hero.w - w - dx * steps) / 2.0;
        let y0 = hero.y + (hero.h - h - dy * steps) / 2.0;
        let icon = (HERO_ICON_SCALE, MARK_LINE);
        for back in 0..n {
            let i = rows[n - 1 - back];
            let card = Rect::new(x0 + dx * back as f32, y0 + dy * back as f32, w, h);
            // A line of the surface parts each card from the one behind.
            let around = Rect::new(card.x - 1.0, card.y - 1.0, card.w + 2.0, card.h + 2.0);
            self.border(&around, HERO_RADIUS + 1.0, p.surface, 1.0, shape::BORDER);
            let picture = (v.preview)(i, PreviewSize::Hero);
            let (g, kind) = ((v.glaze)(i), (v.kind)(i));
            self.preview(&card, HERO_RADIUS, picture, g, kind, icon, 1.0, p);
            self.border(&card, HERO_RADIUS, p.edge, 1.0, shape::BORDER);
        }
    }

    /// A converting file's heat over its picture: its glaze rising from
    /// the bottom to `fraction` of the way up, with a brighter edge where
    /// it has got to, breathing with `heat`. Without a fraction the whole
    /// picture holds a heat that breathes. A contrast theme has no washes:
    /// the level is a line in its highlight colour.
    fn hero_heat(
        &self,
        hero: &Rect,
        fraction: Option<f32>,
        g: Glaze,
        heat: f32,
        p: &Palette,
    ) -> Result<()> {
        let f = fraction.unwrap_or(1.0).clamp(0.0, 1.0);
        if f <= 0.0 {
            return Ok(());
        }
        let top = hero.bottom() - hero.h * f;
        if p.high_contrast {
            if fraction.is_some() {
                let level = Rect::new(hero.x, (top - 1.0).max(hero.y), hero.w, 2.0);
                self.fill(&level, 0.0, p.accent_strong, 1.0);
            }
            return Ok(());
        }
        let breath = 1.0 - HEAT_BREATH * heat.clamp(0.0, 1.0);
        self.clipped(hero, HERO_RADIUS, || {
            let ramp = self.glaze_ramp(g);
            unsafe {
                ramp.SetStartPoint(pt(hero.x, top));
                ramp.SetEndPoint(pt(hero.x, hero.bottom()));
                ramp.SetOpacity(HERO_HEAT * breath);
                let filled = Rect::new(hero.x, top, hero.w, hero.bottom() - top);
                self.rt.FillRectangle(&rectf(&filled), ramp);
            }
            if fraction.is_some() && f < 1.0 {
                let edge = Rect::new(hero.x, top - 1.0, hero.w, 2.0);
                self.fill(&edge, 0.0, p.glaze(g), breath);
            }
            Ok(())
        })
    }

    /// A finished file's check, a celadon disc at the picture's corner, at
    /// `a` as it fades in.
    fn check(&self, hero: &Rect, a: f32, p: &Palette) {
        if a <= 0.0 {
            return;
        }
        let cx = hero.right() - CHECK_INSET - CHECK_R;
        let cy = hero.bottom() - CHECK_INSET - CHECK_R;
        let disc = D2D1_ELLIPSE {
            point: pt(cx, cy),
            radiusX: CHECK_R,
            radiusY: CHECK_R,
        };
        let tick = [
            (cx - 6.0, cy + 0.5),
            (cx - 2.0, cy + 4.5),
            (cx + 6.0, cy - 4.5),
        ];
        unsafe {
            self.rt.FillEllipse(&disc, self.paint(p.accent, a));
            for w in tick.windows(2) {
                self.rt.DrawLine(
                    pt(w[0].0, w[0].1),
                    pt(w[1].0, w[1].1),
                    self.paint(p.on_accent, a),
                    2.0,
                    self.round,
                );
            }
        }
    }

    /// The inspector's controls where its layout placed them: each format
    /// or kind as a row, More as a link, and the rest as buttons. Each
    /// eases its hover and press; what it said before takes neither.
    fn inspector_controls(&self, iv: &InspectorPage, v: &View) -> Result<()> {
        use InspectorControl as C;
        let p = v.palette;
        let targets = match iv.model {
            Inspector::Waiting { targets, .. } | Inspector::Several { targets, .. } => &targets[..],
            _ => &[],
        };
        let offered = match iv.model {
            Inspector::Several { offered, .. } => &offered[..],
            _ => &[],
        };
        for &(c, b) in iv.controls {
            let (hot, pressed) = if iv.live {
                (v.button)(ButtonId::Window(FocusButton::Inspector(c)))
            } else {
                (0.0, 0.0)
            };
            match c {
                C::Target(k) => {
                    let Some(t) = targets.get(k) else {
                        continue;
                    };
                    let g = iv.glazes.get(k).copied().unwrap_or(Glaze::Other);
                    let estimate = t.estimate_label();
                    let row = TargetRow {
                        code: &t.ext,
                        glaze: g,
                        why: t.why,
                        estimate: estimate.as_deref(),
                        suggested: t.suggested,
                    };
                    self.target_row(&b, &row, hot, pressed, p)?;
                }
                C::Kind(k) => {
                    let Some((code, n)) = offered.get(k) else {
                        continue;
                    };
                    let g = iv.glazes.get(k).copied().unwrap_or(Glaze::Other);
                    let files = insp::files_count(*n);
                    let row = TargetRow {
                        code,
                        glaze: g,
                        why: Some(&files),
                        estimate: None,
                        suggested: false,
                    };
                    self.target_row(&b, &row, hot, pressed, p)?;
                }
                C::More => self.more_link(&b, hot, p),
                _ => {
                    if let Some(label) = iv.model.label(c) {
                        self.button(label, &b, hot, pressed, p);
                    }
                }
            }
        }
        Ok(())
    }

    /// A format or a kind the inspector offers, as a 6 DIP rounded row:
    /// its chip, what it is for (muted) and its estimate at the right. The
    /// suggestion has a celadon edge and wash, and its dot before the chip.
    /// `hover` and `pressed` are how far each has eased in, 0 to 1; a
    /// contrast theme, which has no in-between, takes either once it is
    /// more than half in.
    fn target_row(
        &self,
        b: &Rect,
        row: &TargetRow,
        hover: f32,
        pressed: f32,
        p: &Palette,
    ) -> Result<()> {
        let radius = shape::RADIUS_CONTROL;
        let hot = hover > 0.5;
        if hover > 0.0 && !p.high_contrast {
            self.fill(b, radius, p.line, hover);
        }
        if row.suggested {
            self.wash(b, radius, p.accent, SUGGESTED_WASH, p);
            if !p.high_contrast {
                self.border(b, radius, p.accent, SUGGESTED_EDGE, shape::BORDER);
            } else if hot {
                // A contrast theme's suggestion is already outlined in its
                // highlight colour: hover adds a ring in the text colour
                // inside it.
                let inner = b.inset(SUGGESTED_HOVER_INSET);
                let r = (radius - SUGGESTED_HOVER_INSET).max(0.0);
                self.border(&inner, r, p.text, 1.0, shape::BORDER);
            }
        } else {
            let edge = if hot && p.high_contrast {
                p.text
            } else {
                p.edge
            };
            self.border(b, radius, edge, 1.0, shape::BORDER);
        }
        self.press_shadow(b, pressed, p);
        // Muted text on a wash is a step brighter (darker in light).
        let washed = if row.suggested { 1.0 } else { hover };
        let muted = blend(p, p.text_muted, p.muted_on_wash, washed);
        let cy = b.y + b.h / 2.0;
        if row.suggested {
            self.dot(b.x + TARGET_INSET / 2.0, cy, 1.0, p);
        }
        let chip_w = code_chip_width(self.text, row.code, self.chip_floor)?;
        let chip = Rect::new(
            b.x + TARGET_INSET,
            cy - CHIP_HEIGHT / 2.0,
            chip_w,
            CHIP_HEIGHT,
        );
        self.chip(row.code, &chip, row.glaze, 1.0, p)?;
        let mut right = b.right() - TARGET_END;
        if let Some(estimate) = row.estimate {
            let at = Rect::new(chip.right(), b.y, (right - chip.right()).max(0.0), b.h);
            right -= self.tabular(estimate, false, &at, muted)? + 8.0;
        }
        if let Some(why) = row.why {
            let x = chip.right() + 8.0;
            let at = Rect::new(x, b.y, (right - x).max(0.0), b.h);
            self.label(why, &self.text.caption, &at, muted, 1.0);
        }
        Ok(())
    }

    /// "More formats…": an underlined link at the left of its line, easing
    /// to full text colour as `hover` comes in. The layout made `r` as wide
    /// as its words and a link's room past them, so they are not measured
    /// again here.
    fn more_link(&self, r: &Rect, hover: f32, p: &Palette) {
        let c = blend(p, p.link, p.text, hover);
        let tw = (r.w - layout::LINK_PAD).max(0.0);
        let at = Rect::new(r.x, r.y, tw + 1.0, r.h);
        self.label(insp::MORE_FORMATS, &self.text.caption, &at, c, 1.0);
        let under = r.y + r.h / 2.0 + 5.0;
        self.hline(r.x, r.x + tw, under, c, solid(p, effects::UNDERLINE));
    }

    fn footer(&self, l: &Layout, v: &View) -> Result<()> {
        let p = v.palette;
        let (Some(footer), Some(button)) = (l.footer, l.clear_button) else {
            return Ok(());
        };
        let export = v
            .export_all
            .map(|label| (label, layout::export_all_button(&button)));
        let add = layout::add_files_button(&button, export.is_some());
        let convert = layout::convert_button(&footer, &button, export.is_some());
        let status = layout::footer_status(&footer, &convert);
        // The idle hint comes in two lengths: the full one where it fits.
        let text = if v.status == ways_in::IDLE_HINT {
            let full = self.text.measure(ways_in::IDLE_HINT, &self.text.caption);
            ways_in::fit_hint(status.w, full)
        } else {
            v.status
        };
        self.label(text, &self.text.caption, &status, p.text_muted, 1.0);
        if let (true, Some(link)) = (v.undo, layout::undo_link(&status)) {
            self.row_link_label("Undo", &link, false, false, p, 1.0);
        }
        if v.convert_enabled {
            // The footer's one primary action stands raised: its shadow, and
            // the light along its top, which a press sinks out of sight.
            self.shadow(&convert, shape::RADIUS_CONTROL, p);
            let (hover, pressed) = (v.button)(ButtonId::Window(FocusButton::Convert));
            if v.convert_key && layout::convert_keycap(&convert) {
                // Convert names its key after the word.
                self.primary_face(&convert, hover, pressed, p);
                self.keyed(v.convert_label, CONVERT_KEY, &convert, p.on_accent, p);
            } else {
                self.primary(v.convert_label, &convert, hover, pressed, p);
            }
            self.highlight_at(&convert, shape::RADIUS_CONTROL, 0.0, 1.0 - pressed, p);
        } else {
            // Disabled: the label is muted and the edge flat.
            self.button_face(&convert, 0.0, 0.0, false, p);
            self.label(
                "Convert",
                &self.text.body_center,
                &convert,
                p.text_muted,
                1.0,
            );
        }
        let (hover, pressed) = (v.button)(ButtonId::Window(FocusButton::AddFiles));
        self.button(v.empty.button, &add, hover, pressed, p);
        if let Some((label, b)) = export {
            let (hover, pressed) = (v.button)(ButtonId::Window(FocusButton::ExportAll));
            self.button(label, &b, hover, pressed, p);
        }
        let (hover, pressed) = (v.button)(ButtonId::Window(FocusButton::Clear));
        self.button("Clear", &button, hover, pressed, p);
        Ok(())
    }

    /// A quiet secondary control. Pressed: a soft inner shadow, as if the
    /// surface gave a little under the finger. `hover` and `pressed` are
    /// how far each has eased in, 0 to 1.
    fn button(&self, text: &str, b: &Rect, hover: f32, pressed: f32, p: &Palette) {
        self.button_face(b, hover, pressed, false, p);
        self.label(text, &self.text.body_center, b, p.text, 1.0);
    }

    /// A button's fill and border, its fill lifting into the line colour
    /// as `hover` comes in. Selected: the celadon wash and edge. A contrast
    /// theme, which has no in-between, outlines a hover once it is more
    /// than half in.
    fn button_face(&self, b: &Rect, hover: f32, pressed: f32, selected: bool, p: &Palette) {
        let fill = if p.high_contrast {
            p.surface
        } else {
            blend(p, p.surface, p.line, hover)
        };
        self.fill(b, shape::RADIUS_CONTROL, fill, 1.0);
        if selected && !p.high_contrast {
            self.fill(b, shape::RADIUS_CONTROL, p.accent, effects::HOVER_FILL);
        }
        self.press_shadow(b, pressed, p);
        let (edge, width) = match (selected, hover > 0.5, p.high_contrast) {
            (true, _, true) => (p.accent, 2.0),
            (true, _, false) => (p.accent_strong, shape::BORDER),
            (false, true, true) => (p.text, shape::BORDER),
            _ => (p.edge, shape::BORDER),
        };
        self.border(b, shape::RADIUS_CONTROL, edge, 1.0, width);
    }

    /// The soft inner shadow of a pressed control, along its top edge, as
    /// far as the press has eased in. A contrast theme has no shadows.
    fn press_shadow(&self, b: &Rect, pressed: f32, p: &Palette) {
        if p.high_contrast || pressed <= 0.0 {
            return;
        }
        for (k, alpha) in [(1.0, 0.35), (2.0, 0.2), (3.0, 0.1)] {
            let inner = Rect::new(b.x + 1.0, b.y + k, b.w - 2.0, 1.0);
            self.fill(&inner, 0.0, p.ground, alpha * pressed);
        }
    }

    /// The emphasised button: celadon, for the one action a panel is for.
    fn primary(&self, text: &str, b: &Rect, hover: f32, pressed: f32, p: &Palette) {
        self.primary_face(b, hover, pressed, p);
        self.label(text, &self.text.body_center, b, p.on_accent, 1.0);
    }

    /// A button's word and, after it, the key that presses it in a keycap
    /// (a fine outline of the text colour `c`), centred in `b` together.
    fn keyed(&self, word: &str, key: &str, b: &Rect, c: Rgb, p: &Palette) {
        let word_w = self.text.measure(word, &self.text.body);
        let key_w = self.text.measure(key, &self.text.caption);
        let cap_w = (key_w + 2.0 * KEYCAP_PAD).ceil();
        let x = (b.x + (b.w - (word_w + KEYCAP_GAP + cap_w)) / 2.0).round();
        let words = Rect::new(x, b.y, word_w + 1.0, b.h);
        self.label(word, &self.text.body, &words, c, 1.0);
        let cap = Rect::new(
            x + word_w + KEYCAP_GAP,
            b.y + (b.h - KEYCAP_HEIGHT) / 2.0,
            cap_w,
            KEYCAP_HEIGHT,
        );
        // Solid under a contrast theme, which has no in-between alphas.
        let outline = solid(p, KEYCAP_OUTLINE);
        self.border(&cap, KEYCAP_RADIUS, c, outline, shape::BORDER);
        self.label(key, &self.text.caption_center, &cap, c, 1.0);
    }

    /// The primary button's fill, moving `effects::PRIMARY_HOVER` toward
    /// the text colour as `hover` comes in, and its press.
    fn primary_face(&self, b: &Rect, hover: f32, pressed: f32, p: &Palette) {
        let fill = blend(p, p.accent, p.text, effects::PRIMARY_HOVER * hover);
        self.fill(b, shape::RADIUS_CONTROL, fill, 1.0);
        // A contrast theme has no in-between fill to hover to: a 2 DIP
        // line in the highlight's text colour, just inside, shows it.
        if hover > 0.5 && p.high_contrast {
            let inner = b.inset(2.0);
            self.border(&inner, shape::RADIUS_CONTROL - 2.0, p.on_accent, 1.0, 2.0);
        }
        self.press_shadow(b, pressed, p);
    }

    /// A control that steps through values: the value between two small
    /// chevrons. The left end steps back, the rest forward.
    fn stepper(&self, text: &str, b: &Rect, hover: bool, pressed: bool, p: &Palette) {
        self.button_face(b, amount(hover), amount(pressed), false, p);
        let inner = Rect::new(b.x + 20.0, b.y, (b.w - 40.0).max(0.0), b.h);
        self.label(text, &self.text.body_center, &inner, p.text, 1.0);
        let c = if hover { p.text } else { p.text_muted };
        let cy = b.y + b.h / 2.0;
        self.chevron(b.x + 13.0, cy, -1.0, c);
        self.chevron(b.right() - 13.0, cy, 1.0, c);
    }

    /// A small chevron at `(x, y)`, pointing right for `dir` 1, left for -1.
    fn chevron(&self, x: f32, y: f32, dir: f32, c: Rgb) {
        let (tip, back) = (x + 2.0 * dir, x - 2.0 * dir);
        unsafe {
            for (a, b) in [((back, y - 4.0), (tip, y)), ((tip, y), (back, y + 4.0))] {
                self.rt.DrawLine(
                    pt(a.0, a.1),
                    pt(b.0, b.1),
                    self.paint(c, 1.0),
                    shape::ICON_STROKE,
                    self.round,
                );
            }
        }
    }

    /// One segment of a segmented control: fills here, and one shared
    /// outline from `segment_group`, so neighbours never double up.
    fn segment(
        &self,
        text: &str,
        r: &Rect,
        hover: bool,
        pressed: bool,
        selected: bool,
        p: &Palette,
    ) {
        let fill = if hover && !p.high_contrast {
            p.line
        } else {
            p.surface
        };
        self.fill(r, shape::RADIUS_CONTROL, fill, 1.0);
        if selected && !p.high_contrast {
            self.fill(r, shape::RADIUS_CONTROL, p.accent, effects::HOVER_FILL);
        }
        if hover && p.high_contrast {
            self.border(
                &r.inset(1.0),
                shape::RADIUS_CONTROL,
                p.text,
                1.0,
                shape::BORDER,
            );
        }
        self.press_shadow(r, amount(pressed), p);
        self.label(text, &self.text.body_center, r, p.text, 1.0);
    }

    /// A segmented control's outline: one border round the group,
    /// hairline dividers, and the chosen segment's celadon edge on top.
    fn segment_group(&self, segs: &[(bool, Rect)], p: &Palette) {
        let (Some(first), Some(last)) = (segs.first(), segs.last()) else {
            return;
        };
        let group = Rect::new(first.1.x, first.1.y, last.1.right() - first.1.x, first.1.h);
        self.border(&group, shape::RADIUS_CONTROL, p.edge, 1.0, shape::BORDER);
        for (_, r) in &segs[1..] {
            unsafe {
                self.rt.DrawLine(
                    pt(r.x + 0.5, r.y + 1.0),
                    pt(r.x + 0.5, r.bottom() - 1.0),
                    self.paint(p.line, 1.0),
                    shape::BORDER,
                    None,
                );
            }
        }
        if let Some((_, r)) = segs.iter().find(|(chosen, _)| *chosen) {
            self.chosen_edge(r, p);
        }
    }

    /// The edge round a chosen segment or tab: strong celadon, or the
    /// contrast theme's highlight, wider.
    fn chosen_edge(&self, r: &Rect, p: &Palette) {
        let width = if p.high_contrast { 2.0 } else { shape::BORDER };
        self.border(r, shape::RADIUS_CONTROL, p.accent_strong, 1.0, width);
    }

    /// The keyboard focus ring: a strong celadon line just outside the
    /// control.
    fn focus_ring(&self, b: &Rect, p: &Palette) {
        let r = Rect::new(b.x - 3.0, b.y - 3.0, b.w + 6.0, b.h + 6.0);
        self.border(
            &r,
            shape::RADIUS_CONTROL + 3.0,
            focus_colour(p),
            1.0,
            shape::ICON_STROKE,
        );
    }

    /// The glaze wheel, drawn in its own coordinates around its centre.
    fn wheel(&self, wv: &WheelView, p: &Palette) -> Result<()> {
        let w = wv.wheel;
        // The bloom: the disc grows and turns into place as it fades in.
        let (open, grow, turn) = anim::bloom(wv.open_ms, wv.animations);
        let opacity = (open * wv.fade).clamp(0.0, 1.0);
        if opacity <= 0.0 {
            return Ok(());
        }
        let base = turn_scale_at(turn, grow * w.scale, w.center.0, w.center.1);
        let result = self.with_opacity(opacity, || {
            // The wheel stands raised: one soft shadow under all of it,
            // reaching as far as More's ring has poured out.
            self.set_base(base);
            let more = if w.more.is_empty() { 0.0 } else { wv.more };
            let ring = R_MORE_OUTER * (0.66 + 0.34 * more);
            self.disc_shadow(R_OUTER + (ring - R_OUTER).max(0.0) * more, p);
            // More's ring pours out from behind the dish.
            if wv.more > 0.0 && !w.more.is_empty() {
                let k = 0.66 + 0.34 * wv.more;
                self.set_base(compose(&scale_at(k, 0.0, 0.0), &base));
                self.with_opacity(wv.more, || self.outer_ring(wv, p))?;
            }
            self.set_base(base);
            self.inner_ring(wv, p)
        });
        self.set_base(Matrix3x2::identity());
        result
    }

    /// How strongly the pointed-at or pulsing spot is lit.
    fn lit(wv: &WheelView) -> (Option<Spot>, f32) {
        let pulse = wv
            .pulse
            .map(|(_, t)| (t.clamp(0.0, 1.0) * PI).sin() * effects::PULSE_PEAK)
            .unwrap_or(0.0);
        (
            wv.pulse.map(|(s, _)| s).or(wv.hover),
            effects::HOVER_FILL + pulse,
        )
    }

    /// A celadon wash on a segment, and a fine arc on its outer edge.
    fn light(&self, b: Band, wash: f32, edge: f32, p: &Palette) -> Result<()> {
        let Band {
            a0,
            a1,
            r0,
            r1,
            full,
        } = b;
        let sector = self.sector(a0, a1, r0, r1, full)?;
        unsafe {
            if !p.high_contrast {
                self.rt
                    .FillGeometry(&sector, self.paint(p.accent, wash), None);
            } else if wash > 0.0 {
                // No wash under a contrast theme: the segment is outlined.
                self.rt
                    .DrawGeometry(&sector, self.paint(p.accent, 1.0), 2.0, None);
            }
        }
        if edge > 0.0 {
            let arc = self.arc(a0, a1, r1 - 0.75, full)?;
            unsafe {
                self.rt.DrawGeometry(
                    &arc,
                    self.paint(p.accent_strong, solid(p, edge)),
                    shape::ICON_STROKE,
                    self.round,
                );
            }
        }
        Ok(())
    }

    fn inner_ring(&self, wv: &WheelView, p: &Palette) -> Result<()> {
        let w = wv.wheel;
        let n = w.segments();
        // The dish.
        self.circle(R_OUTER, p.surface, 1.0)?;
        self.ring_edge(R_OUTER - 0.5, p.line)?;
        self.top_light(R_OUTER - 1.5, p)?;

        // Hover or pulse: a celadon wash and a fine arc. Icons keep their
        // colour.
        let (lit, wash) = Self::lit(wv);
        if let Some(Spot::Inner(i)) = lit {
            let (a0, a1) = wheel::segment(i, n);
            self.light(Band::new(a0, a1, R_INNER, R_OUTER, n == 1), wash, 1.0, p)?;
        }
        // While its ring is out, More stays softly lit as the way back.
        let more = w.items.len();
        if wv.more > 0.0 && w.is_more(more) && lit != Some(Spot::Inner(more)) {
            let (a0, a1) = wheel::segment(more, n);
            let k = wv.more;
            let band = Band::new(a0, a1, R_INNER, R_OUTER, false);
            self.light(band, effects::HOVER_FILL * 0.5 * k, 0.5 * k, p)?;
        }

        // Dividers between segments.
        if n > 1 {
            for i in 0..n {
                let (a0, _) = wheel::segment(i, n);
                let (c, si) = (a0.cos(), a0.sin());
                unsafe {
                    self.rt.DrawLine(
                        pt(c * R_INNER, si * R_INNER),
                        pt(c * R_OUTER, si * R_OUTER),
                        self.paint(p.line, 1.0),
                        shape::BORDER,
                        None,
                    );
                }
            }
        }

        // Items: icon above, label below. Each segment's follows the disc
        // in turn, growing into place about its middle as it fades in.
        let disc = self.base.get();
        for i in 0..n {
            let (lx, ly) = wheel::label_point(i, n);
            let (icon, label, is_code) = match w.items.get(i) {
                Some(item) => (item.icon, item.label.as_str(), item.is_code),
                None => ("more", "More", false),
            };
            let (a, grow) = anim::segment(wv.open_ms, i, wv.animations);
            if a <= 0.0 {
                continue;
            }
            self.set_base(compose(&scale_about(grow, lx, ly - 9.0), &disc));
            let top = ly - 22.0;
            self.icon(icon, lx - 12.0, top, 1.0, p.text, a);
            if wv.marks.contains(&Spot::Inner(i)) {
                self.dot(lx, top - DOT_GAP, wv.dot * a, p);
            }
            let drawn = if is_code {
                let r = Rect::new(lx - 40.0, ly + 4.0, 80.0, 16.0);
                self.code(label, &r, p.text, a).map(|_| ())
            } else {
                let r = Rect::new(lx - 52.0, ly + 4.0, 104.0, 16.0);
                self.label(label, &self.text.caption_center, &r, p.text, a);
                Ok(())
            };
            self.set_base(disc);
            drawn?;
        }

        // The centre: the title "Convert · Shift" always drawn on two
        // lines, the name in the serif over the key that opens it (the
        // whole title is too wide for the centre on one), then the source
        // type, or where a conversion goes while one is pointed at:
        // "PNG → JPG".
        self.circle(R_INNER, p.ground, 1.0)?;
        self.ring_edge(R_INNER - 0.5, p.line)?;
        let (name, key) = wv
            .title
            .split_once(" · ")
            .unwrap_or((wv.title.as_str(), ""));
        let c = layout::wheel_centre(R_INNER);
        self.label(name, &self.text.display_center, &c.name, p.text, 1.0);
        match wv.options {
            // A link: underlined, brighter under the pointer.
            Some(hot) => {
                let col = if hot { p.text } else { p.link };
                let text = wheel::OPTIONS_TARGET;
                self.label(text, &self.text.caption_center, &c.key, col, 1.0);
                let tw = self.text.measure(text, &self.text.caption).min(c.key.w);
                let under = solid(p, effects::UNDERLINE);
                self.hline(-tw / 2.0, tw / 2.0, c.key.y + 14.0, col, under);
            }
            None => self.label(key, &self.text.caption_center, &c.key, p.text_muted, 1.0),
        }
        let line = match lit.and_then(|s| w.item(s)) {
            Some(item) if matches!(item.action, Action::Convert(_)) => {
                format!("{} → {}", w.source_code, item.label)
            }
            _ => w.source_code.clone(),
        };
        self.code(&line, &c.source, p.text_muted, 1.0)?;
        Ok(())
    }

    /// The ring More pours open: every other format, grouped by kind, each
    /// group led by its kind's icon and name.
    fn outer_ring(&self, wv: &WheelView, p: &Palette) -> Result<()> {
        let w = wv.wheel;
        let slots = wheel::outer_slots(&w.more);
        let band = self.sector(0.0, TAU, R_MORE_INNER, R_MORE_OUTER, true)?;
        // The groove between the dish and this ring is solid ground, so
        // the list behind never shows through it.
        let groove = self.sector(0.0, TAU, R_OUTER - 1.0, R_MORE_INNER, true)?;
        unsafe {
            self.rt
                .FillGeometry(&band, self.paint(p.surface, 1.0), None);
            self.rt
                .FillGeometry(&groove, self.paint(p.ground, 1.0), None);
        }
        self.ring_edge(R_MORE_INNER + 0.5, p.line)?;
        self.ring_edge(R_MORE_OUTER - 0.5, p.line)?;
        self.top_light(R_MORE_OUTER - 1.5, p)?;

        let (lit, wash) = Self::lit(wv);
        if let Some(Spot::Outer(j)) = lit {
            if let Some(slot) = slots.iter().find(|s| s.item == Some(j)) {
                let band = Band::new(slot.a0, slot.a1, R_MORE_INNER, R_MORE_OUTER, false);
                self.light(band, wash, 1.0, p)?;
            }
        }

        // Dividers only where a group starts.
        for slot in slots.iter().filter(|s| s.item.is_none()) {
            let (c, si) = (slot.a0.cos(), slot.a0.sin());
            unsafe {
                self.rt.DrawLine(
                    pt(c * R_MORE_INNER, si * R_MORE_INNER),
                    pt(c * R_MORE_OUTER, si * R_MORE_OUTER),
                    self.paint(p.line, 1.0),
                    shape::BORDER,
                    None,
                );
            }
        }

        for slot in &slots {
            let (x, y) = wheel::outer_point(slot);
            match slot.item {
                // A group's kind, small and quiet, with its name under it.
                None => {
                    if let Some((icon, name)) = wheel::heading_boxes(slot) {
                        self.icon(slot.icon, icon.0, icon.1, 0.75, p.text_muted, 1.0);
                        let r = Rect::new(name.0, name.1, name.2, name.3);
                        self.label(
                            slot.heading.unwrap_or_default(),
                            &self.text.ring_heading,
                            &r,
                            p.text_muted,
                            1.0,
                        );
                    }
                }
                Some(j) => {
                    let r = Rect::new(x - 26.0, y - 8.0, 52.0, 16.0);
                    self.code(&w.more[j].label, &r, p.text, 1.0)?;
                    if wv.marks.contains(&Spot::Outer(j)) {
                        self.dot(x, r.y - DOT_GAP, wv.dot, p);
                    }
                }
            }
        }
        Ok(())
    }

    /// The suggestion's celadon dot, centred on `(x, y)`. It takes the
    /// ring's own opacity, and never moves or changes size.
    fn dot(&self, x: f32, y: f32, a: f32, p: &Palette) {
        unsafe {
            self.rt.FillEllipse(
                &D2D1_ELLIPSE {
                    point: pt(x, y),
                    radiusX: DOT_R,
                    radiusY: DOT_R,
                },
                self.paint(p.accent_strong, a),
            );
        }
    }

    fn circle(&self, r: f32, c: Rgb, a: f32) -> Result<()> {
        unsafe {
            self.rt.FillEllipse(
                &D2D1_ELLIPSE {
                    point: pt(0.0, 0.0),
                    radiusX: r,
                    radiusY: r,
                },
                self.paint(c, a),
            );
        }
        Ok(())
    }

    /// The wheel's shadow: the shadow's layers as discs round its centre,
    /// each `reach` grown by its amount, offset down. A contrast theme has
    /// none.
    fn disc_shadow(&self, reach: f32, p: &Palette) {
        if p.high_contrast {
            return;
        }
        for &(grow, share) in self.shadow {
            let (r, a) = (reach + grow, p.shadow_alpha * share);
            if r <= 0.0 || a <= 0.0 {
                continue;
            }
            unsafe {
                self.rt.FillEllipse(
                    &D2D1_ELLIPSE {
                        point: pt(0.0, depth::OFFSET_Y),
                        radiusX: r,
                        radiusY: r,
                    },
                    self.paint(p.shadow, a),
                );
            }
        }
    }

    /// The 1 px light along the top of a raised disc: an arc at radius `r`
    /// across its top, a little under a third of the way round. A contrast
    /// theme has none.
    fn top_light(&self, r: f32, p: &Palette) -> Result<()> {
        if p.high_contrast || p.highlight_alpha <= 0.0 {
            return Ok(());
        }
        let arc = self.arc(1.2 * PI, 1.8 * PI, r, false)?;
        unsafe {
            self.rt.DrawGeometry(
                &arc,
                self.paint(p.highlight, p.highlight_alpha),
                shape::BORDER,
                self.round,
            );
        }
        Ok(())
    }

    fn ring_edge(&self, r: f32, c: Rgb) -> Result<()> {
        unsafe {
            self.rt.DrawEllipse(
                &D2D1_ELLIPSE {
                    point: pt(0.0, 0.0),
                    radiusX: r,
                    radiusY: r,
                },
                self.paint(c, 1.0),
                shape::BORDER,
                None,
            );
        }
        Ok(())
    }

    /// An annular sector from angle `a0` to `a1`. With `full`, the whole
    /// ring.
    fn sector(&self, a0: f32, a1: f32, r0: f32, r1: f32, full: bool) -> Result<ID2D1PathGeometry> {
        unsafe {
            let g = self.factory.CreatePathGeometry()?;
            let sink = g.Open()?;
            if full {
                sink.SetFillMode(D2D1_FILL_MODE_ALTERNATE);
                for r in [r1, r0] {
                    sink.BeginFigure(pt(r, 0.0), D2D1_FIGURE_BEGIN_FILLED);
                    arc_to(&sink, r, 0.0, TAU);
                    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                }
            } else {
                sink.BeginFigure(pt(r1 * a0.cos(), r1 * a0.sin()), D2D1_FIGURE_BEGIN_FILLED);
                arc_to(&sink, r1, a0, a1);
                sink.AddLine(pt(r0 * a1.cos(), r0 * a1.sin()));
                arc_to(&sink, r0, a1, a0);
                sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            }
            sink.Close()?;
            Ok(g)
        }
    }

    /// An open arc at radius `r`, for the hover edge.
    fn arc(&self, a0: f32, a1: f32, r: f32, full: bool) -> Result<ID2D1PathGeometry> {
        let (a0, a1) = if full { (0.0, TAU) } else { (a0, a1) };
        unsafe {
            let g = self.factory.CreatePathGeometry()?;
            let sink = g.Open()?;
            sink.BeginFigure(pt(r * a0.cos(), r * a0.sin()), D2D1_FIGURE_BEGIN_HOLLOW);
            arc_to(&sink, r, a0, a1);
            sink.EndFigure(if full {
                D2D1_FIGURE_END_CLOSED
            } else {
                D2D1_FIGURE_END_OPEN
            });
            sink.Close()?;
            Ok(g)
        }
    }

    /// What a failed job's engine said, over a scrim. The text scrolls
    /// inside the body; focus starts on Copy.
    fn dialog(&self, w: f32, h: f32, dv: &DetailsView, p: &Palette) -> Result<()> {
        let d = layout::dialog(w, h);
        // A scrim quiets the window behind the dialog, which stands raised
        // over it.
        self.scrim(w, h, 1.0, p);
        self.raised_panel(&d.panel, p);
        self.label(dv.title, &self.text.display, &d.title, p.text, 1.0);

        let body = d.body;
        let content = self
            .text
            .block_height(dv.text, &self.text.mono_block, body.w);
        let scroll = layout::clamp_text_scroll(dv.scroll, content, body.h);
        unsafe {
            self.rt
                .PushAxisAlignedClip(&rectf(&body), D2D1_ANTIALIAS_MODE_ALIASED);
            self.rt.DrawText(
                &wide(dv.text),
                &self.text.mono_block,
                &rectf(&Rect::new(
                    body.x,
                    body.y - scroll,
                    body.w,
                    content.max(body.h) + 1.0,
                )),
                self.paint(p.text_muted, 1.0),
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
            self.rt.PopAxisAlignedClip();
        }
        // A thin thumb, as on the list, when the text runs past the body.
        let max = (content - body.h).max(0.0);
        if max > 0.0 {
            let thumb_h = (body.h * body.h / content).max(24.0).min(body.h);
            let thumb_y = body.y + (body.h - thumb_h) * (scroll / max);
            let thumb = Rect::new(d.panel.right() - 8.0, thumb_y, 2.0, thumb_h);
            self.thumb(&thumb, p);
        }

        for (b, text) in [
            (DialogButton::Copy, dv.copy_label),
            (DialogButton::Close, "Close"),
        ] {
            let r = d.button(b);
            let (hover, pressed) = (dv.button)(ButtonId::Details(b));
            self.button(text, &r, hover, pressed, p);
            if dv.focus == b {
                self.focus_ring(&r, p);
            }
        }
        Ok(())
    }

    /// Help, over a scrim like the details dialog: the ways in, how results
    /// are kept, and every shortcut. The text scrolls inside the body. It
    /// rises into place as it fades in, with the scrim fading in behind it.
    fn help(&self, w: f32, h: f32, hv: &HelpView, p: &Palette) -> Result<()> {
        let a = hv.open.clamp(0.0, 1.0);
        if a <= 0.0 {
            return Ok(());
        }
        self.scrim(w, h, a, p);
        let result = self.with_opacity(a, || {
            self.set_base(moved(0.0, hv.rise));
            self.help_panel(w, h, hv, p)
        });
        self.set_base(Matrix3x2::identity());
        result
    }

    /// Help's panel, its text and its Close.
    fn help_panel(&self, w: f32, h: f32, hv: &HelpView, p: &Palette) -> Result<()> {
        let d = layout::help(w, h);
        self.raised_panel(&d.panel, p);
        self.label(help::TITLE, &self.text.display, &d.title, p.text, 1.0);

        let body = d.body;
        let (placed, content) = help::place(hv.sections, body.w, |s, width| {
            self.text.block_height(s, &self.text.body_block, width)
        });
        let scroll = layout::clamp_text_scroll(hv.scroll, content, body.h);
        unsafe {
            self.rt
                .PushAxisAlignedClip(&rectf(&body), D2D1_ANTIALIAS_MODE_ALIASED);
        }
        for piece in &placed {
            let (text, r, format, c) = match *piece {
                Placed::Heading(t, r) => (t, r, &self.text.heading, p.text),
                Placed::Keys(t, r) => (t, r, &self.text.body_block, p.text),
                Placed::What(t, r) => (t, r, &self.text.body_block, p.text_muted),
            };
            let r = Rect::new(body.x + r.x, body.y + r.y - scroll, r.w, r.h + 1.0);
            if r.bottom() < body.y || r.y > body.bottom() {
                continue;
            }
            self.label(text, format, &r, c, 1.0);
        }
        unsafe { self.rt.PopAxisAlignedClip() };
        let max = (content - body.h).max(0.0);
        if max > 0.0 {
            let thumb_h = (body.h * body.h / content).max(24.0).min(body.h);
            let thumb_y = body.y + (body.h - thumb_h) * (scroll / max);
            let thumb = Rect::new(d.panel.right() - 8.0, thumb_y, 2.0, thumb_h);
            self.thumb(&thumb, p);
        }
        let (hover, pressed) = (hv.button)(ButtonId::Help);
        self.button("Close", &d.close, hover, pressed, p);
        self.focus_ring(&d.close, p);
        Ok(())
    }

    /// The settings panel, over a scrim. Changes apply as they are made,
    /// so there is only Close. It rises into place as it fades in, with the
    /// scrim fading in behind it.
    fn settings(&self, w: f32, h: f32, sv: &SettingsView, p: &Palette) -> Result<()> {
        let a = sv.open.clamp(0.0, 1.0);
        if a <= 0.0 {
            return Ok(());
        }
        self.scrim(w, h, a, p);
        let result = self.with_opacity(a, || {
            self.set_base(moved(0.0, sv.rise));
            self.settings_panel(sv, p)
        });
        self.set_base(Matrix3x2::identity());
        result
    }

    /// The settings panel itself: its tabs, its page and its buttons.
    fn settings_panel(&self, sv: &SettingsView, p: &Palette) -> Result<()> {
        let l = sv.layout;
        self.raised_panel(&l.panel, p);
        self.label("Settings", &self.text.display, &l.title, p.text, 1.0);
        self.tabs(sv, p);
        // A new page fades in where the last one was.
        self.with_opacity(sv.page.clamp(0.0, 1.0), || self.settings_page(sv, p))?;

        for (k, (control, r)) in l.controls.iter().enumerate() {
            let (hover, pressed) = (sv.button)(ButtonId::Settings(*control));
            match control {
                Control::Close => self.button("Close", r, hover, pressed, p),
                Control::CheckUpdates => self.button(sv.update_button, r, hover, pressed, p),
                _ => continue,
            }
            if sv.focus == Some(k) {
                self.focus_ring(r, p);
            }
        }
        let (note, muted) = sv.update;
        let c = if muted { p.text_muted } else { p.text };
        self.label(note, &self.text.caption_wrap, &l.update_note, c, 1.0);
        Ok(())
    }

    /// The page tabs: one segmented control whose celadon marker slides to
    /// the chosen page.
    fn tabs(&self, sv: &SettingsView, p: &Palette) {
        let l = sv.layout;
        let tabs: Vec<(usize, SettingsTab, Rect)> = l
            .controls
            .iter()
            .enumerate()
            .filter_map(|(k, (c, r))| match c {
                Control::Tab(t) => Some((k, *t, *r)),
                _ => None,
            })
            .collect();
        let Some(&(_, _, first)) = tabs.first() else {
            return;
        };
        for &(k, t, r) in &tabs {
            if sv.hover == Some(k) && t != l.tab {
                self.hover_back(&r, shape::RADIUS_CONTROL, 1.0, p);
            }
        }
        let marker = Rect::new(first.x + sv.tab_marker * first.w, first.y, first.w, first.h);
        if !p.high_contrast {
            self.fill(
                &marker,
                shape::RADIUS_CONTROL,
                p.accent,
                effects::HOVER_FILL,
            );
        }
        for &(k, t, r) in &tabs {
            if sv.pressed == Some(k) {
                self.press_shadow(&r, 1.0, p);
            }
            // The chosen tab's label sits on the marker's wash, and a
            // hovered one on the line fill: both take full text.
            let c = if t == l.tab || sv.hover == Some(k) {
                p.text
            } else {
                p.text_muted
            };
            self.label(t.label(), &self.text.body_center, &r, c, 1.0);
        }
        let segs: Vec<(bool, Rect)> = tabs.iter().map(|&(_, _, r)| (false, r)).collect();
        self.segment_group(&segs, p);
        self.chosen_edge(&marker, p);
        for &(k, _, r) in &tabs {
            if sv.focus == Some(k) {
                self.focus_ring(&r, p);
            }
        }
    }

    /// The current page's rows and controls.
    fn settings_page(&self, sv: &SettingsView, p: &Palette) -> Result<()> {
        let l = sv.layout;
        for (i, row) in l.rows.iter().enumerate() {
            // A label given two lines' height wraps onto them.
            let format = if row.label_rect.h > 24.0 {
                &self.text.body_wrap
            } else {
                &self.text.body
            };
            self.label(row.label, format, &row.label_rect, p.text_muted, 1.0);
            let Some(r) = row.value else { continue };
            let (text, muted) = match l.tab {
                SettingsTab::Engines => sv.engines[i],
                _ => sv.output,
            };
            let c = if muted { p.text_muted } else { p.text };
            self.label(text, &self.text.body, &r, c, 1.0);
            if let (Some(hint), Some(hr)) = (row.hint, row.hint_rect) {
                // An engine's purpose has one line, under its name.
                let format = match l.tab {
                    SettingsTab::Engines => &self.text.caption,
                    _ => &self.text.caption_wrap,
                };
                self.label(hint, format, &hr, p.text_muted, 1.0);
            }
        }

        // Segmented groups, outlined once each after their fills.
        let mut groups: Vec<(Control, Vec<(bool, Rect)>)> = vec![];
        for (k, (control, r)) in l.controls.iter().enumerate() {
            // Segments and steppers light at once; buttons ease their hover
            // and press (`lift`, `sink`).
            let (hover, pressed) = (sv.hover == Some(k), sv.pressed == Some(k));
            let (lift, sink) = (sv.button)(ButtonId::Settings(*control));
            let chosen = match control {
                Control::OutputChoose | Control::EngineChoose(_) => {
                    self.button("Choose", r, lift, sink, p);
                    None
                }
                Control::OutputReset | Control::EngineReset(_) => {
                    self.button("Reset", r, lift, sink, p);
                    None
                }
                Control::EngineGet(_) => {
                    // A link, not a button: underlined text, brighter
                    // under the pointer.
                    let c = if hover || pressed { p.text } else { p.link };
                    let tw = self.text.measure(GET_IT, &self.text.body).min(r.w);
                    let lx = r.right() - tw;
                    self.label(
                        GET_IT,
                        &self.text.body,
                        &Rect::new(lx, r.y, tw + 1.0, r.h),
                        c,
                        1.0,
                    );
                    let under = solid(p, effects::UNDERLINE);
                    self.hline(lx, lx + tw, r.y + r.h / 2.0 + 7.0, c, under);
                    None
                }
                Control::Theme(t) => {
                    let chosen = *t == sv.theme;
                    self.segment(t.label(), r, hover, pressed, chosen, p);
                    Some(chosen)
                }
                Control::Suggestions(on) => {
                    let chosen = *on == sv.suggestions;
                    let label = if *on { "On" } else { "Off" };
                    self.segment(label, r, hover, pressed, chosen, p);
                    Some(chosen)
                }
                Control::AutoSave(on) => {
                    let chosen = *on == sv.autosave;
                    let label = if *on { "On" } else { "Off" };
                    self.segment(label, r, hover, pressed, chosen, p);
                    Some(chosen)
                }
                Control::ForgetPicks if sv.has_habits => {
                    self.button("Reset suggestions", r, lift, sink, p);
                    None
                }
                Control::ForgetPicks => {
                    // Disabled: the label goes muted, with no hover or press;
                    // under a contrast theme, the theme's grey text.
                    self.button_face(r, 0.0, 0.0, false, p);
                    let text = "Reset suggestions";
                    let c = if p.high_contrast {
                        p.line
                    } else {
                        p.text_muted
                    };
                    self.label(text, &self.text.body_center, r, c, 1.0);
                    None
                }
                Control::Preset(s, preset) => {
                    let chosen = sv.quality.get(*s) == Value::Preset(*preset);
                    self.segment(preset.label(), r, hover, pressed, chosen, p);
                    Some(chosen)
                }
                Control::Step(s) => {
                    let text = s.label(sv.quality.get(*s));
                    self.stepper(&text, r, hover, pressed, p);
                    None
                }
                Control::Key(wheel) => {
                    self.button_face(r, lift, sink, false, p);
                    let code = sv.keys.key_for(*wheel).code();
                    let (_, cw) = self.text.code_layout(code, r.w, r.h)?;
                    let label_w = self.text.measure(wheel.title(), &self.text.body);
                    if r.w >= 12.0 + label_w + 8.0 + cw.ceil() + 12.0 {
                        // The name on the left, the key on the right.
                        let name = Rect::new(r.x + 12.0, r.y, r.w - 12.0, r.h);
                        self.label(wheel.title(), &self.text.body, &name, p.text, 1.0);
                        let cr = Rect::new(r.right() - 12.0 - cw.ceil(), r.y, cw.ceil(), r.h);
                        self.code(code, &cr, p.text_muted, 1.0)?;
                    } else {
                        // Too narrow for both side by side, as at the
                        // smallest window: one above the other, so the
                        // two never overlap.
                        let name = Rect::new(r.x + 12.0, r.y + 1.0, (r.w - 24.0).max(0.0), 16.0);
                        self.label(wheel.title(), &self.text.caption, &name, p.text, 1.0);
                        let cr = Rect::new(r.x + 12.0, r.y + 15.0, (r.w - 24.0).max(0.0), 16.0);
                        self.code(code, &cr, p.text_muted, 1.0)?;
                    }
                    None
                }
                Control::Tab(_) | Control::CheckUpdates | Control::Close => continue,
            };
            if let Some(chosen) = chosen {
                match groups.last_mut() {
                    Some((c, segs)) if same_group(*c, *control) => segs.push((chosen, *r)),
                    _ => groups.push((*control, vec![(chosen, *r)])),
                }
            }
        }
        for (_, segs) in &groups {
            self.segment_group(segs, p);
        }
        for (k, (control, r)) in l.controls.iter().enumerate() {
            let page = !matches!(
                control,
                Control::Tab(_) | Control::CheckUpdates | Control::Close
            );
            if page && sv.focus == Some(k) {
                self.focus_ring(r, p);
            }
        }
        Ok(())
    }

    /// The options sheet: what the source is, the choices that fit it, and
    /// what they will make. It rises into place as it fades in, with the
    /// scrim fading in behind it.
    fn sheet(&self, w: f32, h: f32, sv: &SheetView, p: &Palette) -> Result<()> {
        let a = (sv.open * sv.fade).clamp(0.0, 1.0);
        if a <= 0.0 {
            return Ok(());
        }
        self.scrim(w, h, a, p);
        let result = self.with_opacity(a, || {
            self.set_base(moved(0.0, sv.rise));
            let l = sv.layout;
            self.raised_panel(&l.panel, p);
            self.label(sv.title, &self.text.display, &l.title, p.text, 1.0);
            self.label(sv.target, &self.text.caption, &l.target, p.text_muted, 1.0);
            self.label(
                sv.context,
                &self.text.caption,
                &l.context,
                p.text_muted,
                1.0,
            );
            for (r, (name, _)) in l.labels.iter().zip(sv.rows) {
                self.label(name, &self.text.body, r, p.text_muted, 1.0);
            }
            let mut presets = vec![];
            for (k, (c, r)) in l.controls.iter().enumerate() {
                let (hover, pressed) = (sv.hover == Some(k), sv.pressed == Some(k));
                match c {
                    SheetControl::Preset(_, preset) => {
                        let chosen = *preset == sv.preset;
                        self.segment(preset.label(), r, hover, pressed, chosen, p);
                        presets.push((chosen, *r));
                    }
                    SheetControl::Step(row) => {
                        let text = sv.rows.get(*row).map_or("", |(_, v)| v.as_str());
                        self.stepper(text, r, hover, pressed, p);
                    }
                    SheetControl::Cancel => {
                        let (hover, pressed) = (sv.button)(ButtonId::Sheet(*c));
                        self.button("Cancel", r, hover, pressed, p)
                    }
                    SheetControl::Convert => {
                        let (hover, pressed) = (sv.button)(ButtonId::Sheet(*c));
                        self.primary("Convert", r, hover, pressed, p)
                    }
                }
            }
            self.segment_group(&presets, p);
            for (k, (_, r)) in l.controls.iter().enumerate() {
                if sv.focus == Some(k) {
                    self.focus_ring(r, p);
                }
            }
            let line = Rect::new(l.result.x, l.result.y + 8.0, l.result.w, 16.0);
            self.label(sv.result, &self.text.caption, &line, p.text_muted, 1.0);
            Ok(())
        });
        self.set_base(Matrix3x2::identity());
        result
    }
}

/// The focus ring's colour: strong celadon, or the contrast theme's text
/// colour.
fn focus_colour(p: &Palette) -> Rgb {
    if p.high_contrast {
        p.text
    } else {
        p.accent_strong
    }
}

/// A stretch of ring between two angles and two radii; with `full`, the
/// whole ring.
#[derive(Clone, Copy)]
struct Band {
    a0: f32,
    a1: f32,
    r0: f32,
    r1: f32,
    full: bool,
}

impl Band {
    fn new(a0: f32, a1: f32, r0: f32, r1: f32, full: bool) -> Self {
        Self {
            a0,
            a1,
            r0,
            r1,
            full,
        }
    }
}

/// Whether two controls are segments of one group.
fn same_group(a: Control, b: Control) -> bool {
    match (a, b) {
        (Control::Theme(_), Control::Theme(_))
        | (Control::Suggestions(_), Control::Suggestions(_))
        | (Control::AutoSave(_), Control::AutoSave(_)) => true,
        (Control::Preset(x, _), Control::Preset(y, _)) => x == y,
        _ => false,
    }
}

/// Continue a figure along a circle of radius `r` around the origin, from
/// angle `a0` (where the figure already is) to `a1`, either direction. Built
/// from cubic Béziers of at most a quarter turn each, which stay within a
/// fraction of a pixel of the true circle at wheel sizes.
fn arc_to(sink: &ID2D1GeometrySink, r: f32, a0: f32, a1: f32) {
    let steps = ((a1 - a0).abs() / (PI / 2.0)).ceil().max(1.0) as usize;
    let span = (a1 - a0) / steps as f32;
    let k = 4.0 / 3.0 * (span / 4.0).tan();
    for i in 0..steps {
        let (s, e) = (a0 + span * i as f32, a0 + span * (i + 1) as f32);
        let (cs, ss, ce, se) = (s.cos(), s.sin(), e.cos(), e.sin());
        unsafe {
            sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                point1: pt(r * (cs - k * ss), r * (ss + k * cs)),
                point2: pt(r * (ce + k * se), r * (se - k * ce)),
                point3: pt(r * ce, r * se),
            });
        }
    }
}

fn geometry(factory: &ID2D1Factory, icon: &Icon) -> Result<ID2D1PathGeometry> {
    unsafe {
        let geom = factory.CreatePathGeometry()?;
        let sink = geom.Open()?;
        let mut open = false;
        let begin = if icon.style == Style::Fill {
            D2D1_FIGURE_BEGIN_FILLED
        } else {
            D2D1_FIGURE_BEGIN_HOLLOW
        };
        for c in icon.path {
            match *c {
                Cmd::Move(x, y) => {
                    if open {
                        sink.EndFigure(D2D1_FIGURE_END_OPEN);
                    }
                    sink.BeginFigure(pt(x, y), begin);
                    open = true;
                }
                Cmd::Line(x, y) => sink.AddLine(pt(x, y)),
                Cmd::Cubic(a, b, c, d, x, y) => sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                    point1: pt(a, b),
                    point2: pt(c, d),
                    point3: pt(x, y),
                }),
                Cmd::Close => {
                    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                    open = false;
                }
            }
        }
        if open {
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
        }
        sink.Close()?;
        Ok(geom)
    }
}

fn client_size(hwnd: HWND) -> D2D_SIZE_U {
    let mut rc = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rc);
    }
    D2D_SIZE_U {
        width: (rc.right - rc.left).max(1) as u32,
        height: (rc.bottom - rc.top).max(1) as u32,
    }
}
