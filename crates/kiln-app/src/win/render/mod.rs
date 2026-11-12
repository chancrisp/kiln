//! Direct2D drawing. Everything is laid out in DIPs; the render target's
//! DPI maps them to pixels, so 100%, 150% and 200% scaling share one path.
//!
//! The main window draws through a Direct2D device context into a
//! flip-model swap chain that DirectComposition shows (`gpu`).

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::f32::consts::{PI, TAU};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use windows::core::Result;
use windows::Win32::Foundation::{HANDLE, HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows_numerics::{Matrix3x2, Vector2};

use kiln_core::Kind;

use crate::anim;
use crate::help::{HelpSection, Placed};
use crate::icons::{Cmd, MARK_PATH};
use crate::inspector::{self as insp, Inspector, InspectorControl};
use crate::layout::{
    self, Control, DialogButton, HelpButton, InspectorLayout, Layout, Rect, SettingsLayout,
    SheetControl, SheetLayout, Wrapped, ROW_HEIGHT,
};
use crate::model::{self, Entry, FocusButton, JobState, Tone};
use crate::previews::{cover_source, Pixels, PreviewSize};
use crate::theme::fluent::space;
use crate::theme::fluent::FluentPalette;
use crate::theme::Rgb;
use crate::ways_in::{self, EmptyLines};
use crate::wheel::{Action, Spot, Wheel, R_OUTER};

use super::motion::ButtonId;
use super::text::Text;

mod ask;
pub(in crate::win) mod cache;
// The controls kit (Task 15) is drawn by nothing but the debug gallery
// until the surfaces take it up (Tasks 16 to 21).
#[cfg_attr(not(debug_assertions), allow(dead_code))]
mod controls;
#[cfg(debug_assertions)]
mod controls_page;
mod details;
mod draw;
pub(in crate::win) mod gpu;
mod help;
mod inspector;
mod list;
mod settings;
mod sheet;
#[cfg(test)]
mod shots;
mod smaller;
#[cfg(test)]
mod tests;
mod title_bar;
#[cfg(debug_assertions)]
mod type_page;
mod wheel;

pub use self::ask::AskView;
pub use self::controls::MenuLine;
use self::draw::*;
use self::list::*;
pub use self::title_bar::TitleView;
pub use self::wheel::{draw_wheel_on, WheelResources};

/// How the wheel looks this frame.
pub struct WheelView<'a> {
    pub wheel: &'a Wheel,
    /// What is lit: a segment, or a line of More's list.
    pub hover: Option<Spot>,
    /// What the hub says.
    pub hub: crate::wheel::HubLines,
    /// How much of it shows, 0 to 1: its opening times its fade out.
    pub opacity: f32,
    /// Its scale about its centre as it opens, 0.92 to 1.
    pub grow: f32,
    /// Over Kiln's window, the smoke behind it (mock `light20`).
    pub smoke: bool,
    /// More's list, while it is open.
    pub flyout: Option<FlyoutView<'a>>,
    /// The file's thumbnail for the hub, where Kiln's window has one: the
    /// picture, its id and how far it has faded in.
    pub thumb: Option<(Arc<Pixels>, u64, f32)>,
    /// The files' kind, for the hub's tile when there is no thumbnail.
    pub kind: Option<Kind>,
}

/// More's list as it is drawn this frame.
pub struct FlyoutView<'a> {
    pub rows: Vec<crate::wheel::FlyoutRow<'a>>,
    pub place: crate::wheel::FlyoutPlace,
    pub lit: Option<Spot>,
    /// 0 to 1 as it fades in.
    pub shown: f32,
}

/// What the floating wheel draws that the wheel over Kiln's window
/// doesn't. The default is none of it.
#[derive(Debug, Clone, Copy, Default)]
pub struct WheelExtras<'a> {
    /// A caption in a pill under the wheel: why the files can become
    /// nothing. Body text on the overlay, wrapped to `CAPTION_WIDTH`.
    pub caption: Option<&'a str>,
}

/// The widest the floating wheel's caption wraps to, in DIPs.
pub const CAPTION_WIDTH: f32 = 320.0;

/// The empty window's mark is drawn with a line this many DIPs wide.
const MARK_LINE: f32 = 2.0;

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
    /// What Nothing says under its title: the key hint.
    pub nothing: &'a str,
    /// How far it has faded in, and how far it still has to rise, in DIPs.
    pub alpha: f32,
    pub rise: f32,
    /// Whether its controls light under the pointer: what it said before
    /// takes no pointer.
    pub live: bool,
}

/// The right-click menu this frame: its lines, where they are, and the
/// lit one.
pub struct MenuView<'a> {
    pub items: &'a [crate::ways_in::MenuItem],
    pub layout: &'a layout::kit::MenuLayout,
    pub lit: Option<usize>,
}

/// The options dialog this frame.
pub struct SheetView<'a> {
    pub layout: &'a SheetLayout,
    pub title: &'a str,
    /// The one line under the title: the source, then what it becomes
    /// ("MOV: QuickTime video from Apple").
    pub sub: &'a str,
    /// What the conversion leaves out, a line each.
    pub notes: &'a [String],
    /// The estimate card's line.
    pub result: &'a str,
    /// Whether `result` is Max size's error.
    pub error: bool,
    /// Each More options line as laid out.
    pub lines: &'a [crate::a11y::SheetLine],
    /// Which Quality radio button is on.
    pub selected: Option<usize>,
    /// The Quality radio buttons' names, for sound and video Kiln writes
    /// itself; empty for the photo's.
    pub radios: &'a [&'static str],
    /// A More options line's open drop-down, its state and its list.
    pub list: Option<(
        layout::SheetControl,
        crate::controls::ComboState,
        layout::kit::FlyoutLayout,
    )>,
    pub more_open: bool,
    /// Indexes into `layout.controls`.
    pub hover: Option<usize>,
    pub pressed: Option<usize>,
    /// The control showing the focus visual: the keyboard's, once a key
    /// was pressed or it opened from the keyboard.
    pub focus: Option<usize>,
    /// False while Convert waits: Max size asks for a size the format
    /// can't reach.
    pub convert_enabled: bool,
    /// Its opacity and scale as it comes in, as a prompt's, and 1 to 0 as
    /// it fades out.
    pub rise: (f32, f32),
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
    /// The wash a just-finished row flashes with, as it settles.
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
    /// What is behind the window: over Mica the base is left transparent,
    /// otherwise it is the solid base colour.
    pub backdrop: crate::win::titlebar::Backdrop,
    /// The title bar over the top of the window.
    pub title: TitleView<'a>,
    pub entries: &'a [Entry],
    /// Lines that stand in for a row's own second line, by the row's
    /// file: a failed export, or a held result that's gone.
    pub notes: &'a HashMap<PathBuf, (String, Tone)>,
    /// Rows, by file, whose results are being exported: they read
    /// "Saving…" and have no link until it's done.
    pub saving: &'a HashSet<PathBuf>,
    /// What those rows read while a copy to another drive is under way,
    /// by file: "Saving…" with how far it is.
    pub saving_lines: &'a HashMap<PathBuf, String>,

    /// The selected rows; each draws the selected wash.
    pub selected: &'a BTreeSet<usize>,
    /// Row `i`'s kind of file, whose glyph its tile shows. `None` for
    /// a type Kiln has no kind for.
    pub kind: &'a dyn Fn(usize) -> Option<Kind>,
    /// Row `i`'s picture at a size, once Windows has made one: its pixels,
    /// the id of its key (the renderer keeps a bitmap by it) and how far
    /// it has faded in, 0 to 1. `None` draws the kind's tile.
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
    /// The pointer is on a finished drag-wheel row's Options.
    pub hover_options: bool,
    pub scroll: f32,
    /// 0 to 1: how far the drag-over highlight has faded in.
    pub drag: f32,
    /// How far, in ms, the clock indeterminate progress bars run on has
    /// gone this frame; `None` with Animation effects off, when they stand
    /// still.
    pub progress_clock: Option<f32>,
    /// Only the running rows' progress changed since the last frame
    /// presented, and the window is otherwise as it was: the frame draws
    /// inside those rows alone and presents just them (`progress_dirty`).
    /// Anything else leaves it false, and the whole window is drawn.
    pub progress_only: bool,
    pub row_anim: &'a dyn Fn(usize) -> RowAnim,
    pub status: &'a str,
    /// The empty window's lines; its key hint names the current wheel
    /// keys.
    pub empty: &'a EmptyLines,
    /// What the status says while the list is empty, shown under the Add
    /// files button. Empty when there's nothing to say.
    pub empty_status: &'a str,
    /// The first-run card's words, while the empty window shows it.
    pub first_run: Option<&'a ways_in::FirstRun>,
    /// Where the empty window's parts are, the card's lines as measured
    /// (`App::empty_state`). Without it they are laid out one line each.
    pub empty_at: Option<layout::EmptyState>,
    /// The footer's Convert button; disabled, it is drawn quiet and takes
    /// no hover.
    pub convert_enabled: bool,
    /// What the footer's button says: "Convert", "Convert N files" for
    /// several, or Cancel while the selection holds a file that is
    /// converting.
    pub convert_label: &'a str,
    /// Where the footer's parts are (`App::footer_row`). Without it they
    /// are placed for one selected file.
    pub footer_row: Option<crate::layout::FooterRow>,
    /// The credit at the window's bottom left (`App::credit`), where
    /// there is room for it.
    pub credit: Option<crate::credit::Credit>,
    /// Rows were just removed: the footer offers Undo.
    pub undo: bool,
    pub wheel: Option<WheelView<'a>>,
    /// The right-click menu, while it is up.
    pub menu: Option<MenuView<'a>>,
    /// The details dialog, while it's open.
    pub details: Option<DetailsView<'a>>,
    /// One of Kiln's prompts, over everything but the caption buttons.
    pub ask: Option<AskView<'a>>,
    /// The Make smaller or Resize dialog, under a prompt.
    pub tool: Option<crate::win::surfaces::ToolView<'a>>,
    pub settings: Option<SettingsView<'a>>,
    pub sheet: Option<SheetView<'a>>,
    /// The help overlay, while it's open.
    pub help: Option<HelpView<'a>>,
    /// The inspector, while it shows beside the list.
    pub inspector: Option<InspectorView<'a>>,
    /// The InfoBar at the top of the list, while one shows.
    pub top_info: Option<TopInfoView<'a>>,
}

/// The InfoBar at the top of the list this frame: where it is
/// (`Renderer::top_info_at`) and what it says.
pub struct TopInfoView<'a> {
    pub layout: crate::layout::kit::InfoBarLayout,
    pub title: &'a str,
    pub message: &'a str,
}

/// Help this frame.
pub struct HelpView<'a> {
    /// Where it is: a ContentDialog with Close in its band.
    pub frame: crate::layout::kit::DialogLayout,
    pub sections: &'a [HelpSection],
    /// Show first-run help's width.
    pub link_w: f32,
    pub scroll: f32,
    /// Show first-run help's and Close's hover and press, as they ease
    /// (`ButtonId::Help`).
    pub button: &'a dyn Fn(ButtonId) -> (f32, f32),
    /// The one of them with the keyboard focus.
    pub focus: HelpButton,
    /// 0 to 1 as it fades in, and how far it still has to rise in DIPs
    /// (`anim::rise`).
    pub open: f32,
    pub rise: f32,
}

/// The details dialog this frame: what a failed job's engine said.
pub struct DetailsView<'a> {
    /// Where it is: a ContentDialog, its text in a card.
    pub layout: crate::layout::kit::DetailsDialog,
    /// "What happened".
    pub title: &'a str,
    /// The plain line over the card.
    pub lead: &'a str,
    /// The engine's words, in the card; empty for none.
    pub text: &'a str,
    pub scroll: f32,
    /// "Copy", or "Copied" for a moment after a copy.
    pub copy_label: &'static str,
    pub focus: DialogButton,
    /// Each button's hover and press, as they ease (`ButtonId::Details`).
    pub button: &'a dyn Fn(ButtonId) -> (f32, f32),
}

/// Settings this frame: the page laid out and what it says.
pub struct SettingsView<'a> {
    pub layout: SettingsLayout,
    pub page: crate::settings_page::PageSpec,
    /// 0 to 1 as it fades in, and how far it still has to rise in DIPs
    /// (`anim::rise`).
    pub open: f32,
    pub rise: f32,
    /// The control the pointer is on, and the one it pressed.
    pub hover: Option<Control>,
    pub pressed: Option<Control>,
    /// The control with the keyboard focus, while the keyboard is in use.
    pub focus: Option<Control>,
    /// Each button's hover and press, as they ease (`ButtonId::settings`).
    pub button: &'a dyn Fn(ButtonId) -> (f32, f32),
    /// An open ComboBox's list: which, its state and where it is.
    pub combo: Option<(
        Control,
        crate::controls::ComboState,
        crate::layout::kit::FlyoutLayout,
    )>,
    /// Each Expander's chevron, 0 shut to 1 open.
    pub chevrons: [f32; 5],
    /// The drag-wheel's knob, 0 off to 1 on.
    pub knobs: [f32; 1],
    /// The scroll indicator's opacity.
    pub scroll_shown: f32,
}

pub struct Renderer {
    /// Direct2D's factory: the device context is made on its device, so
    /// the mark's and strokes' geometries made with it draw there.
    factory: ID2D1Factory1,
    text: Text,
    round: ID2D1StrokeStyle,
    /// A dashed line, for the empty window's drop outline.
    dash: ID2D1StrokeStyle,
    /// Kiln's mark (`icons::MARK_PATH`), as an outline.
    mark: ID2D1PathGeometry,
    /// The device, swap chain and composition, made at the first frame and
    /// again after the device is lost.
    gpu: Option<gpu::Gpu>,
    target: Option<Target>,
    hwnd: HWND,
    /// The device was lost in the last frame: everything on it has been
    /// dropped, and the window must be drawn again (`take_lost`).
    lost: bool,
    /// Frames presented, and devices made, so far.
    presented: u64,
    /// When the last frame was presented.
    presented_at: Option<std::time::Instant>,
    devices: u64,
    /// Devices lost in a row, and whether they are WARP's from now on.
    losses: gpu::Losses,
    /// Test builds: whether the window was cloaked at each present.
    #[cfg(all(test, debug_assertions))]
    cloaked_at_present: Vec<bool>,
    /// Test builds: the rectangles each present showed, `None` for the
    /// whole frame.
    #[cfg(all(test, debug_assertions))]
    dirty_at_present: Vec<Option<Vec<RECT>>>,
    /// Test builds: how many times the caches were cleared.
    #[cfg(all(test, debug_assertions))]
    cache_clears: Cell<u64>,
    /// Debug builds: the next present reports the device removed, as a
    /// driver reset would (`lose_device_for_test`).
    #[cfg(debug_assertions)]
    fake_loss: bool,

    dpi: f32,
    /// The Text size's zoom (`layout::text_zoom`), already in `dpi`; the
    /// type ramp picks its optical size by it.
    zoom: f32,
    /// Frames drawn so far, to tell which previews' bitmaps have gone
    /// undrawn long enough to drop.
    frame: u64,
    /// The inspector's wrapped texts' heights, by text, how it wraps and
    /// width, so the same words are measured once. DirectWrite measures in
    /// DIPs, which the Text size's zoom doesn't change.
    measured: RefCell<HashMap<(String, Wrapped, u32), f32>>,
    /// The InfoBar at the top of the list, while one shows: its title and
    /// message.
    top_info: Option<(String, String)>,
    /// The rounded clips made so far (`cache`).
    clips: RefCell<cache::Clips>,
}

/// The most measured or laid-out texts kept before starting over: far
/// more than one frame uses.
const TEXT_CACHE: usize = 64;

/// What the frame draws with that is made on the device for a palette and
/// DPI: dropped when either changes, and with the device if it is lost.
struct Target {
    brush: ID2D1SolidColorBrush,
    /// What the wheel draws with on this target.
    wheel: RefCell<WheelResources>,
    /// Previews made into brushes, by their key's id, and the frame each
    /// was last drawn in. They go with the device.
    pictures: RefCell<HashMap<u64, Picture>>,
    /// The palette the target was made for.
    palette: FluentPalette,
}

/// A preview's bitmap, ready to fill a shape with.
struct Picture {
    brush: ID2D1BitmapBrush,
    drawn: u64,
}

/// A preview's bitmap is dropped once it has gone this many frames
/// without being drawn.
const PICTURE_FRAMES: u64 = 120;

impl Renderer {
    pub fn new(hwnd: HWND, dpi: f32) -> Result<Self> {
        // SAFETY: a plain factory for this thread, with default options.
        let factory: ID2D1Factory1 =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)? };
        let round = round_stroke(&factory)?;
        let dash = dash_stroke(&factory)?;
        let mark = mark_geometry(&factory)?;
        let text = Text::new()?;
        Ok(Self {
            factory,
            text,
            round,
            dash,
            mark,
            gpu: None,
            target: None,
            hwnd,
            lost: false,
            presented: 0,
            presented_at: None,
            devices: 0,
            losses: gpu::Losses::new(""),
            #[cfg(all(test, debug_assertions))]
            cloaked_at_present: Vec::new(),
            #[cfg(all(test, debug_assertions))]
            dirty_at_present: Vec::new(),
            #[cfg(all(test, debug_assertions))]
            cache_clears: Cell::new(0),
            #[cfg(debug_assertions)]
            fake_loss: false,
            dpi,
            zoom: 1.0,
            frame: 0,
            measured: RefCell::new(HashMap::new()),
            top_info: None,
            clips: RefCell::new(cache::clips()),
        })
    }

    /// Forget every cache: the text layouts and the clips. The device being
    /// lost, the scale, the Text size or the theme changing is the moment:
    /// what is drawn next is made again from what it is then.
    fn clear_caches(&self) {
        #[cfg(all(test, debug_assertions))]
        self.cache_clears.set(self.cache_clears.get() + 1);
        self.text.clear_caches();
        self.clips.borrow_mut().clear();
        self.measured.borrow_mut().clear();
    }

    /// How tall an inspector text runs at `width`, wrapped as `how` says
    /// and cut short at its most lines. The same words are measured once.
    pub fn inspector_text_height(&self, s: &str, how: Wrapped, width: f32) -> f32 {
        let key = (s.to_owned(), how, width.to_bits());
        if let Some(h) = self.measured.borrow().get(&key) {
            return *h;
        }
        use crate::win::text::{Ramp, Style};
        let (ramp, lines) = match how {
            Wrapped::Title(n) => (Ramp::Subtitle, n),
            Wrapped::Body(n) => (Ramp::Caption, n),
            Wrapped::Info(n) => (Ramp::Body, n),
        };
        let line = ramp.style().line;
        let h = self
            .text
            .styled_height(s, Style::new(ramp), self.zoom, width)
            .min(line * lines as f32);
        let mut measured = self.measured.borrow_mut();
        if measured.len() >= TEXT_CACHE {
            measured.clear();
        }
        measured.insert(key, h);
        h
    }

    /// A file's name as the side pane's title draws it, `width` wide: at
    /// most [`layout::TITLE_LINES`] lines, the last ending in an ellipsis
    /// when the name is longer.
    pub fn inspector_title(&self, name: &str, width: f32) -> String {
        use crate::win::text::{Ramp, Style};
        self.text.fit_lines(
            name,
            Style::new(Ramp::Subtitle).wrapping(),
            self.zoom,
            width,
            layout::TITLE_LINES,
        )
    }

    /// How wide a side pane button saying `label` is: its Body words and
    /// a button's padding.
    pub fn inspector_button_width(&self, label: &str) -> f32 {
        use crate::win::text::{Ramp, Style};
        let style = Style::new(Ramp::Body);
        let line = style.ramp.style().line;
        let w = self
            .text
            .styled(label, style, self.zoom, 4096.0, line)
            .map_or(0.0, |l| l.width);
        layout::kit::button_width(w, false).ceil()
    }

    /// How tall the details dialog's text runs when wrapped to `width`.
    pub fn details_height(&self, text: &str, width: f32) -> f32 {
        self.text.block_height(text, &self.text.mono_block, width)
    }

    /// How tall one line of help's text runs in Body, wrapped to `width`.
    pub fn help_line_height(&self, text: &str, width: f32) -> f32 {
        use crate::win::text::{Ramp, Style};
        self.text
            .styled_height(text, Style::new(Ramp::Body), self.zoom, width)
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
        let (x, (_, right), _) = row_text_span(r, e, saving, links);
        let cut = list::line_width(&self.text, line, self.zoom) > (right - x).max(0.0);
        (Rect::new(r.x, r.y, (right - r.x).max(0.0), r.h), cut)
    }

    /// Whether a failed row's line is too long for row `r`, so it's cut
    /// short and Show details offers the whole sentence.
    pub fn line_cut(&self, r: &Rect, e: &Entry) -> bool {
        line_cut(&self.text, self.zoom, r, e)
    }

    /// The Text size's zoom, which `set_dpi` is also given within its DPI.
    /// The formats it picks are kept by optical size, so nothing is
    /// cleared.
    pub fn set_zoom(&mut self, zoom: f32) {
        self.zoom = zoom;
    }

    /// The title bar, in layout units: 48 DIPs tall whatever the Text
    /// size. Back is on it while Settings is open.
    pub fn title_bar(&self, in_settings: bool) -> crate::win::titlebar::Bar {
        title_bar::bar_in_units(self.size_dips(), self.zoom, in_settings)
    }

    /// The window laid out under the title bar, holding `rows` files with
    /// `shown` of the inspector (`layout::layout_with`).
    pub fn layout(&self, rows: usize, shown: f32) -> Layout {
        let (w, h) = self.size_dips();
        let top = self.title_bar(false).rect.h;
        let l = layout::layout_under(w, h, rows, shown, top);
        match self.top_info_at(l.top_info_slot()) {
            Some(bar) => l.with_top_info(bar.rect.h),
            None => l,
        }
    }

    /// Show `info` (title, message) in an InfoBar at the top of the list,
    /// or none.
    pub fn set_top_info(&mut self, info: Option<(String, String)>) {
        self.top_info = info;
    }

    /// The InfoBar at the top of the list laid out at the top of `slot`,
    /// as wide: the title in Body Strong, then the message in Body on the
    /// same line where it fits, else wrapped under it; and its Dismiss.
    pub fn top_info_at(&self, slot: Rect) -> Option<layout::kit::InfoBarLayout> {
        use crate::win::text::{Ramp, Style};
        let (title, message) = self.top_info.as_ref()?;
        let width = |s: &str, ramp: Ramp| {
            let line = ramp.style().line;
            self.text
                .styled(s, Style::new(ramp), self.zoom, 4096.0, line)
                .map_or(0.0, |l| l.width.ceil())
        };
        let wrapped = |w: f32| {
            self.text
                .styled_height(message, Style::new(Ramp::Body).wrapping(), self.zoom, w)
        };
        Some(layout::kit::info_bar(
            slot.x,
            slot.y,
            slot.w,
            width(title, Ramp::BodyStrong),
            width(message, Ramp::Body),
            &wrapped,
            &[],
            true,
        ))
    }

    pub fn set_dpi(&mut self, dpi: f32) {
        if (dpi - self.dpi).abs() < 1e-3 {
            return;
        }
        self.dpi = dpi;
        self.clear_caches();
        // Rebuild what's drawn with at the new scale; the device context
        // and its target take the DPI.
        self.target = None;
        if let Some(g) = &mut self.gpu {
            g.set_dpi(dpi);
        }
    }

    /// Size the swap chain to the window's client area.
    pub fn resize(&mut self) {
        let size = client_size(self.hwnd);
        let Some(g) = &mut self.gpu else {
            return;
        };
        if let Err(e) = g.resize((size.width, size.height)) {
            if gpu::is_loss(e.code()) {
                self.lose();
            } else {
                crate::diag::log(&format!("couldn't resize the swap chain: {e}"));
            }
        }
    }

    /// The swap chain's frame-latency waitable object, signalled when it
    /// can take another frame; `None` before the first frame and while the
    /// device is being made again. A new device brings a new handle.
    pub fn frame_waitable(&self) -> Option<HANDLE> {
        self.gpu.as_ref().map(|g| g.waitable())
    }

    /// Whether the next frame's signal is taken already (`Gpu::held`), so
    /// it is drawn without waiting.
    pub fn holds_signal(&self) -> bool {
        self.gpu.as_ref().is_some_and(|g| g.holds_signal())
    }

    /// Whether a frame drawn now would find the swap chain ready for it,
    /// without waiting: it has no device yet, its signal is taken already
    /// or is there to take (and is taken), or no frame has been presented
    /// for `stall_ms` milliseconds, in case one that presented nothing
    /// left the waitable unsignalled.
    pub fn frame_due(&mut self, stall_ms: u32) -> bool {
        let Some(g) = self.gpu.as_mut() else {
            return true;
        };
        g.take_signal_now()
            || self.presented_at.is_none_or(|t| {
                t.elapsed() >= std::time::Duration::from_millis(u64::from(stall_ms))
            })
    }

    /// The message loop's wait on `frame_waitable` returned: its signal is
    /// taken for the next frame.
    pub fn took_signal(&mut self) {
        if let Some(g) = self.gpu.as_mut() {
            g.took_signal();
        }
    }

    /// Whether the window can be counted on to draw: not after the device
    /// was lost so often in a row that it stopped asking to be drawn.
    pub fn can_draw(&self) -> bool {
        !self.losses.given_up()
    }

    /// How tall a ContentDialog's title, in Subtitle, and its text, in
    /// Body, run wrapped to `width`.
    pub fn dialog_text_heights(&self, title: &str, text: Option<&str>, width: f32) -> (f32, f32) {
        use crate::win::text::{Ramp, Style};
        let h = |s: &str, ramp: Ramp| {
            self.text
                .styled_height(s, Style::new(ramp), self.zoom, width)
        };
        (
            h(title, Ramp::Subtitle),
            text.map_or(0.0, |t| h(t, Ramp::Body)),
        )
    }

    /// How tall the first-run card's lines run on a layer `layer_w` wide:
    /// each step in Body, the right-click line in Caption, wrapped as
    /// `layout::first_run_widths` has them.
    pub fn first_run_text(&self, fr: &ways_in::FirstRun, layer_w: f32) -> layout::CardText {
        first_run_text(&self.text, self.zoom, fr, layer_w)
    }

    /// How wide `label` runs in Body, as a button sets it.
    /// How wide a menu line's label (in Body) or shortcut (in Caption,
    /// `caption`) runs.
    pub fn menu_label_width(&self, s: &str, caption: bool) -> f32 {
        use crate::win::text::{Ramp, Style};
        let (ramp, line) = if caption {
            (Ramp::Caption, 16.0)
        } else {
            (Ramp::Body, 20.0)
        };
        self.text
            .styled(s, Style::new(ramp), self.zoom, 4096.0, line)
            .map_or(s.chars().count() as f32 * 7.0, |l| l.width.ceil())
    }

    /// How wide `s` runs on one line in Body, or Caption when `caption`.
    /// How wide the credit's parts run in Caption.
    pub fn credit_widths(&self) -> crate::credit::Widths {
        credit_widths(&self.text, self.zoom)
    }

    pub fn run_width(&self, s: &str, caption: bool) -> f32 {
        use crate::win::text::{Ramp, Style};
        let ramp = if caption { Ramp::Caption } else { Ramp::Body };
        let line = ramp.style().line;
        self.text
            .styled(s, Style::new(ramp), self.zoom, 4096.0, line)
            .map_or(s.chars().count() as f32 * 7.0, |l| l.width)
    }

    /// How tall `s` runs wrapped to `width` in Body, or Caption when
    /// `caption`; nothing for an empty string.
    pub fn wrapped_height(&self, s: &str, caption: bool, width: f32) -> f32 {
        use crate::win::text::{Ramp, Style};
        if s.is_empty() {
            return 0.0;
        }
        let ramp = if caption { Ramp::Caption } else { Ramp::Body };
        self.text
            .styled_height(s, Style::new(ramp).wrapping(), self.zoom, width.max(1.0))
    }

    pub fn body_label_width(&self, label: &str) -> f32 {
        use crate::win::text::{Ramp, Style};
        self.text
            .styled(label, Style::new(Ramp::Body), self.zoom, 4096.0, 20.0)
            .map_or(label.chars().count() as f32 * 7.0, |l| l.width)
    }

    /// Whether the device was lost in the last frame, so the window must
    /// be drawn again; asking clears it.
    pub fn take_lost(&mut self) -> bool {
        std::mem::take(&mut self.lost)
    }

    /// The device is gone: drop everything made on it (the device, the
    /// swap chain and composition, brushes, ramps, wheel tiles and preview
    /// bitmaps, which are made again from their pixels as they are drawn)
    /// and ask for the window to be drawn again, on a new device.
    ///
    /// A device lost again and again is bounded (`gpu::Losses`): after a
    /// few in a row the next device is WARP's, and after more no repaint is
    /// asked for, until something else draws the window.
    fn lose(&mut self) {
        let reason = self.gpu.as_ref().map(|g| g.removed_reason());
        self.target = None;
        self.clear_caches();
        if let Some(g) = self.gpu.take() {
            g.forget();
        }
        if !self.losses.lost(reason) {
            return;
        }
        self.lost = true;
        // SAFETY: a plain call on the window this renderer draws.
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    /// Debug builds: make the next frame's present report the device
    /// removed, as a graphics driver reset does, so a test can see the
    /// window drawn again on a new device.
    #[cfg(debug_assertions)]
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "only the in-process tests call it")
    )]
    pub fn lose_device_for_test(&mut self) {
        self.fake_loss = true;
    }

    /// Frames presented so far.
    pub(crate) fn presented(&self) -> u64 {
        self.presented
    }

    /// Whether the device is WARP's.
    #[cfg(all(test, debug_assertions))]
    pub(crate) fn warp(&self) -> bool {
        self.gpu.as_ref().is_some_and(|g| g.warp)
    }

    /// Whether the compositor keeps the window cloaked now.
    #[cfg(all(test, debug_assertions))]
    pub(crate) fn window_cloaked(&self) -> bool {
        gpu::cloaked(self.hwnd)
    }

    /// Whether the window was cloaked at each present so far.
    #[cfg(all(test, debug_assertions))]
    pub(crate) fn cloaked_at_present(&self) -> &[bool] {
        &self.cloaked_at_present
    }

    /// How many times the caches were cleared, and how many text layouts
    /// are kept now.
    #[cfg(all(test, debug_assertions))]
    pub(crate) fn caches(&self) -> (u64, usize) {
        (self.cache_clears.get(), self.text.cached_layouts())
    }

    /// The rectangles each present showed so far, `None` for a whole frame.
    #[cfg(all(test, debug_assertions))]
    pub(crate) fn dirty_at_present(&self) -> &[Option<Vec<RECT>>] {
        &self.dirty_at_present
    }

    /// Devices made so far: one, then one more after each loss.
    #[cfg(all(test, debug_assertions))]
    pub(crate) fn devices(&self) -> u64 {
        self.devices
    }

    /// The device, made if there is none.
    fn ensure_gpu(&mut self) -> Result<()> {
        if self.gpu.is_none() {
            let size = client_size(self.hwnd);
            let g = gpu::Gpu::new(
                &self.factory,
                self.hwnd,
                (size.width, size.height),
                self.dpi,
                self.losses.warp_only(),
            )?;
            self.losses.made(&g);
            self.gpu = Some(g);
            self.devices += 1;
            self.target = None;
        }
        Ok(())
    }

    fn ensure_target(&mut self, palette: &FluentPalette) -> Result<()> {
        if let Some(t) = &self.target {
            if t.palette == *palette {
                return Ok(());
            }
            // A new theme.
            self.clear_caches();
        }
        let rt: &ID2D1RenderTarget = &self.gpu.as_ref().expect("the device exists").dc;
        let brush = unsafe { rt.CreateSolidColorBrush(&color(palette.text, 1.0), None)? };
        let wheel = WheelResources::new(rt, &self.text.factory)?;
        self.target = Some(Target {
            brush,
            wheel: RefCell::new(wheel),
            pictures: RefCell::new(HashMap::new()),
            palette: *palette,
        });
        Ok(())
    }

    pub fn draw(&mut self, v: &View) -> Result<()> {
        self.lost = false;
        match self.prepare(v.title.palette) {
            Err(e) if gpu::is_loss(e.code()) => {
                self.lose();
                return Ok(());
            }
            other => other?,
        }
        let g = self.gpu.as_ref().expect("the device exists");
        let t = self.target.as_ref().expect("target exists");
        let (w, h) = self.size_dips();
        let l = self.layout(v.entries.len(), v.shown);
        let p = v.title.palette;
        let rt: &ID2D1RenderTarget = &g.dc;
        // Part of the window, when only running rows changed and the screen
        // holds the frame before.
        let part = if v.progress_only && g.can_present_part() {
            let k = self.dpi / 96.0;
            let client = client_size(self.hwnd);
            let wanted = progress_dirty(&l, v);
            let px = dirty_pixels(&wanted, self.dpi, (client.width, client.height));
            (!px.is_empty()).then(|| {
                let b = bounds_of(&px);
                let at = |n: i32| n as f32 / k;
                let clip = D2D_RECT_F {
                    left: at(b.left),
                    top: at(b.top),
                    right: at(b.right),
                    bottom: at(b.bottom),
                };
                (px, clip)
            })
        } else {
            None
        };

        self.frame += 1;
        let began = std::time::Instant::now();
        unsafe {
            rt.BeginDraw();
            rt.SetTransform(&Matrix3x2::identity());
            // The frame is the whole scene, drawn inside the clip; Clear
            // stays inside it too.
            if let Some((_, clip)) = &part {
                rt.PushAxisAlignedClip(clip, D2D1_ANTIALIAS_MODE_ALIASED);
            }
            // The base: left clear for Mica to show through, else solid.
            let base = match v.backdrop {
                crate::win::titlebar::Backdrop::Mica => D2D1_COLOR_F::default(),
                crate::win::titlebar::Backdrop::Solid => color(v.title.palette.base, 1.0),
            };
            rt.Clear(Some(&base));
        }

        let d = Draw {
            factory: &self.factory,
            rt,
            dpi: self.dpi,
            zoom: self.zoom,
            brush: &t.brush,
            wheel: &t.wheel,
            text: &self.text,
            round: &self.round,
            dash: &self.dash,
            mark: &self.mark,
            base: Cell::new(Matrix3x2::identity()),
            pictures: &t.pictures,
            frame: self.frame,
            clips: &self.clips,
        };
        let gallery = controls_page_on();
        let page = type_page_on() || gallery.is_some();
        let mut result = match () {
            #[cfg(debug_assertions)]
            () if type_page_on() => d.type_page(w, h, p),
            #[cfg(debug_assertions)]
            () if gallery.is_some() => d.controls_page(w, h, gallery.unwrap_or(1), p),
            // Settings is a page in the content layer, in place of the list.
            () if v.settings.is_some() => match &v.settings {
                Some(sv) => d.settings(sv, p),
                None => Ok(()),
            },
            () if v.entries.is_empty() => d.empty_state(&l, v).and_then(|_| d.credit(v, p)),
            () => d
                .list(&l, v)
                .and_then(|_| d.footer(&l, v))
                .and_then(|_| d.credit(v, p)),
        };
        // The type page and the gallery stand alone: no title bar or
        // overlay is drawn over them.
        if !page {
            // What a dialog covers of the title bar, under the overlays.
            if result.is_ok() {
                result = d.title_under(&v.title, v.button);
            }
            if let (Ok(()), Some(r)) = (&result, &v.button_ring) {
                // The window, its footer and its side pane are in the new
                // look: its two-tone focus visual.
                d.focus_visual(r, crate::theme::fluent::radius::CONTROL, p);
            }
            if let (Ok(()), Some(wv)) = (&result, &v.wheel) {
                result = d.wheel(wv, p, w, h);
            }
            if let (Ok(()), Some(dv)) = (&result, &v.details) {
                result = d.dialog(w, h, dv, p);
            }
            if let (Ok(()), Some(sv)) = (&result, &v.sheet) {
                result = d.sheet(w, h, sv, p);
            }
            if let (Ok(()), Some(hv)) = (&result, &v.help) {
                result = d.help(w, h, hv, p);
            }
            if let (Ok(()), Some(tv)) = (&result, &v.tool) {
                result = d.tool_dialog(w, h, tv, p);
            }
            if let (Ok(()), Some(av)) = (&result, &v.ask) {
                result = d.ask(w, h, av, p);
            }
            if let (Ok(()), Some(mv)) = (&result, &v.menu) {
                result = d.row_menu(mv, p);
            }
            // Back and Windows' buttons, over everything: they always work.
            if result.is_ok() {
                result = d.title_over(&v.title);
            }
            if let (Ok(()), true, Some(r)) = (&result, v.title.back_ring, &v.title.bar.back) {
                d.focus_visual(r, crate::theme::fluent::radius::CONTROL, p);
            }
        }
        // SAFETY: ends the BeginDraw above, on this thread's context, after
        // the clip pushed there is popped.
        let end = unsafe {
            if part.is_some() {
                rt.PopAxisAlignedClip();
            }
            rt.EndDraw(None, None)
        };
        super::frame_log::main_draw_took(began.elapsed(), part.is_some());
        // Pictures not drawn for a while go; they're made again if their
        // rows come back into view.
        let frame = self.frame;
        t.pictures
            .borrow_mut()
            .retain(|_, pic| frame - pic.drawn < PICTURE_FRAMES);
        let shown = match end {
            Ok(()) => self.present(part.as_ref().map(|(px, _)| px.as_slice())),
            Err(e) => e.code(),
        };
        if gpu::is_loss(shown) {
            self.lose();
            return Ok(());
        }
        result?;
        shown.ok()
    }

    /// A small window that holds one menu flyout and nothing else, as the
    /// notification-area icon's menu is: the menu `layout` lays out, its
    /// `lines` with `lit` lit, and its shadow, on clear ground.
    pub fn draw_popup_menu(
        &mut self,
        lines: &[MenuLine],
        layout: &layout::kit::MenuLayout,
        lit: Option<usize>,
        p: &FluentPalette,
    ) -> Result<()> {
        self.lost = false;
        match self.prepare(p) {
            Err(e) if gpu::is_loss(e.code()) => {
                self.lose();
                return Ok(());
            }
            other => other?,
        }
        let g = self.gpu.as_ref().expect("the device exists");
        let t = self.target.as_ref().expect("target exists");
        let rt: &ID2D1RenderTarget = &g.dc;
        self.frame += 1;
        // SAFETY: plain calls on this thread's device context, whose target
        // is the back buffer (`prepare`).
        unsafe {
            rt.BeginDraw();
            rt.SetTransform(&Matrix3x2::identity());
            rt.Clear(Some(&D2D1_COLOR_F::default()));
        }
        let d = Draw {
            factory: &self.factory,
            rt,
            dpi: self.dpi,
            zoom: self.zoom,
            brush: &t.brush,
            wheel: &t.wheel,
            text: &self.text,
            round: &self.round,
            dash: &self.dash,
            mark: &self.mark,
            base: Cell::new(Matrix3x2::identity()),
            pictures: &t.pictures,
            frame: self.frame,
            clips: &self.clips,
        };
        let result = d.menu_flyout(layout, lines, lit, p);
        // SAFETY: ends the BeginDraw above.
        let end = unsafe { rt.EndDraw(None, None) };
        let shown = match end {
            Ok(()) => self.present(None),
            Err(e) => e.code(),
        };
        if gpu::is_loss(shown) {
            self.lose();
            return Ok(());
        }
        result?;
        shown.ok()
    }

    /// The device, the back buffer as its target at the window's size, and
    /// what the frame draws with for `palette`, each made if missing.
    fn prepare(&mut self, palette: &FluentPalette) -> Result<()> {
        self.ensure_gpu()?;
        let size = client_size(self.hwnd);
        let dpi = self.dpi;
        self.gpu
            .as_mut()
            .expect("the device exists")
            .begin((size.width, size.height), dpi)?;
        self.ensure_target(palette)
    }

    /// Present the frame drawn, all of it or just `part`, and log it.
    fn present(&mut self, part: Option<&[RECT]>) -> windows::core::HRESULT {
        #[cfg(debug_assertions)]
        if std::mem::take(&mut self.fake_loss) {
            return windows::Win32::Graphics::Dxgi::DXGI_ERROR_DEVICE_REMOVED;
        }
        let hr = self
            .gpu
            .as_mut()
            .expect("the device exists")
            .present_part(part.unwrap_or(&[]));
        if hr.is_ok() {
            #[cfg(all(test, debug_assertions))]
            self.dirty_at_present.push(part.map(<[RECT]>::to_vec));
            self.presented += 1;
            self.presented_at = Some(std::time::Instant::now());
            self.losses.presented();
            #[cfg(all(test, debug_assertions))]
            self.cloaked_at_present.push(gpu::cloaked(self.hwnd));
            super::frame_log::main_drawn();
        }
        hr
    }

    pub fn size_dips(&self) -> (f32, f32) {
        let s = client_size(self.hwnd);
        let k = 96.0 / self.dpi;
        (s.width as f32 * k, s.height as f32 * k)
    }
}

/// The rectangles, in DIPs, that a frame in which only running rows changed
/// has to draw and present: each visible running row, which carries the
/// progress line and the percentage and time left in its second line, and,
/// beside the list, a converting file's progress, percentage and time left in
/// the inspector. Empty when the frame can't be told that way (the
/// inspector is folding, or fading to another page), and the whole window
/// is drawn.
pub(super) fn progress_dirty(l: &Layout, v: &View) -> Vec<Rect> {
    let mut out = vec![];
    let panel = l.panel;
    let first = (v.scroll / ROW_HEIGHT).floor().max(0.0) as usize;
    let visible = (panel.h / ROW_HEIGHT).ceil() as usize + 1;
    for i in first..(first + visible).min(v.entries.len()) {
        if !matches!(v.entries[i].job, JobState::Running { .. }) {
            continue;
        }
        let anim = (v.row_anim)(i);
        let mut r = l.rows[i];
        r.y += anim.rise - v.scroll;
        if let Some(at) = overlap(&r, &panel) {
            out.push(at);
        }
    }
    if let (Some(iv), Some(place)) = (&v.inspector, l.inspector) {
        let page = &iv.now;
        if let Inspector::Converting { .. } = page.model {
            let settled = iv.before.is_none() && page.alpha >= 1.0 && page.rise == 0.0;
            if !settled || v.shown < 1.0 {
                return vec![];
            }
            let dx = place.x - page.laid.x;
            let lay = page.layout;
            for at in [Some(lay.hero), lay.line(0), lay.line(1)]
                .into_iter()
                .flatten()
            {
                let moved = Rect::new(at.x + dx, at.y, at.w, at.h);
                if let Some(at) = overlap(&moved, &panel) {
                    out.push(at);
                }
            }
        }
    }
    out
}

/// Whether the debug type page is drawn instead of the window's content
/// (`type_page`). A release build has no such page.
#[cfg(debug_assertions)]
fn type_page_on() -> bool {
    type_page::on()
}

#[cfg(not(debug_assertions))]
fn type_page_on() -> bool {
    false
}

/// The page of the debug controls gallery drawn instead of the window's
/// content (`controls_page`), if one was asked for. A release build has
/// no such page.
#[cfg(debug_assertions)]
fn controls_page_on() -> Option<u8> {
    controls_page::page()
}

#[cfg(not(debug_assertions))]
fn controls_page_on() -> Option<u8> {
    None
}

/// Where `a` and `b` overlap, if they do.
fn overlap(a: &Rect, b: &Rect) -> Option<Rect> {
    let (left, top) = (a.x.max(b.x), a.y.max(b.y));
    let (right, bottom) = (a.right().min(b.right()), a.bottom().min(b.bottom()));
    (right > left && bottom > top).then(|| Rect::new(left, top, right - left, bottom - top))
}

/// `rects`, in DIPs, as pixel rectangles of a `client`-sized area at `dpi`:
/// each grown to whole pixels and cut to the client area, which the swap
/// chain's buffers are larger than. Those that end up empty are left out.
pub(super) fn dirty_pixels(rects: &[Rect], dpi: f32, client: (u32, u32)) -> Vec<RECT> {
    let k = dpi / 96.0;
    let (w, h) = (client.0 as f32, client.1 as f32);
    rects
        .iter()
        .filter_map(|r| {
            let left = (r.x * k).floor().clamp(0.0, w) as i32;
            let top = (r.y * k).floor().clamp(0.0, h) as i32;
            let right = (r.right() * k).ceil().clamp(0.0, w) as i32;
            let bottom = (r.bottom() * k).ceil().clamp(0.0, h) as i32;
            (right > left && bottom > top).then_some(RECT {
                left,
                top,
                right,
                bottom,
            })
        })
        .collect()
}

/// How tall the first-run card's lines run on a layer `layer_w` wide, set
/// by `text` at Text size zoom `zoom`.
fn first_run_text(
    text: &Text,
    zoom: f32,
    fr: &ways_in::FirstRun,
    layer_w: f32,
) -> layout::CardText {
    use crate::win::text::{Ramp, Style};
    let (step_w, more_w) = layout::first_run_widths(layer_w);
    // Measured with the words they set strong, as they are drawn.
    let h = |s: &str, ramp: Ramp, w: f32| {
        let strong = ways_in::strong_runs(s, fr.strong);
        text.styled_height_runs(s, Style::new(ramp).wrapping(), zoom, w, &strong)
    };
    layout::CardText {
        steps: fr.steps.map(|s| h(s, Ramp::Body, step_w)),
        more: h(fr.more, Ramp::Caption, more_w),
    }
}

/// The smallest rectangle holding all of `rects`, which are not empty.
fn bounds_of(rects: &[RECT]) -> RECT {
    rects.iter().fold(rects[0], |a, r| RECT {
        left: a.left.min(r.left),
        top: a.top.min(r.top),
        right: a.right.max(r.right),
        bottom: a.bottom.max(r.bottom),
    })
}

/// Round caps and joins, for the mark and the wheel's arcs.
fn round_stroke(factory: &ID2D1Factory) -> Result<ID2D1StrokeStyle> {
    let props = D2D1_STROKE_STYLE_PROPERTIES {
        startCap: D2D1_CAP_STYLE_ROUND,
        endCap: D2D1_CAP_STYLE_ROUND,
        dashCap: D2D1_CAP_STYLE_ROUND,
        lineJoin: D2D1_LINE_JOIN_ROUND,
        miterLimit: 10.0,
        dashStyle: D2D1_DASH_STYLE_SOLID,
        dashOffset: 0.0,
    };
    unsafe { factory.CreateStrokeStyle(&props, None) }
}

/// A dashed line, 4 on and 3 off for a 1 DIP stroke, with flat ends.
fn dash_stroke(factory: &ID2D1Factory) -> Result<ID2D1StrokeStyle> {
    let props = D2D1_STROKE_STYLE_PROPERTIES {
        startCap: D2D1_CAP_STYLE_FLAT,
        endCap: D2D1_CAP_STYLE_FLAT,
        dashCap: D2D1_CAP_STYLE_FLAT,
        lineJoin: D2D1_LINE_JOIN_ROUND,
        miterLimit: 10.0,
        dashStyle: D2D1_DASH_STYLE_CUSTOM,
        dashOffset: 0.0,
    };
    unsafe { factory.CreateStrokeStyle(&props, Some(&[4.0, 3.0])) }
}

/// Kiln's mark as an outline, on its 24-unit grid.
fn mark_geometry(factory: &ID2D1Factory) -> Result<ID2D1PathGeometry> {
    unsafe {
        let geom = factory.CreatePathGeometry()?;
        let sink = geom.Open()?;
        let mut open = false;
        let begin = D2D1_FIGURE_BEGIN_HOLLOW;
        for c in MARK_PATH {
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

/// How wide the credit's parts run in Caption at Text size zoom `zoom`:
/// the heart as the colour emoji where Windows has it, else as the plain
/// heart.
pub(in crate::win) fn credit_widths(text: &Text, zoom: f32) -> crate::credit::Widths {
    use crate::credit;
    use crate::win::text::{Ramp, Style};
    let line = Ramp::Caption.style().line;
    let run = |s: &str| {
        text.styled(s, Style::new(Ramp::Caption), zoom, 4096.0, line)
            .map_or(s.chars().count() as f32 * 7.0, |l| l.width)
    };
    let heart = text
        .heart(zoom)
        .ok()
        .flatten()
        .map_or_else(|| run(credit::HEART_PLAIN), |l| l.width);
    credit::Widths {
        made: run(credit::MADE),
        heart,
        by: run(credit::BY),
        name: run(credit::NAME),
    }
}
