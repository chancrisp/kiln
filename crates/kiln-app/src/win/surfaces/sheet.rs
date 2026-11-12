//! The options dialog: one conversion's quality, chosen before it starts
//! (spec 2.2), a ContentDialog with Quality as radio buttons, or as the
//! sound or video slider, and More options' lines as drop-downs.
//!
//! The dialog owns what it shows, the quality it carries and where the
//! pointer and the keyboard are. What reaches past it goes through
//! `cx.out`: the capture a pressed control takes, the animation timer, and
//! the job a Convert starts (`Out::start`).
//! `App` keeps the channel the sheet's worker answers on, and hands the
//! answers back (`Sheet::probed`, `Sheet::carded`).

use super::*;

use crate::a11y::SheetLine;
use crate::controls::{ComboEvent, ComboState, Key, Press};

/// Most files the options sheet reads for its estimate.
const PROBE_LIMIT: usize = 64;

/// What the options sheet's worker reads of `paths` with `probe`, a file at
/// a time, up to [`PROBE_LIMIT`]: a file that can't be read is one with
/// nothing known. A look that panics ends with no facts at all, which
/// `Sheet::probed` takes for files not read (fewer than there are), and
/// the sheet shows its no-facts state instead of reading for good.
fn read_infos(paths: &[PathBuf], probe: impl Fn(&Path) -> Option<MediaInfo>) -> Vec<MediaInfo> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        paths
            .iter()
            .take(PROBE_LIMIT)
            .map(|p| probe(p).unwrap_or_default())
            .collect()
    }))
    .unwrap_or_default()
}

/// How long `KILN_TEST_SLOW_INFO` (milliseconds, up to a minute) asks the
/// sheet's reading of its files to take at least: a number of whole
/// milliseconds, otherwise none.
#[cfg_attr(not(any(debug_assertions, feature = "test-hooks")), allow(dead_code))]
fn parse_slow_info(value: Option<&std::ffi::OsStr>) -> Option<std::time::Duration> {
    let ms: u64 = value?.to_str()?.trim().parse().ok()?;
    (1..=60_000)
        .contains(&ms)
        .then(|| std::time::Duration::from_millis(ms))
}

/// The hold `KILN_TEST_SLOW_INFO` asks for, so a capture can see the card
/// that says "Reading the file…". Off in a build without debug assertions
/// or `test-hooks`.
#[cfg(any(debug_assertions, feature = "test-hooks"))]
fn test_slow_info() -> Option<std::time::Duration> {
    parse_slow_info(std::env::var_os("KILN_TEST_SLOW_INFO").as_deref())
}

#[cfg(not(any(debug_assertions, feature = "test-hooks")))]
fn test_slow_info() -> Option<std::time::Duration> {
    None
}

/// The options sheet's files, read on a worker thread.
pub(in crate::win) struct Probed {
    id: u64,
    infos: Vec<MediaInfo>,
}

/// Whether this PC has a graphics card encoder for H.264 and for HEVC, for
/// the video sheet `id`, found on the same worker thread after its files.
pub(in crate::win) struct CardsFound {
    id: u64,
    cards: [bool; 2],
}

/// Read `paths` for the sheet `id` on a worker thread, and with `cards`
/// look for the graphics card's encoders after them. The answers reach the
/// window through `wake`.
pub(in crate::win) fn look(
    kiln: Arc<Kiln>,
    opts: Options,
    wake: Wake,
    id: u64,
    paths: Vec<PathBuf>,
    cards: bool,
) {
    kiln_core::spawn_parser("kiln-probe", move || {
        // A file that panics the look is a sheet with no facts, never
        // one that is still reading.
        let infos = read_infos(&paths, |p| kiln.probe(p, &opts));
        if let Some(hold) = test_slow_info() {
            std::thread::sleep(hold);
        }
        wake.send(Event::Probed(Probed { id, infos }));
        if cards {
            use kiln_media::video::{encoder_adapter, EncoderChoice, VCodec};
            let has = |c| encoder_adapter(c, EncoderChoice::GraphicsCard).is_some();
            // Without the answer the sheet keeps what it assumed.
            if let Ok(cards) = std::panic::catch_unwind(|| [has(VCodec::H264), has(VCodec::Hevc)]) {
                wake.send(Event::Cards(CardsFound { id, cards }));
            }
        }
    })
    .expect("start a worker");
}

/// What `App` works out for a sheet before it opens: it knows the
/// registry, the rows and the saved defaults.
pub(in crate::win) struct Opening {
    pub id: u64,
    pub paths: Vec<PathBuf>,
    /// The first file's format; the knobs follow it.
    pub from: Format,
    pub to: Format,
    pub knobs: Vec<Knob>,
    /// The saved default for the pair.
    pub quality: Quality,
    /// What the conversion leaves out, from what needs no reading and what
    /// the rows' facts already say of their tracks.
    pub notes: Vec<String>,
    pub text_only: bool,
    /// How big the files are together, as their rows say.
    pub bytes: u64,
    /// For one file, its row's own facts line ("1920 × 1080 video · 0:40 ·
    /// 31 MB"), which the dialog's sub-line uses. `None` when the
    /// file has no row, as on the drag-wheel.
    pub facts: Option<String>,
}

/// The options dialog (spec 2.2) while it is open, and while it fades
/// out: "Convert to MP4", Quality as radio buttons, the estimate card and
/// More options, then Convert and Cancel.
pub(in crate::win) struct Sheet {
    id: u64,
    paths: Vec<PathBuf>,
    from: Format,
    to: Format,
    knobs: Vec<Knob>,
    quality: Quality,
    /// What the files hold, once read; `None` while reading.
    infos: Option<Vec<MediaInfo>>,
    /// What the conversion leaves out, as the job will say it (`sheet_notes`):
    /// worked out when the sheet opens and again once the files are read,
    /// since the tracks of a video are known only then.
    notes: Vec<String>,
    /// Every file becomes the target by way of its text alone, so the
    /// target line says "(text only)" (`model::text_only_for`).
    text_only: bool,
    names: Vec<String>,
    bytes: u64,
    /// The one file's row facts, for the sub-line.
    facts: Option<String>,
    /// The index (into the layout's controls) of the control the pointer
    /// is over, and of the one it went down on; that one acts on release.
    pointer: Pointer,
    /// The control the keyboard is on: Convert as it opens.
    focus: Option<usize>,
    /// The focus visual shows: it was opened from the keyboard, or a key
    /// has been pressed since.
    keys: bool,
    /// More options is open.
    more_open: bool,
    /// A More options line's drop-down, while its list is open.
    list: Option<(SheetControl, ComboState)>,
    /// How far the content is scrolled, when it is taller than the window
    /// lets it be.
    scroll: f32,
    opened: Instant,
    closing: Option<Instant>,
    /// The quality's model, for sound or video Kiln writes itself: Quality
    /// is its radio buttons, its fine steps and fields More options' lines.
    tune: Option<TuneUi>,
    /// Opened from a finished drag-wheel row's Options: its Convert redoes
    /// that row, replacing what it holds without asking (`redo`).
    redo: bool,
    /// Opened by Options picked on the drag-wheel: its Convert is a
    /// drag-wheel conversion, saved next to the original.
    dropped: bool,
    /// One of Kiln's in-window dialogs while it lives: drops wait, and a
    /// close brings the window forward (`system::modal_open`).
    _open: system::InWindow,
}

/// The sound or video quality's model behind the options dialog.
struct TuneUi {
    sheet: SheetKind,
    /// The Quality radio last chosen, which stays on while the quality is
    /// on its step (two radios can share one: FLAC's High and Best).
    picked: Option<usize>,
    /// The file the estimate and Max size are for: the one file once it is
    /// read. Several files, or one not read yet, have none of their own (a
    /// stand-in, `TuneSheet::new`, `VideoSheet::new`).
    info: MediaInfo,
    /// The quality the sheet opened with, so the slider can be worked out
    /// again from it once the file is read, until it is moved.
    opened_with: Quality,
    /// Whether anything on Quality or under More options was changed.
    touched: bool,
}

impl TuneUi {
    /// The slider converting to `to` with `knobs`, when they hold one, for
    /// `quality`, which from now on carries what the sheet offers, even
    /// before the file is read.
    fn new(knobs: &[Knob], to: &str, quality: &mut Quality) -> Option<Self> {
        let info = MediaInfo::default();
        let opened_with = *quality;
        let sheet = SheetKind::opened(knobs, to, &info, quality)?;
        Some(TuneUi {
            sheet,
            picked: None,
            info,
            opened_with,
            touched: false,
        })
    }

    /// The slider was changed: `quality` carries what the conversion will
    /// get.
    fn changed(&mut self, quality: &mut Quality) {
        self.touched = true;
        self.sheet.carry(quality);
    }

    /// The file was read: the slider is worked out again for it
    /// (`SheetKind::reread`).
    fn read(&mut self, info: MediaInfo, quality: &mut Quality) {
        self.sheet = self.sheet.reread(&info, &self.opened_with, self.touched);
        self.info = info;
        self.sheet.carry(quality);
    }

    /// The Quality radio buttons' names as the tree and the drawing need
    /// them.
    fn radios(&self) -> Vec<&'static str> {
        self.sheet
            .radios()
            .into_iter()
            .map(|(name, _)| name)
            .collect()
    }
}

/// What the sheet works out once for a frame or a tree, which the view
/// and the tree borrow: the layout as it is drawn now and its words.
pub(in crate::win) struct SheetFrame {
    layout: SheetLayout,
    title: String,
    /// The one line under the title.
    sub: String,
    /// The estimate card's line: the size, or Max size's error.
    result: String,
    /// The same in words, for Narrator.
    spoken: String,
    /// Whether `result` is Max size's error.
    error: bool,
    /// Each More options line as laid out.
    lines: Vec<SheetLine>,
    /// The Quality radio buttons' names, when they are the quality's.
    radios: Vec<&'static str>,
    /// An open drop-down's list as laid out.
    list: Option<(SheetControl, ComboState, layout::kit::FlyoutLayout)>,
}

/// The sheet control a control id is.
fn index(c: ControlId) -> Option<usize> {
    match c {
        ControlId::Sheet(i) => Some(i),
        _ => None,
    }
}

impl Sheet {
    /// The dialog for `o`, opened at `now`, the keyboard on Convert; from
    /// the keyboard (`keyboard`) its focus visual shows.
    pub fn new(o: Opening, size: (f32, f32), keyboard: bool, now: Instant) -> Self {
        let names = o
            .paths
            .iter()
            .map(|p| {
                p.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        let bytes = o.bytes;
        let facts = o.facts;
        let mut quality = o.quality;
        let tune = TuneUi::new(&o.knobs, &o.to.ext, &mut quality);
        let mut sheet = Sheet {
            id: o.id,
            paths: o.paths,
            from: o.from,
            to: o.to,
            knobs: o.knobs,
            quality,
            infos: None,
            notes: o.notes,
            text_only: o.text_only,
            names,
            bytes,
            facts,
            pointer: Pointer::default(),
            focus: None,
            keys: keyboard,
            more_open: false,
            list: None,
            scroll: 0.0,
            opened: now,
            closing: None,
            tune,
            redo: false,
            dropped: false,
            _open: system::InWindow::open(),
        };
        // Focus starts on the accent button (spec 5.4).
        sheet.focus = sheet.index_of(size, SheetControl::Convert);
        sheet
    }

    /// The files it converts.
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// Whether it is the video sheet, which asks what the graphics card
    /// can encode.
    pub fn wants_cards(&self) -> bool {
        matches!(&self.tune, Some(t) if matches!(t.sheet, SheetKind::Video(_)))
    }

    /// Its Convert redoes a finished drag-wheel row.
    pub fn set_redo(&mut self) {
        self.redo = true;
    }

    /// Its Convert is a drag-wheel conversion.
    pub fn set_dropped(&mut self) {
        self.dropped = true;
    }

    pub fn is_open(&self) -> bool {
        self.closing.is_none()
    }

    /// Whether anything on it moves at `now`: it rises or fades.
    pub fn animating(&self, now: Instant, animations: bool) -> bool {
        anim::within(self.opened, anim::SHEET_MS, now, animations) || self.closing.is_some()
    }

    /// Whether it has faded out and can be dropped.
    pub fn gone(&self, now: Instant, animations: bool) -> bool {
        self.closing
            .is_some_and(|t| !anim::within(t, theme::fluent::motion::QUICK_MS, now, animations))
    }

    /// What the knobs can be set to. With one file read, sizes it already
    /// fits under are left out.
    fn values(&self, knob: Knob) -> Vec<Value> {
        let info = match self.infos.as_deref() {
            Some([one]) => Some(one),
            _ => None,
        };
        q::values(knob, &self.from, &self.to, info)
    }

    fn shown(&self, knob: Knob) -> Value {
        q::shown(&self.quality, knob, &self.values(knob))
    }

    /// Step a knob along its values, wrapping at the ends.
    fn step(&mut self, row: usize, delta: isize) {
        let Some(&knob) = self.knobs.get(row) else {
            return;
        };
        let list = self.values(knob);
        let v = q::stepped(&list, self.shown(knob), delta, true);
        q::set(&mut self.quality, knob, v);
    }

    /// Whether Convert can be pressed: not while Max size asks for a size
    /// the format can't reach.
    pub fn can_convert(&self) -> bool {
        self.tune
            .as_ref()
            .is_none_or(|t| t.sheet.can_convert(&t.info))
    }

    /// Step a field under More options.
    fn tune_field(&mut self, f: TuneField, delta: isize) {
        if let Some(t) = &mut self.tune {
            t.sheet.step_field(f, delta, &t.info);
            t.changed(&mut self.quality);
        }
    }

    /// Choose Quality radio `i`: the quality goes to its step, and what it
    /// carries follows a real move.
    fn pick_radio(&mut self, i: usize) {
        if let Some(t) = &mut self.tune {
            let at = t.sheet.at();
            t.sheet.choose_radio(i);
            t.picked = Some(i);
            if t.sheet.at() != at {
                t.changed(&mut self.quality);
            }
        }
    }

    /// Which Quality radio button is on, if any: the preset's, or for
    /// sound and video the one the quality is on (none between them).
    fn selected(&self) -> Option<usize> {
        if let Some(t) = &self.tune {
            return t.sheet.radio_on(t.picked);
        }
        kiln_core::Preset::ALL
            .iter()
            .position(|p| *p == self.quality.preset)
    }

    /// A More options line's values as its drop-down lists them, and the
    /// one it has: a knob's values, or a field's (`field_choices`).
    /// `None` for a line that keeps its spin box.
    fn choices(&self, c: SheetControl) -> Option<(Vec<String>, usize)> {
        match c {
            SheetControl::Step(row) => {
                let knob = *self.knobs.get(row)?;
                let unit = q::size_unit(&self.to);
                let list = self.values(knob);
                let now = self.shown(knob);
                let at = list.iter().position(|v| *v == now).unwrap_or(0);
                Some((list.iter().map(|&v| q::label(knob, v, unit)).collect(), at))
            }
            SheetControl::Field(f) => {
                let t = self.tune.as_ref()?;
                t.sheet.field_choices(f, &t.info)
            }
            _ => None,
        }
    }

    /// Whether a More options line can be changed now.
    fn line_enabled(&self, c: SheetControl) -> bool {
        match (c, &self.tune) {
            (SheetControl::Field(f), Some(t)) => t.sheet.field_enabled(f, &t.info),
            _ => true,
        }
    }

    /// Choose value `i` of line `c`'s drop-down.
    fn pick(&mut self, c: SheetControl, i: usize) {
        match c {
            SheetControl::Step(row) => {
                let Some(&knob) = self.knobs.get(row) else {
                    return;
                };
                if let Some(&v) = self.values(knob).get(i) {
                    q::set(&mut self.quality, knob, v);
                }
            }
            SheetControl::Field(f) => {
                if let Some(t) = &mut self.tune {
                    let before = t.sheet.field_value(f);
                    t.sheet.pick_field(f, i, &t.info);
                    if t.sheet.field_value(f) != before {
                        t.changed(&mut self.quality);
                    }
                }
            }
            _ => {}
        }
    }

    /// Open line `c`'s drop-down, when it has one and can be changed.
    fn open_list(&mut self, cx: &mut Ctx, c: SheetControl) {
        if !self.line_enabled(c) {
            return;
        }
        let Some((names, at)) = self.choices(c) else {
            return;
        };
        let mut state = ComboState::new(names.len(), at);
        if state.click() == ComboEvent::Opened {
            self.focus = self.index_of(cx.size, c).or(self.focus);
            self.list = Some((c, state));
            self.pointer.hover = None;
        }
        cx.out.invalidate = true;
    }

    /// The open drop-down's list, laid out over its line and kept inside
    /// the window.
    fn list_layout(&self, size: (f32, f32), now: Instant) -> Option<layout::kit::FlyoutLayout> {
        let (c, state) = self.list?;
        let anchor = self.layout(size, now).rect_of(c)?;
        Some(layout::kit::combo_flyout(
            anchor,
            state.len,
            state.value,
            layout::Rect::new(0.0, 0.0, size.0, size.1),
        ))
    }

    /// The list item under a point, if the list is open.
    fn list_item(&self, cx: &Ctx, x: f32, y: f32) -> Option<usize> {
        let l = self.list_layout(cx.size, cx.now)?;
        l.items
            .iter()
            .position(|r| r.contains(x, y))
            .map(|k| l.first + k)
    }

    /// What a key or click did to the open drop-down: a new value is
    /// chosen, or the list closes as it was.
    fn list_event(&mut self, cx: &mut Ctx, e: ComboEvent) {
        let Some((c, _)) = self.list else {
            return;
        };
        match e {
            ComboEvent::Changed(i) => {
                self.list = None;
                self.pick(c, i);
            }
            ComboEvent::Closed => self.list = None,
            ComboEvent::Moved(_) | ComboEvent::Opened | ComboEvent::Ignored => {}
        }
        cx.out.invalidate = true;
    }

    /// The lines under More options: a stepper for each knob that isn't
    /// Quality, then the slider's fields.
    fn more(&self) -> Vec<SheetControl> {
        let mut more: Vec<SheetControl> = self
            .knobs
            .iter()
            .enumerate()
            .filter(|(_, k)| !matches!(k, Knob::Preset | Knob::AudioTune | Knob::VideoTune))
            .map(|(i, _)| SheetControl::Step(i))
            .collect();
        if let Some(t) = &self.tune {
            more.extend(t.sheet.fields().iter().map(|f| SheetControl::Field(*f)));
        }
        more
    }

    /// What the layout needs.
    fn shape(&self) -> layout::SheetShape {
        let preset = self.knobs.iter().position(|k| *k == Knob::Preset);
        let tune_radios = match (&self.tune, preset) {
            (Some(t), None) => t.sheet.radios().len(),
            _ => 0,
        };
        layout::SheetShape {
            quality: preset,
            tune_radios,
            more: self.more(),
            more_open: self.more_open,
            notes: self.notes.len(),
            card: !self.result_line().0.is_empty(),
            scroll: self.scroll,
        }
    }

    /// The dialog laid out in a window `size`.
    pub fn layout(&self, size: (f32, f32), _now: Instant) -> SheetLayout {
        layout::options_dialog(size, &self.shape())
    }

    /// The index of control `c` in the layout's Tab order.
    fn index_of(&self, size: (f32, f32), c: SheetControl) -> Option<usize> {
        self.layout(size, self.opened)
            .controls
            .iter()
            .position(|(k, _)| *k == c)
    }

    /// The buttons the pointer is over and has pressed, for how they ease.
    /// The layout is worked out only while the pointer is on a control.
    pub fn buttons(&self, size: (f32, f32), now: Instant) -> (Option<ButtonId>, Option<ButtonId>) {
        let (hover, pressed) = (
            self.pointer.hover.and_then(index),
            self.pointer.pressed.and_then(index),
        );
        if hover.is_none() && pressed.is_none() {
            return (None, None);
        }
        let l = self.layout(size, now);
        let at = |k: Option<usize>| {
            let (c, _) = l.controls.get(k?)?;
            ButtonId::sheet(*c)
        };
        (at(hover), at(pressed))
    }

    /// The sheet control the keyboard is on.
    fn focus_control(&self, size: (f32, f32), now: Instant) -> Option<SheetControl> {
        let i = self.focus?;
        Some(self.layout(size, now).controls.get(i)?.0)
    }

    /// More options opened or closed, or the file was read, and the
    /// controls after it moved: the keyboard stays on the control it was
    /// on, or where it was if that one went.
    fn keep_focus(&mut self, cx: &Ctx, before: Option<SheetControl>) {
        let l = self.layout(cx.size, cx.now);
        let at = before.and_then(|c| l.controls.iter().position(|(k, _)| *k == c));
        self.focus = at.or(self.focus.map(|i| i.min(l.controls.len() - 1)));
        self.pointer.hover = None;
        self.reveal(cx);
    }

    /// Scroll so the focused control shows in the content.
    fn reveal(&mut self, cx: &Ctx) {
        let l = self.layout(cx.size, cx.now);
        let Some(&(c, r)) = self.focus.and_then(|i| l.controls.get(i)) else {
            return;
        };
        if matches!(c, SheetControl::Convert | SheetControl::Cancel) {
            return;
        }
        let content = l.frame.content;
        if r.y < content.y {
            self.scroll -= content.y - r.y;
        } else if r.bottom() > content.bottom() {
            self.scroll += r.bottom() - content.bottom();
        }
        self.scroll = self.scroll.clamp(0.0, l.scroll_max);
    }

    /// The files were read. `notes` says what the conversion leaves out,
    /// as the job will know it, from the files, the target and what was
    /// found of them.
    pub fn probed(
        &mut self,
        cx: &mut Ctx,
        p: Probed,
        notes: impl FnOnce(&[PathBuf], &str, Option<&[MediaInfo]>) -> Vec<String>,
    ) {
        if self.id != p.id {
            return;
        }
        let before = self.focus_control(cx.size, cx.now);
        // Estimates only count when every file was read.
        self.infos = Some(if p.infos.len() == self.paths.len() {
            p.infos
        } else {
            vec![]
        });
        // The tracks of a video are known now, as the job will know them.
        self.notes = notes(&self.paths, &self.to.ext, self.infos.as_deref());
        // The slider sizes one file; with several it has none to size.
        let one = match self.infos.as_deref() {
            Some([one]) => one.clone(),
            _ => MediaInfo::default(),
        };
        if let Some(t) = &mut self.tune {
            t.read(one, &mut self.quality);
        }
        self.keep_focus(cx, before);
        cx.out.invalidate = true;
    }

    /// The graphics card's encoders were looked for: the video's sizes are
    /// worked out again, as the size model now knows which encoder writes
    /// the picture (R82: until then it took the graphics card's), and a
    /// saved Graphics card with none for the codec is Automatic.
    pub fn carded(&mut self, cx: &mut Ctx, c: CardsFound) {
        if self.id != c.id {
            return;
        }
        let before = self.focus_control(cx.size, cx.now);
        if let Some(t) = self.tune.as_mut() {
            t.sheet.set_cards(c.cards);
            let info = t.info.clone();
            t.read(info, &mut self.quality);
            self.keep_focus(cx, before);
            cx.out.invalidate = true;
        }
    }

    /// Close it: Convert starts the job, Cancel and Escape do not.
    fn close(&mut self, cx: &mut Ctx, convert: bool) {
        if !self.is_open() || (convert && !self.can_convert()) {
            return;
        }
        self.closing = Some(Instant::now());
        self.pointer.hover = None;
        self.list = None;
        if convert {
            cx.out.start = Some(Start {
                action: Action::Convert(self.to.ext.clone()),
                paths: self.paths.clone(),
                quality: self.quality,
                via: if self.dropped {
                    Via::DropSheet
                } else {
                    Via::Window
                },
                redo: self.redo,
            });
        }
        cx.out.animate = true;
    }

    /// Cancel, as Escape and the Window pattern's Close do.
    pub fn cancel(&mut self, cx: &mut Ctx) {
        self.close(cx, false);
    }

    /// Press a control. `delta` steps a spin box's value back or
    /// forward, or the slider; a drop-down opens its list.
    fn activate(&mut self, cx: &mut Ctx, c: SheetControl, delta: isize) {
        match c {
            SheetControl::Cancel => return self.close(cx, false),
            SheetControl::Convert => return self.close(cx, true),
            SheetControl::Preset(_, preset) => self.quality.preset = preset,
            SheetControl::Tune(i) => self.pick_radio(i),
            SheetControl::Step(_) | SheetControl::Field(_) if self.choices(c).is_some() => {
                return self.open_list(cx, c)
            }
            SheetControl::Step(row) => self.step(row, delta),
            SheetControl::Advanced => return self.advanced(cx, !self.more_open),
            SheetControl::Field(f) => self.tune_field(f, delta),
        }
        // The radio pressed takes the keyboard, as RadioButtons' does.
        if c.is_radio() {
            self.focus = self.index_of(cx.size, c).or(self.focus);
        }
        cx.out.invalidate = true;
    }

    /// Open or close More options. The content grows or shrinks around
    /// it, and the keyboard stays where it was.
    fn advanced(&mut self, cx: &mut Ctx, open: bool) {
        if self.more_open == open || self.more().is_empty() {
            return;
        }
        let before = self.focus_control(cx.size, cx.now);
        self.more_open = open;
        if let Some(t) = self.tune.as_mut() {
            if t.sheet.advanced_open() != open {
                t.sheet.toggle_advanced();
            }
        }
        self.keep_focus(cx, before);
        if open {
            // Its lines come into view under it, its header kept.
            let l = self.layout(cx.size, cx.now);
            if let (Some(e), Some(last)) = (l.expander, l.lines.last()) {
                let content = l.frame.content;
                let want = (last.2.bottom() + 8.0 - content.bottom()).max(0.0);
                let keep = (e.header.y - content.y).max(0.0);
                self.scroll = (self.scroll + want.min(keep)).clamp(0.0, l.scroll_max);
            }
        }
        cx.out.invalidate = true;
    }

    /// The pointer moved. The dialog takes every move: over an open
    /// list it lights the item under it.
    pub fn moved(&mut self, cx: &mut Ctx, x: f32, y: f32, _held: bool) -> Reply {
        if let Some((c, mut state)) = self.list {
            if let Some(i) = self.list_item(cx, x, y) {
                if state.highlight != i {
                    state.highlight = i;
                    self.list = Some((c, state));
                    cx.out.invalidate = true;
                }
            }
            return Reply::Taken;
        }
        let hover = self.hit(cx, x, y);
        if self.is_open() && self.pointer.hover != hover {
            self.pointer.hover = hover;
            cx.out.invalidate = true;
        }
        Reply::Taken
    }

    /// The radio button `delta` along from the one the keyboard is on,
    /// picked, as RadioButtons' arrow keys do; the ends hold.
    fn radio_step(&mut self, cx: &mut Ctx, l: &SheetLayout, delta: isize) {
        let Some(i) = self.focus else {
            return;
        };
        let j = i as isize + delta;
        let Some(&(c, _)) = usize::try_from(j).ok().and_then(|j| l.controls.get(j)) else {
            return;
        };
        if c.is_radio() {
            self.focus = Some(j as usize);
            self.activate(cx, c, 1);
        }
    }

    /// Tab or Shift+Tab: the next control, the radio buttons one stop
    /// (the one on, or the first), round from Cancel to the first.
    fn tab(&mut self, l: &SheetLayout, back: bool) {
        let n = l.controls.len();
        let from = self.focus.unwrap_or(n - 1);
        let in_radios = l.controls[from].0.is_radio();
        let mut i = from;
        loop {
            i = if back { (i + n - 1) % n } else { (i + 1) % n };
            let c = l.controls[i].0;
            if i == from || !(c.is_radio() && in_radios) {
                break;
            }
        }
        // Into the radio buttons: on the one that is on.
        if l.controls[i].0.is_radio() {
            let first = l
                .controls
                .iter()
                .position(|(c, _)| c.is_radio())
                .unwrap_or(i);
            i = self.selected().map_or(first, |k| first + k);
        }
        self.focus = Some(i);
    }

    /// Tab and Shift+Tab go round the controls; the arrow keys pick a
    /// radio button or step a line's value; Enter converts unless Cancel
    /// or More options has the keyboard; Space presses; Escape cancels.
    /// A key Windows repeats (`repeat`) presses nothing with Enter or
    /// Space.
    fn on_key(&mut self, cx: &mut Ctx, key: VIRTUAL_KEY, mods: Mods) {
        if !self.is_open() {
            return;
        }
        self.keys = true;
        if mods.repeat && matches!(key, VK_RETURN | VK_SPACE) {
            return;
        }
        let as_key = match key {
            VK_UP => Some(Key::Up),
            VK_DOWN => Some(Key::Down),
            VK_HOME => Some(Key::Home),
            VK_END => Some(Key::End),
            VK_RETURN => Some(Key::Enter),
            VK_SPACE => Some(Key::Space),
            VK_ESCAPE => Some(Key::Esc),
            VK_TAB => Some(Key::Tab),
            VK_F4 => Some(Key::F4),
            _ => None,
        };
        let press = |k: Key| Press {
            key: k,
            alt: false,
            ctrl: mods.ctrl,
            shift: mods.shift,
        };
        // An open list keeps the keys: Esc and Tab close it as it was.
        if let Some((c, mut state)) = self.list {
            if let Some(k) = as_key {
                let e = state.key(press(k));
                self.list = Some((c, state));
                self.list_event(cx, e);
            }
            return;
        }
        let l = self.layout(cx.size, cx.now);
        let focused = self.focus.and_then(|i| l.controls.get(i)).map(|(c, _)| *c);
        let combo = focused.filter(|&c| self.line_enabled(c) && self.choices(c).is_some());
        match (focused, key) {
            (_, VK_ESCAPE) => return self.close(cx, false),
            (_, VK_TAB) => self.tab(&l, mods.shift),
            (Some(c), VK_LEFT | VK_UP) if c.is_radio() => return self.radio_step(cx, &l, -1),
            (Some(c), VK_RIGHT | VK_DOWN) if c.is_radio() => return self.radio_step(cx, &l, 1),
            // A drop-down: Enter, Space and F4 open its list; Up and Down
            // choose the value before or after (WinUI ComboBox).
            (Some(c), VK_RETURN | VK_SPACE | VK_F4 | VK_UP | VK_DOWN) if combo == Some(c) => {
                let (len, at) = self
                    .choices(c)
                    .map_or((0, 0), |(names, at)| (names.len(), at));
                let mut state = ComboState::new(len, at);
                match state.key(press(as_key.unwrap_or(Key::Enter))) {
                    ComboEvent::Opened => return self.open_list(cx, c),
                    ComboEvent::Changed(i) => self.pick(c, i),
                    _ => {}
                }
            }
            (Some(c @ (SheetControl::Step(_) | SheetControl::Field(_))), VK_LEFT | VK_RIGHT) => {
                let delta = if key == VK_LEFT { -1 } else { 1 };
                match c {
                    SheetControl::Step(row) => self.step(row, delta),
                    SheetControl::Field(f) => self.tune_field(f, delta),
                    _ => {}
                }
            }
            (_, VK_UP | VK_DOWN) => {
                let n = l.controls.len() as isize;
                let delta = if key == VK_UP { -1 } else { 1 };
                let at = self.focus.map_or(n - 1, |i| i as isize);
                self.focus = Some((at + delta).rem_euclid(n) as usize);
            }
            (Some(c @ SheetControl::Advanced), VK_RETURN | VK_SPACE) => {
                return self.activate(cx, c, 1)
            }
            (Some(SheetControl::Cancel), VK_RETURN) => return self.close(cx, false),
            (_, VK_RETURN) => return self.close(cx, true),
            (Some(c), VK_SPACE) => return self.activate(cx, c, 1),
            _ => return,
        }
        self.reveal(cx);
        cx.out.invalidate = true;
    }

    /// What UI Automation asks of control `c`.
    pub fn uia(&mut self, cx: &mut Ctx, c: SheetControl, verb: &a11y::Verb) {
        use a11y::Verb;
        if !self.is_open() {
            return;
        }
        self.focus = self.index_of(cx.size, c).or(self.focus);
        match (c, verb) {
            (SheetControl::Step(_) | SheetControl::Field(_), Verb::SetValue(v)) => {
                if !self.line_enabled(c) {
                    return;
                }
                let to = self.choices(c).and_then(|(names, _)| {
                    names.iter().position(|n| n.eq_ignore_ascii_case(v.trim()))
                });
                if let Some(i) = to {
                    self.list = None;
                    self.pick(c, i);
                }
            }
            (SheetControl::Step(_) | SheetControl::Field(_), Verb::Expand(open)) => {
                let is_open = self.list.is_some_and(|(k, _)| k == c);
                if *open && !is_open {
                    return self.open_list(cx, c);
                }
                if !*open && is_open {
                    self.list = None;
                }
            }
            (SheetControl::Advanced, Verb::Expand(open)) => {
                return self.advanced(cx, *open);
            }
            (_, Verb::Invoke | Verb::Select) => return self.activate(cx, c, 1),
            _ => {}
        }
        self.reveal(cx);
        cx.out.invalidate = true;
    }

    /// UI Automation picked item `i` of line `c`'s drop-down list.
    pub fn uia_pick(&mut self, cx: &mut Ctx, c: SheetControl, i: usize) {
        if !self.is_open() || !self.line_enabled(c) {
            return;
        }
        self.focus = self.index_of(cx.size, c).or(self.focus);
        self.list = None;
        self.pick(c, i);
        cx.out.invalidate = true;
    }

    /// The estimate card's line and the same in words: the result's size,
    /// "About 1.1 MB", or Max size's error; for the other pairs the
    /// result's size and a rough file size. Empty when there is no size
    /// to give, and then the card is left out.
    fn result_line(&self) -> (String, String, bool) {
        if let Some(t) = &self.tune {
            let ts = &t.sheet;
            if let Some(e) = ts.fit_error(&t.info) {
                return (e.clone(), e, true);
            }
            return match ts.readout(&t.info) {
                Some(line) => {
                    let spoken = crate::tune_sheet::spoken(&line);
                    (line, spoken, false)
                }
                None => (String::new(), String::new(), false),
            };
        }
        let line = match self.infos.as_deref() {
            // Only a pair that has a size to say asks for a card before
            // there is one: with none to give, the card would come and go.
            None if self.could_estimate() => "Reading the file…".to_owned(),
            None => String::new(),
            Some(i) => q::result_line(&self.from, &self.to, &self.quality, i),
        };
        (line.clone(), line, false)
    }

    /// Whether reading the files could give this pair a size to say: an
    /// estimate needs a length and, for video, a picture size.
    fn could_estimate(&self) -> bool {
        let read = MediaInfo {
            duration: Some(60.0),
            width: Some(1280),
            height: Some(720),
            ..MediaInfo::default()
        };
        kiln_core::quality::estimate(&self.to, &self.quality, &read).is_some()
    }

    /// The title, and the one line under it: the source as its row says
    /// it, "clip.mov · 1920 × 1080 video · 1:20 · 31 MB". What the format
    /// is stays out: the title says "Convert to MOV".
    fn heads(&self) -> (String, String) {
        let title = match self.paths.len() {
            1 => format!("Convert to {}", self.to.code()),
            n => format!("Convert {n} files to {}", self.to.code()),
        };
        let mut sub = match (&self.facts, self.names.as_slice()) {
            (Some(facts), [name]) => format!("{name}{}{facts}", crate::model::DETAIL_SEPARATOR),
            _ => {
                let infos = self.infos.as_deref();
                let one = infos.and_then(|i| (i.len() == 1).then(|| &i[0]));
                q::source_line(&self.names, self.bytes, one)
            }
        };
        // A PDF made into another kind of document keeps its text only.
        if self.text_only {
            sub.push_str(crate::model::DETAIL_SEPARATOR);
            sub.push_str("Text only");
        }
        (title, sub)
    }

    /// A More options line: its label, value, whether it can be changed,
    /// and its drop-down's values.
    fn line_words(&self, c: SheetControl) -> SheetLine {
        let unit = q::size_unit(&self.to);
        let (label, value) = match c {
            SheetControl::Step(row) => {
                let knob = self.knobs[row];
                (
                    q::knob_name(knob, &self.to).to_owned(),
                    q::label(knob, self.shown(knob), unit),
                )
            }
            SheetControl::Field(f) => (
                f.name().to_owned(),
                self.tune
                    .as_ref()
                    .map_or_else(String::new, |t| t.sheet.field_value(f)),
            ),
            _ => return SheetLine::default(),
        };
        SheetLine {
            label,
            value,
            enabled: self.line_enabled(c),
            choices: self.choices(c).map(|(names, _)| names),
        }
    }

    /// What it works out once for a frame or a tree, at `now`.
    pub fn frame(&self, size: (f32, f32), now: Instant) -> SheetFrame {
        let layout = self.layout(size, now);
        let (title, sub) = self.heads();
        let (result, spoken, error) = self.result_line();
        let lines = layout
            .lines
            .iter()
            .map(|(c, _, _)| self.line_words(*c))
            .collect();
        let radios = self
            .tune
            .as_ref()
            .filter(|_| !layout.radios.is_empty())
            .map(TuneUi::radios)
            .unwrap_or_default();
        let list = self
            .list
            .and_then(|(c, state)| Some((c, state, self.list_layout(size, now)?)));
        SheetFrame {
            layout,
            title,
            sub,
            result,
            spoken,
            error,
            lines,
            radios,
            list,
        }
    }
}

impl Surface for Sheet {
    type View<'a> = Option<SheetView<'a>>;
    type A11y<'a> = Option<a11y::SheetIn<'a>>;

    fn hit(&self, cx: &Ctx, x: f32, y: f32) -> Option<ControlId> {
        let l = self.layout(cx.size, cx.now);
        let content = l.frame.content;
        l.controls
            .iter()
            .position(|(c, r)| {
                // Content scrolled out of view can't be pressed.
                let band = matches!(c, SheetControl::Convert | SheetControl::Cancel);
                r.contains(x, y) && (band || content.contains(x, y))
            })
            .map(ControlId::Sheet)
    }

    fn hover(&mut self, cx: &mut Ctx, x: f32, y: f32) -> Reply {
        self.moved(cx, x, y, false)
    }

    /// The dialog takes every press; one on the smoke cancels. With a
    /// drop-down's list open, a press off the list closes it and does
    /// nothing else. A press on the slider puts its thumb there and holds
    /// it.
    fn press(&mut self, cx: &mut Ctx, x: f32, y: f32) -> Reply {
        if self.list.is_some() {
            let on_list = self
                .list_layout(cx.size, cx.now)
                .is_some_and(|l| l.rect.contains(x, y));
            if !on_list {
                self.list = None;
                self.pointer.pressed = None;
            }
            cx.out.invalidate = true;
            return Reply::Taken;
        }
        let inside = self.layout(cx.size, cx.now).panel.contains(x, y);
        let hit = self.hit(cx, x, y);
        match (self.is_open(), hit) {
            (true, Some(c)) => {
                self.pointer.pressed = Some(c);
                cx.out.effects.push(Effect::Capture);
            }
            (true, None) if !inside => self.close(cx, false),
            _ => {}
        }
        cx.out.invalidate = true;
        Reply::Taken
    }

    /// The control pressed acts if the pointer is still on it: a line's
    /// back chevron steps back, anywhere else on it steps forward.
    fn release(&mut self, cx: &mut Ctx, x: f32, y: f32) -> Reply {
        if let Some((c, mut state)) = self.list {
            if let Some(i) = self.list_item(cx, x, y) {
                let e = state.pick(i);
                self.list = Some((c, state));
                self.list_event(cx, e);
            }
            return Reply::Taken;
        }
        let pressed = self.pointer.pressed.take();
        let Some(i) = pressed.and_then(index) else {
            return Reply::Pass;
        };
        if self.hit(cx, x, y) == Some(ControlId::Sheet(i)) {
            let (c, r) = self.layout(cx.size, cx.now).controls[i];
            let (_, back, _) = layout::kit::spin_parts(r);
            let delta = if back.contains(x, y) { -1 } else { 1 };
            self.activate(cx, c, delta);
        }
        cx.out.invalidate = true;
        Reply::Taken
    }

    fn key(&mut self, cx: &mut Ctx, key: VIRTUAL_KEY, mods: Mods) -> Reply {
        self.on_key(cx, key, mods);
        Reply::Taken
    }

    /// The mouse wheel scrolls the content when it is taller than the
    /// window lets it be; nothing under the dialog scrolls.
    fn scroll(&mut self, cx: &mut Ctx, by: TextScroll) -> Reply {
        if let TextScroll::Wheel(notches) = by {
            let max = self.layout(cx.size, cx.now).scroll_max;
            if max > 0.0 {
                self.scroll = (self.scroll - notches * 48.0).clamp(0.0, max);
                cx.out.invalidate = true;
            }
        }
        Reply::Block
    }

    fn leave(&mut self) {
        self.pointer.hover = None;
    }

    fn view<'a>(
        &'a self,
        cx: &Ctx,
        frame: Frame<'a>,
        _button: &'a dyn Fn(ButtonId) -> (f32, f32),
    ) -> Option<SheetView<'a>> {
        let f = frame.sheet?;
        let rise = anim::dialog_in(anim::elapsed_ms(self.opened, cx.now), cx.animations);
        Some(SheetView {
            layout: &f.layout,
            title: &f.title,
            sub: &f.sub,
            notes: &self.notes,
            result: &f.result,
            error: f.error,
            lines: &f.lines,
            selected: self.selected(),
            radios: &f.radios,
            list: f.list.clone(),
            more_open: self.more_open,
            hover: self.pointer.hover.and_then(index),
            pressed: self.pointer.pressed.and_then(index),
            focus: self.focus.filter(|_| self.keys),
            convert_enabled: self.can_convert(),
            rise,
            fade: self.closing.map_or(1.0, |t| {
                1.0 - anim::eased(t, theme::fluent::motion::QUICK_MS, cx.now, cx.animations)
            }),
        })
    }

    fn a11y<'a>(&'a self, _cx: &Ctx, frame: Frame<'a>) -> Option<a11y::SheetIn<'a>> {
        if !self.is_open() {
            return None;
        }
        let f = frame.sheet?;
        Some(a11y::SheetIn {
            layout: &f.layout,
            title: &f.title,
            sub: &f.sub,
            notes: &self.notes,
            result: &f.spoken,
            error: f.error,
            lines: &f.lines,
            selected: self.selected(),
            radios: &f.radios,
            list: f.list.clone(),
            more_open: self.more_open,
            focus: self.focus,
            convert_enabled: self.can_convert(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiln_core::Preset;

    #[test]
    fn the_slow_info_hold_is_whole_milliseconds_up_to_a_minute() {
        use std::ffi::OsStr;
        use std::time::Duration;
        let p = |s: &str| parse_slow_info(Some(OsStr::new(s)));
        assert_eq!(p("1500"), Some(Duration::from_millis(1500)));
        assert_eq!(p(" 60000 "), Some(Duration::from_secs(60)));
        assert_eq!(p("0"), None);
        assert_eq!(p("60001"), None);
        assert_eq!(p("-5"), None);
        assert_eq!(p("1.5"), None);
        assert_eq!(p("soon"), None);
        assert_eq!(parse_slow_info(None), None);
    }

    struct Nothing;
    impl Measure for Nothing {
        fn mono_height(&self, _text: &str, _width: f32) -> f32 {
            0.0
        }
        fn body_height(&self, _text: &str, _width: f32) -> f32 {
            0.0
        }
    }

    const SIZE: (f32, f32) = (900.0, 700.0);

    fn with<R>(f: impl FnOnce(&mut Ctx) -> R) -> (R, Out) {
        with_size(SIZE, f)
    }

    fn with_size<R>(size: (f32, f32), f: impl FnOnce(&mut Ctx) -> R) -> (R, Out) {
        let settings = Settings::default();
        let mut cx = Ctx::new(&Nothing, &settings, size);
        let r = f(&mut cx);
        (r, cx.out)
    }

    /// A sheet converting `from` to `to`, as the formats the registry knows.
    fn sheet(from: &str, to: &str, keyboard: bool) -> Sheet {
        sheet_in(from, to, keyboard, SIZE)
    }

    fn sheet_in(from: &str, to: &str, keyboard: bool, size: (f32, f32)) -> Sheet {
        let kiln = Kiln::new();
        let reg = kiln.registry();
        let from = reg.format(from).unwrap().clone();
        let to = reg.format(to).unwrap().clone();
        let knobs = kiln_core::quality::knobs(&from, &to);
        let quality = Settings::default().quality.quality_for(&from, &to);
        let paths = vec![PathBuf::from(format!("clip.{}", from.ext))];
        Sheet::new(
            Opening {
                id: 1,
                paths,
                from,
                to,
                knobs,
                quality,
                notes: vec![],
                text_only: false,
                bytes: 0,
                facts: None,
            },
            size,
            keyboard,
            Instant::now(),
        )
    }

    fn layout_of(s: &Sheet) -> SheetLayout {
        s.layout(SIZE, Instant::now())
    }

    fn center(r: layout::Rect) -> (f32, f32) {
        (r.x + r.w / 2.0, r.y + r.h / 2.0)
    }

    fn rect_of(s: &Sheet, pick: impl Fn(&SheetControl) -> bool) -> layout::Rect {
        layout_of(s)
            .controls
            .iter()
            .find(|(c, _)| pick(c))
            .expect("the control is on the sheet")
            .1
    }

    fn press_at(s: &mut Sheet, r: layout::Rect) -> Out {
        let (x, y) = center(r);
        with(|cx| s.press(cx, x, y)).1
    }

    fn release_at(s: &mut Sheet, r: layout::Rect) -> Out {
        let (x, y) = center(r);
        with(|cx| s.release(cx, x, y)).1
    }

    fn click(s: &mut Sheet, r: layout::Rect) -> Out {
        press_at(s, r);
        release_at(s, r)
    }

    fn key(s: &mut Sheet, key: VIRTUAL_KEY) -> Out {
        with(|cx| s.key(cx, key, Mods::default())).1
    }

    fn focused(s: &Sheet) -> Option<SheetControl> {
        s.focus_control(SIZE, Instant::now())
    }

    #[test]
    fn it_opens_as_a_dialog_with_quality_radios_and_focus_on_convert() {
        let s = sheet("png", "jpg", false);
        assert!(s.is_open());
        assert!(s.tune.is_none());
        assert!(s.infos.is_none());
        assert!(!s.wants_cards());
        assert_eq!(s.heads().0, "Convert to JPG");
        let l = layout_of(&s);
        // Small, High and Best as radio buttons, High on as Settings has it.
        let radios: Vec<SheetControl> = l
            .controls
            .iter()
            .map(|(c, _)| *c)
            .filter(|c| c.is_radio())
            .collect();
        assert_eq!(
            radios,
            Preset::ALL.map(|p| SheetControl::Preset(0, p)).to_vec()
        );
        assert_eq!(s.selected(), Some(1));
        // Convert, the accent, then Cancel; the keyboard starts on Convert.
        let n = l.controls.len();
        assert_eq!(l.controls[n - 2].0, SheetControl::Convert);
        assert_eq!(l.controls[n - 1].0, SheetControl::Cancel);
        assert_eq!(focused(&s), Some(SheetControl::Convert));
        // Size is under More options, closed.
        assert_eq!(s.more(), [SheetControl::Step(1)]);
        assert!(l.lines.is_empty());
        assert!(l.controls.iter().any(|(c, _)| *c == SheetControl::Advanced));
    }

    #[test]
    fn a_pair_with_no_size_to_say_shows_no_card_while_its_file_is_read() {
        // A photo has no estimate, so there is nothing to wait for.
        let s = sheet("jpg", "webp", false);
        assert!(s.infos.is_none());
        assert_eq!(s.result_line().0, "");
        assert!(!s.shape().card);
        // A video's size comes once its length is read: the card says so.
        let s = sheet("mp4", "wmv", false);
        assert!(s.infos.is_none());
        assert_eq!(s.result_line().0, "Reading the file…");
    }

    #[test]
    fn a_radio_pressed_and_released_changes_the_quality() {
        let mut s = sheet("png", "jpg", false);
        let small = SheetControl::Preset(0, Preset::Small);
        let r = rect_of(&s, |c| *c == small);
        let down = press_at(&mut s, r);
        assert!(down.effects.iter().any(|e| matches!(e, Effect::Capture)));
        // Nothing changes until the release.
        assert_eq!(s.quality.preset, Preset::High);
        let up = release_at(&mut s, r);
        assert_eq!(s.quality.preset, Preset::Small);
        assert!(up.invalidate);
        assert_eq!(focused(&s), Some(small), "the radio takes the keyboard");
        // Letting go off the control takes the press back.
        let best = rect_of(&s, |c| *c == SheetControl::Preset(0, Preset::Best));
        press_at(&mut s, best);
        let away = rect_of(&s, |c| *c == SheetControl::Cancel);
        release_at(&mut s, away);
        assert_eq!(s.quality.preset, Preset::Small);
        assert!(s.is_open());
    }

    #[test]
    fn the_arrows_pick_a_radio_and_tab_leaves_the_group() {
        let mut s = sheet("png", "jpg", true);
        // From Convert, Shift+Tab twice: More options, then the radio on.
        with(|cx| s.key(cx, VK_TAB, Mods::held(true, false)));
        assert_eq!(focused(&s), Some(SheetControl::Advanced));
        with(|cx| s.key(cx, VK_TAB, Mods::held(true, false)));
        assert_eq!(focused(&s), Some(SheetControl::Preset(0, Preset::High)));
        key(&mut s, VK_DOWN);
        assert_eq!(s.quality.preset, Preset::Best);
        assert_eq!(focused(&s), Some(SheetControl::Preset(0, Preset::Best)));
        // The ends hold.
        key(&mut s, VK_RIGHT);
        assert_eq!(s.quality.preset, Preset::Best);
        key(&mut s, VK_UP);
        key(&mut s, VK_LEFT);
        assert_eq!(s.quality.preset, Preset::Small);
        // Tab leaves the group: More options, Convert, Cancel, and round
        // to the radio that is on.
        key(&mut s, VK_TAB);
        assert_eq!(focused(&s), Some(SheetControl::Advanced));
        key(&mut s, VK_TAB);
        assert_eq!(focused(&s), Some(SheetControl::Convert));
        key(&mut s, VK_TAB);
        assert_eq!(focused(&s), Some(SheetControl::Cancel));
        key(&mut s, VK_TAB);
        assert_eq!(focused(&s), Some(SheetControl::Preset(0, Preset::Small)));
    }

    #[test]
    fn more_options_opens_its_lines_and_the_arrows_step_them() {
        let mut s = sheet("png", "jpg", false);
        let more = rect_of(&s, |c| *c == SheetControl::Advanced);
        click(&mut s, more);
        assert!(s.more_open);
        let l = layout_of(&s);
        assert_eq!(l.lines.len(), 1);
        let (c, label, control) = l.lines[0];
        assert_eq!(c, SheetControl::Step(1));
        assert!(label.right() <= control.x);
        let knob = s.knobs[1];
        let was = s.shown(knob);
        s.focus = s.index_of(SIZE, c);
        key(&mut s, VK_RIGHT);
        assert_ne!(s.shown(knob), was);
        key(&mut s, VK_LEFT);
        assert_eq!(s.shown(knob), was);
        // Enter on More options closes it, and converts nothing.
        s.focus = s.index_of(SIZE, SheetControl::Advanced);
        let out = key(&mut s, VK_RETURN);
        assert!(out.start.is_none() && s.is_open() && !s.more_open);
    }

    #[test]
    fn the_sound_quality_is_radio_buttons_and_its_fine_steps_are_under_more_options() {
        // Small, High, Best as radios; every step a drop-down named
        // Quality under More options.
        let mut s = sheet("wav", "mp3", true);
        assert!(s.tune.is_some(), "mp3 has a quality");
        let l = layout_of(&s);
        assert_eq!(l.radios.len(), 3);
        let order: Vec<SheetControl> = l.controls.iter().map(|(c, _)| *c).collect();
        assert_eq!(
            order[..3],
            [
                SheetControl::Tune(0),
                SheetControl::Tune(1),
                SheetControl::Tune(2)
            ]
        );
        // It opens on High, which is on.
        assert_eq!(s.selected(), Some(1));
        let t = s.tune.as_ref().unwrap();
        let (marks, steps) = (t.sheet.marks(), t.sheet.steps());
        assert_eq!(t.sheet.at(), marks[1], "it opens on High");
        // A click on Best puts the quality on Best's step and turns it on.
        let best = rect_of(&s, |c| *c == SheetControl::Tune(2));
        click(&mut s, best);
        assert_eq!(s.tune.as_ref().unwrap().sheet.at(), marks[2]);
        assert_eq!(s.selected(), Some(2));
        assert_eq!(focused(&s), Some(SheetControl::Tune(2)));
        // The arrows pick the radio before or after, as RadioButtons' do.
        key(&mut s, VK_LEFT);
        assert_eq!(s.tune.as_ref().unwrap().sheet.at(), marks[1]);
        assert_eq!(s.selected(), Some(1));
        key(&mut s, VK_LEFT);
        assert_eq!(s.selected(), Some(0));
        key(&mut s, VK_LEFT);
        assert_eq!(s.selected(), Some(0), "the end holds");
        // Fine steps are a drop-down under More options, named Quality,
        // listing every step; picking one between the marks turns no radio
        // on.
        let fields: Vec<SheetControl> = s.more();
        assert_eq!(fields[0], SheetControl::Field(TuneField::Steps));
        assert!(fields.iter().all(|c| matches!(c, SheetControl::Field(_))));
        let (names, now) = s.choices(fields[0]).expect("a drop-down");
        assert_eq!(names.len(), steps);
        assert_eq!(now, marks[0]);
        let between = (0..steps).find(|i| !marks.contains(i)).unwrap();
        s.pick(fields[0], between);
        assert_eq!(s.tune.as_ref().unwrap().sheet.at(), between);
        assert_eq!(s.selected(), None);
        // The names the steps carry ("q5", "V2") are only there.
        let f = s.frame(SIZE, Instant::now());
        assert_eq!(f.radios, ["Small", "High", "Best"]);
        // Its other fields are More options' lines too; there is no
        // Encoder, Bitrate mode or constant Bitrate among them (spec 2.5).
        // The card says the size alone, and only once there is one to say: a length read, "About 1.1 MB".
        assert_eq!(s.result_line().0, "", "no length, no estimate, no card");
        s.tune.as_mut().unwrap().info.duration = Some(180.0);
        let (line, spoken, error) = s.result_line();
        assert!(
            line.starts_with("About ") && line.ends_with(" MB"),
            "{line}"
        );
        assert!(
            spoken.starts_with("About ") && spoken.ends_with(" megabytes"),
            "{spoken}"
        );
        assert!(!error);
        // Tab: the radios are one stop.
        s.focus = s.index_of(SIZE, SheetControl::Tune(1));
        key(&mut s, VK_TAB);
        assert!(!focused(&s).is_some_and(|c| c.is_radio()));
    }

    #[test]
    fn a_copyable_video_opens_on_original_no_re_encode_and_has_four_radios() {
        // the video's radios are Small, High, Best and Original (no
        // re-encode); a video whose picture can be copied opens on the last.
        let mut s = sheet("mov", "mp4", false);
        let clip = MediaInfo {
            duration: Some(42.0),
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            video_codec: Some(kiln_media::video::VideoCodec::H264),
            video_kbps: Some(6000),
            ..MediaInfo::default()
        };
        with(|cx| {
            s.probed(
                cx,
                Probed {
                    id: 1,
                    infos: vec![clip],
                },
                |_, _, _| vec![],
            )
        });
        let f = s.frame(SIZE, Instant::now());
        assert_eq!(
            f.radios,
            ["Small", "High", "Best", "Original (no re-encode)"]
        );
        assert_eq!(f.layout.radios.len(), 4);
        assert_eq!(s.selected(), Some(3));
        // Small turns Original off; Original turns it on again.
        let small = rect_of(&s, |c| *c == SheetControl::Tune(0));
        click(&mut s, small);
        assert_eq!(s.selected(), Some(0));
        assert!(s.tune.as_ref().unwrap().touched);
        let original = rect_of(&s, |c| *c == SheetControl::Tune(3));
        click(&mut s, original);
        assert_eq!(s.selected(), Some(3));
        let t = s.tune.as_ref().unwrap();
        assert_eq!(t.sheet.at(), t.sheet.steps() - 1);
        // A step one under Original, picked under More options, is not a
        // radio.
        let steps = SheetControl::Field(TuneField::Steps);
        assert!(s.more().contains(&steps));
        let n = s.tune.as_ref().unwrap().sheet.steps();
        s.pick(steps, n - 2);
        assert_eq!(s.selected(), None);
    }

    #[test]
    fn a_steps_own_words_show_only_in_the_quality_drop_down() {
        // "q5", "V2" and "kbps" are for the people who open More
        // options; the radios, the estimate and what is read out have none.
        let word = |s: &str| {
            let low = s.to_ascii_lowercase();
            low.contains("kbps")
                || low.contains("mbps")
                || low.contains("megabits")
                || low.contains("kilobits")
                || s.split(|c: char| !c.is_alphanumeric()).any(|w| {
                    let mut c = w.chars();
                    matches!(c.next(), Some('q' | 'Q' | 'v' | 'V'))
                        && c.clone().count() > 0
                        && c.all(|d| d.is_ascii_digit())
                })
        };
        for (from, to) in [
            ("wav", "mp3"),
            ("wav", "ogg"),
            ("wav", "m4a"),
            ("wav", "opus"),
        ] {
            let mut s = sheet(from, to, true);
            s.tune.as_mut().unwrap().info.duration = Some(180.0);
            let f = s.frame(SIZE, Instant::now());
            for text in f
                .radios
                .iter()
                .copied()
                .chain([f.result.as_str(), f.spoken.as_str()])
            {
                assert!(!word(text), "{to}: {text}");
            }
            // The drop-down has them: some step is named by its number.
            let (names, _) = s.choices(SheetControl::Field(TuneField::Steps)).unwrap();
            if to == "ogg" {
                assert!(names.iter().any(|n| word(n)), "{names:?}");
            }
        }
    }

    #[test]
    fn two_radios_on_one_step_keep_the_one_last_chosen() {
        // FLAC's High and Best are both level 5.
        let mut s = sheet("wav", "flac", true);
        let t = s.tune.as_ref().unwrap();
        let marks = t.sheet.marks();
        if marks[1] == marks[2] {
            let best = rect_of(&s, |c| *c == SheetControl::Tune(2));
            click(&mut s, best);
            assert_eq!(s.selected(), Some(2));
            let high = rect_of(&s, |c| *c == SheetControl::Tune(1));
            click(&mut s, high);
            assert_eq!(s.selected(), Some(1));
        } else {
            panic!("FLAC's High and Best share a step: {marks:?}");
        }
    }

    #[test]
    fn more_options_lines_are_drop_downs() {
        // a line opens a list of its values; a pick closes it with
        // the new value, Esc closes it as it was and leaves the dialog
        // open.
        let mut s = sheet("png", "jpg", true);
        let more = rect_of(&s, |c| *c == SheetControl::Advanced);
        click(&mut s, more);
        let c = SheetControl::Step(1);
        let knob = s.knobs[1];
        let was = s.shown(knob);
        let f = s.frame(SIZE, Instant::now());
        let line = f.lines[0].clone();
        let names = line.choices.clone().expect("a drop-down");
        assert!(names.len() > 1);
        assert_eq!(
            names.iter().position(|n| *n == line.value),
            s.choices(c).map(|x| x.1)
        );
        // A click opens the list over the line.
        let r = rect_of(&s, |k| *k == c);
        click(&mut s, r);
        assert!(matches!(s.list, Some((k, st)) if k == c && st.open));
        // Esc closes the list only.
        key(&mut s, VK_ESCAPE);
        assert!(s.list.is_none() && s.is_open());
        assert_eq!(s.shown(knob), was);
        // Enter opens it from the keyboard; Down and Enter pick the next.
        s.focus = s.index_of(SIZE, c);
        key(&mut s, VK_RETURN);
        assert!(s.list.is_some());
        let at = s.list.unwrap().1.value;
        key(&mut s, if at + 1 < names.len() { VK_DOWN } else { VK_UP });
        let out = key(&mut s, VK_RETURN);
        assert!(s.list.is_none() && out.start.is_none() && s.is_open());
        assert_ne!(s.shown(knob), was);
        // A click on an item picks it.
        click(&mut s, r);
        let list = s.list_layout(SIZE, Instant::now()).unwrap();
        let first = list.items[0];
        let (x, y) = center(first);
        with(|cx| s.moved(cx, x, y, false));
        with(|cx| s.press(cx, x, y));
        with(|cx| s.release(cx, x, y));
        assert!(s.list.is_none());
        let unit = q::size_unit(&s.to);
        assert_eq!(q::label(knob, s.shown(knob), unit), names[list.first]);
        // A press off the list closes it and does nothing else.
        click(&mut s, r);
        let away = rect_of(&s, |k| *k == SheetControl::Convert);
        let out = press_at(&mut s, away);
        assert!(s.list.is_none() && out.start.is_none() && s.is_open());
        // UI Automation sets a value by its name.
        let target = names.last().unwrap().clone();
        with(|cx| s.uia(cx, c, &a11y::Verb::SetValue(target.clone())));
        assert_eq!(q::label(knob, s.shown(knob), unit), target);
    }

    #[test]
    fn the_dialog_has_one_sub_line_the_source_then_the_target() {
        let s = sheet("png", "jpg", false);
        let (title, sub) = s.heads();
        assert_eq!(title, "Convert to JPG");
        // No description of the format: the title says where it goes.
        assert_eq!(sub, "clip.png · 0 bytes");
        assert!(!sub.contains("Photo that opens anywhere"), "{sub}");
        assert!(!sub.contains("  "), "tight separators: {sub}");
        // A PDF becoming another kind of document keeps its text only.
        let mut pdf = sheet("png", "jpg", false);
        pdf.text_only = true;
        assert_eq!(pdf.heads().1, "clip.png · 0 bytes · Text only");
    }

    #[test]
    fn the_sub_line_is_the_rows_own_facts_line() {
        // name, then the facts as the row says them, one separator.
        let mut s = sheet("mp4", "mov", false);
        s.facts = Some("1920 × 1080 video · 0:40 · 31 MB".to_owned());
        assert_eq!(s.heads().1, "clip.mp4 · 1920 × 1080 video · 0:40 · 31 MB");
    }

    #[test]
    fn escape_cancels_and_starts_nothing() {
        let mut s = sheet("png", "jpg", false);
        let out = key(&mut s, VK_ESCAPE);
        assert!(!s.is_open());
        assert!(out.start.is_none());
        assert!(out.animate);
        // Once closed, a key does nothing.
        let again = key(&mut s, VK_RETURN);
        assert!(again.start.is_none());
    }

    #[test]
    fn enter_confirms_and_asks_for_the_conversion() {
        let mut s = sheet("png", "jpg", true);
        let out = key(&mut s, VK_RETURN);
        assert!(!s.is_open());
        let start = out.start.expect("Convert starts a job");
        assert_eq!(start.action, Action::Convert("jpg".into()));
        assert_eq!(start.paths, [PathBuf::from("clip.png")]);
        assert!(!start.redo);
        assert_eq!(start.via, Via::Window);
        // Options picked on the drag-wheel convert as a drop; a finished
        // row's Options redo that row.
        let mut d = sheet("png", "jpg", true);
        d.set_dropped();
        d.set_redo();
        let start = key(&mut d, VK_RETURN).start.unwrap();
        assert_eq!(start.via, Via::DropSheet);
        assert!(start.redo);
    }

    #[test]
    fn a_held_enter_converts_nothing_until_pressed_again() {
        let mut s = sheet("png", "jpg", true);
        let held = Mods::with_repeat(false, false, true);
        let out = with(|cx| s.key(cx, VK_RETURN, held)).1;
        assert!(out.start.is_none() && s.is_open());
        assert!(key(&mut s, VK_RETURN).start.is_some());
    }

    #[test]
    fn enter_on_cancel_cancels() {
        let mut s = sheet("png", "jpg", false);
        s.focus = s.index_of(SIZE, SheetControl::Cancel);
        let out = key(&mut s, VK_RETURN);
        assert!(!s.is_open());
        assert!(out.start.is_none());
    }

    #[test]
    fn a_click_on_the_smoke_cancels_and_one_on_the_dialog_does_not() {
        let mut s = sheet("png", "jpg", false);
        let panel = layout_of(&s).panel;
        let (x, y) = (panel.x + 1.0, panel.y + 1.0);
        let out = with(|cx| s.press(cx, x, y)).1;
        assert!(s.is_open());
        assert!(out.start.is_none());
        let (reply, out) = with(|cx| s.press(cx, 1.0, 1.0));
        assert_eq!(reply, Reply::Taken);
        assert!(!s.is_open());
        assert!(out.start.is_none());
    }

    #[test]
    fn in_a_small_window_the_content_scrolls_to_what_has_the_focus() {
        // The video dialog with More options open fills more than the
        // smallest window.
        let size = layout::MIN_WINDOW;
        let mut s = sheet_in("mkv", "mp4", true, size);
        with_size(size, |cx| s.advanced(cx, true));
        let l = s.layout(size, Instant::now());
        assert!(l.scroll_max > 0.0, "{l:?}");
        // The last line, by the keyboard, comes into view.
        let last = l.lines.last().unwrap().0;
        s.focus = s.index_of(size, last);
        with_size(size, |cx| s.reveal(cx));
        let l = s.layout(size, Instant::now());
        let r = l.rect_of(last).unwrap();
        assert!(r.bottom() <= l.frame.content.bottom() + 0.5, "{r:?}");
        // The mouse wheel scrolls it back, and nothing under it.
        let (reply, _) = with_size(size, |cx| s.scroll(cx, TextScroll::Wheel(20.0)));
        assert_eq!(reply, Reply::Block);
        assert_eq!(s.layout(size, Instant::now()).scroll, 0.0);
        // A control scrolled out of the content can't be pressed there.
        let l = s.layout(size, Instant::now());
        let (_, hidden) = *l.controls.iter().rev().nth(2).unwrap();
        let (x, y) = center(hidden);
        if !l.frame.content.contains(x, y) {
            assert_eq!(with_size(size, |cx| s.hit(cx, x, y)).0, None);
        }
    }

    #[test]
    fn answers_for_another_sheet_are_ignored() {
        let mut s = sheet("png", "jpg", false);
        let (_, out) = with(|cx| {
            s.probed(
                cx,
                Probed {
                    id: 2,
                    infos: vec![MediaInfo::default()],
                },
                |_, _, _| panic!("not this sheet's"),
            )
        });
        assert!(s.infos.is_none());
        assert!(!out.invalidate);
        let (_, out) = with(|cx| {
            s.probed(
                cx,
                Probed {
                    id: 1,
                    infos: vec![MediaInfo::default()],
                },
                |_, _, _| vec!["Leaves out a track.".to_owned()],
            )
        });
        assert_eq!(s.infos.as_ref().map(Vec::len), Some(1));
        assert_eq!(s.notes, ["Leaves out a track."]);
        assert!(out.invalidate);
    }

    #[test]
    fn a_probe_that_panics_leaves_the_sheet_with_no_facts() {
        let paths: Vec<PathBuf> = ["a.mkv", "b.mkv", "c.mkv"]
            .iter()
            .map(PathBuf::from)
            .collect();
        let read = |p: &Path| {
            Some(MediaInfo {
                sound_tracks: if p.ends_with("a.mkv") { 2 } else { 1 },
                ..MediaInfo::default()
            })
        };
        // Read, a file at a time, in order; one that can't be read is a
        // file with nothing known.
        let infos = read_infos(&paths, read);
        assert_eq!(infos.len(), 3);
        assert_eq!(infos[0].sound_tracks, 2);
        let unreadable = read_infos(&paths, |p: &Path| read(p).filter(|_| !p.ends_with("b.mkv")));
        assert_eq!(unreadable.len(), 3);
        assert_eq!(unreadable[1], MediaInfo::default());
        // Only so many are read: the sheet then has no estimates to give
        // (`on_probed` takes fewer infos than files as "not read").
        let many: Vec<PathBuf> = (0..PROBE_LIMIT + 5)
            .map(|i| PathBuf::from(format!("{i}.mkv")))
            .collect();
        assert_eq!(read_infos(&many, read).len(), PROBE_LIMIT);
        // A look that panics ends with no facts, which `on_probed` shows as
        // the sheet's no-facts state, and not as a sheet that goes on
        // reading its files for good.
        let none = read_infos(&paths, |p: &Path| {
            if p.ends_with("b.mkv") {
                panic!("a file the reader chokes on");
            }
            read(p)
        });
        assert!(none.is_empty());
        assert_ne!(none.len(), paths.len());
    }
}
