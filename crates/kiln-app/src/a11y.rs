//! What Kiln shows, as a tree of named roles for screen readers, voice
//! control and other assistive technology. Pure: [`tree`] builds it from
//! the model and the layout, so it can be tested without a window.
//! `win::uia` serves it through UI Automation.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kiln_core::Preset;

use crate::help::HelpSection;
use crate::layout::{
    self, Control, DialogButton, Rect, SettingsLayout, SettingsRow, SettingsTab, SheetControl,
    SheetLayout,
};
use crate::model::{Entry, FocusButton, JobState};
use crate::quality::{QualityDefaults, Value};
use crate::settings::{KeyMap, ThemeChoice};
use crate::ways_in::EmptyLines;
use crate::wheel::{self, Action, Item, Spot, Wheel, R_INNER, R_MORE_INNER, R_MORE_OUTER, R_OUTER};

/// What an element is, as UI Automation's control types name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The window itself, the root.
    Window,
    Button,
    Hyperlink,
    Text,
    Group,
    List,
    ListItem,
    Menu,
    MenuItem,
    Tab,
    TabItem,
    Spinner,
    RadioButton,
    /// A panel over a scrim that keeps the keys until it closes.
    Dialog,
}

/// Which element a node is. It stays the same while the element does, so
/// a screen reader keeps its place across repaints: a row by its file, a
/// control by what it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Id {
    Root,
    HelpButton,
    SettingsButton,
    /// The empty window's drop panel, and its lines of text.
    DropPanel,
    DropLine(u8),
    AddFiles,
    List,
    Row(PathBuf),
    RowCancel(PathBuf),
    /// "Show details", "Export", or an exported row's "Saved to" line.
    RowLink(PathBuf),
    Status,
    ExportAll,
    Clear,
    Wheel,
    WheelItem(Spot),
    WheelOptions,
    Settings,
    SettingsTabs,
    Setting(Control),
    /// A settings row, by its label, and the value it shows.
    SettingsRow(&'static str),
    SettingsValue(&'static str),
    UpdateNote,
    Details,
    DetailsText,
    DetailsButton(DialogButton),
    Help,
    HelpSection(usize),
    HelpLine(usize, usize),
    HelpClose,
    Sheet,
    SheetLine(u8),
    SheetRow(usize),
    SheetControl(SheetControl),
}

impl Id {
    /// A number for this id that is the same on every run: FNV-1a of how
    /// it prints. UI Automation's runtime id is built from it.
    pub fn key(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in format!("{self:?}").bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        h
    }
}

/// What an element is doing now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct States {
    pub selected: bool,
    /// Has the keyboard focus (within the window).
    pub focused: bool,
    pub disabled: bool,
    /// Can take the keyboard focus.
    pub focusable: bool,
    /// Scrolled out of the list's view.
    pub offscreen: bool,
}

/// The control patterns an element supports: what it lets a screen reader
/// or voice control do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Patterns {
    pub invoke: bool,
    pub selection: bool,
    pub selection_item: bool,
    pub value: bool,
    pub scroll_item: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: Id,
    /// `id.key()`, worked out once.
    pub key: u64,
    pub role: Role,
    pub name: String,
    /// Where it is, in the window's layout units.
    pub rect: Rect,
    pub states: States,
    /// A spinner's value as it reads: "1080p".
    pub value: Option<String>,
    /// More about it: a row's folder, a setting's hint.
    pub help: Option<String>,
    /// How a running row is getting on: "45%".
    pub status: Option<String>,
    /// Its text changes as things happen, and is announced.
    pub live: bool,
    /// The key that does it, said apart from its name: "Tab".
    pub shortcut: Option<&'static str>,
    pub children: Vec<Node>,
}

impl Node {
    pub fn new(id: Id, role: Role, name: impl Into<String>, rect: Rect) -> Self {
        Node {
            key: id.key(),
            id,
            role,
            name: name.into(),
            rect,
            states: States::default(),
            value: None,
            help: None,
            status: None,
            live: false,
            shortcut: None,
            children: vec![],
        }
    }

    fn focusable(mut self) -> Self {
        self.states.focusable = true;
        self
    }

    fn selected(mut self, on: bool) -> Self {
        self.states.selected = on;
        self
    }

    fn help(mut self, help: Option<&str>) -> Self {
        self.help = help.filter(|h| !h.is_empty()).map(str::to_owned);
        self
    }

    fn with(mut self, children: Vec<Node>) -> Self {
        self.children = children;
        self
    }

    /// The node with `id`, here or below.
    pub fn find(&self, id: &Id) -> Option<&Node> {
        if self.id == *id {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(id))
    }

    pub fn find_mut(&mut self, id: &Id) -> Option<&mut Node> {
        if self.id == *id {
            return Some(self);
        }
        self.children.iter_mut().find_map(|c| c.find_mut(id))
    }

    /// The node with the keyboard focus, if it's not the window itself.
    pub fn focused(&self) -> Option<&Node> {
        if self.states.focused {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.focused())
    }

    /// Every node, parents before children.
    pub fn walk<'a>(&'a self, f: &mut impl FnMut(&'a Node)) {
        f(self);
        for c in &self.children {
            c.walk(f);
        }
    }

    /// Each node's place: the child indexes that lead to it from here.
    pub fn paths(&self) -> Vec<(u64, Vec<usize>)> {
        fn go(n: &Node, path: &mut Vec<usize>, out: &mut Vec<(u64, Vec<usize>)>) {
            out.push((n.key, path.clone()));
            for (i, c) in n.children.iter().enumerate() {
                path.push(i);
                go(c, path, out);
                path.pop();
            }
        }
        let mut out = vec![];
        go(self, &mut vec![], &mut out);
        out
    }

    /// The node at `path` below this one.
    pub fn at(&self, path: &[usize]) -> Option<&Node> {
        path.iter().try_fold(self, |n, &i| n.children.get(i))
    }

    /// Where to go from the node at `path`: its parent, a sibling or a
    /// child. The root's parent and siblings are the window's own
    /// business, so they're `Neighbour::Nothing`.
    pub fn neighbour(&self, path: &[usize], to: Toward) -> Neighbour {
        let Some(n) = self.at(path) else {
            return Neighbour::Nothing;
        };
        let key = |n: Option<&Node>| n.map_or(Neighbour::Nothing, |n| Neighbour::Node(n.key));
        match (to, path.split_last()) {
            (Toward::FirstChild, _) => key(n.children.first()),
            (Toward::LastChild, _) => key(n.children.last()),
            (_, None) => Neighbour::Nothing,
            (Toward::Parent, Some((_, []))) => Neighbour::Root,
            (Toward::Parent, Some((_, parent))) => key(self.at(parent)),
            (Toward::Next | Toward::Previous, Some((&i, parent))) => {
                let j = if to == Toward::Next {
                    i.checked_add(1)
                } else {
                    i.checked_sub(1)
                };
                key(j.and_then(|j| self.at(parent)?.children.get(j)))
            }
        }
    }

    /// The deepest node on screen under a point, in layout units: later
    /// siblings are drawn over earlier ones, so they're tried first.
    pub fn hit(&self, x: f32, y: f32) -> &Node {
        for c in self.children.iter().rev() {
            if !c.states.offscreen && c.rect.contains(x, y) {
                return c.hit(x, y);
            }
        }
        self
    }

    pub fn patterns(&self) -> Patterns {
        let radios = self.children.iter().any(|c| c.role == Role::RadioButton);
        Patterns {
            invoke: matches!(self.role, Role::Button | Role::Hyperlink | Role::MenuItem),
            selection: matches!(self.role, Role::List | Role::Tab)
                || (self.role == Role::Group && radios),
            selection_item: matches!(
                self.role,
                Role::ListItem | Role::TabItem | Role::RadioButton
            ),
            value: self.role == Role::Spinner,
            scroll_item: self.role == Role::ListItem,
        }
    }
}

/// A direction to move through the tree, as UI Automation navigates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toward {
    Parent,
    Next,
    Previous,
    FirstChild,
    LastChild,
}

/// Where a move through the tree lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Neighbour {
    /// The window itself.
    Root,
    Node(u64),
    Nothing,
}

/// Whether the window should do `req` now: its element is still there
/// and, for anything but moving the focus or scrolling, can be used. A
/// request made against an older tree is dropped rather than acting on
/// something the person can no longer see.
pub fn accepts(tree: &Node, req: &Request) -> bool {
    match tree.find(&req.id) {
        None => false,
        Some(_) if matches!(req.verb, Verb::Focus | Verb::ScrollIntoView) => true,
        Some(n) => !n.states.disabled,
    }
}

/// How often a running row's progress is told again, at most.
pub const PROGRESS_EVERY: Duration = Duration::from_millis(250);

/// Whole percent, 0 to 100.
pub fn percent(fraction: f32) -> u32 {
    (fraction.clamp(0.0, 1.0) * 100.0).round() as u32
}

/// Holds each running row's progress to a few changes a second, so a
/// screen reader isn't told every frame's percent.
#[derive(Debug, Default)]
pub struct ProgressGate {
    told: HashMap<PathBuf, Told>,
}

/// A row's progress as told, and the newest percent, which may be held.
#[derive(Debug, Clone, Copy)]
struct Told {
    shown: u32,
    at: Instant,
    newest: u32,
}

impl ProgressGate {
    /// The percent to show for the row at `path`: the new one once
    /// `PROGRESS_EVERY` has passed since the last change, the last one
    /// before that.
    pub fn shown(&mut self, path: &Path, fraction: Option<f32>, now: Instant) -> Option<u32> {
        let Some(f) = fraction else {
            self.told.remove(path);
            return None;
        };
        let pct = percent(f);
        match self.told.get_mut(path) {
            Some(t) if t.shown == pct => {
                t.newest = pct;
                Some(pct)
            }
            Some(t) if now.saturating_duration_since(t.at) < PROGRESS_EVERY => {
                t.newest = pct;
                Some(t.shown)
            }
            _ => {
                let told = Told {
                    shown: pct,
                    at: now,
                    newest: pct,
                };
                self.told.insert(path.to_owned(), told);
                Some(pct)
            }
        }
    }

    /// When the earliest held percent may be told, if any is held. The
    /// window builds the tree again then, so a held percent is always
    /// said, even if nothing else happens.
    pub fn due(&self) -> Option<Instant> {
        self.told
            .values()
            .filter(|t| t.newest != t.shown)
            .map(|t| t.at + PROGRESS_EVERY)
            .min()
    }

    /// Forget rows that aren't running any more.
    pub fn keep(&mut self, running: impl Fn(&Path) -> bool) {
        self.told.retain(|p, _| running(p));
    }
}

/// Added to a timer's wait: a timer fires on the system tick and can come
/// in a little early.
pub const TIMER_SLACK: Duration = Duration::from_millis(2);

/// Milliseconds for a timer that should fire at `due` or just after:
/// rounded up, with `TIMER_SLACK`, and at least 1.
pub fn timer_ms(now: Instant, due: Instant) -> u32 {
    let wait = due.saturating_duration_since(now) + TIMER_SLACK;
    let ms = wait.as_nanos().div_ceil(1_000_000);
    ms.clamp(1, u128::from(u32::MAX)) as u32
}

/// Decides which status lines to announce. Each set of the status is
/// said on its own, at once, never merged with the next: a new line, or
/// an alert even if its words are the same as the last.
#[derive(Debug, Default)]
pub struct StatusTeller {
    last: String,
}

impl StatusTeller {
    /// The lines to announce from `sets`, in order, each with whether it
    /// is an alert. With `announce` false, as before any client has
    /// connected, the sets are only noted.
    pub fn tell(
        &mut self,
        sets: impl IntoIterator<Item = (String, bool)>,
        announce: bool,
    ) -> Vec<(String, bool)> {
        let mut said = Vec::new();
        for (text, alert) in sets {
            if announce && !text.is_empty() && (text != self.last || alert) {
                said.push((text.clone(), alert));
            }
            self.last = text;
        }
        said
    }
}

/// What a screen reader or voice control asked an element to do. The
/// window does it as the click or key that does the same would.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verb {
    Invoke,
    Select,
    AddToSelection,
    RemoveFromSelection,
    SetValue(String),
    Focus,
    ScrollIntoView,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub id: Id,
    pub verb: Verb,
}

/// A listed file as the tree needs it.
pub struct RowIn<'a> {
    pub entry: &'a Entry,
    pub selected: bool,
    /// A line that stands in for the row's own: "Saving…", or a note
    /// such as an export that failed.
    pub instead: Option<&'a str>,
    /// The link the row shows: "Show details" or "Export".
    pub link: Option<&'static str>,
    /// How far a running row has got, in whole percent, as last told.
    pub progress: Option<u32>,
}

/// The open wheel.
pub struct WheelIn<'a> {
    pub wheel: &'a Wheel,
    pub paths: &'a [PathBuf],
    pub hover: Option<Spot>,
    pub more_open: bool,
    /// Whether its centre offers "Options… (Tab)" for the lit format.
    pub options: bool,
}

/// The settings panel and the values its controls show.
pub struct SettingsIn<'a> {
    pub layout: &'a SettingsLayout,
    pub focus: Option<usize>,
    pub quality: &'a QualityDefaults,
    pub theme: ThemeChoice,
    pub suggestions: bool,
    pub has_habits: bool,
    pub keys: KeyMap,
    /// The Start exports in line.
    pub output: &'a str,
    /// Each helper app's line.
    pub engines: [&'a str; 4],
    pub update_button: &'a str,
    pub update_note: &'a str,
}

/// The details dialog.
pub struct DetailsIn<'a> {
    pub title: &'a str,
    pub text: &'a str,
    /// "Copy", or "Copied" for a moment.
    pub copy: &'a str,
    pub focus: DialogButton,
}

/// The options sheet.
pub struct SheetIn<'a> {
    pub layout: &'a SheetLayout,
    pub title: &'a str,
    pub target: &'a str,
    pub context: &'a str,
    pub result: &'a str,
    /// Each knob's name and its value as it reads.
    pub rows: &'a [(String, String)],
    pub preset: Preset,
    pub focus: Option<usize>,
}

/// Everything the tree is built from.
pub struct Input<'a> {
    /// The window's client area, in layout units.
    pub size: (f32, f32),
    pub rows: Vec<RowIn<'a>>,
    pub scroll: f32,
    /// The row the keyboard is on.
    pub focus_row: Option<usize>,
    /// The header or footer button Tab moved the keyboard to, which has
    /// the focus instead of the list.
    pub focus_button: Option<FocusButton>,
    /// What the footer says.
    pub status: &'a str,
    pub empty: &'a EmptyLines,
    /// What the empty window says under its Add files button.
    pub empty_status: &'a str,
    /// "Export all (3)", while results are held.
    pub export_all: Option<&'a str>,
    pub wheel: Option<WheelIn<'a>>,
    pub settings: Option<SettingsIn<'a>>,
    pub details: Option<DetailsIn<'a>>,
    pub help: Option<&'a [HelpSection]>,
    pub sheet: Option<SheetIn<'a>>,
}

/// The list's name.
pub const LIST_NAME: &str = "Files";
/// The wheel centre's "Options… (Tab)", with the key said apart.
pub const OPTIONS_NAME: &str = "Options…";
/// The settings tabs' name.
pub const TABS_NAME: &str = "Settings pages";

/// A row's name: the file's name, then its second line. A failed row
/// gives its whole sentence, never the line cut short to fit.
pub fn row_name(e: &Entry, instead: Option<&str>) -> String {
    let line = match (instead, &e.job) {
        (Some(s), _) => s.to_owned(),
        (None, JobState::Failed { message, .. }) => {
            message.split_whitespace().collect::<Vec<_>>().join(" ")
        }
        (None, _) => e.line().0,
    };
    // The row's dots read as pauses.
    format!("{}, {}", e.name, line.replace("  ·  ", ", "))
}

/// The element for a header or footer button.
pub fn button_id(b: FocusButton) -> Id {
    match b {
        FocusButton::AddFiles => Id::AddFiles,
        FocusButton::ExportAll => Id::ExportAll,
        FocusButton::Clear => Id::Clear,
        FocusButton::Help => Id::HelpButton,
        FocusButton::Settings => Id::SettingsButton,
    }
}

/// The header or footer button an element is, if it's one.
pub fn button_of(id: &Id) -> Option<FocusButton> {
    match id {
        Id::AddFiles => Some(FocusButton::AddFiles),
        Id::ExportAll => Some(FocusButton::ExportAll),
        Id::Clear => Some(FocusButton::Clear),
        Id::HelpButton => Some(FocusButton::Help),
        Id::SettingsButton => Some(FocusButton::Settings),
        _ => None,
    }
}

/// A wheel item's name: a format's plain description, or the tool or
/// action as the wheel labels it.
pub fn item_name(item: &Item) -> String {
    match &item.action {
        Action::Convert(ext) => wheel::target_label(ext),
        _ => item.label.clone(),
    }
}

/// The tree: the window and everything on it a person can read or use.
/// While a panel, dialog or wheel is up, it's all there is, as it's all
/// the keys reach.
pub fn tree(input: &Input) -> Node {
    let (w, h) = input.size;
    let window = Rect::new(0.0, 0.0, w, h);
    let children = if let Some(s) = &input.sheet {
        vec![sheet(s)]
    } else if let Some(s) = &input.settings {
        vec![settings(s)]
    } else if let Some(d) = &input.details {
        vec![details(d, w, h)]
    } else if let Some(sections) = input.help {
        vec![help(sections, w, h)]
    } else if let Some(wh) = &input.wheel {
        vec![menu(wh)]
    } else {
        main(input)
    };
    let mut root = Node::new(Id::Root, Role::Window, "Kiln", window).with(children);
    if let Some(id) = focus(input) {
        if let Some(n) = root.find_mut(&id) {
            n.states.focused = true;
        }
    }
    root
}

/// Where the keyboard is, as the window's keys see it.
fn focus(input: &Input) -> Option<Id> {
    if let Some(s) = &input.sheet {
        return Some(match s.focus.and_then(|i| s.layout.controls.get(i)) {
            Some((c, _)) => Id::SheetControl(*c),
            None => Id::Sheet,
        });
    }
    if let Some(s) = &input.settings {
        return Some(match s.focus.and_then(|i| s.layout.controls.get(i)) {
            Some((c, _)) => Id::Setting(*c),
            None => Id::Settings,
        });
    }
    if let Some(d) = &input.details {
        return Some(Id::DetailsButton(d.focus));
    }
    if input.help.is_some() {
        return Some(Id::HelpClose);
    }
    if let Some(wh) = &input.wheel {
        return Some(wh.hover.map_or(Id::Wheel, Id::WheelItem));
    }
    if let Some(b) = input.focus_button {
        return Some(button_id(b));
    }
    if input.rows.is_empty() {
        return None;
    }
    Some(match input.focus_row.and_then(|i| input.rows.get(i)) {
        Some(r) => Id::Row(r.entry.path.clone()),
        None => Id::List,
    })
}

/// A line of text, left out when it says nothing.
fn text(id: Id, name: &str, rect: Rect) -> Option<Node> {
    (!name.trim().is_empty()).then(|| Node::new(id, Role::Text, name, rect))
}

fn union(rects: impl IntoIterator<Item = Rect>) -> Rect {
    let mut it = rects.into_iter();
    let Some(first) = it.next() else {
        return Rect::default();
    };
    let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.right(), first.bottom());
    for r in it {
        x0 = x0.min(r.x);
        y0 = y0.min(r.y);
        x1 = x1.max(r.right());
        y1 = y1.max(r.bottom());
    }
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

/// The window with nothing over it: the header's buttons, then the drop
/// panel or the list and its footer.
fn main(input: &Input) -> Vec<Node> {
    let (w, h) = input.size;
    let l = layout::layout(w, h, input.rows.len());
    // The header and footer buttons are on the Tab path.
    let mut out = vec![
        Node::new(Id::HelpButton, Role::Button, "Help", l.help_button)
            .focusable()
            .help(Some(crate::help::HELP_TIP)),
        Node::new(
            Id::SettingsButton,
            Role::Button,
            "Settings",
            l.settings_button,
        )
        .focusable()
        .help(Some(crate::help::SETTINGS_TIP)),
    ];
    let lines = input.empty;
    if input.rows.is_empty() {
        let e = layout::empty_state(&l.panel);
        let children = [
            text(Id::DropLine(0), lines.instruction, e.instruction),
            Some(
                Node::new(Id::AddFiles, Role::Button, lines.button, e.button)
                    .focusable()
                    .help(Some(crate::help::ADD_FILES_TIP)),
            ),
            text(Id::DropLine(1), input.empty_status, e.status),
            text(Id::DropLine(2), &lines.keys, e.keys),
            text(Id::DropLine(3), lines.privacy, e.privacy),
            text(Id::DropLine(4), lines.tagline, e.tagline),
        ];
        out.push(
            Node::new(Id::DropPanel, Role::Group, lines.headline, l.panel)
                .with(children.into_iter().flatten().collect()),
        );
        return out;
    }

    let panel = l.panel;
    let rows = input
        .rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut rect = l.rows[i];
            rect.y -= input.scroll;
            row(
                r,
                rect,
                rect.bottom() <= panel.y || rect.y >= panel.bottom(),
            )
        })
        .collect();
    out.push(
        Node::new(Id::List, Role::List, LIST_NAME, panel)
            .focusable()
            .with(rows),
    );
    let (Some(footer), Some(clear)) = (l.footer, l.clear_button) else {
        return out;
    };
    let add = layout::add_files_button(&clear, input.export_all.is_some());
    let mut status = text(
        Id::Status,
        input.status,
        layout::footer_status(&footer, &add),
    );
    if let Some(s) = &mut status {
        s.live = true;
    }
    out.extend(status);
    out.push(
        Node::new(Id::AddFiles, Role::Button, lines.button, add)
            .focusable()
            .help(Some(crate::help::ADD_FILES_TIP)),
    );
    if let Some(label) = input.export_all {
        out.push(
            Node::new(
                Id::ExportAll,
                Role::Button,
                label,
                layout::export_all_button(&clear),
            )
            .focusable()
            .help(Some(crate::help::EXPORT_ALL_TIP)),
        );
    }
    out.push(Node::new(Id::Clear, Role::Button, "Clear", clear).focusable());
    out
}

/// One file's row, with its buttons.
fn row(r: &RowIn, rect: Rect, offscreen: bool) -> Node {
    let e = r.entry;
    let path = &e.path;
    let mut n = Node::new(
        Id::Row(path.clone()),
        Role::ListItem,
        row_name(e, r.instead),
        rect,
    )
    .focusable()
    .selected(r.selected)
    // The row's line ends with the folder's name; here is all of it.
    .help(Some(&e.folder));
    n.states.offscreen = offscreen;
    let mut children = vec![];
    if let JobState::Running { .. } = &e.job {
        n.status = r.progress.map(|p| format!("{p}%"));
        // Named with the file, as a list of buttons reads them out of their
        // rows: "Stop a.png", not "Stop, Stop, Stop".
        children.push(Node::new(
            Id::RowCancel(path.clone()),
            Role::Button,
            format!("{} {}", crate::help::STOP_TIP, e.name),
            layout::row_cancel(&rect),
        ));
    }
    if let Some(link) = r.link {
        children.push(Node::new(
            Id::RowLink(path.clone()),
            Role::Button,
            format!("{link} {}", e.name),
            layout::row_link(&rect),
        ));
    } else if r.instead.is_none() && e.saved().is_some() {
        // "Saved to Downloads." opens that folder.
        children.push(Node::new(
            Id::RowLink(path.clone()),
            Role::Hyperlink,
            e.line().0,
            layout::row_line(&rect),
        ));
    }
    for c in &mut children {
        c.states.offscreen = offscreen;
    }
    n.with(children)
}

/// The wheel, as a menu of what it offers.
fn menu(wh: &WheelIn) -> Node {
    let w = wh.wheel;
    let s = w.scale.max(0.01);
    let (cx, cy) = w.center;
    let around = |(x, y): (f32, f32), side: f32| {
        let side = side * s;
        Rect::new(cx + x * s - side / 2.0, cy + y * s - side / 2.0, side, side)
    };
    let reach = if wh.more_open { w.extent() } else { R_OUTER };
    let what = match wh.paths {
        [one] => one
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        many => format!("{} files", many.len()),
    };
    let name = format!("{} {what}", w.kind.title());
    let n = w.segments();
    let spot_item = |spot: Spot, name: String, rect: Rect| {
        Node::new(Id::WheelItem(spot), Role::MenuItem, name, rect)
            .focusable()
            .selected(wh.hover == Some(spot))
    };
    let mut items: Vec<Node> = w
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            spot_item(
                Spot::Inner(i),
                item_name(item),
                around(wheel::label_point(i, n), R_OUTER - R_INNER),
            )
        })
        .collect();
    if !w.more.is_empty() {
        let i = w.items.len();
        let outer = if wh.more_open {
            let slots = wheel::outer_slots(&w.more);
            w.more
                .iter()
                .enumerate()
                .map(|(j, item)| {
                    let at = slots
                        .iter()
                        .find(|sl| sl.item == Some(j))
                        .map_or((0.0, 0.0), wheel::outer_point);
                    spot_item(
                        Spot::Outer(j),
                        item_name(item),
                        around(at, R_MORE_OUTER - R_MORE_INNER),
                    )
                })
                .collect()
        } else {
            vec![]
        };
        items.push(
            spot_item(
                Spot::Inner(i),
                "More".to_owned(),
                around(wheel::label_point(i, n), R_OUTER - R_INNER),
            )
            .with(outer),
        );
    }
    if wh.options {
        let o = layout::wheel_centre(R_INNER).options;
        let mut options = Node::new(
            Id::WheelOptions,
            Role::MenuItem,
            OPTIONS_NAME,
            Rect::new(cx + o.x * s, cy + o.y * s, o.w * s, o.h * s),
        );
        options.shortcut = Some("Tab");
        items.push(options);
    }
    Node::new(Id::Wheel, Role::Menu, name, around((0.0, 0.0), 2.0 * reach))
        .focusable()
        .with(items)
}

/// The row a settings control sits on: the nearest label to its left on
/// the same line.
fn row_of(rows: &[SettingsRow], r: &Rect) -> Option<usize> {
    rows.iter()
        .enumerate()
        .filter(|(_, row)| {
            let l = row.label_rect;
            l.y < r.bottom() && r.y < l.bottom() && l.right() <= r.x + 0.5
        })
        .max_by(|a, b| a.1.label_rect.right().total_cmp(&b.1.label_rect.right()))
        .map(|(i, _)| i)
}

/// The settings panel: its tabs, the page's rows by their labels, then
/// Check for updates and Close.
fn settings(s: &SettingsIn) -> Node {
    let l = s.layout;
    let control = |c: Control, r: Rect, label: &'static str| -> Node {
        let n = match c {
            Control::Tab(t) => {
                Node::new(Id::Setting(c), Role::TabItem, t.label(), r).selected(t == l.tab)
            }
            Control::OutputChoose | Control::EngineChoose(_) => {
                Node::new(Id::Setting(c), Role::Button, "Choose", r)
            }
            Control::OutputReset | Control::EngineReset(_) => {
                Node::new(Id::Setting(c), Role::Button, "Reset", r)
            }
            Control::EngineGet(_) => Node::new(Id::Setting(c), Role::Hyperlink, "Get it", r),
            Control::Theme(t) => {
                Node::new(Id::Setting(c), Role::RadioButton, t.label(), r).selected(t == s.theme)
            }
            Control::Suggestions(on) => Node::new(
                Id::Setting(c),
                Role::RadioButton,
                if on { "On" } else { "Off" },
                r,
            )
            .selected(on == s.suggestions),
            Control::ForgetPicks => {
                let mut n = Node::new(Id::Setting(c), Role::Button, "Forget picks", r);
                n.states.disabled = !s.has_habits;
                n
            }
            Control::Preset(set, p) => Node::new(Id::Setting(c), Role::RadioButton, p.label(), r)
                .selected(s.quality.get(set) == Value::Preset(p)),
            Control::Step(set) => {
                let mut n = Node::new(Id::Setting(c), Role::Spinner, label, r);
                n.value = Some(set.label(s.quality.get(set)));
                n
            }
            Control::Key(wk) => Node::new(
                Id::Setting(c),
                Role::Button,
                format!("{}, {}", wk.title(), s.keys.key_for(wk).name()),
                r,
            ),
            Control::CheckUpdates => Node::new(Id::Setting(c), Role::Button, s.update_button, r),
            Control::Close => Node::new(Id::Setting(c), Role::Button, "Close", r),
        };
        n.focusable()
    };

    let mut children = vec![];
    let tabs: Vec<Node> = l
        .controls
        .iter()
        .filter(|(c, _)| matches!(c, Control::Tab(_)))
        .map(|&(c, r)| control(c, r, ""))
        .collect();
    let tabs_rect = union(tabs.iter().map(|t| t.rect));
    children.push(Node::new(Id::SettingsTabs, Role::Tab, TABS_NAME, tabs_rect).with(tabs));

    // The page: each row with the controls on its line.
    let mut by_row: Vec<Vec<(Control, Rect)>> = vec![vec![]; l.rows.len()];
    let mut loose = vec![];
    for &(c, r) in &l.controls {
        if matches!(c, Control::Tab(_) | Control::CheckUpdates | Control::Close) {
            continue;
        }
        match row_of(&l.rows, &r) {
            Some(i) => by_row[i].push((c, r)),
            None => loose.push((c, r)),
        }
    }
    for (i, (row, controls)) in l.rows.iter().zip(by_row).enumerate() {
        // A stepper alone on its line is named by its label; it needs no
        // group around it.
        if let ([(c @ Control::Step(_), r)], None) = (&controls[..], row.value) {
            children.push(control(*c, *r, row.label).help(row.hint));
            continue;
        }
        let mut parts = vec![];
        if let Some(v) = row.value {
            let line = match l.tab {
                SettingsTab::Engines => s.engines.get(i).copied().unwrap_or(""),
                _ => s.output,
            };
            parts.extend(text(Id::SettingsValue(row.label), line, v));
        }
        parts.extend(controls.iter().map(|&(c, r)| control(c, r, row.label)));
        let rect = union(std::iter::once(row.label_rect).chain(parts.iter().map(|p| p.rect)));
        children.push(
            Node::new(Id::SettingsRow(row.label), Role::Group, row.label, rect)
                .help(row.hint)
                .with(parts),
        );
    }
    children.extend(loose.into_iter().map(|(c, r)| control(c, r, "")));
    for &(c, r) in &l.controls {
        match c {
            Control::CheckUpdates => {
                children.push(control(c, r, ""));
                children.extend(text(Id::UpdateNote, s.update_note, l.update_note));
            }
            Control::Close => children.push(control(c, r, "")),
            _ => {}
        }
    }
    Node::new(Id::Settings, Role::Dialog, "Settings", l.panel).with(children)
}

/// The details dialog: what a failed job's engine said, Copy and Close.
fn details(d: &DetailsIn, w: f32, h: f32) -> Node {
    let l = layout::dialog(w, h);
    let children = [
        text(Id::DetailsText, d.text, l.body),
        Some(
            Node::new(
                Id::DetailsButton(DialogButton::Copy),
                Role::Button,
                d.copy,
                l.copy,
            )
            .focusable(),
        ),
        Some(
            Node::new(
                Id::DetailsButton(DialogButton::Close),
                Role::Button,
                "Close",
                l.close,
            )
            .focusable(),
        ),
    ];
    Node::new(Id::Details, Role::Dialog, d.title, l.panel)
        .with(children.into_iter().flatten().collect())
}

/// Help: each section with its lines, then Close.
fn help(sections: &[HelpSection], w: f32, h: f32) -> Node {
    let l = layout::help(w, h);
    let mut children: Vec<Node> = sections
        .iter()
        .enumerate()
        .map(|(k, s)| {
            let lines = s
                .lines
                .iter()
                .enumerate()
                .filter_map(|(m, line)| {
                    let said = if line.keys.is_empty() {
                        line.what.clone()
                    } else {
                        format!("{}: {}", line.keys, line.what)
                    };
                    text(Id::HelpLine(k, m), &said, l.body)
                })
                .collect();
            Node::new(Id::HelpSection(k), Role::Group, s.title, l.body).with(lines)
        })
        .collect();
    children.push(Node::new(Id::HelpClose, Role::Button, "Close", l.close).focusable());
    Node::new(Id::Help, Role::Dialog, crate::help::TITLE, l.panel).with(children)
}

/// The options sheet: what it converts, a group or spinner for each
/// knob, the result, then Cancel and Convert.
fn sheet(s: &SheetIn) -> Node {
    let l = s.layout;
    let mut children: Vec<Node> = [
        text(Id::SheetLine(0), s.target, l.target),
        text(Id::SheetLine(1), s.context, l.context),
    ]
    .into_iter()
    .flatten()
    .collect();
    for (row, (name, value)) in s.rows.iter().enumerate() {
        let label = l.labels.get(row).copied().unwrap_or_default();
        let mut radios = vec![];
        for &(c, r) in &l.controls {
            match c {
                SheetControl::Step(k) if k == row => {
                    let mut n = Node::new(Id::SheetControl(c), Role::Spinner, name, r).focusable();
                    n.value = Some(value.clone());
                    children.push(n);
                }
                SheetControl::Preset(k, p) if k == row => radios.push(
                    Node::new(Id::SheetControl(c), Role::RadioButton, p.label(), r)
                        .focusable()
                        .selected(p == s.preset),
                ),
                _ => {}
            }
        }
        if !radios.is_empty() {
            let rect = union(std::iter::once(label).chain(radios.iter().map(|n| n.rect)));
            children.push(Node::new(Id::SheetRow(row), Role::Group, name, rect).with(radios));
        }
    }
    children.extend(text(Id::SheetLine(2), s.result, l.result));
    for &(c, r) in &l.controls {
        let name = match c {
            SheetControl::Cancel => "Cancel",
            SheetControl::Convert => "Convert",
            _ => continue,
        };
        children.push(Node::new(Id::SheetControl(c), Role::Button, name, r).focusable());
    }
    Node::new(Id::Sheet, Role::Dialog, s.title, l.panel).with(children)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::help::help_content;
    use crate::layout::SettingsShape;
    use crate::settings::Settings;
    use crate::ways_in::empty_lines;
    use crate::wheel::{Action, WheelKind};

    fn entry(name: &str, job: JobState) -> Entry {
        Entry {
            path: PathBuf::from(r"C:\photos").join(name),
            name: name.to_owned(),
            folder: r"C:\photos".to_owned(),
            code: "PNG".to_owned(),
            size: 3_100_000,
            targets: vec!["jpg".into(), "webp".into()],
            job,
            facts: None,
            suggestion: None,
            facts_at: None,
        }
    }

    fn rows(entries: &[Entry]) -> Vec<RowIn<'_>> {
        entries
            .iter()
            .map(|e| RowIn {
                entry: e,
                selected: false,
                instead: None,
                link: e.link(false),
                progress: match &e.job {
                    JobState::Running { fraction, .. } => fraction.map(percent),
                    _ => None,
                },
            })
            .collect()
    }

    fn input<'a>(empty: &'a EmptyLines, rows: Vec<RowIn<'a>>) -> Input<'a> {
        Input {
            size: (800.0, 600.0),
            rows,
            scroll: 0.0,
            focus_row: None,
            focus_button: None,
            status: "",
            empty,
            empty_status: "",
            export_all: None,
            wheel: None,
            settings: None,
            details: None,
            help: None,
            sheet: None,
        }
    }

    /// Every node's role and name, one line each, indented by depth.
    fn outline(n: &Node) -> Vec<String> {
        fn go(n: &Node, depth: usize, out: &mut Vec<String>) {
            out.push(format!("{}{:?} {}", "  ".repeat(depth), n.role, n.name));
            for c in &n.children {
                go(c, depth + 1, out);
            }
        }
        let mut out = vec![];
        go(n, 0, &mut out);
        out
    }

    fn named<'a>(n: &'a Node, role: Role, name: &str) -> &'a Node {
        let mut found = None;
        n.walk(&mut |c| {
            if found.is_none() && c.role == role && c.name == name {
                found = Some(c);
            }
        });
        found.unwrap_or_else(|| panic!("no {role:?} named {name:?} in {:#?}", outline(n)))
    }

    /// Every element has a name, and every key is its own.
    fn sound(n: &Node) {
        let mut keys = std::collections::HashSet::new();
        n.walk(&mut |c| {
            assert!(!c.name.trim().is_empty(), "{:?} has no name", c.id);
            assert!(keys.insert(c.key), "{:?} shares its key", c.id);
            assert_eq!(c.key, c.id.key());
        });
    }

    #[test]
    fn tab_reaches_the_header_and_footer_buttons() {
        let lines = empty_lines(&Settings::default());
        let es = [entry(
            "a.png",
            JobState::Done {
                action: Action::Convert("jpg".into()),
                outputs: vec![PathBuf::from("held/a.jpg")],
                saved: None,
            },
        )];
        let mut i = input(&lines, rows(&es));
        i.export_all = Some("Export all (1)");
        i.focus_row = Some(0);
        let t = tree(&i);
        for id in [
            Id::HelpButton,
            Id::SettingsButton,
            Id::AddFiles,
            Id::ExportAll,
            Id::Clear,
        ] {
            let n = t.find(&id).unwrap();
            assert!(n.states.focusable, "{id:?}");
            assert!(!n.states.focused, "{id:?}");
            assert_eq!(button_of(&id).map(button_id), Some(id));
        }
        assert_eq!(t.focused().unwrap().id, Id::Row(es[0].path.clone()));
        // On a button, it has the focus and the row doesn't.
        for b in [
            FocusButton::AddFiles,
            FocusButton::ExportAll,
            FocusButton::Clear,
            FocusButton::Help,
            FocusButton::Settings,
        ] {
            i.focus_button = Some(b);
            let t = tree(&i);
            let focused: Vec<&Id> = {
                let mut all = vec![];
                t.walk(&mut |n| {
                    if n.states.focused {
                        all.push(&n.id);
                    }
                });
                all
            };
            assert_eq!(focused, [&button_id(b)]);
        }
        assert_eq!(button_of(&Id::List), None);
    }

    #[test]
    fn the_empty_windows_buttons_take_the_focus() {
        let lines = empty_lines(&Settings::default());
        let mut i = input(&lines, vec![]);
        let t = tree(&i);
        for id in [Id::AddFiles, Id::HelpButton, Id::SettingsButton] {
            assert!(t.find(&id).unwrap().states.focusable, "{id:?}");
        }
        assert!(t.focused().is_none());
        i.focus_button = Some(FocusButton::AddFiles);
        let t = tree(&i);
        assert_eq!(t.focused().unwrap().id, Id::AddFiles);
    }

    #[test]
    fn empty_window_names_its_drop_panel_and_buttons() {
        let lines = empty_lines(&Settings::default());
        let t = tree(&input(&lines, vec![]));
        sound(&t);
        assert_eq!((t.role, t.name.as_str()), (Role::Window, "Kiln"));
        named(&t, Role::Button, "Help");
        named(&t, Role::Button, "Settings");
        let panel = named(&t, Role::Group, "Drop files here");
        assert!(panel
            .children
            .iter()
            .any(|c| c.role == Role::Button && c.name == "Add files"));
        assert!(panel.children.iter().any(|c| c.role == Role::Text));
        assert!(t.find(&Id::List).is_none());
        assert!(t.focused().is_none());
        assert!(named(&t, Role::Button, "Add files").patterns().invoke);
    }

    #[test]
    fn listed_files_are_list_items_in_a_list() {
        let lines = empty_lines(&Settings::default());
        let es = [
            entry("a.png", JobState::Idle),
            entry("b.png", JobState::Idle),
        ];
        let mut rs = rows(&es);
        rs[1].selected = true;
        let mut i = input(&lines, rs);
        i.focus_row = Some(1);
        i.status = "Added 2 files.";
        let t = tree(&i);
        sound(&t);
        let list = named(&t, Role::List, LIST_NAME);
        assert!(list.patterns().selection);
        assert_eq!(list.children.len(), 2);
        let a = &list.children[0];
        assert_eq!(a.role, Role::ListItem);
        assert_eq!(
            a.name,
            format!("a.png, {}", es[0].line().0.replace("  ·  ", ", "))
        );
        assert_eq!(a.help.as_deref(), Some(r"C:\photos"));
        assert!(a.patterns().selection_item && a.patterns().scroll_item);
        assert!(!a.states.selected && list.children[1].states.selected);
        assert_eq!(
            t.focused().map(|n| &n.id),
            Some(&Id::Row(es[1].path.clone()))
        );
        let status = named(&t, Role::Text, "Added 2 files.");
        assert!(status.live);
        named(&t, Role::Button, "Add files");
        named(&t, Role::Button, "Clear");
        assert!(t.find(&Id::ExportAll).is_none());
    }

    #[test]
    fn a_running_row_says_how_far_and_offers_stop() {
        let lines = empty_lines(&Settings::default());
        let es = [entry(
            "a.png",
            JobState::Running {
                action: Action::Convert("jpg".into()),
                fraction: Some(0.45),
            },
        )];
        let t = tree(&input(&lines, rows(&es)));
        sound(&t);
        let row = named(&t, Role::ListItem, "a.png, Converting to JPG");
        assert_eq!(row.status.as_deref(), Some("45%"));
        let cancel = &row.children[0];
        assert_eq!(
            (cancel.role, cancel.name.as_str()),
            (Role::Button, "Stop a.png")
        );
        assert!(cancel.patterns().invoke);
    }

    #[test]
    fn a_held_row_offers_export_and_export_all_shows() {
        let lines = empty_lines(&Settings::default());
        let es = [entry(
            "a.png",
            JobState::Done {
                action: Action::Convert("jpg".into()),
                outputs: vec![PathBuf::from("held/a.jpg")],
                saved: None,
            },
        )];
        let mut i = input(&lines, rows(&es));
        i.export_all = Some("Export all (1)");
        let t = tree(&i);
        sound(&t);
        let row = named(&t, Role::ListItem, "a.png, Ready: a.jpg");
        assert_eq!(row.children.len(), 1);
        assert_eq!(
            (row.children[0].role, row.children[0].name.as_str()),
            (Role::Button, "Export a.png")
        );
        named(&t, Role::Button, "Export all (1)");
    }

    #[test]
    fn each_held_rows_export_names_its_file() {
        let lines = empty_lines(&Settings::default());
        let held = |name: &str| {
            entry(
                name,
                JobState::Done {
                    action: Action::Convert("jpg".into()),
                    outputs: vec![PathBuf::from(format!("held/{name}.jpg"))],
                    saved: None,
                },
            )
        };
        let es = [held("a.png"), held("b.png"), held("c.png")];
        let t = tree(&input(&lines, rows(&es)));
        let mut links = vec![];
        t.walk(&mut |n| {
            if matches!(n.id, Id::RowLink(_)) {
                links.push(n.name.clone());
            }
        });
        assert_eq!(links, ["Export a.png", "Export b.png", "Export c.png"]);
    }

    #[test]
    fn a_saved_row_links_to_its_folder() {
        let lines = empty_lines(&Settings::default());
        let es = [entry(
            "a.png",
            JobState::Done {
                action: Action::Convert("jpg".into()),
                outputs: vec![PathBuf::from(r"C:\out\a.jpg")],
                saved: Some(PathBuf::from(r"C:\out")),
            },
        )];
        let t = tree(&input(&lines, rows(&es)));
        sound(&t);
        let row = t.find(&Id::Row(es[0].path.clone())).unwrap();
        let link = &row.children[0];
        assert_eq!(link.role, Role::Hyperlink);
        assert_eq!(link.name, es[0].line().0);
    }

    #[test]
    fn a_failed_row_reads_its_whole_sentence() {
        let lines = empty_lines(&Settings::default());
        let message = "FFmpeg could not read this file. It may be damaged, or \
                       in a format this version of FFmpeg doesn't know.";
        let es = [entry(
            "a.mov",
            JobState::Failed {
                message: message.into(),
                details: "moov atom not found".into(),
                engine: Some("FFmpeg"),
            },
        )];
        let t = tree(&input(&lines, rows(&es)));
        sound(&t);
        let row = named(&t, Role::ListItem, &format!("a.mov, {message}"));
        assert_eq!(row.children[0].name, "Show details a.mov");
        assert_eq!(row.children[0].role, Role::Button);
    }

    #[test]
    fn a_scrolled_out_row_is_offscreen() {
        let lines = empty_lines(&Settings::default());
        let es: Vec<Entry> = (0..40)
            .map(|k| entry(&format!("{k}.png"), JobState::Idle))
            .collect();
        let t = tree(&input(&lines, rows(&es)));
        let list = t.find(&Id::List).unwrap();
        assert!(!list.children[0].states.offscreen);
        assert!(list.children[39].states.offscreen);
    }

    fn item(ext: &str) -> Item {
        Item {
            label: ext.to_ascii_uppercase(),
            is_code: true,
            icon: "image",
            action: Action::Convert(ext.into()),
            kind: None,
        }
    }

    #[test]
    fn the_wheel_is_a_menu_of_plain_descriptions() {
        let lines = empty_lines(&Settings::default());
        let es = [entry("a.png", JobState::Idle)];
        let w = Wheel {
            kind: WheelKind::Convert,
            items: vec![item("jpg"), item("webp")],
            more: vec![item("bmp")],
            center: (400.0, 300.0),
            scale: 1.0,
            source_code: "PNG".into(),
        };
        let paths = [es[0].path.clone()];
        let mut i = input(&lines, rows(&es));
        i.wheel = Some(WheelIn {
            wheel: &w,
            paths: &paths,
            hover: Some(Spot::Inner(1)),
            more_open: true,
            options: true,
        });
        let t = tree(&i);
        sound(&t);
        // Only the menu: the list behind it doesn't take the keys.
        assert_eq!(t.children.len(), 1);
        let menu = named(&t, Role::Menu, "Convert a.png");
        let jpg = named(menu, Role::MenuItem, &wheel::target_label("jpg"));
        assert!(jpg.patterns().invoke);
        let webp = named(menu, Role::MenuItem, &wheel::target_label("webp"));
        assert!(webp.states.focused && webp.states.selected);
        let more = named(menu, Role::MenuItem, "More");
        named(more, Role::MenuItem, &wheel::target_label("bmp"));
        let options = named(menu, Role::MenuItem, OPTIONS_NAME);
        assert_eq!(options.shortcut, Some("Tab"));
    }

    #[test]
    fn a_wheel_for_many_files_says_how_many() {
        let lines = empty_lines(&Settings::default());
        let w = Wheel {
            kind: WheelKind::Tools,
            items: vec![],
            more: vec![],
            center: (400.0, 300.0),
            scale: 1.0,
            source_code: "MIXED".into(),
        };
        let paths = [PathBuf::from("a.png"), PathBuf::from("b.png")];
        let mut i = input(&lines, vec![]);
        i.wheel = Some(WheelIn {
            wheel: &w,
            paths: &paths,
            hover: None,
            more_open: false,
            options: false,
        });
        let t = tree(&i);
        assert!(named(&t, Role::Menu, "Tools 2 files").states.focused);
    }

    fn settings_input<'a>(
        l: &'a SettingsLayout,
        quality: &'a QualityDefaults,
        focus: Option<usize>,
    ) -> SettingsIn<'a> {
        SettingsIn {
            layout: l,
            focus,
            quality,
            theme: ThemeChoice::default(),
            suggestions: true,
            has_habits: false,
            keys: KeyMap::default(),
            output: "Next to each file",
            engines: ["Found", "Found", "Found", "Not found"],
            update_button: "Check for updates",
            update_note: "",
        }
    }

    #[test]
    fn settings_names_its_tabs_rows_and_controls() {
        let lines = empty_lines(&Settings::default());
        let quality = QualityDefaults::default();
        let l = layout::settings(800.0, 600.0, SettingsShape::default());
        let mut i = input(&lines, vec![]);
        i.settings = Some(settings_input(&l, &quality, Some(0)));
        let t = tree(&i);
        sound(&t);
        let dialog = named(&t, Role::Dialog, "Settings");
        let tabs = named(dialog, Role::Tab, TABS_NAME);
        assert!(tabs.patterns().selection);
        let names: Vec<&str> = tabs.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["General", "Quality", "Helper apps"]);
        assert!(tabs.children[0].states.selected && tabs.children[0].states.focused);
        assert!(tabs.children[0].patterns().selection_item);

        let output = named(dialog, Role::Group, "Start exports in");
        assert!(output.help.is_some());
        named(output, Role::Text, "Next to each file");
        named(output, Role::Button, "Choose");
        let theme = named(dialog, Role::Group, "Theme");
        assert!(theme.patterns().selection);
        assert!(theme.children.iter().all(|c| c.role == Role::RadioButton));
        assert_eq!(
            theme.children.iter().filter(|c| c.states.selected).count(),
            1
        );
        let suggestions = named(dialog, Role::Group, "Suggestions");
        assert!(named(suggestions, Role::RadioButton, "On").states.selected);
        assert!(
            named(suggestions, Role::Button, "Forget picks")
                .states
                .disabled
        );
        let keys = &dialog.children[3];
        assert_eq!(keys.role, Role::Group);
        assert_eq!(keys.children.len(), 3);
        assert!(keys.children[0].name.starts_with("Convert, "));
        named(dialog, Role::Button, "Check for updates");
        named(dialog, Role::Button, "Close");
    }

    #[test]
    fn quality_steppers_are_spinners_named_by_their_labels() {
        let lines = empty_lines(&Settings::default());
        let quality = QualityDefaults::default();
        let shape = SettingsShape {
            tab: SettingsTab::Quality,
            ..Default::default()
        };
        let l = layout::settings(800.0, 600.0, shape);
        let mut i = input(&lines, vec![]);
        i.settings = Some(settings_input(&l, &quality, None));
        let t = tree(&i);
        sound(&t);
        for label in ["Resolution", "Frame rate", "Image size", "Sound quality"] {
            let s = named(&t, Role::Spinner, label);
            assert!(s.patterns().value);
            assert!(s.value.as_deref().is_some_and(|v| !v.is_empty()));
        }
        let video = named(&t, Role::Group, "Video");
        let presets: Vec<&str> = video.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(presets.len(), Preset::ALL.len());
        assert!(t.find(&Id::Settings).unwrap().states.focused);
    }

    #[test]
    fn helper_apps_rows_read_their_state() {
        let lines = empty_lines(&Settings::default());
        let quality = QualityDefaults::default();
        let shape = SettingsShape {
            tab: SettingsTab::Engines,
            engine_missing: [false, false, false, true],
            ..Default::default()
        };
        let l = layout::settings(800.0, 600.0, shape);
        let mut i = input(&lines, vec![]);
        i.settings = Some(settings_input(&l, &quality, None));
        let t = tree(&i);
        sound(&t);
        let last = crate::settings::Engine::ALL[3];
        let row = named(&t, Role::Group, last.label());
        named(row, Role::Text, "Not found");
        named(row, Role::Hyperlink, "Get it");
        named(row, Role::Button, "Choose");
        assert_eq!(row.help.as_deref(), Some(last.purpose()));
    }

    #[test]
    fn the_details_dialog_reads_its_text_and_buttons() {
        let lines = empty_lines(&Settings::default());
        let es = [entry("a.mov", JobState::Idle)];
        let mut i = input(&lines, rows(&es));
        i.details = Some(DetailsIn {
            title: "What FFmpeg said",
            text: "moov atom not found",
            copy: "Copy",
            focus: DialogButton::Copy,
        });
        let t = tree(&i);
        sound(&t);
        assert_eq!(t.children.len(), 1);
        let d = named(&t, Role::Dialog, "What FFmpeg said");
        named(d, Role::Text, "moov atom not found");
        assert!(named(d, Role::Button, "Copy").states.focused);
        named(d, Role::Button, "Close");
    }

    #[test]
    fn help_reads_each_section() {
        let lines = empty_lines(&Settings::default());
        let sections = help_content(&KeyMap::default());
        let mut i = input(&lines, vec![]);
        i.help = Some(&sections);
        let t = tree(&i);
        sound(&t);
        let d = named(&t, Role::Dialog, crate::help::TITLE);
        assert_eq!(d.children.len(), sections.len() + 1);
        assert!(named(d, Role::Button, "Close").states.focused);
    }

    #[test]
    fn keys_stay_the_same_for_the_same_element() {
        let a = Id::Row(PathBuf::from(r"C:\a.png"));
        assert_eq!(a.key(), Id::Row(PathBuf::from(r"C:\a.png")).key());
        assert_ne!(a.key(), Id::Row(PathBuf::from(r"C:\b.png")).key());
        assert_ne!(Id::HelpButton.key(), Id::SettingsButton.key());
    }

    #[test]
    fn hit_finds_the_deepest_element_on_screen() {
        let lines = empty_lines(&Settings::default());
        let es = [entry("a.png", JobState::Idle)];
        let t = tree(&input(&lines, rows(&es)));
        let row = t.find(&Id::Row(es[0].path.clone())).unwrap();
        let (x, y) = (row.rect.x + 4.0, row.rect.y + 4.0);
        assert_eq!(t.hit(x, y).id, row.id);
        let paths = t.paths();
        let (_, path) = paths.iter().find(|(k, _)| *k == row.key).unwrap();
        assert_eq!(t.at(path).map(|n| &n.id), Some(&row.id));
    }

    #[test]
    fn two_selected_rows_both_read_selected() {
        let lines = empty_lines(&Settings::default());
        let es = [
            entry("a.png", JobState::Idle),
            entry("b.png", JobState::Idle),
            entry("c.png", JobState::Idle),
        ];
        let mut rs = rows(&es);
        rs[0].selected = true;
        rs[2].selected = true;
        let t = tree(&input(&lines, rs));
        let list = t.find(&Id::List).unwrap();
        assert!(list.patterns().selection);
        let selected: Vec<bool> = list.children.iter().map(|c| c.states.selected).collect();
        assert_eq!(selected, [true, false, true]);
    }

    #[test]
    fn the_options_sheet_reads_its_knobs_and_buttons() {
        let lines = empty_lines(&Settings::default());
        let l = layout::sheet(
            800.0,
            600.0,
            &[kiln_core::Knob::Preset, kiln_core::Knob::Size],
        );
        let knobs = [
            ("Quality".to_owned(), "High".to_owned()),
            ("Size".to_owned(), "1080p".to_owned()),
        ];
        let mut i = input(&lines, vec![]);
        i.sheet = Some(SheetIn {
            layout: &l,
            title: "Convert to MP4",
            target: "MP4: video",
            context: "interview.mov, 3.4 MB",
            result: "1080p, about 1.1 MB",
            rows: &knobs,
            preset: Preset::High,
            focus: Some(0),
        });
        let t = tree(&i);
        sound(&t);
        assert_eq!(t.children.len(), 1);
        let d = named(&t, Role::Dialog, "Convert to MP4");
        let quality = named(d, Role::Group, "Quality");
        assert!(quality.patterns().selection);
        let chosen: Vec<&str> = quality
            .children
            .iter()
            .filter(|c| c.states.selected)
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(chosen, [Preset::High.label()]);
        assert!(quality.children[0].states.focused);
        let size = named(d, Role::Spinner, "Size");
        assert_eq!(size.value.as_deref(), Some("1080p"));
        named(d, Role::Text, "1080p, about 1.1 MB");
        named(d, Role::Button, "Cancel");
        named(d, Role::Button, "Convert");
    }

    #[test]
    fn requests_for_elements_that_are_gone_are_dropped() {
        let lines = empty_lines(&Settings::default());
        let quality = QualityDefaults::default();
        let es = [entry("a.png", JobState::Idle)];
        let ask = |id: Id, verb: Verb| Request { id, verb };

        // With Settings open, the list's buttons aren't there to press.
        let l = layout::settings(800.0, 600.0, SettingsShape::default());
        let mut i = input(&lines, rows(&es));
        i.settings = Some(settings_input(&l, &quality, None));
        let open = tree(&i);
        assert!(open.find(&Id::Clear).is_none());
        assert!(!accepts(&open, &ask(Id::Clear, Verb::Invoke)));
        assert!(!accepts(&open, &ask(Id::AddFiles, Verb::Invoke)));
        let theme = Id::Setting(Control::Theme(ThemeChoice::default()));
        assert!(accepts(&open, &ask(theme.clone(), Verb::Select)));
        // Forget picks is disabled without picks: it can take the focus
        // but not be pressed.
        let forget = Id::Setting(Control::ForgetPicks);
        assert!(!accepts(&open, &ask(forget.clone(), Verb::Invoke)));
        assert!(accepts(&open, &ask(forget, Verb::Focus)));

        // With Settings closed, its controls are gone.
        let closed = tree(&input(&lines, rows(&es)));
        assert!(accepts(&closed, &ask(Id::Clear, Verb::Invoke)));
        assert!(!accepts(&closed, &ask(theme, Verb::Select)));
        let step = Id::Setting(Control::Step(crate::quality::DefaultSetting::Resolution));
        assert!(!accepts(&closed, &ask(step, Verb::SetValue("720p".into()))));
        // A row that left the list is gone too.
        let gone = Id::Row(PathBuf::from(r"C:\photos\gone.png"));
        assert!(!accepts(&closed, &ask(gone, Verb::Select)));
        assert!(accepts(
            &closed,
            &ask(Id::Row(es[0].path.clone()), Verb::Select)
        ));
    }

    #[test]
    fn neighbours_agree_with_paths() {
        let lines = empty_lines(&Settings::default());
        let es = [
            entry("a.png", JobState::Idle),
            entry(
                "b.png",
                JobState::Running {
                    action: Action::Convert("jpg".into()),
                    fraction: Some(0.5),
                },
            ),
        ];
        let t = tree(&input(&lines, rows(&es)));
        let key_at = |p: &[usize]| t.at(p).map(|n| n.key);
        for (key, path) in &t.paths() {
            let n = t.at(path).unwrap();
            assert_eq!(n.key, *key);
            let first = t.neighbour(path, Toward::FirstChild);
            let last = t.neighbour(path, Toward::LastChild);
            match n.children.len() {
                0 => assert_eq!((first, last), (Neighbour::Nothing, Neighbour::Nothing)),
                k => {
                    let mut p = path.clone();
                    p.push(0);
                    assert_eq!(first, Neighbour::Node(key_at(&p).unwrap()));
                    *p.last_mut().unwrap() = k - 1;
                    assert_eq!(last, Neighbour::Node(key_at(&p).unwrap()));
                }
            }
            let Some((&i, parent)) = path.split_last() else {
                // The window's own parent and siblings are the HWND's.
                assert_eq!(t.neighbour(path, Toward::Parent), Neighbour::Nothing);
                assert_eq!(t.neighbour(path, Toward::Next), Neighbour::Nothing);
                continue;
            };
            let up = t.neighbour(path, Toward::Parent);
            if parent.is_empty() {
                assert_eq!(up, Neighbour::Root);
            } else {
                assert_eq!(up, Neighbour::Node(key_at(parent).unwrap()));
            }
            let sibling = |j: Option<usize>| {
                j.and_then(|j| {
                    let mut p = parent.to_vec();
                    p.push(j);
                    key_at(&p)
                })
                .map_or(Neighbour::Nothing, Neighbour::Node)
            };
            assert_eq!(t.neighbour(path, Toward::Next), sibling(Some(i + 1)));
            assert_eq!(
                t.neighbour(path, Toward::Previous),
                sibling(i.checked_sub(1))
            );
        }
    }

    #[test]
    fn progress_is_told_a_few_times_a_second_at_most() {
        let mut gate = ProgressGate::default();
        let path = Path::new(r"C:\photos\a.mov");
        let t0 = Instant::now();
        assert_eq!(gate.shown(path, Some(0.10), t0), Some(10));
        // Inside 250 ms a new percent waits, so the tree doesn't change.
        let soon = t0 + Duration::from_millis(100);
        assert_eq!(gate.shown(path, Some(0.13), soon), Some(10));
        assert_eq!(gate.shown(path, Some(0.104), soon), Some(10));
        let lines = empty_lines(&Settings::default());
        let running = |f: f32| {
            entry(
                "a.mov",
                JobState::Running {
                    action: Action::Convert("mp4".into()),
                    fraction: Some(f),
                },
            )
        };
        let (before, after) = ([running(0.10)], [running(0.13)]);
        let mut a = rows(&before);
        a[0].progress = Some(10);
        let mut b = rows(&after);
        b[0].progress = gate.shown(path, Some(0.13), soon);
        let (ta, tb) = (tree(&input(&lines, a)), tree(&input(&lines, b)));
        assert_eq!(ta, tb);
        // Once 250 ms have passed it moves on.
        let later = t0 + PROGRESS_EVERY;
        assert_eq!(gate.shown(path, Some(0.13), later), Some(13));
        assert_eq!(gate.shown(path, Some(0.20), later), Some(13));
        // A job that stopped saying how far is forgotten.
        assert_eq!(gate.shown(path, None, later), None);
        assert_eq!(gate.shown(path, Some(0.5), later), Some(50));
        gate.keep(|_| false);
        assert!(gate.told.is_empty());
    }

    #[test]
    fn a_held_percent_is_due_once_its_quarter_second_is_up() {
        let mut gate = ProgressGate::default();
        let (a, b) = (Path::new(r"C:\photos\a.mov"), Path::new(r"C:\photos\b.mov"));
        let t0 = Instant::now();
        gate.shown(a, Some(0.10), t0);
        gate.shown(b, Some(0.50), t0 + Duration::from_millis(50));
        // Nothing is held: nothing is due.
        assert_eq!(gate.due(), None);
        // b's 55% is held, so it's due 250 ms after b's 50% was told.
        let soon = t0 + Duration::from_millis(100);
        assert_eq!(gate.shown(b, Some(0.55), soon), Some(50));
        assert_eq!(gate.due(), Some(t0 + Duration::from_millis(300)));
        // a's held 13% is due sooner.
        assert_eq!(gate.shown(a, Some(0.13), soon), Some(10));
        assert_eq!(gate.due(), Some(t0 + PROGRESS_EVERY));
        // Back to the percent already told, a holds nothing.
        assert_eq!(gate.shown(a, Some(0.10), soon), Some(10));
        assert_eq!(gate.due(), Some(t0 + Duration::from_millis(300)));
        // Told when due, nothing is held any more.
        let due = gate.due().unwrap();
        assert_eq!(gate.shown(b, Some(0.55), due), Some(55));
        assert_eq!(gate.due(), None);
        // A row that stops saying how far holds nothing.
        gate.shown(b, Some(0.60), due + Duration::from_millis(10));
        assert!(gate.due().is_some());
        gate.shown(b, None, due + Duration::from_millis(20));
        assert_eq!(gate.due(), None);
    }

    #[test]
    fn a_flush_timer_never_fires_before_it_is_due() {
        let t0 = Instant::now();
        // Rounded up, never down, with slack for an early tick.
        let due = t0 + Duration::from_micros(249_400);
        assert_eq!(timer_ms(t0, due), 252);
        assert_eq!(timer_ms(t0, t0 + PROGRESS_EVERY), 252);
        // Already due, or past it: as soon as a timer can.
        assert_eq!(timer_ms(t0 + PROGRESS_EVERY, t0), 2);
        assert!(timer_ms(t0, t0) >= 1);
    }

    #[test]
    fn every_status_set_is_said_on_its_own() {
        let mut teller = StatusTeller::default();
        let set = |t: &str, alert: bool| (t.to_owned(), alert);
        // Before a client connects, nothing is said, but the line is noted.
        assert!(teller.tell([set("0 files.", false)], false).is_empty());
        assert!(teller.tell([set("0 files.", false)], true).is_empty());
        // Two batches finishing at once are both said, in order.
        let two = [
            set("Converted 1 file. 1 failed.", true),
            set("Converted 2 files.", false),
        ];
        assert_eq!(teller.tell(two.clone(), true), two);
        // The same plain line again says nothing new.
        assert!(teller
            .tell([set("Converted 2 files.", false)], true)
            .is_empty());
        // The same alert twice is said twice.
        let alert = set("Couldn't open Terminal.", true);
        for _ in 0..2 {
            assert_eq!(teller.tell([alert.clone()], true), vec![alert.clone()]);
        }
        // An empty status is never said.
        assert!(teller.tell([set("", false)], true).is_empty());
    }
}
