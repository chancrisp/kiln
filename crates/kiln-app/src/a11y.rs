//! What Kiln shows, as a tree of named roles for screen readers, voice
//! control and other assistive technology. Pure: [`tree`] builds it from
//! the model and the layout, so it can be tested without a window.
//! `win::uia` serves it through UI Automation.

use std::path::PathBuf;

use kiln_core::Preset;

use crate::help::HelpSection;
use crate::layout::{
    self, Control, DialogButton, Rect, SettingsLayout, SettingsRow, SettingsTab, SheetControl,
    SheetLayout,
};
use crate::model::{Entry, JobState};
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
    let mut out = vec![
        Node::new(Id::HelpButton, Role::Button, "Help", l.help_button)
            .help(Some(crate::help::HELP_TIP)),
        Node::new(
            Id::SettingsButton,
            Role::Button,
            "Settings",
            l.settings_button,
        )
        .help(Some(crate::help::SETTINGS_TIP)),
    ];
    let lines = input.empty;
    if input.rows.is_empty() {
        let e = layout::empty_state(&l.panel);
        let children = [
            text(Id::DropLine(0), lines.instruction, e.instruction),
            Some(
                Node::new(Id::AddFiles, Role::Button, lines.button, e.button)
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
            .help(Some(crate::help::EXPORT_ALL_TIP)),
        );
    }
    out.push(Node::new(Id::Clear, Role::Button, "Clear", clear));
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
    if let JobState::Running { fraction, .. } = &e.job {
        n.status = fraction.map(|f| format!("{}%", (f.clamp(0.0, 1.0) * 100.0).round()));
        children.push(Node::new(
            Id::RowCancel(path.clone()),
            Role::Button,
            "Cancel",
            layout::row_cancel(&rect),
        ));
    }
    if let Some(link) = r.link {
        children.push(Node::new(
            Id::RowLink(path.clone()),
            Role::Button,
            link,
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
        items.push(Node::new(
            Id::WheelOptions,
            Role::MenuItem,
            wheel::OPTIONS_TARGET,
            Rect::new(cx + o.x * s, cy + o.y * s, o.w * s, o.h * s),
        ));
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
            })
            .collect()
    }

    fn input<'a>(empty: &'a EmptyLines, rows: Vec<RowIn<'a>>) -> Input<'a> {
        Input {
            size: (800.0, 600.0),
            rows,
            scroll: 0.0,
            focus_row: None,
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
    fn a_running_row_says_how_far_and_offers_cancel() {
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
            (Role::Button, "Cancel")
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
            (Role::Button, "Export")
        );
        named(&t, Role::Button, "Export all (1)");
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
        assert_eq!(row.children[0].name, "Show details");
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
        named(menu, Role::MenuItem, wheel::OPTIONS_TARGET);
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
}
