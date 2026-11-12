//! What Kiln shows, as a tree of named roles for screen readers, voice
//! control and other assistive technology. Pure: [`tree`] builds it from
//! the model and the layout, so it can be tested without a window.
//! `win::uia` serves it through UI Automation.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::controls::ComboState;
use crate::help::HelpSection;
use crate::inspector::{Inspector, InspectorControl, Target, NOTHING_TITLE};
use crate::layout::kit::FlyoutLayout;
use crate::layout::{
    self, Control, DialogButton, HelpButton, InspectorLayout, Rect, SettingsLayout, SheetControl,
    SheetLayout,
};
use crate::model::{Entry, FocusButton, JobState};
use crate::tune_sheet::spoken;
use crate::ways_in::{EmptyLines, FirstRun};
use crate::wheel::{self, Action, Item, Spot, Wheel, R_INNER, R_OUTER};

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
    /// A running row's progress, a range from 0 to 100.
    ProgressBar,
    /// The sound slider: a range of steps, with its value as text.
    Slider,
    /// A panel over a scrim that keeps the keys until it closes.
    Dialog,
    /// A picture: the inspector's preview of the selected file.
    Image,
    /// A ComboBox: its value, and while open its choices as list items.
    ComboBox,
    /// A text box that can be typed in.
    Edit,
}

/// Which element a node is. It stays the same while the element does, so
/// a screen reader keeps its place across repaints: a row by its file, a
/// control by what it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Id {
    Root,
    /// The title bar Kiln draws, and its buttons: Back while Settings is
    /// open, Help and Settings, and Windows' Minimise, Maximise and Close.
    TitleBar,
    BackButton,
    HelpButton,
    SettingsButton,
    CaptionButton(Caption),
    /// The empty window's drop panel, and its lines of text.
    DropPanel,
    DropLine(u8),
    /// The empty window's first-run card, its lines in reading order (the
    /// three steps, then the right-click line), and its Dismiss.
    FirstRunCard,
    FirstRunLine(u8),
    FirstRunDismiss,
    /// The InfoBar at the top of the list, its message, and its Dismiss.
    TopInfo,
    TopInfoText,
    TopInfoDismiss,
    AddFiles,
    /// The footer's Convert button.
    ConvertButton,
    /// "Undo", in the footer after rows were removed.
    UndoRemove,
    /// The credit at the window's bottom left: its words, and the link.
    CreditText,
    CreditLink,
    List,
    Row(PathBuf),
    RowCancel(PathBuf),
    /// A running row's progress, with its percent as a range value.
    RowProgress(PathBuf),
    /// "Retry", on a cancelled or failed row.
    RowRetry(PathBuf),
    /// A finished drag-wheel conversion's "Options".
    RowOptions(PathBuf),
    /// "Show details", or a saved row's Show in folder.
    RowLink(PathBuf),
    Status,
    Clear,
    Wheel,
    WheelItem(Spot),
    /// The wheel's hub: what is lit, and the hint.
    WheelHub,
    /// More's open list, and its group `k` under its heading.
    Flyout,
    FlyoutGroup(usize),
    /// The right-click menu, and its line `k`.
    RowMenu,
    RowMenuItem(usize),
    /// The notification-area icon's menu, and its line `k`.
    TrayMenu,
    TrayMenuItem(usize),
    /// The Settings page, its title, a section, and a card.
    Settings,
    SettingsTitle,
    SettingsSection(crate::settings_page::Section),
    SettingsCard(crate::settings_page::Card),
    /// A button, a radio button or a helper's Expander on the Settings
    /// page.
    Setting(Control),
    /// A ComboBox on the Settings page.
    Combo(Control),
    /// A ToggleSwitch on the Settings page.
    Toggle(Control),
    /// An open helper's line and the caption under it.
    SettingsLine(crate::settings_page::Helper, u8),
    Details,
    DetailsLead,
    DetailsText,
    DetailsButton(DialogButton),
    Help,
    HelpSection(usize),
    HelpLine(usize, usize),
    /// Help's Show first-run help.
    HelpGuide,
    HelpClose,
    Sheet,
    SheetLine(u8),
    SheetRow(usize),
    SheetControl(SheetControl),
    /// The inspector beside the list: the selected file and what can be
    /// done with it.
    Inspector,
    /// A control in the inspector.
    InspectorControl(InspectorControl),
    /// The inspector's picture of the selected file.
    InspectorPreview,
    /// The inspector's lines of text, in reading order.
    InspectorText(usize),
    /// Choice `n` of a control: a ComboBox's choice, a radio button in a
    /// group, a menu's item, an InfoBar's line.
    Choice(Box<Id>, usize),
    /// One of Kiln's prompts, open in the window: its text, and its
    /// buttons in order.
    Ask,
    AskText,
    AskButton(usize),
    /// The Make smaller or Resize dialog: its lines of text (the
    /// sub-line, the heading, the estimate's two lines), its choices,
    /// Custom's box, More options and its line `n`, and its accent
    /// button (0) and Cancel (1).
    Tool,
    ToolText(u8),
    ToolChoices,
    ToolCustom,
    ToolMore,
    ToolLine(usize),
    ToolButton(u8),
}

/// One of Windows' buttons at the right of the title bar, which Kiln draws
/// and names since Windows no longer does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Caption {
    Min,
    Max,
    Close,
}

impl Caption {
    pub const ALL: [Caption; 3] = [Caption::Min, Caption::Max, Caption::Close];

    /// Its name: Maximise reads Restore while the window is maximised.
    pub fn name(self, maximized: bool) -> &'static str {
        match self {
            Caption::Min => "Minimise",
            Caption::Max if maximized => "Restore",
            Caption::Max => "Maximise",
            Caption::Close => "Close",
        }
    }
}

/// The title bar's group's name.
pub const TITLE_BAR_NAME: &str = "Title bar";

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
    /// A disclosure's state: `Some(true)` open, `Some(false)` closed,
    /// `None` for anything that doesn't open.
    pub expanded: Option<bool>,
    /// A ToggleSwitch's state: `Some(true)` on, `Some(false)` off, `None`
    /// for anything that isn't one.
    pub toggled: Option<bool>,
    /// A dialog that keeps the window until it is answered.
    pub modal: bool,
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
    /// A range value, such as a percent done.
    pub range_value: bool,
    /// Opens and closes, as Advanced does.
    pub expand_collapse: bool,
    /// Turns on and off, as a ToggleSwitch.
    pub toggle: bool,
    /// A modal dialog's window: it says it is modal, and closing it is
    /// Cancel.
    pub window: bool,
    /// Read by character, word and line: the Details log.
    pub text: bool,
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
    /// A progress bar's value, 0 to 100, or a slider's step.
    pub range: Option<f64>,
    /// A slider's last step (its first is 0). A progress bar's range ends
    /// at 100 and has none.
    pub range_max: Option<f64>,
    /// A slider's large change, in steps: the way to its next mark.
    pub range_large: Option<f64>,
    /// More about it: a row's folder, a setting's hint.
    pub help: Option<String>,
    /// How a running row is getting on: "45%".
    pub status: Option<String>,
    /// Its text changes as things happen, and is announced.
    pub live: bool,
    /// The key that does it, said apart from its name: "Tab".
    pub shortcut: Option<&'static str>,
    /// A text box's: what it takes, so a value set through UI Automation
    /// that it would refuse is refused there.
    pub text_rule: Option<crate::controls::TextRule>,
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
            range: None,
            range_max: None,
            range_large: None,
            help: None,
            status: None,
            live: false,
            shortcut: None,
            text_rule: None,
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
        let toggle = self.states.toggled.is_some();
        Patterns {
            // A ToggleSwitch toggles; it isn't invoked.
            invoke: matches!(self.role, Role::Button | Role::Hyperlink | Role::MenuItem) && !toggle,
            selection: matches!(self.role, Role::List | Role::Tab | Role::ComboBox)
                || (self.role == Role::Group && radios),
            selection_item: matches!(
                self.role,
                Role::ListItem | Role::TabItem | Role::RadioButton
            ),
            value: matches!(
                self.role,
                Role::Spinner | Role::Slider | Role::ComboBox | Role::Edit
            ),
            scroll_item: self.role == Role::ListItem,
            // An indeterminate progress bar has no value to give.
            range_value: self.role == Role::Slider
                || (self.role == Role::ProgressBar && self.range.is_some()),
            expand_collapse: self.states.expanded.is_some(),
            toggle,
            window: self.role == Role::Dialog && self.states.modal,
            text: self.id == Id::DetailsText,
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
    /// Put a slider on this step, as UI Automation's range value set it.
    SetStep(usize),
    /// Open (true) or close a disclosure, as Advanced.
    Expand(bool),
    /// Flip a ToggleSwitch.
    Toggle,
    Focus,
    ScrollIntoView,
    /// Close a modal dialog, as its Window pattern does: Cancel.
    Close,
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
    /// Its result is being saved: it has no link or Options until it is.
    pub saving: bool,
    /// The link the row shows: "Show details" or "Export". `None` while
    /// rows show no links (see `Input::row_links`).
    pub link: Option<&'static str>,
    /// How far a running row has got, in whole percent, as last told.
    pub progress: Option<u32>,
}

/// The open wheel.
pub struct WheelIn<'a> {
    pub wheel: &'a Wheel,
    pub paths: &'a [PathBuf],
    pub hover: Option<Spot>,
    /// What the hub says.
    pub hub: &'a wheel::HubLines,
    /// More's list while it is open: its lines and where they are.
    pub flyout: Option<(&'a [wheel::FlyoutRow<'a>], &'a wheel::FlyoutPlace)>,
    /// Whether the files become a target (by its extension) by way of their
    /// text alone, so its name says "(text only)".
    pub text_only: &'a dyn Fn(&str) -> bool,
}

/// The Settings page: where its parts are, what it says, the control
/// with the keyboard focus and an open ComboBox's list.
pub struct SettingsIn {
    pub layout: SettingsLayout,
    pub page: crate::settings_page::PageSpec,
    pub focus: Option<Control>,
    pub combo: Option<(
        Control,
        crate::controls::ComboState,
        crate::layout::kit::FlyoutLayout,
    )>,
}

/// One of Kiln's prompts, open in the window.
pub struct AskIn<'a> {
    pub layout: crate::layout::kit::DialogLayout,
    pub title: &'a str,
    pub text: Option<&'a str>,
    /// Its buttons' words, in order.
    pub buttons: Vec<&'static str>,
    /// The button with the keyboard focus.
    pub focus: usize,
}

/// The Make smaller or Resize dialog.
pub struct ToolIn<'a> {
    pub layout: crate::smaller::ToolLayout,
    pub model: &'a crate::smaller::ToolDialog,
    pub focus: crate::smaller::Stop,
    /// The estimate card's two lines.
    pub estimate: Option<(String, String)>,
}

/// The details dialog.
pub struct DetailsIn<'a> {
    /// Where it is: a ContentDialog, its text in a card.
    pub layout: crate::layout::kit::DetailsDialog,
    pub title: &'a str,
    /// The plain line over the card.
    pub lead: &'a str,
    pub text: &'a str,
    /// "Copy", or "Copied" for a moment.
    pub copy: &'a str,
    pub focus: DialogButton,
}

/// The options dialog (spec 2.2).
pub struct SheetIn<'a> {
    pub layout: &'a SheetLayout,
    pub title: &'a str,
    /// The one line under the title: the source, then what it becomes.
    pub sub: &'a str,
    /// What the conversion leaves out, a line each.
    pub notes: &'a [String],
    /// The estimate card's line, in words; Max size's error while `error`.
    pub result: &'a str,
    pub error: bool,
    /// Each More options line as laid out.
    pub lines: &'a [SheetLine],
    /// Which Quality radio button is on.
    pub selected: Option<usize>,
    /// The Quality radio buttons' names, for sound and video Kiln writes
    /// itself; empty for the photo's, which are Small, High and Best.
    pub radios: &'a [&'static str],
    /// A More options line's open drop-down: its line, its state and its
    /// list as laid out.
    pub list: Option<(SheetControl, ComboState, FlyoutLayout)>,
    pub more_open: bool,
    pub focus: Option<usize>,
    pub convert_enabled: bool,
}

/// A More options line of the options dialog, as its tree and its drawing
/// need it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SheetLine {
    pub label: String,
    pub value: String,
    /// Whether it can be changed now.
    pub enabled: bool,
    /// The values its drop-down lists; `None` for a line that keeps
    /// its spin box.
    pub choices: Option<Vec<String>>,
}

/// The title bar as the tree needs it: where it and its buttons are, in
/// layout units. Back is there while Settings is open, Help and Settings
/// while it isn't.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TitleIn {
    pub rect: Rect,
    pub back: Option<Rect>,
    pub help: Option<Rect>,
    pub settings: Option<Rect>,
    pub min: Rect,
    pub max: Rect,
    pub close: Rect,
    pub maximized: bool,
    /// Tab put the keyboard on Back.
    pub back_focused: bool,
}

/// Everything the tree is built from.
pub struct Input<'a> {
    /// The window's client area, in layout units.
    pub size: (f32, f32),
    /// The title bar over the top of it.
    pub title: TitleIn,
    pub rows: Vec<RowIn<'a>>,
    /// Whether rows show their own action links: their link, Cancel and
    /// an exported row's "Saved to" line. Not while the inspector shows,
    /// which carries the actions.
    pub row_links: bool,
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
    /// The first-run card's words, while it shows in the empty window.
    pub first_run: Option<&'a FirstRun>,
    /// The InfoBar at the top of the list, while one shows.
    pub top_info: Option<TopInfoIn<'a>>,
    /// Where the empty window's parts are, as drawn. Without it, they are
    /// laid out with the card's lines one line each.
    pub empty_at: Option<layout::EmptyState>,
    /// The credit in the empty window's bottom-left corner, where it is
    /// drawn. The footer's is in `footer_row`.
    pub credit: Option<crate::credit::Credit>,
    /// "Save all (3)", while results are held.

    /// Why the footer's Convert is disabled; `None` when it can be used.
    pub convert_block: Option<&'a str>,
    /// What the footer's accent button says: "Convert", or "Convert 5
    /// files" for several (UX-05).
    pub convert_label: &'a str,
    /// Where the footer's parts are (`layout::footer_row`). Without it
    /// they are placed for one selected file.
    pub footer_row: Option<layout::FooterRow>,
    /// Rows were just removed and can come back: how the footer says so,
    /// "500 files removed", for the Undo link's name.
    pub undo: Option<&'a str>,
    pub wheel: Option<WheelIn<'a>>,
    pub settings: Option<SettingsIn>,
    pub details: Option<DetailsIn<'a>>,
    pub help: Option<HelpIn<'a>>,
    pub sheet: Option<SheetIn<'a>>,
    /// The inspector, while it shows beside the list.
    pub inspector: Option<InspectorIn<'a>>,
    /// One of Kiln's prompts, over everything, while it is open.
    pub ask: Option<AskIn<'a>>,
    /// The Make smaller or Resize dialog, under a prompt.
    pub tool: Option<ToolIn<'a>>,
    /// The right-click menu, over the list, while it is up.
    pub row_menu: Option<RowMenuIn<'a>>,
}

/// The right-click menu as the tree needs it.
pub struct RowMenuIn<'a> {
    pub items: &'a [crate::ways_in::MenuItem],
    pub layout: &'a layout::kit::MenuLayout,
    /// The lit line, which has the keyboard.
    pub lit: Option<usize>,
}

/// The right-click menu: a Menu of its items, each named as it reads and
/// with its shortcut; separators are left out of the tree.
fn row_menu(m: &RowMenuIn) -> Node {
    let children = m
        .items
        .iter()
        .zip(&m.layout.rows)
        .enumerate()
        .filter(|(_, (&item, _))| item != crate::ways_in::MenuItem::Separator)
        .map(|(k, (&item, r))| {
            let (label, key) = item.parts();
            let mut n = Node::new(Id::RowMenuItem(k), Role::MenuItem, label, *r).focusable();
            n.shortcut = match key.as_str() {
                "" => None,
                "Enter" => Some("Enter"),
                "Ctrl+Enter" => Some("Ctrl+Enter"),
                "Ctrl+S" => Some("Ctrl+S"),
                "Del" => Some("Del"),
                "Ctrl+O" => Some("Ctrl+O"),
                "Ctrl+V" => Some("Ctrl+V"),
                _ => None,
            };
            n
        })
        .collect();
    // Unnamed, as WinUI leaves a context flyout: read as "menu" once.
    Node::new(Id::RowMenu, Role::Menu, "", m.layout.rect).with(children)
}

/// One line of the notification-area icon's menu as the tree needs it:
/// its words and, for an item that is checked or not, which; `None` for a
/// separator.
pub type TrayLineIn<'a> = Option<(&'a str, Option<bool>)>;

/// The tree of the small window the notification-area icon's menu is
/// drawn in: the window, holding a Menu of its items. A checkable item
/// toggles; separators are left out of the tree, and each item keeps the
/// number of its line. The lit item has the keyboard.
pub fn tray_menu(
    lines: &[TrayLineIn],
    layout: &layout::kit::MenuLayout,
    lit: Option<usize>,
) -> Node {
    let children = lines
        .iter()
        .zip(&layout.rows)
        .enumerate()
        .filter_map(|(k, (line, r))| {
            let (label, checked) = (*line)?;
            let mut n = Node::new(Id::TrayMenuItem(k), Role::MenuItem, label, *r).focusable();
            n.states.toggled = checked;
            n.states.focused = lit == Some(k);
            Some(n)
        })
        .collect();
    let menu = Node::new(Id::TrayMenu, Role::Menu, "", layout.rect).with(children);
    Node::new(Id::Root, Role::Window, "Kiln", Rect::default()).with(vec![menu])
}

/// The inspector as the tree needs it.
pub struct InspectorIn<'a> {
    /// Where it is: the right of the list's panel.
    pub rect: Rect,
    /// What it says.
    pub model: &'a Inspector,
    /// Where its parts are. A part that isn't placed isn't in the tree.
    pub layout: &'a InspectorLayout,
    /// The selected file's name, which its buttons are named with. Empty
    /// for several files or none.
    pub file: &'a str,
    /// The whole path of the selected file's folder, which the Folder link
    /// opens and says as its help. Empty for several files or none.
    pub folder: &'a str,
}

/// Help, while it is open: what it says, where its parts are in window
/// coordinates, and which button has the keyboard.
pub struct HelpIn<'a> {
    pub sections: &'a [HelpSection],
    /// Where each line is now, and whether it is scrolled out of the
    /// content. A line without one is taken to show.
    pub places: Vec<HelpPlace>,
    /// The dialog, and its content where the lines scroll.
    pub frame: Rect,
    pub body: Rect,
    /// Show first-run help, and whether it is scrolled out of the content.
    pub guide: Rect,
    pub guide_offscreen: bool,
    pub close: Rect,
    /// Close until Tab moves it.
    pub focus: HelpButton,
}

/// Where one line of help is, in window coordinates, and whether it is
/// scrolled out of the content.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HelpPlace {
    pub section: usize,
    pub line: usize,
    pub rect: Rect,
    pub offscreen: bool,
}

/// Whether a line at `rect` is out of the body it is scrolled in: wholly
/// above or below it.
pub fn clipped_out(rect: &Rect, body: &Rect) -> bool {
    rect.bottom() <= body.y || rect.y >= body.bottom()
}

/// The list's name.
pub const LIST_NAME: &str = "Files";
/// The settings tabs' name.
/// The inspector's name.
pub const INSPECTOR_NAME: &str = "Selected file";

/// What an inspector control is called: a format as its target says it
/// ("Convert to WEBP, Small image for the web, about 410 KB"), a kind
/// by what choosing it does, and the rest with the file they act on:
/// "Cancel harbour.png", or the result for Export and Show in folder,
/// "Save harbour.webp". Several files' More is named with their kinds,
/// "More formats for 3 images".
pub fn inspector_name(c: InspectorControl, model: &Inspector, file: &str) -> String {
    use InspectorControl as C;
    let result = match model {
        Inspector::Done { result, .. } => result.as_str(),
        _ => file,
    };
    match (c, model) {
        (C::Target(i), _) => model
            .targets()
            .get(i)
            .map_or_else(String::new, Target::name),
        (C::Kind(i), Inspector::Several { offered, .. }) => match offered.get(i) {
            Some((code, 1)) => format!("Select only the {code} file"),
            Some((code, n)) => format!("Select only the {n} {code} files"),
            None => String::new(),
        },
        (C::Kind(_), _) => String::new(),
        (C::Smaller, _) => format!("Make {file} smaller"),
        (C::Open, _) => format!("Open {result}"),
        (C::Folder, _) => format!("Open the folder of {file}"),
        (C::More, Inspector::Several { kinds, .. }) => format!("More formats for {kinds}"),
        (C::More, _) => format!("More formats for {file}"),

        (C::Options, _) => format!("Options for {file}"),
        (C::ShowInFolder, _) => format!("Show {result} in folder"),
        (C::Retry, _) => format!("Retry {file}"),
        (C::ShowDetails, _) => format!("Show details for {file}"),
        // As the row's link says it: the helper app is what it gets.
        (C::GetHelper, _) => model.label(c).unwrap_or_default().to_owned(),
    }
}

/// The inspector's controls that its layout places, in reading order,
/// each with where it is: a format or a kind in its target row, More in
/// its line, and the buttons in their slots left to right. A control
/// that didn't fit isn't drawn, so it is neither a Tab stop nor in the
/// tree.
pub fn inspector_controls(model: &Inspector, l: &InspectorLayout) -> Vec<(InspectorControl, Rect)> {
    use InspectorControl as C;
    let mut slots = l.buttons.iter();
    model
        .controls()
        .into_iter()
        .filter_map(|c| {
            let at = match c {
                C::Target(i) | C::Kind(i) => l.targets.get(i).copied(),
                C::Smaller => l.smaller,
                C::More => l.more,
                // The value of the last fact, which is the folder.
                C::Folder => l.facts.last().map(|f| {
                    let column = layout::FACT_LABEL;
                    Rect::new(f.x + column, f.y, (f.w - column).max(0.0), f.h)
                }),
                C::Options
                | C::ShowInFolder
                | C::Retry
                | C::ShowDetails
                | C::GetHelper
                | C::Open => slots.next().copied(),
            };
            at.map(|r| (c, r))
        })
        .collect()
}

/// A row's name: the file's name, then its second line as labelled parts
/// ("File size 3.9 MB. Folder files."), each a sentence. A failed row gives
/// its whole sentence, never the line cut short to fit.
pub fn row_name(e: &Entry, instead: Option<&str>) -> String {
    let line = match (instead, &e.job) {
        (Some(s), _) => s.to_owned(),
        (None, JobState::Failed { message, .. }) => {
            message.split_whitespace().collect::<Vec<_>>().join(" ")
        }
        // A running row says what it is doing; how far along is its
        // status, told at a gentler pace than the line changes.
        (None, JobState::Running { action, .. }) => crate::model::doing(action),
        (None, JobState::Idle) => e.spoken_detail(),
        (None, _) => e.line().0,
    };
    // The row's dots read as pauses between sentences.
    format!(
        "{}. {}",
        e.name,
        line.replace(crate::model::DETAIL_SEPARATOR, ". ")
    )
}

/// The InfoBar at the top of the list: its title and message, and where
/// its parts are drawn.
#[derive(Debug, Clone)]
pub struct TopInfoIn<'a> {
    pub title: &'a str,
    pub message: &'a str,
    pub bar: layout::kit::InfoBarLayout,
}

/// The InfoBar at the top of the list: a live group named by its title,
/// its message as text, then Dismiss.
fn top_info(t: &TopInfoIn) -> Node {
    let mut children: Vec<Node> = text(Id::TopInfoText, t.message.trim(), t.bar.message)
        .into_iter()
        .collect();
    if let Some(d) = t.bar.dismiss {
        children.push(Node::new(Id::TopInfoDismiss, Role::Button, TOP_INFO_DISMISS, d).focusable());
    }
    // Named by its title, or by its message when it has none.
    let name = if t.title.is_empty() {
        t.message
    } else {
        t.title
    };
    let mut n = Node::new(Id::TopInfo, Role::Group, name, t.bar.rect).with(children);
    // Said as it comes, as every InfoBar is (spec 5.4).
    n.status = Some(crate::controls::Severity::Info.word().to_owned());
    n.live = true;
    n
}

/// The name of the Dismiss on the InfoBar at the top of the list.
pub const TOP_INFO_DISMISS: &str = "Dismiss";

/// The element for a header or footer button.
pub fn button_id(b: FocusButton) -> Id {
    match b {
        FocusButton::Convert => Id::ConvertButton,
        FocusButton::AddFiles => Id::AddFiles,
        FocusButton::Dismiss => Id::FirstRunDismiss,
        FocusButton::InfoDismiss => Id::TopInfoDismiss,
        FocusButton::Credit => Id::CreditLink,

        FocusButton::Clear => Id::Clear,
        FocusButton::Help => Id::HelpButton,
        FocusButton::Settings => Id::SettingsButton,
        FocusButton::Inspector(c) => Id::InspectorControl(c),
    }
}

/// The header or footer button an element is, if it's one.
pub fn button_of(id: &Id) -> Option<FocusButton> {
    match id {
        Id::ConvertButton => Some(FocusButton::Convert),
        Id::AddFiles => Some(FocusButton::AddFiles),
        Id::FirstRunDismiss => Some(FocusButton::Dismiss),
        Id::TopInfoDismiss => Some(FocusButton::InfoDismiss),
        Id::CreditLink => Some(FocusButton::Credit),

        Id::Clear => Some(FocusButton::Clear),
        Id::HelpButton => Some(FocusButton::Help),
        Id::SettingsButton => Some(FocusButton::Settings),
        Id::InspectorControl(c) => Some(FocusButton::Inspector(*c)),
        _ => None,
    }
}

/// A wheel item's name: a format's plain description (or, for a target
/// `text_only` says the files become by way of their text, "DOCX (text
/// only)"), or the tool or action as the wheel labels it.
pub fn item_name(item: &Item, text_only: &dyn Fn(&str) -> bool) -> String {
    match &item.action {
        Action::Convert(ext) => wheel::target_label_for(ext, text_only(ext)),
        _ => item.label.clone(),
    }
}

/// The tree: the window and everything on it a person can read or use.
/// While a panel, dialog or wheel is up, it's all there is, as it's all
/// the keys reach.
pub fn tree(input: &Input) -> Node {
    let (w, h) = input.size;
    let window = Rect::new(0.0, 0.0, w, h);
    let children = if let Some(a) = &input.ask {
        vec![ask(a)]
    } else if let Some(t) = &input.tool {
        vec![tool_dialog(t)]
    } else if let Some(s) = &input.sheet {
        vec![sheet(s)]
    } else if let Some(s) = &input.settings {
        vec![settings(s)]
    } else if let Some(d) = &input.details {
        vec![details(d)]
    } else if let Some(hi) = &input.help {
        vec![help(hi)]
    } else if let Some(wh) = &input.wheel {
        vec![menu(wh)]
    } else {
        let mut list = main(input);
        // The right-click menu pops up over the list.
        list.extend(input.row_menu.as_ref().map(row_menu));
        list
    };
    let main_view = input.ask.is_none()
        && input.tool.is_none()
        && input.sheet.is_none()
        && input.settings.is_none()
        && input.details.is_none()
        && input.help.is_none()
        && input.wheel.is_none();
    let children = std::iter::once(title_bar(&input.title, main_view))
        .chain(children)
        .collect();
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
    if let Some(a) = &input.ask {
        return Some(Id::AskButton(a.focus));
    }
    if let Some(t) = &input.tool {
        use crate::smaller::Stop;
        return Some(match t.focus {
            Stop::Radios => Id::Choice(Box::new(Id::ToolChoices), t.model.radios().focus),
            Stop::Custom => Id::ToolCustom,
            Stop::Expander => Id::ToolMore,
            Stop::Line(k) => {
                let open = t
                    .model
                    .smaller()
                    .and_then(|s| s.combo(*t.model.more_lines().get(k)?))
                    .filter(|c| c.open);
                match open {
                    Some(c) => Id::Choice(Box::new(Id::ToolLine(k)), c.highlight),
                    None => Id::ToolLine(k),
                }
            }
            Stop::Accent => Id::ToolButton(0),
            Stop::Cancel => Id::ToolButton(1),
        });
    }
    if let Some(s) = &input.sheet {
        return Some(match s.focus.and_then(|i| s.layout.controls.get(i)) {
            Some((c, _)) => match &s.list {
                // An open drop-down has the keyboard on its highlight.
                Some((k, state, _)) if k == c => {
                    Id::Choice(Box::new(Id::SheetControl(*c)), state.highlight)
                }
                _ => Id::SheetControl(*c),
            },
            None => Id::Sheet,
        });
    }
    if let Some(s) = &input.settings {
        if input.title.back_focused {
            return Some(Id::BackButton);
        }
        return Some(match s.focus {
            Some(c) => match &s.combo {
                Some((k, state, _)) if *k == c => {
                    Id::Choice(Box::new(Id::Combo(c)), state.highlight)
                }
                _ => setting_id(&s.page, c),
            },
            None => Id::Settings,
        });
    }
    if let Some(d) = &input.details {
        return Some(Id::DetailsButton(d.focus));
    }
    if let Some(hi) = &input.help {
        return Some(match hi.focus {
            HelpButton::Guide => Id::HelpGuide,
            HelpButton::Close => Id::HelpClose,
        });
    }
    if let Some(wh) = &input.wheel {
        return Some(wh.hover.map_or(Id::Wheel, Id::WheelItem));
    }
    if let Some(m) = &input.row_menu {
        // Opened by the pointer, nothing is lit: the keyboard is on its
        // first item, as on a WinUI flyout's.
        let first = m
            .items
            .iter()
            .position(|&i| i != crate::ways_in::MenuItem::Separator);
        return Some(m.lit.or(first).map_or(Id::RowMenu, Id::RowMenuItem));
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

/// The credit: its words up to the name, then the name, a link to the
/// author's page that says where it goes.
fn credit(c: &crate::credit::Credit) -> [Node; 2] {
    use crate::credit;
    let words = Rect::new(c.made.x, c.line.y, c.by.right() - c.made.x, c.line.h);
    [
        Node::new(Id::CreditText, Role::Text, credit::LEAD, words),
        Node::new(Id::CreditLink, Role::Hyperlink, credit::NAME, c.name)
            .focusable()
            .help(Some(credit::LINK_HELP)),
    ]
}

/// The first-run card: a group named by its heading, holding its steps,
/// each read with its number, the right-click line, then Dismiss. A part
/// past the bottom of the `layer` it sits on is off screen.
fn first_run_card(fr: &FirstRun, c: &layout::FirstRunCard, layer: &Rect) -> Node {
    let off = |r: &Rect| r.bottom() > layer.bottom() + 0.5;
    let mut children: Vec<Node> = fr
        .steps
        .iter()
        .zip(&c.steps)
        .enumerate()
        .map(|(k, (s, r))| (format!("{}. {s}", k + 1), *r))
        .chain([(fr.more.to_owned(), c.more)])
        .enumerate()
        .filter_map(|(k, (s, r))| {
            let mut n = text(Id::FirstRunLine(k as u8), &s, r)?;
            n.states.offscreen = off(&r);
            Some(n)
        })
        .collect();
    let mut dismiss =
        Node::new(Id::FirstRunDismiss, Role::Button, fr.dismiss, c.dismiss).focusable();
    dismiss.states.offscreen = off(&c.dismiss);
    children.push(dismiss);
    Node::new(Id::FirstRunCard, Role::Group, fr.title, c.card).with(children)
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

/// The title bar: Back, on the Tab path, while Settings is open; Help and
/// Settings, on the Tab path, while nothing covers the window
/// (`main_view`); then
/// Minimise, Maximise or Restore, and Close, which always work.
fn title_bar(t: &TitleIn, main_view: bool) -> Node {
    let mut buttons = vec![];
    if let Some(r) = t.back {
        // On the Tab path, before Settings' page.
        buttons.push(Node::new(Id::BackButton, Role::Button, "Back", r).focusable());
    }
    if main_view {
        if let Some(r) = t.help {
            buttons.push(
                Node::new(Id::HelpButton, Role::Button, "Help", r)
                    .focusable()
                    .help(Some(crate::help::HELP_TIP)),
            );
        }
        if let Some(r) = t.settings {
            buttons.push(
                Node::new(Id::SettingsButton, Role::Button, "Settings", r)
                    .focusable()
                    .help(Some(crate::help::SETTINGS_TIP)),
            );
        }
    }
    for (c, r) in Caption::ALL.into_iter().zip([t.min, t.max, t.close]) {
        buttons.push(Node::new(
            Id::CaptionButton(c),
            Role::Button,
            c.name(t.maximized),
            r,
        ));
    }
    Node::new(Id::TitleBar, Role::Group, TITLE_BAR_NAME, t.rect).with(buttons)
}

/// The window with nothing over it: the drop panel or the list and its
/// footer, under the title bar.
fn main(input: &Input) -> Vec<Node> {
    let (w, h) = input.size;
    let rows = input.rows.len();
    let shown = layout::inspector_fits(w) as u8 as f32;
    let l = layout::layout_under(w, h, rows, shown, input.title.rect.h);
    let l = match &input.top_info {
        Some(t) => l.with_top_info(t.bar.rect.h),
        None => l,
    };
    // The footer's buttons are on the Tab path, after the title bar's.
    let mut out = vec![];
    let lines = input.empty;
    if input.rows.is_empty() {
        // The InfoBar at the top of the layer is read first.
        if let Some(t) = &input.top_info {
            out.push(top_info(t));
        }
        let e = input.empty_at.unwrap_or_else(|| {
            let card = input.first_run.map(|_| layout::CardText::ONE_LINE);
            layout::empty_state(&l.panel, card)
        });
        let mut children: Vec<Node> = [
            text(Id::DropLine(0), lines.body, e.body),
            Some(
                Node::new(Id::AddFiles, Role::Button, lines.button, e.button)
                    .focusable()
                    .help(Some(crate::help::ADD_FILES_TIP)),
            ),
            text(Id::DropLine(1), input.empty_status, e.status),
        ]
        .into_iter()
        .flatten()
        .collect();
        if let (Some(fr), Some(c)) = (input.first_run, e.card) {
            children.push(first_run_card(fr, &c, &e.layer));
        }
        out.push(Node::new(Id::DropPanel, Role::Group, lines.headline, l.panel).with(children));
        // The credit in the corner, read last, as it is drawn.
        if let Some(c) = &input.credit {
            out.extend(credit(c));
        }
        return out;
    }

    // The InfoBar at the top of the list is read before the list.
    if let Some(t) = &input.top_info {
        out.push(top_info(t));
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
                input.row_links,
            )
        })
        .collect();
    out.push(
        // The list's part of the panel: all of it, or what the inspector
        // leaves.
        Node::new(Id::List, Role::List, LIST_NAME, l.list)
            .focusable()
            .with(rows),
    );
    // The inspector follows the list, as it does on the Tab path.
    if let Some(ins) = &input.inspector {
        out.push(inspector(ins));
    }
    let Some(footer) = l.footer else {
        return out;
    };
    // The footer's parts as drawn (mock screen 2): the status, Add files,
    // Clear list and Convert, read in that order, as Tab meets them.
    let f = input
        .footer_row
        .unwrap_or_else(|| layout::footer_row(&footer, false));
    let mut status = text(Id::Status, input.status, f.status);
    if let Some(s) = &mut status {
        s.live = true;
    }
    out.extend(status);
    if let (Some(what), Some(link)) = (input.undo, layout::undo_link(&f.status)) {
        out.push(Node::new(
            Id::UndoRemove,
            Role::Button,
            format!("Undo, {what}"),
            link,
        ));
    }
    // The credit under the status, then, as drawn, left to right: Add
    // files, Clear list, Convert.
    if let Some(c) = &f.credit {
        out.extend(credit(c));
    }
    out.push(
        Node::new(Id::AddFiles, Role::Button, lines.button, f.add)
            .focusable()
            .help(Some(crate::help::ADD_FILES_TIP)),
    );

    out.push(Node::new(Id::Clear, Role::Button, crate::model::CLEAR_LIST, f.clear).focusable());
    // Disabled, it says why instead of the key.
    let mut convert_node = Node::new(
        Id::ConvertButton,
        Role::Button,
        input.convert_label,
        f.convert,
    )
    .focusable()
    .help(Some(
        input.convert_block.unwrap_or(crate::help::CONVERT_TIP),
    ));
    convert_node.states.disabled = input.convert_block.is_some();
    out.push(convert_node);
    out
}

/// The inspector: its picture of the file, its lines of text and its
/// controls, in reading order. Only what its layout places is there.
fn inspector(ins: &InspectorIn) -> Node {
    let l = ins.layout;
    let mut children = vec![];
    // A picture of one file. Nothing's mark and Several's stack picture no
    // file of their own.
    let pictured = !matches!(ins.model, Inspector::Nothing | Inspector::Several { .. });
    if pictured && l.hero.h > 0.0 {
        children.push(Node::new(
            Id::InspectorPreview,
            Role::Image,
            format!("Preview of {}", ins.file),
            l.hero,
        ));
    }
    // Each text where the layout put it, as it is drawn: the title, then
    // the texts under it in the order `InspectorLayout::lines` has them.
    let line = |k: usize| l.line(k).unwrap_or_default();
    // The InfoBar's title and message, where it is placed.
    let bar = ins.model.bar();
    let bar_lines = |lines: &mut Vec<(String, Rect)>| {
        if let (Some(bar), Some(at)) = (&bar, &l.info) {
            if !bar.title.is_empty() {
                lines.push((bar.title.clone(), at.title));
            }
            if !bar.message.is_empty() {
                // Its lines read as sentences.
                let said: Vec<String> = bar
                    .message
                    .lines()
                    .map(|l| {
                        if l.ends_with('.') {
                            l.to_owned()
                        } else {
                            format!("{l}.")
                        }
                    })
                    .collect();
                lines.push((said.join(" "), at.message));
            }
        }
    };
    let mut lines: Vec<(String, Rect)> = vec![];
    match ins.model {
        Inspector::Nothing => {
            lines.push((NOTHING_TITLE.to_owned(), l.title));
            lines.push((crate::inspector::NOTHING_HINT.to_owned(), line(0)));
        }
        Inspector::Waiting {
            facts,
            reason,
            targets,
            smaller,
            ..
        } => {
            lines.push((ins.file.to_owned(), l.title));
            let first = reason.as_ref().map(|(r, _)| r.as_str());
            lines.extend(first.map(|w| (w.to_owned(), line(0))));
            for ((label, value), r) in facts.iter().zip(&l.facts) {
                // The Folder's value is a link of its own.
                let folder =
                    *label == "Folder" && ins.model.controls().contains(&InspectorControl::Folder);
                if !folder {
                    lines.push((format!("{label} {value}"), *r));
                }
            }
            if !targets.is_empty() || *smaller {
                lines.extend(l.label.map(|r| (crate::inspector::TURN_INTO.to_owned(), r)));
            }
        }
        Inspector::Converting {
            doing,
            percent,
            left,
            ..
        } => {
            lines.push((ins.file.to_owned(), l.title));
            lines.push((doing.clone(), line(0)));
            lines.extend(percent.map(|p| (format!("{p}%"), line(1))));
            lines.extend(left.as_ref().map(|t| (t.clone(), line(3))));
        }
        Inspector::Done { again, smaller, .. } => {
            lines.push((ins.file.to_owned(), l.title));
            bar_lines(&mut lines);
            if !again.is_empty() || *smaller {
                lines.extend(
                    l.label
                        .map(|r| (crate::inspector::CONVERT_AGAIN.to_owned(), r)),
                );
            }
        }
        Inspector::Failed { .. } => {
            // The name, then what happened, as it is drawn.
            lines.push((ins.file.to_owned(), l.title));
            bar_lines(&mut lines);
        }
        Inspector::Several {
            count,
            kinds,
            targets,
            offered,
            smaller,
            ..
        } => {
            lines.push((crate::inspector::files_count(*count), l.title));
            lines.push((kinds.clone(), line(0)));
            bar_lines(&mut lines);
            let heading = if !targets.is_empty() || *smaller {
                Some(crate::inspector::TURN_THEM_INTO)
            } else if !offered.is_empty() {
                Some(crate::inspector::ONE_KIND)
            } else {
                None
            };
            lines.extend(heading.zip(l.label).map(|(h, r)| (h.to_owned(), r)));
        }
    }
    // A text its part had no room for isn't drawn, so isn't read either.
    let placed = lines.into_iter().filter(|(_, r)| r.h > 0.0);
    children.extend(
        placed
            .enumerate()
            .filter_map(|(k, (name, r))| text(Id::InspectorText(k), &name, r)),
    );
    for (c, r) in inspector_controls(ins.model, l) {
        let name = inspector_name(c, ins.model, ins.file);
        let node = if c == InspectorControl::Folder {
            // Named by the folder's own name, helped by the whole path.
            Node::new(Id::InspectorControl(c), Role::Hyperlink, name, r).help(Some(ins.folder))
        } else {
            Node::new(Id::InspectorControl(c), Role::Button, name, r)
        };
        children.push(node.focusable());
    }
    Node::new(Id::Inspector, Role::Group, INSPECTOR_NAME, ins.rect).with(children)
}

/// One file's row, with its buttons. Without `links`, it has no link,
/// Cancel or "Saved to" line of its own, as it draws none.
fn row(r: &RowIn, rect: Rect, offscreen: bool, links: bool) -> Node {
    let e = r.entry;
    // Show in folder stays on the row while the side pane shows.
    let link = r.link.filter(|l| crate::model::link_on_row(l, links));
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
        {
            children.push(Node::new(
                Id::RowCancel(path.clone()),
                Role::Button,
                format!("{} {}", crate::help::STOP_TIP, e.name),
                layout::row_cancel(&rect),
            ));
        }
        // How far along, as a value a screen reader can read and watch.
        if let Some(p) = r.progress {
            let line = layout::row_bar(
                &rect,
                layout::row_text_ends(&rect, true, false, None, false, false).1,
            );
            let mut bar = Node::new(
                Id::RowProgress(path.clone()),
                Role::ProgressBar,
                format!("Progress {}", e.name),
                line,
            );
            bar.range = Some(f64::from(p));
            children.push(bar);
        }
    }
    // Options comes right after the link, which then sits before it. As
    // drawn: on any finished drop's row but one being saved.
    let options = links && e.options_for().is_some() && !r.saving;
    if let Some(link) = link {
        children.push(Node::new(
            Id::RowLink(path.clone()),
            Role::Button,
            format!("{link} {}", e.name),
            layout::row_link_at(&rect, link, options),
        ));
    }
    if links && e.retryable() && r.instead.is_none() {
        // Beside the link when the row shows one, as it is drawn.
        children.push(Node::new(
            Id::RowRetry(path.clone()),
            Role::Button,
            format!("Retry {}", e.name),
            layout::row_retry(&rect, link),
        ));
    }
    if options {
        children.push(Node::new(
            Id::RowOptions(path.clone()),
            Role::Button,
            format!("Options for {}", e.name),
            layout::row_options(&rect),
        ));
    }
    for c in &mut children {
        c.states.offscreen = offscreen;
    }
    n.with(children)
}

/// The wheel, as a menu of what it offers: the hub's words, then each
/// segment of the ring; More holds its list while it is open, each group
/// of it under its heading, and "Options…" last.
fn menu(wh: &WheelIn) -> Node {
    let w = wh.wheel;
    let s = w.scale.max(0.01);
    let (cx, cy) = w.center;
    let around = |(x, y): (f32, f32), side: f32| {
        let side = side * s;
        Rect::new(cx + x * s - side / 2.0, cy + y * s - side / 2.0, side, side)
    };
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
    let hub_words = if wh.hub.hint.is_empty() {
        wh.hub.pair.clone()
    } else {
        format!("{}, {}", wh.hub.pair, wh.hub.hint)
    };
    // More's list, while it is open, each group of it under its heading.
    let list_of = |title: &str| {
        wh.flyout.map(|(rows, place)| {
            let view = place.view();
            let mut groups: Vec<Node> = vec![];
            let mut k = 0;
            for (row, r) in rows.iter().zip(&place.rows) {
                let offscreen = r.bottom() <= view.y || r.y >= view.bottom();
                let line = match row {
                    wheel::FlyoutRow::Heading(title) => {
                        groups.push(Node::new(
                            Id::FlyoutGroup(k),
                            Role::Group,
                            (*title).to_owned(),
                            *r,
                        ));
                        k += 1;
                        continue;
                    }
                    wheel::FlyoutRow::Item(spot, item) => {
                        spot_item(*spot, item_name(item, wh.text_only), *r)
                    }
                    // Always usable: it names the format its dialog is for.
                    wheel::FlyoutRow::Options(ext) => {
                        spot_item(Spot::Options, wheel::options_line(ext), *r)
                    }
                };
                let mut line = line;
                line.states.offscreen = offscreen;
                match (row, groups.last_mut()) {
                    (wheel::FlyoutRow::Item(..), Some(g)) => g.children.push(line),
                    _ => groups.push(line),
                }
            }
            Node::new(Id::Flyout, Role::Menu, title.to_owned(), place.rect).with(groups)
        })
    };
    // The list alone is one menu: the formats and tools, in their groups.
    if w.list_only {
        let list = list_of("Formats and tools");
        let rect = list
            .as_ref()
            .map_or(Rect::new(cx, cy, 0.0, 0.0), |l| l.rect);
        return Node::new(Id::Wheel, Role::Menu, name, rect)
            .focusable()
            .with(list.into_iter().collect());
    }
    let mut items = vec![Node::new(
        Id::WheelHub,
        Role::Text,
        hub_words,
        around((0.0, 0.0), 2.0 * wheel::R_HUB),
    )];
    items.extend(w.items.iter().enumerate().map(|(i, item)| {
        spot_item(
            Spot::Inner(i),
            item_name(item, wh.text_only),
            around(wheel::label_point(i, n), R_OUTER - R_INNER),
        )
    }));
    if w.has_more() {
        let i = w.items.len();
        let list = list_of("More");
        items.push(
            spot_item(
                Spot::Inner(i),
                "More".to_owned(),
                around(wheel::label_point(i, n), R_OUTER - R_INNER),
            )
            .with(list.into_iter().collect()),
        );
    }
    Node::new(
        Id::Wheel,
        Role::Menu,
        name,
        around((0.0, 0.0), 2.0 * R_OUTER),
    )
    .focusable()
    .with(items)
}

/// The tree's id for Settings' control `c`: a ComboBox's, a
/// ToggleSwitch's, or the button's, radio button's or Expander's own.
pub fn setting_id(page: &crate::settings_page::PageSpec, c: Control) -> Id {
    use crate::settings_page::Body;
    match page.cards().map(|k| &k.body).find(|b| match b {
        Body::Combo { control, .. } | Body::Toggle { control, .. } => *control == c,
        _ => false,
    }) {
        Some(Body::Combo { .. }) => Id::Combo(c),
        Some(Body::Toggle { .. }) => Id::Toggle(c),
        _ => Id::Setting(c),
    }
}

/// The Settings page: its title, then each section, a group of its cards.
/// A card is a group named by its label, its caption as its help, holding
/// its control; a helper app's card is its Expander, its status as its
/// item status. Anything scrolled out of the page's view is off screen.
fn settings(s: &SettingsIn) -> Node {
    use crate::a11y::kit as lk;
    use crate::settings_page::Body;
    let l = &s.layout;
    let off = |r: &Rect| r.bottom() <= l.view.y || r.y >= l.view.bottom();
    let rect_of = |c: Control| l.find(c).map(|i| l.controls[i].1).unwrap_or_default();
    let focused = |c: Control| s.focus == Some(c);
    let group = |spec: &crate::settings_page::CardSpec, r: Rect, kids: Vec<Node>| {
        Node::new(Id::SettingsCard(spec.card), Role::Group, &spec.label, r).with(kids)
    };
    let mut children = vec![];
    let mut title = Node::new(
        Id::SettingsTitle,
        Role::Text,
        crate::settings_page::TITLE,
        l.title,
    );
    title.states.offscreen = off(&l.title);
    children.push(title);
    let mut cards = l.cards.iter().zip(s.page.cards());
    for ((section, specs), (_, heading)) in s.page.sections.iter().zip(&l.sections) {
        let mut nodes = vec![];
        for _ in specs {
            let Some((card, spec)) = cards.next() else {
                break;
            };
            let mut node = match &spec.body {
                Body::Combo {
                    control,
                    choices,
                    value,
                    ..
                } => {
                    let names: Vec<&str> = choices.iter().map(String::as_str).collect();
                    let open = s.combo.as_ref().filter(|(k, _, _)| k == control);
                    let state = open.map_or_else(
                        || crate::controls::ComboState::new(names.len(), *value),
                        |(_, st, _)| *st,
                    );
                    let combo = lk::combo_box(
                        Id::Combo(*control),
                        &spec.label,
                        &names,
                        &state,
                        rect_of(*control),
                        open.map(|(_, _, list)| list),
                        focused(*control),
                    );
                    group(spec, card.rect, vec![combo])
                }
                Body::Toggle { control, on } => {
                    let t = lk::toggle_switch(
                        Id::Toggle(*control),
                        &spec.label,
                        *on,
                        rect_of(*control),
                        false,
                        focused(*control),
                    );
                    group(spec, card.rect, vec![t])
                }
                Body::Radios { setting, value } => {
                    let radios = kiln_core::Preset::ALL
                        .iter()
                        .map(|p| {
                            let c = Control::Preset(*setting, *p);
                            let mut n =
                                Node::new(Id::Setting(c), Role::RadioButton, p.label(), rect_of(c))
                                    .focusable()
                                    .selected(*value == Some(*p));
                            n.states.focused = focused(c);
                            n
                        })
                        .collect();
                    group(spec, card.rect, radios)
                }
                Body::Buttons(buttons) => {
                    let nodes = buttons
                        .iter()
                        .map(|(c, label)| {
                            lk::button(Id::Setting(*c), label, rect_of(*c), false, focused(*c))
                        })
                        .collect();
                    group(spec, card.rect, nodes)
                }
                Body::Expander(e) => {
                    let c = Control::Helper(e.helper);
                    let mut content = vec![];
                    if let (Some(label), Some(r)) = (e.line_label, card.line_label) {
                        content.extend(text(Id::SettingsLine(e.helper, 2), label, r));
                    }
                    if let Some(r) = card.line {
                        content.extend(text(Id::SettingsLine(e.helper, 0), &e.line, r));
                    }
                    if let (Some(n), Some(r)) = (&e.line_note, card.line_note) {
                        content.extend(text(Id::SettingsLine(e.helper, 1), n, r));
                    }
                    content.extend(e.buttons.iter().map(|(b, label)| {
                        // Its words are the same on every helper, so the
                        // name says which one it installs.
                        let name = match b {
                            Control::Winget(engine) => {
                                format!("Install {} with winget", engine.label())
                            }
                            _ => label.clone(),
                        };
                        lk::button(Id::Setting(*b), &name, rect_of(*b), false, focused(*b))
                    }));
                    let mut n = lk::expander(
                        Id::Setting(c),
                        &spec.label,
                        e.open,
                        card.header,
                        content,
                        focused(c),
                    );
                    n.status = e.badge.map(|b| b.text.to_owned());
                    n
                }
            };
            node = node.help(spec.note.as_deref());
            // Narrator reads a focused control's own HelpText, not its
            // card's: the note goes on each control the card holds, as
            // WinUI's SettingsCard gives its Description. An Expander is
            // the card itself.
            if !matches!(spec.body, Body::Expander(_)) {
                for c in node.children.iter_mut().filter(|c| c.states.focusable) {
                    c.help.clone_from(&node.help);
                }
            }
            mark_off(&mut node, &off);
            nodes.push(node);
        }
        let rect = union(nodes.iter().map(|n| n.rect).chain([*heading]));
        let mut g = Node::new(
            Id::SettingsSection(*section),
            Role::Group,
            section.label(),
            rect,
        )
        .with(nodes);
        g.states.offscreen = off(&rect);
        children.push(g);
    }
    Node::new(
        Id::Settings,
        Role::Group,
        crate::settings_page::TITLE,
        l.view,
    )
    .with(children)
}

/// Mark `n` and everything under it off screen where `off` says so.
fn mark_off(n: &mut Node, off: &dyn Fn(&Rect) -> bool) {
    n.states.offscreen = n.states.offscreen || off(&n.rect);
    for c in &mut n.children {
        mark_off(c, off);
    }
}

/// One of Kiln's prompts: a modal dialog named by its title, its text,
/// then its buttons in order. Its text is its help text too, so Narrator
/// reads what is at stake with its name as the focus enters it.
fn ask(a: &AskIn) -> Node {
    let l = &a.layout;
    let mut children: Vec<Node> = a
        .text
        .and_then(|t| text(Id::AskText, t, l.content))
        .into_iter()
        .collect();
    children.extend(
        a.buttons
            .iter()
            .zip(&l.buttons)
            .enumerate()
            .map(|(k, (name, r))| kit::button(Id::AskButton(k), name, *r, false, false)),
    );
    let mut d = Node::new(Id::Ask, Role::Dialog, a.title, l.rect)
        .help(a.text)
        .with(children);
    d.states.modal = true;
    d
}

/// The Make smaller or Resize dialog: a modal dialog named by its title,
/// its sub-line, its choices as RadioButtons (those that would do nothing
/// disabled, saying so as their help text), Custom's box while Custom is
/// picked, the estimate's lines, More options with its lines while open,
/// then its accent button and Cancel. A part scrolled out of its content
/// is off screen.
fn tool_dialog(t: &ToolIn) -> Node {
    let l = &t.layout;
    let d = t.model;
    let content = l.frame.content;
    let off = |r: &Rect| r.bottom() <= content.y || r.y >= content.bottom();
    let shown = |mut n: Node| {
        n.states.offscreen = off(&n.rect);
        n
    };
    let mut children: Vec<Node> = vec![];
    let sub = d.sub_line();
    children.extend(text(Id::ToolText(0), &sub, l.sub).map(shown));
    let (heading, _) = d.heading();
    children.extend(text(Id::ToolText(1), heading, l.heading).map(shown));
    let labels = d.labels();
    let mut group = kit::radio_group(
        Id::ToolChoices,
        heading,
        &labels,
        d.radios(),
        &l.radios,
        t.focus == crate::smaller::Stop::Radios,
    );
    for (i, c) in group.children.iter_mut().enumerate() {
        c.states.disabled = d.disabled(i);
        c.help = d.tail(i).map(str::to_owned);
        c.states.offscreen = off(&c.rect);
    }
    children.push(group);
    if let Some((b, _)) = l.custom {
        let mut n = kit::text_box(
            Id::ToolCustom,
            d.custom_name(),
            d.custom(),
            b,
            false,
            t.focus == crate::smaller::Stop::Custom,
        );
        n.states.offscreen = off(&b);
        children.push(n);
    }
    if let (Some(hint), Some(r)) = (d.hint(), l.hint) {
        children.extend(text(Id::ToolText(4), hint, r).map(shown));
    }
    if let (Some((bold, detail)), Some((_, br, dr))) = (&t.estimate, l.card) {
        // The estimate changes as it is worked out: said as it comes.
        children.extend(text(Id::ToolText(2), bold, br).map(shown).map(|mut n| {
            n.live = true;
            n
        }));
        children.extend(text(Id::ToolText(3), detail, dr).map(shown));
    }
    if let (Some(e), Some(s)) = (l.expander, d.smaller()) {
        let lines = l
            .lines
            .iter()
            .enumerate()
            .map(|(k, p)| {
                let focused = t.focus == crate::smaller::Stop::Line(k);
                let mut n = if p.line.is_toggle() {
                    kit::toggle_switch(
                        Id::ToolLine(k),
                        p.line.label(),
                        s.line_on(p.line),
                        p.control,
                        false,
                        focused,
                    )
                } else {
                    let choices = s.line_choices(p.line);
                    let refs: Vec<&str> = choices.iter().map(String::as_str).collect();
                    let combo = s
                        .combo(p.line)
                        .copied()
                        .unwrap_or_else(|| crate::controls::ComboState::new(choices.len(), 0));
                    kit::combo_box(
                        Id::ToolLine(k),
                        p.line.label(),
                        &refs,
                        &combo,
                        p.control,
                        l.list.as_ref(),
                        focused,
                    )
                };
                n.help = p.line.note().map(str::to_owned);
                n.states.offscreen = off(&p.control);
                n
            })
            .collect();
        let mut n = kit::expander(
            Id::ToolMore,
            "More options",
            s.more.open,
            e.header,
            lines,
            t.focus == crate::smaller::Stop::Expander,
        );
        n.states.offscreen = off(&e.header);
        children.push(n);
    }
    let b = &l.frame.buttons;
    children.push(kit::button(
        Id::ToolButton(0),
        d.title(),
        b[0],
        !d.can_run(),
        false,
    ));
    children.push(kit::button(Id::ToolButton(1), "Cancel", b[1], false, false));
    let mut n = Node::new(Id::Tool, Role::Dialog, d.title(), l.frame.rect)
        .help(Some(&sub))
        .with(children);
    n.states.modal = true;
    n
}

/// The details dialog: what a failed job's engine said, Copy and Close.
/// A modal ContentDialog: closing it, through its Window pattern, is
/// Close.
fn details(d: &DetailsIn) -> Node {
    let l = &d.layout;
    let b = &l.frame.buttons;
    let children = [
        text(Id::DetailsLead, d.lead, l.lead),
        text(Id::DetailsText, d.text, l.text),
        Some(
            Node::new(
                Id::DetailsButton(DialogButton::Copy),
                Role::Button,
                d.copy,
                b[0],
            )
            .focusable(),
        ),
        Some(
            Node::new(
                Id::DetailsButton(DialogButton::Close),
                Role::Button,
                "Close",
                b[1],
            )
            .focusable(),
        ),
    ];
    let mut n = Node::new(Id::Details, Role::Dialog, d.title, l.frame.rect)
        .with(children.into_iter().flatten().collect());
    n.states.modal = true;
    n
}

/// Help: a modal dialog holding each section with its lines, then Show
/// first-run help (a hyperlink) and Close.
fn help(hi: &HelpIn) -> Node {
    let mut children: Vec<Node> = hi
        .sections
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
                    let place = hi.places.iter().find(|p| (p.section, p.line) == (k, m));
                    let mut n = text(Id::HelpLine(k, m), &said, place.map_or(hi.body, |p| p.rect))?;
                    n.states.offscreen = place.is_some_and(|p| p.offscreen);
                    Some(n)
                })
                .collect();
            // Where its lines are: two sections never share one box.
            let mine: Vec<Rect> = hi
                .places
                .iter()
                .filter(|p| p.section == k)
                .map(|p| p.rect)
                .collect();
            let rect = if mine.is_empty() {
                hi.body
            } else {
                union(mine)
            };
            Node::new(Id::HelpSection(k), Role::Group, s.title, rect).with(lines)
        })
        .collect();
    let mut guide = kit::hyperlink(
        Id::HelpGuide,
        crate::ways_in::SHOW_FIRST_RUN,
        hi.guide,
        false,
    );
    guide.states.offscreen = hi.guide_offscreen;
    children.push(guide);
    children.push(Node::new(Id::HelpClose, Role::Button, "Close", hi.close).focusable());
    let mut n = Node::new(Id::Help, Role::Dialog, crate::help::TITLE, hi.frame).with(children);
    n.states.modal = true;
    n
}

/// The options sheet: what it converts, what the conversion leaves out, a
/// group or spinner for each knob, the result, then Cancel and Convert.
fn sheet(s: &SheetIn) -> Node {
    let l = s.layout;
    let focused = s.focus.and_then(|i| l.controls.get(i)).map(|(c, _)| *c);
    let content = l.frame.content;
    let off = |r: &Rect| r.bottom() <= content.y || r.y >= content.bottom();
    let shown = |mut n: Node| {
        n.states.offscreen = off(&n.rect);
        n
    };
    let mut children: Vec<Node> = text(Id::SheetLine(0), s.sub, l.sub)
        .into_iter()
        .map(shown)
        .collect();
    // After the lines 0 to 4 the dialog already has.
    children.extend(s.notes.iter().enumerate().filter_map(|(k, note)| {
        let line = u8::try_from(5 + k).ok()?;
        text(Id::SheetLine(line), note, l.note_line(k)).map(shown)
    }));
    // Quality: a group of radio buttons, each with SelectionItem.
    let radios: Vec<Node> = l
        .controls
        .iter()
        .filter(|(c, _)| c.is_radio())
        .enumerate()
        .map(|(i, &(c, r))| {
            let name = s
                .radios
                .get(i)
                .copied()
                .unwrap_or(["Small", "High", "Best"][i.min(2)]);
            let mut n = Node::new(Id::SheetControl(c), Role::RadioButton, name, r)
                .focusable()
                .selected(s.selected == Some(i));
            n.states.focused = focused == Some(c);
            shown(n)
        })
        .collect();
    if let (Some(h), false) = (l.heading, radios.is_empty()) {
        let rect = union(std::iter::once(h).chain(radios.iter().map(|n| n.rect)));
        children.push(shown(
            Node::new(Id::SheetRow(0), Role::Group, "Quality", rect).with(radios),
        ));
    }
    // The estimate: said as it changes.
    if let Some(mut line) = text(Id::SheetLine(2), s.result, l.result) {
        line.live = true;
        if s.error {
            line.status = Some(crate::controls::Severity::Critical.word().to_owned());
        }
        children.push(shown(line));
    }
    if let Some(e) = l.expander {
        let lines: Vec<Node> = l
            .lines
            .iter()
            .zip(s.lines)
            .map(|(&(c, _, r), line)| {
                // A drop-down is a ComboBox, its list's items under it
                // while it is open; a spin box a Spinner.
                let mut n = match &line.choices {
                    Some(choices) => {
                        let names: Vec<&str> = choices.iter().map(String::as_str).collect();
                        let open = s.list.as_ref().filter(|(k, ..)| *k == c);
                        let now = ComboState::new(
                            names.len(),
                            names.iter().position(|v| *v == line.value).unwrap_or(0),
                        );
                        let (state, list) = match open {
                            Some((_, state, list)) => (*state, Some(list)),
                            None => (now, None),
                        };
                        kit::combo_box(
                            Id::SheetControl(c),
                            &line.label,
                            &names,
                            &state,
                            r,
                            list,
                            focused == Some(c),
                        )
                    }
                    None => {
                        let mut n = Node::new(Id::SheetControl(c), Role::Spinner, &line.label, r)
                            .focusable();
                        n.value = Some(spoken(&line.value));
                        n.states.focused = focused == Some(c);
                        n
                    }
                };
                n.states.disabled = !line.enabled;
                shown(n)
            })
            .collect();
        let c = SheetControl::Advanced;
        let mut n = kit::expander(
            Id::SheetControl(c),
            "More options",
            s.more_open,
            e.header,
            lines,
            focused == Some(c),
        );
        n.states.offscreen = off(&e.header);
        children.push(n);
    }
    for &(c, r) in &l.controls {
        let (name, disabled) = match c {
            SheetControl::Convert => ("Convert", !s.convert_enabled),
            SheetControl::Cancel => ("Cancel", false),
            _ => continue,
        };
        children.push(kit::button(
            Id::SheetControl(c),
            name,
            r,
            disabled,
            focused == Some(c),
        ));
    }
    let mut n = Node::new(Id::Sheet, Role::Dialog, s.title, l.panel).with(children);
    n.states.modal = true;
    n
}

/// UI Automation nodes for phase 3's controls (`crate::controls`), named
/// and stated as WinUI's are. The surfaces that hold the controls put
/// these in their trees with ids of their own.
pub mod kit {
    use super::{Id, Node, Role};
    use crate::controls::{ComboState, MenuState, Radios, Severity, TextBoxState};
    use crate::layout::kit::{FlyoutLayout, RadioParts};
    use crate::layout::Rect;

    fn stated(mut n: Node, focused: bool, disabled: bool) -> Node {
        n.states.focusable = true;
        n.states.focused = focused;
        n.states.disabled = disabled;
        n
    }

    /// A Button: standard, accent or subtle, all the same to a screen
    /// reader.
    pub fn button(id: Id, name: &str, rect: Rect, disabled: bool, focused: bool) -> Node {
        stated(Node::new(id, Role::Button, name, rect), focused, disabled)
    }

    /// A HyperlinkButton.
    pub fn hyperlink(id: Id, name: &str, rect: Rect, focused: bool) -> Node {
        stated(Node::new(id, Role::Hyperlink, name, rect), focused, false)
    }

    /// A ToggleSwitch named for what it turns on, on or off.
    pub fn toggle_switch(
        id: Id,
        name: &str,
        on: bool,
        rect: Rect,
        disabled: bool,
        focused: bool,
    ) -> Node {
        let mut n = stated(Node::new(id, Role::Button, name, rect), focused, disabled);
        n.states.toggled = Some(on);
        n
    }

    /// RadioButtons: a group named `name` of one radio button per label,
    /// the picked one selected, the focus on `r.focus` while the group has
    /// it.
    pub fn radio_group(
        id: Id,
        name: &str,
        labels: &[&str],
        r: &Radios,
        rows: &[RadioParts],
        focused: bool,
    ) -> Node {
        let children = labels
            .iter()
            .zip(rows)
            .enumerate()
            .map(|(i, (label, parts))| {
                let mut b = Node::new(
                    Id::Choice(Box::new(id.clone()), i),
                    Role::RadioButton,
                    *label,
                    parts.row,
                );
                b.states.selected = r.selected == Some(i);
                stated(b, focused && r.focus == i, false)
            })
            .collect();
        let bounds = rows.iter().fold(None::<Rect>, |acc, p| {
            Some(match acc {
                None => p.row,
                Some(a) => {
                    let (x, y) = (a.x.min(p.row.x), a.y.min(p.row.y));
                    let (rr, b) = (a.right().max(p.row.right()), a.bottom().max(p.row.bottom()));
                    Rect::new(x, y, rr - x, b - y)
                }
            })
        });
        Node::new(id, Role::Group, name, bounds.unwrap_or_default()).with(children)
    }

    /// A ComboBox: its value; while open, the choices shown as list items,
    /// the value selected and the highlight focused.
    pub fn combo_box(
        id: Id,
        name: &str,
        choices: &[&str],
        c: &ComboState,
        rect: Rect,
        list: Option<&FlyoutLayout>,
        focused: bool,
    ) -> Node {
        let open = c.open && list.is_some();
        let mut n = stated(
            Node::new(id.clone(), Role::ComboBox, name, rect),
            focused && !open,
            false,
        );
        n.value = choices.get(c.value).map(|s| (*s).to_owned());
        n.states.expanded = Some(open);
        if let (true, Some(list)) = (open, list) {
            n.children = choices
                .iter()
                .enumerate()
                .map(|(i, choice)| {
                    let shown = i
                        .checked_sub(list.first)
                        .and_then(|k| list.items.get(k).copied());
                    let mut item = Node::new(
                        Id::Choice(Box::new(id.clone()), i),
                        Role::ListItem,
                        *choice,
                        shown.unwrap_or_default(),
                    );
                    item.states.selected = i == c.value;
                    item.states.offscreen = shown.is_none();
                    stated(item, focused && i == c.highlight, false)
                })
                .collect();
        }
        n
    }

    /// A text box: its text as its value, which can be set.
    pub fn text_box(
        id: Id,
        name: &str,
        t: &TextBoxState,
        rect: Rect,
        disabled: bool,
        focused: bool,
    ) -> Node {
        let mut n = stated(Node::new(id, Role::Edit, name, rect), focused, disabled);
        n.value = Some(t.text().to_owned());
        n.text_rule = Some(t.rule());
        n
    }

    /// An Expander: a group named by its header that opens and closes,
    /// with its content's nodes while open.
    pub fn expander(
        id: Id,
        header: &str,
        open: bool,
        rect: Rect,
        content: Vec<Node>,
        focused: bool,
    ) -> Node {
        let mut n = stated(Node::new(id, Role::Group, header, rect), focused, false);
        n.states.expanded = Some(open);
        if open {
            n.children = content;
        }
        n
    }

    /// A ProgressBar: its percent, or no value while it can't tell.
    pub fn progress(id: Id, name: &str, fraction: Option<f32>, rect: Rect) -> Node {
        let mut n = Node::new(id, Role::ProgressBar, name, rect);
        n.range = fraction.map(|f| f64::from(super::percent(f)));
        n
    }

    /// An InfoBar: a group named by its title (or its message), its
    /// severity as its status, announced as it appears, holding its title,
    /// its message and its buttons.
    pub fn info_bar(
        id: Id,
        severity: Severity,
        title: &str,
        message: &str,
        rect: Rect,
        buttons: Vec<Node>,
    ) -> Node {
        let name = if title.is_empty() { message } else { title };
        let mut n = Node::new(id.clone(), Role::Group, name, rect);
        n.status = Some(severity.word().to_owned());
        n.live = true;
        let lines = [title, message].into_iter().filter(|s| !s.is_empty());
        n.children = lines
            .enumerate()
            .map(|(i, s)| Node::new(Id::Choice(Box::new(id.clone()), i), Role::Text, s, rect))
            .chain(buttons)
            .collect();
        n
    }

    /// One item of a menu: its words, whether it can be used, and its
    /// shortcut.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct MenuItem<'a> {
        pub label: &'a str,
        pub enabled: bool,
        pub shortcut: Option<&'static str>,
    }

    /// A menu flyout of `items` laid in `rows` (separators have none), the
    /// lit item focused.
    pub fn menu(id: Id, items: &[MenuItem], m: &MenuState, rect: Rect, rows: &[Rect]) -> Node {
        let children = items
            .iter()
            .enumerate()
            .map(|(i, it)| {
                let mut n = Node::new(
                    Id::Choice(Box::new(id.clone()), i),
                    Role::MenuItem,
                    it.label,
                    rows.get(i).copied().unwrap_or_default(),
                );
                n.shortcut = it.shortcut;
                stated(n, m.highlight == Some(i), !it.enabled)
            })
            .collect();
        Node::new(id, Role::Menu, "", rect).with(children)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::a11y::Patterns;
        use crate::layout::kit;

        fn r() -> Rect {
            Rect::new(0.0, 0.0, 100.0, 32.0)
        }

        fn only(p: Patterns) -> Patterns {
            p
        }

        #[test]
        fn a_button_is_invoked() {
            let b = button(Id::AddFiles, "Add files", r(), false, true);
            assert_eq!(b.role, Role::Button);
            assert_eq!(b.name, "Add files");
            assert!(b.states.focusable && b.states.focused && !b.states.disabled);
            assert_eq!(
                only(b.patterns()),
                Patterns {
                    invoke: true,
                    ..Patterns::default()
                }
            );
            let off = button(Id::AddFiles, "Add files", r(), true, false);
            assert!(off.states.disabled);
            let link = hyperlink(Id::HelpGuide, "Show first-run help", r(), false);
            assert_eq!(link.role, Role::Hyperlink);
            assert!(link.patterns().invoke);
        }

        #[test]
        fn a_toggle_switch_toggles_and_says_on_or_off() {
            let t = toggle_switch(Id::AddFiles, "Start with Windows", true, r(), false, true);
            assert_eq!(t.role, Role::Button);
            assert_eq!(t.name, "Start with Windows");
            assert_eq!(t.states.toggled, Some(true));
            let p = t.patterns();
            assert!(p.toggle && !p.invoke, "toggled, never invoked");
            assert!(t.states.focused);
            let off = toggle_switch(Id::AddFiles, "x", false, r(), true, false);
            assert_eq!(off.states.toggled, Some(false));
            assert!(off.states.disabled);
        }

        #[test]
        fn radio_buttons_are_a_group_with_one_selected() {
            let rows = kit::radio_rows(0.0, 0.0, &[40.0, 40.0, 40.0]);
            let mut radios = Radios::new(3, Some(1));
            let g = radio_group(
                Id::AddFiles,
                "Max size",
                &["10 MB", "25 MB", "50 MB"],
                &radios,
                &rows,
                true,
            );
            assert_eq!(g.role, Role::Group);
            assert_eq!(g.name, "Max size");
            assert!(g.patterns().selection);
            let names: Vec<&str> = g.children.iter().map(|c| c.name.as_str()).collect();
            assert_eq!(names, ["10 MB", "25 MB", "50 MB"]);
            for (i, c) in g.children.iter().enumerate() {
                assert_eq!(c.role, Role::RadioButton);
                assert_eq!(c.id, Id::Choice(Box::new(Id::AddFiles), i));
                assert_eq!(c.states.selected, i == 1);
                assert_eq!(c.states.focused, i == 1);
                assert_eq!(c.rect, rows[i].row);
                assert!(c.patterns().selection_item);
            }
            // Without the focus, none is focused.
            radios.focus = 2;
            let g = radio_group(
                Id::AddFiles,
                "Max size",
                &["a", "b", "c"],
                &radios,
                &rows,
                false,
            );
            assert!(g.children.iter().all(|c| !c.states.focused));
        }

        #[test]
        fn a_combo_box_has_a_value_and_lists_its_choices_while_open() {
            let choices = ["Light", "Dark", "Use Windows' setting"];
            let mut c = ComboState::new(3, 2);
            let shut = combo_box(Id::AddFiles, "Theme", &choices, &c, r(), None, true);
            assert_eq!(shut.role, Role::ComboBox);
            assert_eq!(shut.name, "Theme");
            assert_eq!(shut.value.as_deref(), Some("Use Windows' setting"));
            assert_eq!(shut.states.expanded, Some(false));
            assert!(shut.children.is_empty());
            let p = shut.patterns();
            assert!(p.expand_collapse && p.value && p.selection && !p.invoke);
            assert!(shut.states.focused);
            c.click();
            c.key(crate::controls::Press::key(crate::controls::Key::Up));
            let bounds = Rect::new(0.0, 0.0, 800.0, 600.0);
            let list = kit::combo_flyout(r(), 3, 2, bounds);
            let open = combo_box(Id::AddFiles, "Theme", &choices, &c, r(), Some(&list), true);
            assert_eq!(open.states.expanded, Some(true));
            assert_eq!(open.children.len(), 3);
            for (i, item) in open.children.iter().enumerate() {
                assert_eq!(item.role, Role::ListItem);
                assert_eq!(item.name, choices[i]);
                assert_eq!(item.states.selected, i == 2, "the value");
                assert_eq!(item.states.focused, i == 1, "the highlight");
                assert_eq!(item.rect, list.items[i]);
            }
            assert!(!open.states.focused, "the focus is in the list");
            // Choices past what the list shows are off screen.
            let long: Vec<String> = (0..40).map(|i| format!("{i}")).collect();
            let long: Vec<&str> = long.iter().map(String::as_str).collect();
            let mut c = ComboState::new(40, 30);
            c.click();
            let small = Rect::new(0.0, 0.0, 800.0, 200.0);
            let list = kit::combo_flyout(r(), 40, 30, small);
            let n = combo_box(Id::AddFiles, "n", &long, &c, r(), Some(&list), true);
            assert_eq!(n.children.len(), 40);
            let shown = n.children.iter().filter(|c| !c.states.offscreen).count();
            assert_eq!(shown, list.items.len());
            assert!(!n.children[30].states.offscreen);
        }

        #[test]
        fn a_text_box_is_an_edit_with_a_settable_value() {
            let t = TextBoxState::new("15", 4, true);
            let n = text_box(Id::AddFiles, "Custom size in MB", &t, r(), false, true);
            assert_eq!(n.role, Role::Edit);
            assert_eq!(n.name, "Custom size in MB");
            assert_eq!(n.value.as_deref(), Some("15"));
            let p = n.patterns();
            assert!(p.value && !p.invoke && !p.toggle);
            assert!(n.states.focusable && n.states.focused);
        }

        #[test]
        fn an_expander_opens_and_closes_and_holds_its_content_while_open() {
            let inner = vec![button(Id::Clear, "Clear", r(), false, false)];
            let shut = expander(
                Id::AddFiles,
                "More options",
                false,
                r(),
                inner.clone(),
                true,
            );
            assert_eq!(shut.role, Role::Group);
            assert_eq!(shut.name, "More options");
            assert_eq!(shut.states.expanded, Some(false));
            assert!(shut.children.is_empty(), "nothing inside while shut");
            assert!(shut.patterns().expand_collapse);
            assert!(shut.states.focusable && shut.states.focused);
            let open = expander(Id::AddFiles, "More options", true, r(), inner, false);
            assert_eq!(open.states.expanded, Some(true));
            assert_eq!(open.children.len(), 1);
        }

        #[test]
        fn a_progress_bar_gives_its_percent_unless_it_cant_tell() {
            let p = progress(Id::AddFiles, "Converting a.mp4", Some(0.42), r());
            assert_eq!(p.role, Role::ProgressBar);
            assert_eq!(p.range, Some(42.0));
            assert!(p.patterns().range_value);
            let busy = progress(Id::AddFiles, "Converting a.mp4", None, r());
            assert_eq!(busy.range, None);
            assert!(!busy.patterns().range_value);
        }

        #[test]
        fn an_infobar_is_named_and_says_its_severity() {
            let close = button(Id::Clear, "Close", r(), false, false);
            let n = info_bar(
                Id::AddFiles,
                Severity::Critical,
                "Couldn't read this PDF",
                "It looks damaged.",
                r(),
                vec![close],
            );
            assert_eq!(n.role, Role::Group);
            assert_eq!(n.name, "Couldn't read this PDF");
            assert_eq!(n.status.as_deref(), Some("Error"));
            assert!(n.live, "announced as it appears");
            let kids: Vec<(Role, &str)> = n
                .children
                .iter()
                .map(|c| (c.role, c.name.as_str()))
                .collect();
            assert_eq!(
                kids,
                [
                    (Role::Text, "Couldn't read this PDF"),
                    (Role::Text, "It looks damaged."),
                    (Role::Button, "Close"),
                ]
            );
            let untitled = info_bar(
                Id::AddFiles,
                Severity::Info,
                "",
                "Plain words.",
                r(),
                vec![],
            );
            assert_eq!(untitled.name, "Plain words.");
            assert_eq!(untitled.status.as_deref(), Some("Information"));
            assert_eq!(untitled.children.len(), 1);
        }

        #[test]
        fn a_menu_lists_its_items_with_the_lit_one_focused() {
            let items = [
                MenuItem {
                    label: "Show in folder",
                    enabled: true,
                    shortcut: None,
                },
                MenuItem {
                    label: "Copy path",
                    enabled: false,
                    shortcut: Some("Ctrl+Shift+C"),
                },
                MenuItem {
                    label: "Remove from list",
                    enabled: true,
                    shortcut: Some("Delete"),
                },
            ];
            let mut m = MenuState::new(vec![true, false, true], false);
            m.highlight = Some(2);
            let rows = [r(), r(), r()];
            let n = menu(Id::AddFiles, &items, &m, r(), &rows);
            assert_eq!(n.role, Role::Menu);
            assert_eq!(n.children.len(), 3);
            for (i, c) in n.children.iter().enumerate() {
                assert_eq!(c.role, Role::MenuItem);
                assert_eq!(c.name, items[i].label);
                assert_eq!(c.states.disabled, !items[i].enabled);
                assert_eq!(c.states.focused, i == 2);
                assert_eq!(c.shortcut, items[i].shortcut);
                assert!(c.patterns().invoke);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::help::help_content;
    use crate::inspector::{Inspector, Kept, Target};
    use crate::layout::InspectorShape;
    use crate::model::Saved;
    use crate::ways_in::empty_lines;
    use crate::wheel::{Action, WheelKind};

    fn entry(name: &str, job: JobState) -> Entry {
        Entry {
            path: PathBuf::from(r"C:\photos").join(name),
            name: name.to_owned(),
            folder: r"C:\photos".to_owned(),
            code: "PNG".to_owned(),
            size: 3_100_000,
            modified: None,
            targets: vec!["jpg".into(), "webp".into()],
            job,
            facts: None,
            suggestion: None,
            facts_at: None,
            last: None,
            from_drag: false,
            check: kiln_core::FileCheck::Ok,
            missing: None,
            save_failed: false,
            job_note: None,
            result_bytes: None,
            waits_on: None,
            ffmpeg_found: true,
        }
    }

    fn rows(entries: &[Entry]) -> Vec<RowIn<'_>> {
        entries
            .iter()
            .map(|e| RowIn {
                entry: e,
                selected: false,
                instead: None,
                saving: false,
                link: e.link(false),
                progress: match &e.job {
                    JobState::Running { fraction, .. } => fraction.map(percent),
                    _ => None,
                },
            })
            .collect()
    }

    /// The title bar of an 800-unit window at 100%, as `win::titlebar`
    /// lays it out: Back in Settings, else Help and Settings.
    fn title_in(settings: bool, maximized: bool) -> TitleIn {
        let w = 800.0;
        let cap = |i: f32| Rect::new(w - 46.0 * i, 0.0, 46.0, 48.0);
        TitleIn {
            rect: Rect::new(0.0, 0.0, w, 48.0),
            back: settings.then(|| Rect::new(4.0, 8.0, 40.0, 32.0)),
            help: (!settings).then(|| Rect::new(w - 138.0 - 88.0, 8.0, 40.0, 32.0)),
            settings: (!settings).then(|| Rect::new(w - 138.0 - 44.0, 8.0, 40.0, 32.0)),
            min: cap(3.0),
            max: cap(2.0),
            close: cap(1.0),
            maximized,
            back_focused: false,
        }
    }

    fn input<'a>(empty: &'a EmptyLines, rows: Vec<RowIn<'a>>) -> Input<'a> {
        Input {
            size: (800.0, 600.0),
            title: title_in(false, false),
            rows,
            row_links: true,
            scroll: 0.0,
            focus_row: None,
            focus_button: None,
            status: "",
            empty,
            empty_status: "",
            first_run: None,
            top_info: None,
            empty_at: None,
            credit: None,

            convert_block: None,
            convert_label: "Convert",
            footer_row: None,
            undo: None,
            wheel: None,
            settings: None,
            details: None,
            help: None,
            sheet: None,
            inspector: None,
            ask: None,
            tool: None,
            row_menu: None,
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

    /// Where a waiting file's Folder link is: the last fact's value.
    fn folder_rect(il: &InspectorLayout) -> Rect {
        let f = il.facts.last().unwrap();
        Rect::new(f.x + layout::FACT_LABEL, f.y, f.w - layout::FACT_LABEL, f.h)
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

    /// The title bar's buttons' roles and names, in order.
    fn title_buttons(t: &Node) -> Vec<(Role, String)> {
        let bar = t.find(&Id::TitleBar).expect("a title bar");
        assert_eq!((bar.role, bar.name.as_str()), (Role::Group, "Title bar"));
        bar.children
            .iter()
            .map(|c| (c.role, c.name.clone()))
            .collect()
    }

    fn buttons(names: &[&str]) -> Vec<(Role, String)> {
        names
            .iter()
            .map(|n| (Role::Button, (*n).to_owned()))
            .collect()
    }

    #[test]
    fn the_title_bar_is_a_group_of_its_buttons_first_in_the_window() {
        let lines = empty_lines();
        let mut i = input(&lines, vec![]);
        let t = tree(&i);
        sound(&t);
        assert_eq!(t.children[0].id, Id::TitleBar);
        assert_eq!(
            title_buttons(&t),
            buttons(&["Help", "Settings", "Minimise", "Maximise", "Close"])
        );
        let bar = t.find(&Id::TitleBar).unwrap();
        assert_eq!(bar.rect, i.title.rect);
        for c in &bar.children {
            assert!(c.patterns().invoke, "{:?}", c.id);
        }
        // Help and Settings are on the Tab path; Windows' buttons aren't.
        for id in [Id::HelpButton, Id::SettingsButton] {
            assert!(t.find(&id).unwrap().states.focusable, "{id:?}");
        }
        for c in Caption::ALL {
            let n = t.find(&Id::CaptionButton(c)).unwrap();
            assert!(!n.states.focusable, "{c:?}");
        }
        assert_eq!(
            t.find(&Id::CaptionButton(Caption::Close)).unwrap().rect,
            i.title.close
        );
        // Maximised, Maximise reads Restore.
        i.title = title_in(false, true);
        assert_eq!(
            title_buttons(&tree(&i)),
            buttons(&["Help", "Settings", "Minimise", "Restore", "Close"])
        );
    }

    #[test]
    fn the_title_bar_keeps_windows_buttons_under_any_dialog() {
        let lines = empty_lines();
        let mut i = input(&lines, vec![]);
        i.details = Some(DetailsIn {
            layout: crate::layout::kit::details_dialog((800.0, 600.0), 28.0, 20.0, 16.0),
            title: "What happened",
            lead: "This file looks damaged, or isn't really an MP4.",
            text: "moov atom not found",
            copy: "Copy",
            focus: DialogButton::Copy,
        });
        let t = tree(&i);
        sound(&t);
        assert_eq!(
            title_buttons(&t),
            buttons(&["Minimise", "Maximise", "Close"])
        );
        // The dialog is all there is besides.
        assert_eq!(t.children.len(), 2);
        assert_eq!(t.children[1].id, Id::Details);
        // The log is read by character, word and line (Text pattern); its
        // lead is plain text.
        let log = named(&t, Role::Text, "moov atom not found");
        assert!(log.patterns().text);
        let lead = named(
            &t,
            Role::Text,
            "This file looks damaged, or isn't really an MP4.",
        );
        assert!(!lead.patterns().text);
    }

    #[test]
    fn settings_brings_back_and_takes_help_and_settings_away() {
        let lines = empty_lines();
        let mut i = input(&lines, vec![]);
        i.title = title_in(true, false);
        i.settings = Some(settings_input(None));
        let t = tree(&i);
        sound(&t);
        assert_eq!(
            title_buttons(&t),
            buttons(&["Back", "Minimise", "Maximise", "Close"])
        );
        let back = t.find(&Id::BackButton).unwrap();
        assert!(back.patterns().invoke);
        assert_eq!(Some(back.rect), i.title.back);
        assert!(t.find(&Id::HelpButton).is_none());
        // The keys stay with Settings.
        assert_eq!(t.focused().unwrap().id, Id::Settings);
    }

    fn ask_in<'a>(title: &'a str, text: Option<&'a str>, buttons: &[&'static str]) -> AskIn<'a> {
        AskIn {
            layout: crate::layout::kit::dialog((800.0, 600.0), 28.0, 20.0, buttons.len()),
            title,
            text,
            buttons: buttons.to_vec(),
            focus: 0,
        }
    }

    #[test]
    fn an_open_prompt_is_a_modal_dialog_named_by_its_title() {
        let lines = empty_lines();
        let mut i = input(&lines, vec![]);
        // Over Settings, even: the prompt is on top, and alone.
        i.title = title_in(true, false);
        i.settings = Some(settings_input(None));
        i.ask = Some(ask_in(
            "Save before closing?",
            Some("1 converted file isn't saved yet."),
            &["Save all", "Discard", "Cancel"],
        ));
        let t = tree(&i);
        sound(&t);
        assert_eq!(t.children.len(), 2, "the title bar, then the dialog");
        let d = &t.children[1];
        assert_eq!(d.id, Id::Ask);
        assert_eq!(
            (d.role, d.name.as_str()),
            (Role::Dialog, "Save before closing?")
        );
        assert!(d.states.modal);
        assert!(d.patterns().window);
        assert!(!d.patterns().invoke);
        // What is at stake is the dialog's help text, which Narrator reads
        // with its name as the focus enters it.
        assert_eq!(d.help.as_deref(), Some("1 converted file isn't saved yet."));
        let names: Vec<_> = d
            .children
            .iter()
            .map(|c| (c.role, c.name.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                (Role::Text, "1 converted file isn't saved yet."),
                (Role::Button, "Save all"),
                (Role::Button, "Discard"),
                (Role::Button, "Cancel"),
            ]
        );
        for (k, b) in d.children[1..].iter().enumerate() {
            assert_eq!(b.id, Id::AskButton(k));
            assert!(b.patterns().invoke && b.states.focusable);
            assert_eq!(b.rect, i.ask.as_ref().unwrap().layout.buttons[k]);
        }
        // The keyboard is on the accent button, and only Windows' own
        // buttons stay on the title bar.
        assert_eq!(t.focused().unwrap().id, Id::AskButton(0));
        assert_eq!(
            title_buttons(&t),
            buttons(&["Back", "Minimise", "Maximise", "Close"])
        );
        assert!(!t.find(&Id::BackButton).unwrap().states.focused);
        // Tab moved it.
        i.ask.as_mut().unwrap().focus = 2;
        assert_eq!(tree(&i).focused().unwrap().id, Id::AskButton(2));
        // A prompt that says it all in its title has no text line.
        i.ask = Some(ask_in(
            "That's 2000 files. Add them all?",
            None,
            &["Add them all", "Cancel"],
        ));
        let t = tree(&i);
        let d = t.find(&Id::Ask).unwrap();
        assert_eq!(d.children.len(), 2);
        assert!(t.find(&Id::AskText).is_none());
    }

    #[test]
    fn back_is_on_the_tab_path_and_can_hold_the_focus() {
        let lines = empty_lines();
        let mut i = input(&lines, vec![]);
        i.title = title_in(true, false);
        i.settings = Some(settings_input(None));
        let t = tree(&i);
        assert!(t.find(&Id::BackButton).unwrap().states.focusable);
        // Tab put the keyboard on Back.
        i.title.back_focused = true;
        let t = tree(&i);
        sound(&t);
        assert_eq!(t.focused().unwrap().id, Id::BackButton);
    }

    #[test]
    fn content_starts_under_the_title_bar_at_its_height() {
        let lines = empty_lines();
        let mut i = input(&lines, vec![]);
        // The empty window's content layer starts right under the bar.
        let at = |i: &Input| tree(i).find(&Id::DropPanel).unwrap().rect.y;
        assert_eq!(at(&i), 48.0);
        // At a Text size of 150% the bar is 48 DIPs, 32 units.
        i.title.rect.h = 32.0;
        assert_eq!(at(&i), 32.0);
    }

    #[test]
    fn caption_ids_are_their_own() {
        let ids = [
            Id::TitleBar,
            Id::BackButton,
            Id::CaptionButton(Caption::Min),
            Id::CaptionButton(Caption::Max),
            Id::CaptionButton(Caption::Close),
            Id::HelpButton,
            Id::SettingsButton,
        ];
        let keys: std::collections::HashSet<u64> = ids.iter().map(Id::key).collect();
        assert_eq!(keys.len(), ids.len());
        assert_eq!(Caption::Min.name(true), "Minimise");
        assert_eq!(Caption::Close.name(false), "Close");
    }

    #[test]
    fn tab_reaches_the_header_and_footer_buttons() {
        let lines = empty_lines();
        let es = [entry(
            "a.png",
            JobState::Done {
                action: Action::Convert("jpg".into()),
                outputs: vec![PathBuf::from("held/a.jpg")],
                saved: None,
            },
        )];
        let mut i = input(&lines, rows(&es));
        i.focus_row = Some(0);
        let t = tree(&i);
        for id in [
            Id::HelpButton,
            Id::SettingsButton,
            Id::ConvertButton,
            Id::AddFiles,
            Id::Clear,
        ] {
            let n = t.find(&id).unwrap();
            assert!(n.states.focusable, "{id:?}");
            assert!(!n.states.focused, "{id:?}");
            assert_eq!(button_of(&id).map(button_id), Some(id));
        }
        assert_eq!(t.focused().unwrap().id, Id::Row(es[0].path.clone()));
        // The footer's buttons read in the order they are drawn.
        let at = |id: &Id| t.children.iter().position(|c| c.id == *id).unwrap();
        let footer = [Id::AddFiles, Id::Clear, Id::ConvertButton].map(|id| at(&id));
        assert!(footer.windows(2).all(|w| w[0] < w[1]), "{footer:?}");
        // On a button, it has the focus and the row doesn't.
        for b in [
            FocusButton::Convert,
            FocusButton::AddFiles,
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
    fn the_credit_is_a_hyperlink_under_the_status_before_add_files() {
        use crate::credit::{self, Widths};
        let lines = empty_lines();
        let es = [entry("a.png", JobState::Idle)];
        let mut i = input(&lines, rows(&es));
        i.status = "1 file.";
        let foot = layout::layout(800.0, 600.0, 1).footer.unwrap();
        let row = credit::in_footer(layout::footer_row(&foot, false), Widths::GUESS);
        let c = row.credit.expect("room for the credit");
        i.footer_row = Some(row);
        let t = tree(&i);
        sound(&t);
        let link = t.find(&Id::CreditLink).unwrap();
        assert_eq!(link.role, Role::Hyperlink);
        assert_eq!(link.name, "Chandler Crisp");
        assert_eq!(link.help.as_deref(), Some(credit::LINK_HELP));
        assert_eq!(link.rect, c.name);
        assert!(link.states.focusable);
        assert!(link.patterns().invoke);
        let words = t.find(&Id::CreditText).unwrap();
        assert_eq!(words.name, "Made with ❤️ by");
        // Read after the status, before the footer's buttons, as drawn.
        let at = |id: &Id| t.children.iter().position(|c| c.id == *id).unwrap();
        let order = [
            Id::Status,
            Id::CreditText,
            Id::CreditLink,
            Id::AddFiles,
            Id::Clear,
        ]
        .map(|id| at(&id));
        assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}");
        assert_eq!(button_of(&Id::CreditLink), Some(FocusButton::Credit));
        assert_eq!(button_id(FocusButton::Credit), Id::CreditLink);
        i.focus_button = Some(FocusButton::Credit);
        assert_eq!(tree(&i).focused().unwrap().id, Id::CreditLink);
        // Without room for it, there is none.
        i.footer_row = Some(layout::footer_row(&foot, false));
        i.focus_button = None;
        assert!(tree(&i).find(&Id::CreditLink).is_none());
    }

    #[test]
    fn the_empty_window_reads_the_credit_last() {
        use crate::credit::{self, Widths};
        let lines = empty_lines();
        let mut i = input(&lines, vec![]);
        let l = layout::layout(800.0, 600.0, 0);
        let e = layout::empty_state(&l.panel, None);
        i.credit = credit::in_empty(&e, Widths::GUESS);
        assert!(i.credit.is_some());
        let t = tree(&i);
        sound(&t);
        let last: Vec<&Id> = t.children.iter().rev().take(2).map(|c| &c.id).collect();
        assert_eq!(last, [&Id::CreditLink, &Id::CreditText]);
        let link = t.find(&Id::CreditLink).unwrap();
        assert_eq!(link.role, Role::Hyperlink);
        assert!(link.states.focusable);
    }

    #[test]
    fn the_empty_windows_buttons_take_the_focus() {
        let lines = empty_lines();
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
        let lines = empty_lines();
        let t = tree(&input(&lines, vec![]));
        sound(&t);
        assert_eq!((t.role, t.name.as_str()), (Role::Window, "Kiln"));
        named(&t, Role::Button, "Help");
        named(&t, Role::Button, "Settings");
        let panel = named(&t, Role::Group, "Drop files to convert");
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
        let lines = empty_lines();
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
        // Labelled parts, each a sentence.
        assert_eq!(a.name, "a.png. PNG. File size 3.1 MB. Folder photos");
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
        named(&t, Role::Button, "Clear list");
    }

    #[test]
    fn a_running_row_says_how_far_and_offers_cancel() {
        let lines = empty_lines();
        let es = [entry(
            "a.png",
            JobState::Running {
                action: Action::Convert("jpg".into()),
                fraction: Some(0.45),
                started: None,
            },
        )];
        let t = tree(&input(&lines, rows(&es)));
        sound(&t);
        let row = named(&t, Role::ListItem, "a.png. Converting to JPG");
        assert_eq!(row.status.as_deref(), Some("45%"));
        let cancel = &row.children[0];
        assert_eq!(
            (cancel.role, cancel.name.as_str()),
            (Role::Button, "Cancel a.png")
        );
        assert!(cancel.patterns().invoke);
        // The percent is a range value an assistive tool can read.
        let bar = row
            .children
            .iter()
            .find(|c| c.role == Role::ProgressBar)
            .expect("a progress bar");
        assert_eq!(bar.name, "Progress a.png");
        assert_eq!(bar.range, Some(45.0));
        assert!(bar.patterns().range_value && !bar.patterns().invoke);
    }

    /// Help open in an 800 x 600 window, its parts where the dialog puts
    /// them, every line showing.
    fn help_in(sections: &[HelpSection]) -> HelpIn<'_> {
        let frame = layout::kit::help_dialog((800.0, 600.0), 28.0, 300.0);
        HelpIn {
            sections,
            places: vec![],
            frame: frame.rect,
            body: frame.content,
            guide: Rect::new(frame.content.x, frame.content.bottom() - 20.0, 130.0, 20.0),
            guide_offscreen: false,
            close: frame.buttons[0],
            focus: HelpButton::Close,
        }
    }

    #[test]
    fn a_help_section_is_where_its_lines_are() {
        let lines = empty_lines();
        let sections = crate::help::help_content(crate::help::Results::Beside);
        let mut hi = help_in(&sections);
        let body = hi.body;
        let at = |y: f32| Rect::new(body.x, body.y + y, 200.0, 20.0);
        hi.places = vec![
            HelpPlace {
                section: 0,
                line: 0,
                rect: at(30.0),
                offscreen: false,
            },
            HelpPlace {
                section: 1,
                line: 0,
                rect: at(90.0),
                offscreen: false,
            },
            HelpPlace {
                section: 1,
                line: 1,
                rect: at(114.0),
                offscreen: false,
            },
        ];
        let mut i = input(&lines, vec![]);
        i.help = Some(hi);
        let t = tree(&i);
        let convert = named(&t, Role::Group, "Convert files");
        let shortcuts = named(&t, Role::Group, "Shortcuts");
        assert_eq!(convert.rect, at(30.0));
        assert_eq!(shortcuts.rect.y, body.y + 90.0);
        assert!(
            shortcuts.rect.y >= convert.rect.bottom(),
            "they don't overlap"
        );
    }

    #[test]
    fn help_lines_scrolled_out_of_the_content_are_off_screen() {
        let lines = empty_lines();
        let sections = crate::help::help_content(crate::help::Results::Beside);
        let mut hi = help_in(&sections);
        let body = hi.body;
        let shown = Rect::new(body.x, body.y + 10.0, 100.0, 20.0);
        let hidden = Rect::new(body.x, body.bottom() + 40.0, 100.0, 20.0);
        let above = Rect::new(body.x, body.y - 60.0, 100.0, 20.0);
        assert!(!clipped_out(&shown, &body));
        assert!(clipped_out(&hidden, &body));
        assert!(clipped_out(&above, &body));
        hi.places = vec![
            HelpPlace {
                section: 1,
                line: 0,
                rect: shown,
                offscreen: false,
            },
            HelpPlace {
                section: 1,
                line: 1,
                rect: hidden,
                offscreen: true,
            },
        ];
        hi.guide = hidden;
        hi.guide_offscreen = true;
        let mut i = input(&lines, vec![]);
        i.help = Some(hi);
        let t = tree(&i);
        let first = t.find(&Id::HelpLine(1, 0)).unwrap();
        let second = t.find(&Id::HelpLine(1, 1)).unwrap();
        assert!(!first.states.offscreen);
        assert!(second.states.offscreen);
        assert_eq!(second.rect, hidden);
        assert!(t.find(&Id::HelpGuide).unwrap().states.offscreen);
    }

    #[test]
    fn the_footer_button_stays_convert_and_says_why_when_it_is_disabled() {
        let lines = empty_lines();
        let es = [entry("a.png", JobState::Idle)];
        let mut i = input(&lines, rows(&es));
        let t = tree(&i);
        let b = t.find(&Id::ConvertButton).unwrap();
        assert_eq!(b.name, "Convert");
        assert_eq!(b.help.as_deref(), Some(crate::help::CONVERT_TIP));
        assert!(!b.states.disabled);
        // a selection that can't be converted leaves the name as it
        // is, disables the button and gives the reason as its help.
        let why =
            "These files have no format in common: PNG (1), MP3 (1). Select one kind at a time.";
        i.convert_block = Some(why);
        let t = tree(&i);
        let b = t.find(&Id::ConvertButton).unwrap();
        assert_eq!(b.name, "Convert");
        assert_eq!(b.help.as_deref(), Some(why));
        assert!(b.states.disabled);
    }

    #[test]
    fn a_finished_drag_wheel_row_offers_options_named_with_its_file() {
        let lines = empty_lines();
        let done = |name: &str, action: Action, from_drag: bool| {
            let mut e = entry(
                name,
                JobState::Done {
                    action,
                    outputs: vec![PathBuf::from(format!(r"C:\out\{name}.webp"))],
                    saved: Some(Saved::Folder(PathBuf::from(r"C:\out"))),
                },
            );
            e.from_drag = from_drag;
            e
        };
        let mut failed = entry(
            "d.png",
            JobState::Failed {
                message: "It broke.".into(),
                details: String::new(),
                engine: None,
                store_product: None,
            },
        );
        failed.from_drag = true;
        let es = [
            done("a.png", Action::Convert("webp".into()), true),
            // Converted over Kiln's window, a tool, a failure: no Options.
            done("b.png", Action::Convert("webp".into()), false),
            done(
                "c.png",
                Action::Tool(kiln_core::Tool::RemoveEdges, kiln_core::Kind::Image),
                true,
            ),
            failed,
        ];
        let t = tree(&input(&lines, rows(&es)));
        sound(&t);
        let options = t.find(&Id::RowOptions(es[0].path.clone())).unwrap();
        assert_eq!(options.name, "Options for a.png");
        assert_eq!(options.role, Role::Button);
        assert!(options.patterns().invoke);
        // Right after Show in folder, where it is drawn, and Show in folder
        // before it.
        let row = t.find(&Id::Row(es[0].path.clone())).unwrap();
        assert_eq!(options.rect, layout::row_options(&row.rect));
        let export = t.find(&Id::RowLink(es[0].path.clone())).unwrap();
        let label = es[0].link(false).unwrap();
        assert_eq!(export.rect, layout::row_link_at(&row.rect, label, true));
        assert!(export.rect.right() < options.rect.x);
        // In reading order too: Show in folder, then Options.
        let kids: Vec<&Id> = row.children.iter().map(|c| &c.id).collect();
        let at = |id: &Id| kids.iter().position(|k| *k == id).unwrap();
        assert!(at(&export.id) < at(&options.id));
        // A row without Options keeps its link at the edge.
        let b = t.find(&Id::Row(es[1].path.clone())).unwrap();
        let b_link = t.find(&Id::RowLink(es[1].path.clone())).unwrap();
        assert_eq!(
            b_link.rect,
            layout::row_link(&b.rect, es[1].link(false).unwrap())
        );
        for e in &es[1..] {
            assert!(
                t.find(&Id::RowOptions(e.path.clone())).is_none(),
                "{}",
                e.name
            );
        }
        // A note standing in for the line (an export that failed) leaves
        // Options there, as it is drawn, and Export before it.
        let mut noted = rows(&es);
        noted[0].instead = Some("Couldn't save to Pictures.");
        let t = tree(&input(&lines, noted));
        let row = t.find(&Id::Row(es[0].path.clone())).unwrap();
        let options = t.find(&Id::RowOptions(es[0].path.clone())).unwrap();
        assert_eq!(options.rect, layout::row_options(&row.rect));
        let export = t.find(&Id::RowLink(es[0].path.clone())).unwrap();
        let label = es[0].link(false).unwrap();
        assert_eq!(export.rect, layout::row_link_at(&row.rect, label, true));
        // Being saved, it has neither until it is.
        let mut saving = rows(&es);
        saving[0].instead = Some(crate::model::SAVING);
        saving[0].saving = true;
        saving[0].link = None;
        let t = tree(&input(&lines, saving));
        assert!(t.find(&Id::RowOptions(es[0].path.clone())).is_none());
        assert!(t.find(&Id::RowLink(es[0].path.clone())).is_none());
    }

    #[test]
    fn a_cancelled_or_stopped_row_offers_retry_named_with_its_file() {
        let lines = empty_lines();
        let mut cancelled = entry("a.png", JobState::Cancelled);
        let mut stopped = entry(
            "b.png",
            JobState::Failed {
                message: "The conversion stopped before it finished. Try again.".into(),
                details: "ffmpeg output".into(),
                engine: Some("FFmpeg"),
                store_product: None,
            },
        );
        let mut damaged = entry(
            "c.png",
            JobState::Failed {
                message:
                    "This file looks damaged, or isn't really a PNG. Try opening it in another app."
                        .into(),
                details: "x".into(),
                engine: None,
                store_product: None,
            },
        );
        for e in [&mut cancelled, &mut stopped, &mut damaged] {
            e.last = Some(Action::Convert("jpg".into()));
        }
        let es = [cancelled, stopped, damaged];
        let t = tree(&input(&lines, rows(&es)));
        sound(&t);
        let names = |i: usize| -> Vec<String> {
            t.find(&Id::Row(es[i].path.clone()))
                .unwrap()
                .children
                .iter()
                .map(|c| c.name.clone())
                .collect()
        };
        assert_eq!(names(0), ["Retry a.png"]);
        assert_eq!(names(1), ["Show details b.png", "Retry b.png"]);
        // A damaged file fails the same way again: no Retry.
        assert_eq!(names(2), ["Show details c.png"]);
        let retry = t.find(&Id::RowRetry(es[1].path.clone())).unwrap();
        assert!(retry.patterns().invoke);
    }

    #[test]
    fn while_the_inspector_shows_rows_have_no_links_and_retry_stays_where_drawn() {
        let lines = empty_lines();
        let mut failed = entry(
            "a.png",
            JobState::Failed {
                message: "The conversion stopped before it finished. Try again.".into(),
                details: "ffmpeg output".into(),
                engine: Some("FFmpeg"),
                store_product: None,
            },
        );
        failed.last = Some(Action::Convert("jpg".into()));
        let running = entry(
            "b.png",
            JobState::Running {
                action: Action::Convert("jpg".into()),
                fraction: Some(0.5),
                started: None,
            },
        );
        let saved = entry(
            "c.png",
            JobState::Done {
                action: Action::Convert("jpg".into()),
                outputs: vec![PathBuf::from(r"C:\out\c.jpg")],
                saved: Some(Saved::Folder(PathBuf::from(r"C:\out"))),
            },
        );
        let idle = entry("d.png", JobState::Idle);
        let es = [failed, running, saved, idle];
        let retry_rect = |t: &Node| {
            let row = t.find(&Id::Row(es[0].path.clone())).unwrap();
            let retry = t.find(&Id::RowRetry(es[0].path.clone())).unwrap();
            (row.rect, retry.rect)
        };

        // With links: Show details, Cancel, the saved line and Convert, and
        // Retry beside Show details.
        let t = tree(&input(&lines, rows(&es)));
        sound(&t);
        let (row, retry) = retry_rect(&t);
        assert_eq!(retry, layout::row_retry(&row, es[0].link(false)));
        assert!(t.find(&Id::RowCancel(es[1].path.clone())).is_some());

        // Without (the side pane carries them): no row has a link but Show
        // in folder, and Retry is the pane's too. Cancel stays on a running
        // row (Task 18, mock screen 5).
        let mut i = input(&lines, rows(&es));
        i.row_links = false;
        for r in &mut i.rows {
            r.link = None;
        }
        let t = tree(&i);
        sound(&t);
        let mut links = vec![];
        t.walk(&mut |n| {
            if matches!(n.id, Id::RowLink(_)) {
                links.push(n.name.clone());
            }
        });
        assert!(links.is_empty(), "{links:?}");
        assert!(t.find(&Id::RowCancel(es[1].path.clone())).is_some());
        assert!(t.find(&Id::RowRetry(es[0].path.clone())).is_none());
        // The progress bar is not a link: it stays.
        assert!(t.find(&Id::RowProgress(es[1].path.clone())).is_some());
    }

    #[test]
    fn a_row_on_its_way_to_its_folder_says_so_and_offers_no_save() {
        let lines = empty_lines();
        let es = [entry(
            "a.png",
            JobState::Done {
                action: Action::Convert("jpg".into()),
                outputs: vec![PathBuf::from("held/a.jpg")],
                saved: None,
            },
        )];
        let t = tree(&input(&lines, rows(&es)));
        sound(&t);
        let row = named(&t, Role::ListItem, "a.png. Saving…");
        assert!(row.children.is_empty());
        let mut names = vec![];
        t.walk(&mut |n| names.push(n.name.clone()));
        assert!(!names.iter().any(|n| n.starts_with("Save")), "{names:?}");
    }

    #[test]
    fn a_saved_row_links_to_its_folder() {
        let lines = empty_lines();
        let es = [entry(
            "a.png",
            JobState::Done {
                action: Action::Convert("jpg".into()),
                outputs: vec![PathBuf::from(r"C:\out\a.jpg")],
                saved: Some(Saved::Folder(PathBuf::from(r"C:\out"))),
            },
        )];
        let t = tree(&input(&lines, rows(&es)));
        sound(&t);
        let row = t.find(&Id::Row(es[0].path.clone())).unwrap();
        let link = &row.children[0];
        assert_eq!(link.role, Role::Button);
        assert_eq!(link.name, "Show in folder a.png");
    }

    #[test]
    fn a_failed_row_reads_its_whole_sentence() {
        let lines = empty_lines();
        let message = "FFmpeg could not read this file. It may be damaged, or \
                       in a format this version of FFmpeg doesn't know.";
        let es = [entry(
            "a.mov",
            JobState::Failed {
                message: message.into(),
                details: "moov atom not found".into(),
                engine: Some("FFmpeg"),
                store_product: None,
            },
        )];
        let t = tree(&input(&lines, rows(&es)));
        sound(&t);
        let row = named(&t, Role::ListItem, &format!("a.mov. {message}"));
        assert_eq!(row.children[0].name, "Show details a.mov");
        assert_eq!(row.children[0].role, Role::Button);
    }

    #[test]
    fn a_row_whose_line_is_a_sentence_reads_one_full_stop_between_parts() {
        // a JPG that looks cut short read "...again.. Folder files".
        let mut e = entry("cut.jpg", JobState::Idle);
        e.check = kiln_core::FileCheck::CutShort;
        e.targets.clear();
        let name = row_name(&e, None);
        assert!(!name.contains(".."), "{name}");
        assert!(
            name.contains("Looks cut short. Try copying or downloading it again. Folder "),
            "{name}"
        );
    }

    #[test]
    fn a_scrolled_out_row_is_offscreen() {
        let lines = empty_lines();
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
            action: Action::Convert(ext.into()),
            kind: None,
        }
    }

    fn hub(pair: &str, hint: &str) -> wheel::HubLines {
        wheel::HubLines {
            pair: pair.into(),
            hint: hint.into(),
        }
    }

    #[test]
    fn the_wheel_is_a_menu_of_plain_descriptions_with_its_hub_and_mores_list() {
        let lines = empty_lines();
        let es = [entry("a.png", JobState::Idle)];
        let w = Wheel {
            kind: WheelKind::Convert,
            origin: wheel::Origin::Window,
            items: vec![item("jpg"), item("webp")],
            flyout: wheel::grouped(vec![Item {
                kind: Some(kiln_core::Kind::Image),
                ..item("bmp")
            }]),
            center: (400.0, 300.0),
            scale: 1.0,
            source_code: "PNG".into(),
            source_kind: None,
            list_only: false,
            settable: vec!["jpg".into(), "webp".into(), "bmp".into()],
        };
        let paths = [es[0].path.clone()];
        let h = hub("PNG \u{2192} WEBP", "about 410 KB");
        let more_rows = w.flyout_rows(Some(Spot::Inner(1)));
        let place = wheel::flyout_place(&w, &more_rows, Rect::new(0.0, 48.0, 1100.0, 672.0), 0.0);
        let mut i = input(&lines, rows(&es));
        i.wheel = Some(WheelIn {
            wheel: &w,
            paths: &paths,
            hover: Some(Spot::Inner(1)),
            hub: &h,
            flyout: Some((&more_rows, &place)),
            text_only: &|_| false,
        });
        let t = tree(&i);
        sound(&t);
        // Only the menu, after the title bar: the list behind it doesn't
        // take the keys.
        assert_eq!(t.children.len(), 2);
        let menu = named(&t, Role::Menu, "Convert a.png");
        // The hub reads first: what is lit and its estimate.
        assert_eq!(menu.children[0].role, Role::Text);
        assert_eq!(menu.children[0].name, "PNG \u{2192} WEBP, about 410 KB");
        let jpg = named(menu, Role::MenuItem, &wheel::target_label("jpg"));
        assert!(jpg.patterns().invoke);
        let webp = named(menu, Role::MenuItem, &wheel::target_label("webp"));
        assert!(webp.states.focused && webp.states.selected);
        // More holds its list: each group under its heading, then
        // Options for the format last lit.
        let more = named(menu, Role::MenuItem, "More");
        let list = named(more, Role::Menu, "More");
        assert_eq!(list.rect, place.rect);
        let images = named(list, Role::Group, "Photos");
        named(images, Role::MenuItem, &wheel::target_label("bmp"));
        let options = named(list, Role::MenuItem, "Options for WEBP\u{2026}");
        assert_eq!(options.id, Id::WheelItem(Spot::Options));
        assert!(options.patterns().invoke);
        assert!(!options.states.disabled);
    }

    #[test]
    fn the_list_alone_is_one_menu_of_its_groups_with_a_footer_that_names_its_format() {
        let lines = empty_lines();
        let es = [entry("a.png", JobState::Idle)];
        let w = Wheel {
            kind: WheelKind::Convert,
            origin: wheel::Origin::Window,
            items: vec![],
            flyout: wheel::grouped(vec![
                Item {
                    kind: Some(kiln_core::Kind::Image),
                    ..item("jpg")
                },
                Item {
                    kind: Some(kiln_core::Kind::Image),
                    ..item("bmp")
                },
            ]),
            center: (700.0, 300.0),
            scale: 1.0,
            source_code: "PNG".into(),
            source_kind: None,
            list_only: true,
            settable: vec!["jpg".into(), "bmp".into()],
        };
        let paths = [es[0].path.clone()];
        let h = hub("PNG", "Pick a format");
        let check = |lit: Option<Spot>, name: &str| {
            let more_rows = w.flyout_rows(lit);
            let place =
                wheel::flyout_place(&w, &more_rows, Rect::new(0.0, 48.0, 700.0, 600.0), 0.0);
            let mut i = input(&lines, rows(&es));
            i.wheel = Some(WheelIn {
                wheel: &w,
                paths: &paths,
                hover: lit,
                hub: &h,
                flyout: Some((&more_rows, &place)),
                text_only: &|_| false,
            });
            let t = tree(&i);
            sound(&t);
            let menu = named(&t, Role::Menu, "Convert a.png");
            // No hub and no ring: the list's menu, its groups and the footer.
            let list = named(menu, Role::Menu, "Formats and tools");
            assert_eq!(list.rect, place.rect);
            let photos = named(list, Role::Group, "Photos");
            named(photos, Role::MenuItem, &wheel::target_label("jpg"));
            // Always usable, whatever is lit, and for the format it names.
            let footer = named(list, Role::MenuItem, name);
            assert!(!footer.states.disabled && footer.patterns().invoke);
            assert!(!menu.children.iter().any(|c| c.id == Id::WheelHub));
            if let Some(spot) = lit {
                let lit_item = t.find(&Id::WheelItem(spot)).unwrap();
                assert!(lit_item.states.selected);
            }
        };
        check(None, "Options for JPG\u{2026}");
        check(Some(Spot::Flyout(1)), "Options for BMP\u{2026}");
    }

    #[test]
    fn mores_list_is_in_the_tree_only_while_it_is_open_and_says_what_is_scrolled_away() {
        let lines = empty_lines();
        let w = Wheel {
            kind: WheelKind::Convert,
            origin: wheel::Origin::Window,
            items: vec![item("jpg")],
            flyout: wheel::grouped((0..20).map(|_| item("bmp")).collect()),
            center: (300.0, 300.0),
            scale: 1.0,
            source_code: "PNG".into(),
            source_kind: None,
            list_only: false,
            settable: vec![],
        };
        let paths = [PathBuf::from("a.png")];
        let h = hub("PNG", "Pick a format");
        let mut i = input(&lines, vec![]);
        i.wheel = Some(WheelIn {
            wheel: &w,
            paths: &paths,
            hover: None,
            hub: &h,
            flyout: None,
            text_only: &|_| false,
        });
        let t = tree(&i);
        let more = named(&t, Role::MenuItem, "More");
        assert!(more.children.is_empty());
        assert_eq!(named(&t, Role::Text, "PNG, Pick a format").role, Role::Text);
        // Open in a short window, the lines past its bottom are offscreen.
        let rows = w.flyout_rows(None);
        let place = wheel::flyout_place(&w, &rows, Rect::new(0.0, 0.0, 900.0, 300.0), 0.0);
        let mut i = input(&lines, vec![]);
        i.wheel = Some(WheelIn {
            wheel: &w,
            paths: &paths,
            hover: Some(Spot::Flyout(0)),
            hub: &h,
            flyout: Some((&rows, &place)),
            text_only: &|_| false,
        });
        let t = tree(&i);
        sound(&t);
        let first = t.find(&Id::WheelItem(Spot::Flyout(0))).unwrap();
        assert!(first.states.focused && !first.states.offscreen);
        let last = t.find(&Id::WheelItem(Spot::Flyout(19))).unwrap();
        assert!(last.states.offscreen);
    }

    #[test]
    fn a_wheel_for_many_files_says_how_many() {
        let lines = empty_lines();
        let w = Wheel {
            kind: WheelKind::Tools,
            origin: wheel::Origin::Window,
            items: vec![],
            flyout: vec![],
            center: (400.0, 300.0),
            scale: 1.0,
            source_code: "MIXED".into(),
            source_kind: None,
            list_only: false,
            settable: vec![],
        };
        let paths = [PathBuf::from("a.png"), PathBuf::from("b.png")];
        let h = hub("MIXED", "Pick a tool");
        let mut i = input(&lines, vec![]);
        i.wheel = Some(WheelIn {
            wheel: &w,
            paths: &paths,
            hover: None,
            hub: &h,
            flyout: None,
            text_only: &|_| false,
        });
        let t = tree(&i);
        assert!(named(&t, Role::Menu, "Tools 2 files").states.focused);
    }

    #[test]
    fn a_wheel_names_a_text_only_target_so() {
        let lines = empty_lines();
        let w = Wheel {
            kind: WheelKind::Convert,
            origin: wheel::Origin::Window,
            items: vec![item("docx"), item("png")],
            flyout: wheel::grouped(vec![item("epub")]),
            center: (400.0, 300.0),
            scale: 1.0,
            source_code: "PDF".into(),
            source_kind: None,
            list_only: false,
            settable: vec![],
        };
        let paths = [PathBuf::from("a.pdf")];
        let rule = |ext: &str| ext == "docx" || ext == "epub";
        let h = hub("PDF", "Pick a format");
        let rows = w.flyout_rows(None);
        let place = wheel::flyout_place(&w, &rows, Rect::new(0.0, 48.0, 1100.0, 672.0), 0.0);
        let mut i = input(&lines, vec![]);
        i.wheel = Some(WheelIn {
            wheel: &w,
            paths: &paths,
            hover: None,
            hub: &h,
            flyout: Some((&rows, &place)),
            text_only: &rule,
        });
        let t = tree(&i);
        sound(&t);
        let menu = named(&t, Role::Menu, "Convert a.pdf");
        named(menu, Role::MenuItem, "DOCX (text only)");
        named(menu, Role::MenuItem, "EPUB (text only)");
        // Another target is named for what it is.
        named(menu, Role::MenuItem, &wheel::target_label("png"));
        assert_eq!(item_name(&w.items[0], &rule), "DOCX (text only)");
        assert_eq!(item_name(&w.items[1], &rule), wheel::target_label("png"));
    }

    /// Settings as it opens at 800 x 600, the keyboard on `focus`.
    fn settings_input(focus: Option<Control>) -> SettingsIn {
        settings_with(&crate::settings::Settings::default(), focus, [false; 5])
    }

    fn settings_with(
        s: &crate::settings::Settings,
        focus: Option<Control>,
        open: [bool; 5],
    ) -> SettingsIn {
        use crate::settings::EngineStatus;
        use crate::settings_page::{self as sp, Facts};
        let mut status: [EngineStatus; 4] = std::array::from_fn(|_| EngineStatus::Missing);
        status[0] = EngineStatus::Included(PathBuf::from(r"C:\Kiln\ffmpeg\bin\ffmpeg.exe"));
        let facts = Facts {
            engines: Some(status),
            ..Facts::default()
        };
        let page = sp::page(s, &facts, open, false);
        let view = layout::content_layer(800.0, 600.0, layout::TITLE_BAR);
        SettingsIn {
            layout: layout::settings(view, 0.0, &page, &layout::Estimate),
            page,
            focus,
            combo: None,
        }
    }

    #[test]
    fn settings_reads_its_sections_cards_and_controls() {
        use crate::settings_page::{Card, Section};
        let lines = empty_lines();
        let mut i = input(&lines, vec![]);
        i.settings = Some(settings_input(Some(Control::Theme)));
        let t = tree(&i);
        sound(&t);
        let page = t.find(&Id::Settings).unwrap();
        assert_eq!(page.role, Role::Group);
        assert_eq!(page.name, "Settings");
        assert_eq!(page.children[0].id, Id::SettingsTitle);
        let sections: Vec<&str> = page.children[1..].iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            sections,
            [
                "Appearance",
                "Saving",
                "Quality",
                "Drag-wheel",
                "Helper apps",
                "About"
            ]
        );
        assert_eq!(
            page.children[1].id,
            Id::SettingsSection(Section::Appearance)
        );
        // A card is a group named by its label and helped by its note,
        // holding its control, named the same.
        let theme = t.find(&Id::SettingsCard(Card::Theme)).unwrap();
        assert_eq!(theme.name, "Theme");
        assert_eq!(
            theme.help.as_deref(),
            Some("System follows Windows' app mode.")
        );
        let combo = t.find(&Id::Combo(Control::Theme)).unwrap();
        assert_eq!(combo.role, Role::ComboBox);
        assert_eq!(combo.name, "Theme");
        assert_eq!(combo.value.as_deref(), Some("System"));
        assert_eq!(combo.states.expanded, Some(false));
        assert!(combo.states.focused);
        assert_eq!(t.focused().unwrap().id, Id::Combo(Control::Theme));
        // The drag-wheel is a ToggleSwitch, off for a new install.
        let wheel = t.find(&Id::Toggle(Control::DragWheel)).unwrap();
        assert_eq!(wheel.states.toggled, Some(false));
        assert!(wheel.patterns().toggle);
        // Small, High and Best are radio buttons in the Video card.
        let video = t.find(&Id::SettingsCard(Card::Video)).unwrap();
        let radios: Vec<(&str, bool)> = video
            .children
            .iter()
            .map(|c| (c.name.as_str(), c.states.selected))
            .collect();
        assert_eq!(radios, [("Small", false), ("High", true), ("Best", false)]);
        assert!(video.children.iter().all(|c| c.role == Role::RadioButton));
        named(&t, Role::Button, "Check for updates");
        named(&t, Role::Button, "Open");
    }

    #[test]
    fn the_drag_wheel_toggle_reads_its_name_note_and_state() {
        use crate::settings::{DRAG_WHEEL_LABEL, DRAG_WHEEL_NOTE};
        use crate::settings_page::Card;
        let lines = empty_lines();
        for on in [false, true] {
            let s = crate::settings::Settings {
                drag_wheel: on,
                ..crate::settings::Settings::default()
            };
            let mut i = input(&lines, vec![]);
            i.settings = Some(settings_with(&s, None, [false; 5]));
            let t = tree(&i);
            sound(&t);
            let card = t.find(&Id::SettingsCard(Card::DragWheel)).unwrap();
            assert_eq!(card.name, DRAG_WHEEL_LABEL);
            assert_eq!(card.help.as_deref(), Some(DRAG_WHEEL_NOTE));
            let toggle = t.find(&Id::Toggle(Control::DragWheel)).unwrap();
            assert_eq!(toggle.name, DRAG_WHEEL_LABEL);
            assert_eq!(toggle.states.toggled, Some(on));
            assert!(toggle.states.focusable);
            // Narrator reads the note on the control it focuses.
            assert_eq!(toggle.help.as_deref(), Some(DRAG_WHEEL_NOTE));
            assert!(toggle
                .help
                .as_deref()
                .is_some_and(|h| h.contains("starts with Windows")));
        }
    }

    #[test]
    fn every_cards_note_is_on_the_controls_narrator_focuses() {
        use crate::settings::DRAG_WHEEL_REFUSED;
        use crate::settings_page::Card;
        let lines = empty_lines();
        let s = crate::settings::Settings {
            save_to: Some(PathBuf::from(r"D:\Converted")),
            drag_wheel: true,
            ..crate::settings::Settings::default()
        };
        let mut i = input(&lines, vec![]);
        let mut si = settings_with(&s, None, [false; 5]);
        // Windows refused the hooks: the alert is the note.
        si.page = crate::settings_page::page(
            &s,
            &crate::settings_page::Facts::default(),
            [false; 5],
            true,
        );
        i.settings = Some(si);
        let t = tree(&i);
        let mut checked = 0;
        for spec in i.settings.as_ref().unwrap().page.cards() {
            let card = t.find(&Id::SettingsCard(spec.card));
            let Some(card) = card else {
                // A helper card is its Expander.
                continue;
            };
            for c in card.children.iter().filter(|c| c.states.focusable) {
                assert_eq!(c.help, spec.note, "{:?} in {:?}", c.id, spec.card);
                checked += 1;
            }
        }
        assert!(checked > 10, "{checked}");
        let combo = t.find(&Id::Combo(Control::SaveWhere)).unwrap();
        assert_eq!(combo.help.as_deref(), Some(r"D:\Converted"));
        let toggle = t.find(&Id::Toggle(Control::DragWheel)).unwrap();
        assert_eq!(toggle.help.as_deref(), Some(DRAG_WHEEL_REFUSED));
        assert!(t.find(&Id::SettingsCard(Card::SaveWhere)).is_some());
    }

    #[test]
    fn a_helper_app_is_an_expander_with_its_status() {
        use crate::settings::Engine;
        use crate::settings_page::Helper;
        let lines = empty_lines();
        let mut i = input(&lines, vec![]);
        i.settings = Some(settings_input(None));
        let t = tree(&i);
        sound(&t);
        let ffmpeg = t
            .find(&Id::Setting(Control::Helper(Helper::Ffmpeg)))
            .unwrap();
        assert_eq!(ffmpeg.name, "FFmpeg");
        assert_eq!(ffmpeg.status.as_deref(), Some("Included"));
        assert_eq!(ffmpeg.states.expanded, Some(false));
        assert!(ffmpeg.patterns().expand_collapse);
        assert!(ffmpeg.children.is_empty());
        // Open, it holds its line and its buttons.
        let mut open = [false; 5];
        open[Helper::Calibre.index()] = true;
        let mut i = input(&lines, vec![]);
        i.settings = Some(settings_with(
            &crate::settings::Settings::default(),
            None,
            open,
        ));
        let t = tree(&i);
        sound(&t);
        let calibre = t
            .find(&Id::Setting(Control::Helper(Helper::Calibre)))
            .unwrap();
        assert_eq!(calibre.states.expanded, Some(true));
        assert_eq!(calibre.status.as_deref(), Some("Not found"));
        assert_eq!(calibre.help.as_deref(), Some(Engine::Calibre.purpose()));
        named(
            calibre,
            Role::Text,
            "Kiln finds it again as soon as it's installed.",
        );
        named(calibre, Role::Button, "Get Calibre");
        named(calibre, Role::Button, "Choose\u{2026}");
    }

    #[test]
    fn what_settings_has_scrolled_out_of_view_is_off_screen() {
        use crate::settings_page::Card;
        let lines = empty_lines();
        let mut i = input(&lines, vec![]);
        i.settings = Some(settings_input(None));
        let t = tree(&i);
        assert!(
            !t.find(&Id::SettingsCard(Card::Theme))
                .unwrap()
                .states
                .offscreen
        );
        let licences = t.find(&Id::SettingsCard(Card::Licences)).unwrap();
        assert!(licences.states.offscreen);
        assert!(
            t.find(&Id::Setting(Control::Licences))
                .unwrap()
                .states
                .offscreen
        );
    }

    #[test]
    fn an_open_combo_box_lists_its_choices_with_the_highlight_focused() {
        let lines = empty_lines();
        let mut s = settings_input(Some(Control::Theme));
        let anchor = s.layout.controls[s.layout.find(Control::Theme).unwrap()].1;
        let mut state = crate::controls::ComboState::new(9, 0);
        state.click();
        state.highlight = 2;
        let list = layout::kit::combo_flyout(anchor, 9, 0, Rect::new(0.0, 0.0, 800.0, 600.0));
        s.combo = Some((Control::Theme, state, list));
        let mut i = input(&lines, vec![]);
        i.settings = Some(s);
        let t = tree(&i);
        sound(&t);
        let combo = t.find(&Id::Combo(Control::Theme)).unwrap();
        assert_eq!(combo.states.expanded, Some(true));
        let names: Vec<&str> = combo.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
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
        );
        assert_eq!(
            t.focused().unwrap().id,
            Id::Choice(Box::new(Id::Combo(Control::Theme)), 2)
        );
    }

    #[test]
    fn the_details_dialog_reads_its_text_and_buttons() {
        let lines = empty_lines();
        let es = [entry("a.mov", JobState::Idle)];
        let mut i = input(&lines, rows(&es));
        i.details = Some(DetailsIn {
            layout: crate::layout::kit::details_dialog((800.0, 600.0), 28.0, 20.0, 16.0),
            title: "What happened",
            lead: "This file looks damaged, or isn't really an MP4.",
            text: "moov atom not found",
            copy: "Copy",
            focus: DialogButton::Copy,
        });
        let t = tree(&i);
        sound(&t);
        // The dialog, after the title bar.
        assert_eq!(t.children.len(), 2);
        let d = named(&t, Role::Dialog, "What happened");
        // A modal ContentDialog: closing it is Close.
        assert!(d.states.modal && d.patterns().window);
        // The plain line, then the engine's words.
        let lead = named(
            d,
            Role::Text,
            "This file looks damaged, or isn't really an MP4.",
        );
        let said = named(d, Role::Text, "moov atom not found");
        assert!(lead.rect.y < said.rect.y);
        assert!(named(d, Role::Button, "Copy").states.focused);
        named(d, Role::Button, "Close");
    }

    #[test]
    fn help_is_a_modal_dialog_reading_each_section_then_its_link_and_close() {
        let lines = empty_lines();
        let sections = help_content(crate::help::Results::Beside);
        let mut i = input(&lines, vec![]);
        i.help = Some(help_in(&sections));
        let t = tree(&i);
        sound(&t);
        let d = named(&t, Role::Dialog, crate::help::TITLE);
        assert!(d.states.modal);
        assert!(d.patterns().window);
        // Each section, then Show first-run help and Close.
        assert_eq!(d.children.len(), sections.len() + 2);
        assert!(named(d, Role::Button, "Close").states.focused);
        let guide = named(d, Role::Hyperlink, "Show first-run help");
        assert!(guide.states.focusable && !guide.states.focused);
        assert!(guide.patterns().invoke);
        assert_eq!(d.children[sections.len()].id, Id::HelpGuide);
        // Tab moves the keyboard to it.
        let mut hi = help_in(&sections);
        hi.focus = HelpButton::Guide;
        i.help = Some(hi);
        let t = tree(&i);
        assert_eq!(t.focused().unwrap().id, Id::HelpGuide);
        // The sections read as the mock has them.
        let convert = named(&t, Role::Group, "Convert files");
        assert_eq!(
            convert.children[0].name,
            "Drop files on the window, or choose Add files, then pick a format. The new file is saved next to the original."
        );
        let shortcuts = named(&t, Role::Group, "Shortcuts");
        assert_eq!(
            shortcuts.children[0].name,
            "Enter: Convert the selected files"
        );
        assert!(shortcuts
            .children
            .iter()
            .any(|c| c.name == "Ctrl+, (comma): Settings"));
    }

    #[test]
    fn the_empty_window_reads_its_lines_then_the_first_run_card() {
        let lines = empty_lines();
        let fr = crate::ways_in::first_run(crate::help::Results::Beside);
        let mut i = input(&lines, vec![]);
        i.first_run = Some(&fr);
        i.empty_status = "There are no files to paste.";
        let t = tree(&i);
        sound(&t);
        let panel = named(&t, Role::Group, "Drop files to convert");
        let ids: Vec<&Id> = panel.children.iter().map(|c| &c.id).collect();
        assert_eq!(
            ids,
            [
                &Id::DropLine(0),
                &Id::AddFiles,
                &Id::DropLine(1),
                &Id::FirstRunCard
            ]
        );
        assert_eq!(
            panel.children[0].name,
            "Photos, videos, sound, documents and e-books."
        );
        assert_eq!(panel.children[2].name, "There are no files to paste.");
        // The card: a group named by its heading, its steps read with
        // their numbers, the right-click line, then Dismiss.
        let card = &panel.children[3];
        assert_eq!(
            (card.role, card.name.as_str()),
            (Role::Group, "How it works")
        );
        let said: Vec<(&Id, &str)> = card
            .children
            .iter()
            .map(|c| (&c.id, c.name.as_str()))
            .collect();
        assert_eq!(
            said,
            [
                (&Id::FirstRunLine(0), "1. Drop files here, or choose Add files."),
                (
                    &Id::FirstRunLine(1),
                    "2. Pick a format. To keep the format and shrink the file, pick Make smaller."
                ),
                (
                    &Id::FirstRunLine(2),
                    "3. The new file is saved next to the original. Show in folder takes you to it."
                ),
                (
                    &Id::FirstRunLine(3),
                    "You can also right-click files in File Explorer and choose Open with, then Kiln."
                ),
                (&Id::FirstRunDismiss, "Dismiss"),
            ]
        );
        for c in &card.children[..4] {
            assert_eq!(c.role, Role::Text);
            assert!(!c.states.offscreen);
        }
        let dismiss = &card.children[4];
        assert_eq!(dismiss.role, Role::Button);
        assert!(dismiss.states.focusable && dismiss.patterns().invoke);
        // Dismiss takes the keyboard as the empty window's other buttons do.
        assert_eq!(button_of(&Id::FirstRunDismiss), Some(FocusButton::Dismiss));
        assert_eq!(button_id(FocusButton::Dismiss), Id::FirstRunDismiss);
        i.focus_button = Some(FocusButton::Dismiss);
        assert_eq!(tree(&i).focused().unwrap().id, Id::FirstRunDismiss);
        // Dismissed: no card.
        i.first_run = None;
        i.focus_button = None;
        let t = tree(&i);
        assert!(t.find(&Id::FirstRunCard).is_none());
        assert!(t.find(&Id::FirstRunDismiss).is_none());
    }

    #[test]
    fn the_first_run_cards_parts_are_where_they_are_drawn_and_off_screen_past_the_layer() {
        let lines = empty_lines();
        let fr = crate::ways_in::first_run(crate::help::Results::Beside);
        let mut i = input(&lines, vec![]);
        i.first_run = Some(&fr);
        // Where the window drew them.
        let layer = layout::content_layer(800.0, 600.0, 48.0);
        let wrapped = layout::CardText {
            steps: [20.0, 40.0, 40.0],
            more: 16.0,
        };
        let e = layout::empty_state(&layer, Some(wrapped));
        i.empty_at = Some(e);
        let t = tree(&i);
        let c = e.card.unwrap();
        assert_eq!(t.find(&Id::FirstRunCard).unwrap().rect, c.card);
        assert_eq!(t.find(&Id::FirstRunLine(1)).unwrap().rect, c.steps[1]);
        assert_eq!(t.find(&Id::FirstRunLine(3)).unwrap().rect, c.more);
        assert_eq!(t.find(&Id::FirstRunDismiss).unwrap().rect, c.dismiss);
        assert_eq!(t.find(&Id::AddFiles).unwrap().rect, e.button);
        // A short window: what runs past the layer's bottom is off screen.
        i.size = (480.0, 360.0);
        let short = layout::content_layer(480.0, 360.0, 48.0);
        let e = layout::empty_state(&short, Some(wrapped));
        i.empty_at = Some(e);
        let t = tree(&i);
        assert!(!t.find(&Id::FirstRunLine(0)).unwrap().states.offscreen);
        assert!(t.find(&Id::FirstRunLine(3)).unwrap().states.offscreen);
        assert!(!t.find(&Id::FirstRunDismiss).unwrap().states.offscreen);
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
        let lines = empty_lines();
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
        let lines = empty_lines();
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
    fn the_options_dialog_reads_its_quality_estimate_more_options_and_buttons() {
        let lines = empty_lines();
        let mut shape = layout::sheet_shape(&[kiln_core::Knob::Preset, kiln_core::Knob::Size]);
        shape.more_open = true;
        let l = layout::options_dialog((800.0, 600.0), &shape);
        let more = [SheetLine {
            label: "Size".to_owned(),
            value: "1080p".to_owned(),
            enabled: true,
            choices: Some(vec!["Original".into(), "1080p".into(), "720p".into()]),
        }];
        let mut i = input(&lines, vec![]);
        i.sheet = Some(SheetIn {
            layout: &l,
            title: "Convert to MP4",
            sub: "interview.mov, 3.4 MB. MP4: video",
            notes: &[],
            result: "1080p, about 1.1 MB",
            error: false,
            lines: &more,
            selected: Some(1),
            radios: &[],
            list: None,
            more_open: true,
            focus: Some(0),
            convert_enabled: true,
        });
        let t = tree(&i);
        sound(&t);
        // The dialog, after the title bar: modal, a window to UI Automation.
        assert_eq!(t.children.len(), 2);
        let d = named(&t, Role::Dialog, "Convert to MP4");
        assert!(d.states.modal);
        // Quality: a group of radio buttons, High selected.
        let quality = named(d, Role::Group, "Quality");
        assert!(quality.patterns().selection);
        let names: Vec<&str> = quality.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Small", "High", "Best"]);
        assert!(quality
            .children
            .iter()
            .all(|c| c.role == Role::RadioButton && c.patterns().selection_item));
        let chosen: Vec<&str> = quality
            .children
            .iter()
            .filter(|c| c.states.selected)
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(chosen, ["High"]);
        assert!(quality.children[0].states.focused);
        // The estimate, said as it changes.
        let est = named(d, Role::Text, "1080p, about 1.1 MB");
        assert!(est.live);
        // More options: an expander holding its lines as drop-downs, closed.
        let m = named(d, Role::Group, "More options");
        assert_eq!(m.states.expanded, Some(true));
        assert!(m.patterns().expand_collapse);
        let size = named(m, Role::ComboBox, "Size");
        assert_eq!(size.value.as_deref(), Some("1080p"));
        assert_eq!(size.states.expanded, Some(false));
        assert!(size.children.is_empty());
        // One sub-line under the title.
        named(d, Role::Text, "interview.mov, 3.4 MB. MP4: video");
        // Open, its list's items are under it and the keyboard is on the
        // highlight.
        let open = layout::kit::combo_flyout(
            l.lines[0].2,
            3,
            1,
            layout::Rect::new(0.0, 0.0, 800.0, 600.0),
        );
        let mut state = crate::controls::ComboState::new(3, 1);
        state.click();
        let c = SheetControl::Step(1);
        let at = l.controls.iter().position(|(k, _)| *k == c);
        if let Some(sheet) = i.sheet.as_mut() {
            sheet.list = Some((c, state, open));
            sheet.focus = at;
        }
        let t = tree(&i);
        let d = named(&t, Role::Dialog, "Convert to MP4");
        let size = named(
            named(d, Role::Group, "More options"),
            Role::ComboBox,
            "Size",
        );
        assert_eq!(size.states.expanded, Some(true));
        let items: Vec<&str> = size.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(items, ["Original", "1080p", "720p"]);
        assert_eq!(
            focus(&i),
            Some(Id::Choice(Box::new(Id::SheetControl(c)), 1))
        );
        named(d, Role::Button, "Cancel");
        named(d, Role::Button, "Convert");
        // Convert first, then Cancel, as the band draws them.
        let buttons: Vec<&str> = d
            .children
            .iter()
            .filter(|c| c.role == Role::Button)
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(buttons, ["Convert", "Cancel"]);
    }

    #[test]
    fn the_options_dialog_reads_its_notes_and_a_max_size_it_cant_reach() {
        let lines = empty_lines();
        let notes = [
            kiln_core::first_of_tracks(2),
            kiln_core::NO_SUBTITLES.to_owned(),
        ];
        let mut shape = layout::sheet_shape(&[kiln_core::Knob::VideoTune]);
        shape.notes = notes.len();
        shape.tune_radios = 4;
        let l = layout::options_dialog((800.0, 600.0), &shape);
        let radios = ["Small", "High", "Best", crate::tune_sheet::ORIGINAL_RADIO];
        let mut i = input(&lines, vec![]);
        i.sheet = Some(SheetIn {
            layout: &l,
            title: "Convert to MP4",
            sub: "interview.mkv, 3.4 MB. MP4: Video that plays almost anywhere",
            notes: &notes,
            result: "Can't get this under 5 MB.",
            error: true,
            lines: &[],
            selected: Some(3),
            radios: &radios,
            list: None,
            more_open: false,
            focus: None,
            convert_enabled: false,
        });
        let t = tree(&i);
        sound(&t);
        let d = named(&t, Role::Dialog, "Convert to MP4");
        // Each note is a text node with its sentence, on its own line,
        // after the source's line.
        for (k, note) in notes.iter().enumerate() {
            let n = named(d, Role::Text, note);
            assert_eq!(n.rect, l.note_line(k), "{note}");
        }
        let texts: Vec<&str> = d
            .children
            .iter()
            .filter(|c| c.role == Role::Text)
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(
            texts,
            [
                "interview.mkv, 3.4 MB. MP4: Video that plays almost anywhere",
                "Keeps the first of 2 sound tracks.",
                "Subtitles aren't kept.",
                "Can't get this under 5 MB."
            ]
        );
        // The error is said as one, and Convert waits.
        let e = named(d, Role::Text, "Can't get this under 5 MB.");
        assert!(e.live);
        assert_eq!(e.status.as_deref(), Some("Error"));
        assert!(named(d, Role::Button, "Convert").states.disabled);
        // The video's quality is four radio buttons: Small, High, Best
        // and Original (no re-encode), the last on, each one a selection
        // item with a name of its own.
        let group = named(d, Role::Group, "Quality");
        let names: Vec<(&str, bool)> = group
            .children
            .iter()
            .map(|c| (c.name.as_str(), c.states.selected))
            .collect();
        assert_eq!(
            names,
            [
                ("Small", false),
                ("High", false),
                ("Best", false),
                ("Original (no re-encode)", true)
            ]
        );
        assert!(group.children.iter().all(|c| c.role == Role::RadioButton
            && c.states.focusable
            && c.patterns().selection_item));
        // No slider, and none of a step's own words in the tree here.
        assert!(!d.children.iter().any(|c| c.role == Role::Slider));
    }

    #[test]
    fn the_sound_and_video_options_have_no_encoder_or_bitrate_mode() {
        use crate::tune_sheet::{SheetKind, TuneField, TuneSheet, VideoSheet};
        use kiln_media::video::{VideoCodec, VideoTarget};
        use kiln_media::Target;
        let song = kiln_core::MediaInfo {
            duration: Some(221.0),
            has_audio: true,
            channels: Some(2),
            rate: Some(44_100),
            ..Default::default()
        };
        let clip = kiln_core::MediaInfo {
            duration: Some(42.0),
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            video_codec: Some(VideoCodec::H264),
            video_kbps: Some(6000),
            ..Default::default()
        };
        let q = kiln_core::Quality::default();
        let sheets = [
            (
                SheetKind::Audio(TuneSheet::new(Target::Mp3, &song, &q)),
                song,
            ),
            (
                SheetKind::Video(VideoSheet::new(VideoTarget::Mp4, &clip, &q)),
                clip,
            ),
        ];
        for (mut ts, info) in sheets {
            ts.set_mark(1);
            let names: Vec<&str> = ts.fields().iter().map(|f| f.name()).collect();
            for cut in ["Encoder", "Bitrate mode"] {
                assert!(!names.contains(&cut), "{names:?}");
            }
            // Max size, which was Fit under, is one of More options' lines.
            assert!(ts.fields().contains(&TuneField::FitUnder), "{names:?}");
            assert_eq!(TuneField::FitUnder.name(), "Max size");
            assert!(ts.can_convert(&info));
            // The video keeps Frame rate here, as Settings no longer
            // has it.
            if matches!(ts, SheetKind::Video(_)) {
                assert!(ts.fields().contains(&TuneField::FrameRate), "{names:?}");
            }
        }
    }

    /// harbour.png, waiting: WEBP suggested at about 410 KB, then AVIF and
    /// JPG, and More.
    fn harbour_waiting() -> Inspector {
        let target = |ext: &str, estimate, suggested| Target {
            ext: ext.to_owned(),
            why: kiln_core::describe_format(ext),
            estimate,
            suggested,
        };
        Inspector::Waiting {
            row: 0,
            facts: vec![
                ("Size", "2.4 MB".to_owned()),
                ("Dimensions", "1600 × 1200".to_owned()),
                ("Folder", "photos".to_owned()),
            ],
            targets: vec![
                target("webp", Some(410_000), true),
                target("avif", Some(380_000), false),
                target("jpg", None, false),
            ],
            more: true,
            helper: None,
            reason: None,
            smaller: false,
        }
    }

    /// How wide "More formats…" is at 13 px, near enough.
    const MORE_W: f32 = 88.0;

    /// The inspector at the right of a window `size` holding `rows` files,
    /// laid out for `shape`.
    fn inspector_at(
        size: (f32, f32),
        rows: usize,
        shape: InspectorShape,
    ) -> (Rect, layout::InspectorLayout) {
        let rect = layout::layout(size.0, size.1, rows).inspector.unwrap();
        (rect, layout::inspector_layout(&rect, &shape))
    }

    #[test]
    fn the_inspector_is_a_pane_after_the_list_with_named_controls() {
        let lines = empty_lines();
        let es = [entry("harbour.png", JobState::Idle)];
        let model = harbour_waiting();
        let size = (1300.0, 760.0);
        let shape = InspectorShape::Waiting {
            what_h: 18.0,
            facts: 3,
            smaller: false,
            targets: 3,
            more: Some(MORE_W),
            buttons: 0,
        };
        let (rect, il) = inspector_at(size, es.len(), shape);
        let mut i = input(&lines, rows(&es));
        i.size = size;
        i.row_links = false;
        i.inspector = Some(InspectorIn {
            rect,
            model: &model,
            layout: &il,
            file: "harbour.png",
            folder: "C:\\photos\\2026",
        });
        let t = tree(&i);
        sound(&t);
        let pane = t.find(&Id::Inspector).unwrap();
        assert_eq!(pane.name, "Selected file");
        assert_eq!((pane.role, pane.rect), (Role::Group, rect));
        let first = t
            .find(&Id::InspectorControl(InspectorControl::Target(0)))
            .unwrap();
        assert_eq!(first.role, Role::Button);
        assert_eq!(
            first.name,
            "Convert to WEBP, Small image for the web, about 410 KB"
        );
        assert!(first.states.focusable && first.patterns().invoke);
        assert_eq!(first.rect, il.targets[0]);
        let preview = t.find(&Id::InspectorPreview).unwrap();
        assert_eq!(preview.name, "Preview of harbour.png");
        assert_eq!((preview.role, preview.rect), (Role::Image, il.hero));
        let more = t
            .find(&Id::InspectorControl(InspectorControl::More))
            .unwrap();
        assert_eq!(more.name, "More formats for harbour.png");
        // As wide as its words and a link's room past them, as drawn.
        assert_eq!(more.rect, il.more.unwrap());
        assert_eq!(more.rect.w, MORE_W + layout::LINK_PAD);
        // The texts are text: the file and each fact. A healthy file has no
        // line saying what its format is.
        named(&t, Role::Text, "harbour.png");
        assert!(
            !pane
                .children
                .iter()
                .any(|c| c.name == "Lossless image with transparency"),
            "no kind line under the name"
        );
        assert_eq!(named(&t, Role::Text, "Size 2.4 MB").rect, il.facts[0]);
        // The folder is a link, named for what it does and helped by the
        // whole path, in the place of the fact's value; it has no text.
        assert!(!pane.children.iter().any(|c| c.name == "Folder photos"));
        let folder = t
            .find(&Id::InspectorControl(InspectorControl::Folder))
            .unwrap();
        assert_eq!(folder.role, Role::Hyperlink);
        assert_eq!(folder.name, "Open the folder of harbour.png");
        assert_eq!(folder.help.as_deref(), Some("C:\\photos\\2026"));
        assert!(folder.states.focusable && folder.patterns().invoke);
        assert_eq!(folder.rect.x, il.facts[2].x + layout::FACT_LABEL);
        // Its buttons in reading order, which is the Tab order.
        use InspectorControl as C;
        let buttons: Vec<Id> = pane
            .children
            .iter()
            .filter(|c| c.role == Role::Button)
            .map(|c| c.id.clone())
            .collect();
        assert_eq!(
            buttons,
            [C::Target(0), C::Target(1), C::Target(2), C::More].map(Id::InspectorControl)
        );
        let links: Vec<Id> = pane
            .children
            .iter()
            .filter(|c| c.role == Role::Hyperlink)
            .map(|c| c.id.clone())
            .collect();
        assert_eq!(links, [Id::InspectorControl(C::Folder)]);
        // The pane comes after the list among the root's children, before
        // the footer.
        let at = |id: &Id| t.children.iter().position(|c| c.id == *id).unwrap();
        assert_eq!(at(&Id::Inspector), at(&Id::List) + 1);
        // Side by side: the list holds only its part of the panel.
        assert!(t.find(&Id::List).unwrap().rect.right() < rect.x);
        assert_eq!(t.hit(rect.x + 4.0, rect.bottom() - 4.0).id, Id::Inspector);
        assert!(at(&Id::Inspector) < at(&Id::AddFiles));
        // The footer reads as it is drawn, left to right: Convert last.
        assert!(at(&Id::AddFiles) < at(&Id::Clear));
        assert!(at(&Id::Clear) < at(&Id::ConvertButton));
        // Tab onto a target: it has the focus.
        i.focus_button = Some(FocusButton::Inspector(C::Target(1)));
        let t = tree(&i);
        assert_eq!(t.focused().unwrap().id, Id::InspectorControl(C::Target(1)));
    }

    #[test]
    fn inspector_buttons_are_named_with_their_file() {
        use InspectorControl as C;
        let done = |kept| Inspector::Done {
            row: 0,
            result: "voice memo.mp3".to_owned(),
            change: None,
            kept,
            note: None,
            options: false,
            place: String::new(),
            smaller: false,
            again: vec![],
        };
        assert_eq!(
            inspector_name(C::ShowInFolder, &done(Kept::InFolder), "voice memo.m4a"),
            "Show voice memo.mp3 in folder"
        );
        let failed = Inspector::Failed {
            row: 0,
            sentence: "The conversion stopped before it finished. Try again.".to_owned(),
            tone: crate::model::Tone::Crack,
            retry: true,
            details: true,
            title: "Couldn\'t convert this file".to_owned(),
            cancelled: false,
        };
        assert_eq!(
            inspector_name(C::Retry, &failed, "harbour.png"),
            "Retry harbour.png"
        );
        assert_eq!(
            inspector_name(C::ShowDetails, &failed, "harbour.png"),
            "Show details for harbour.png"
        );
        let waiting = harbour_waiting();
        assert_eq!(
            inspector_name(C::More, &waiting, "harbour.png"),
            "More formats for harbour.png"
        );
        assert_eq!(
            inspector_name(C::Target(1), &waiting, "harbour.png"),
            "Convert to AVIF, Very small, modern image, about 380 KB"
        );
        // Several files with no format in common offer each kind.
        let several = Inspector::Several {
            rows: vec![0, 1, 2],
            count: 3,
            mixed: false,
            smaller: false,
            kinds: "2 videos, 1 document".to_owned(),
            targets: vec![],
            none_shared: Some("These files have no format in common.".to_owned()),
            offered: vec![("MP4".to_owned(), 2), ("DOCX".to_owned(), 1)],
            more: false,
            helper: None,
        };
        assert_eq!(
            inspector_name(C::Kind(0), &several, ""),
            "Select only the 2 MP4 files"
        );
        assert_eq!(
            inspector_name(C::Kind(1), &several, ""),
            "Select only the DOCX file"
        );
        // Get is named for the helper app it gets, as the row's link is.
        let needs = Inspector::Waiting {
            row: 0,
            facts: vec![],
            targets: vec![],
            more: false,
            helper: Some("Get FFmpeg"),
            reason: None,
            smaller: false,
        };
        assert_eq!(
            inspector_name(C::GetHelper, &needs, "clip.mkv"),
            "Get FFmpeg"
        );
    }

    #[test]
    fn a_file_waiting_on_its_helper_app_has_get_where_its_formats_would_be() {
        use InspectorControl as C;
        let lines = empty_lines();
        let es = [entry("clip.mkv", JobState::Idle)];
        let model = Inspector::Waiting {
            row: 0,
            facts: vec![
                ("Size", "40 MB".to_owned()),
                ("Folder", "videos".to_owned()),
            ],
            targets: vec![],
            more: false,
            helper: Some("Get FFmpeg"),
            reason: None,
            smaller: false,
        };
        let size = (1300.0, 760.0);
        let shape = InspectorShape::Waiting {
            what_h: 0.0,
            facts: 2,
            smaller: false,
            targets: 0,
            more: None,
            buttons: 1,
        };
        let (rect, il) = inspector_at(size, es.len(), shape);
        // It takes the button slot, after the facts.
        assert_eq!(
            inspector_controls(&model, &il),
            [(C::Folder, folder_rect(&il)), (C::GetHelper, il.buttons[0])]
        );
        let mut i = input(&lines, rows(&es));
        i.size = size;
        i.row_links = false;
        i.inspector = Some(InspectorIn {
            rect,
            model: &model,
            layout: &il,
            file: "clip.mkv",
            folder: "",
        });
        let t = tree(&i);
        sound(&t);
        let get = named(&t, Role::Button, "Get FFmpeg");
        assert_eq!(get.id, Id::InspectorControl(C::GetHelper));
        assert_eq!(get.rect, il.buttons[0]);
        assert!(get.states.focusable && get.patterns().invoke);
        // The row has no link while the inspector shows, so this is the way.
        assert!(t.find(&Id::RowLink(es[0].path.clone())).is_none());
        // Tab onto it: it has the focus.
        i.focus_button = Some(FocusButton::Inspector(C::GetHelper));
        let t = tree(&i);
        assert_eq!(t.focused().unwrap().id, Id::InspectorControl(C::GetHelper));
    }

    #[test]
    fn several_files_waiting_on_one_helper_app_have_get_in_the_tree() {
        use InspectorControl as C;
        let lines = empty_lines();
        let es = [
            entry("a.mp4", JobState::Idle),
            entry("b.mp4", JobState::Idle),
        ];
        let model = Inspector::Several {
            rows: vec![0, 1],
            count: 2,
            mixed: false,
            smaller: false,
            kinds: "2 videos".to_owned(),
            targets: vec![],
            more: false,
            none_shared: Some(
                "MP4 files need FFmpeg, which isn't installed. Use Get FFmpeg to download it."
                    .to_owned(),
            ),
            offered: vec![],
            helper: Some("Get FFmpeg"),
        };
        let size = (1300.0, 760.0);
        let shape = InspectorShape::Several {
            targets: 0,
            info: Some(layout::InfoShape {
                message_h: 40.0,
                titled: false,
                actions: [0.0; 4],
                count: 0,
            }),
            smaller: false,
            more: None,
            buttons: 1,
        };
        let (rect, il) = inspector_at(size, es.len(), shape);
        // Get takes the button slot, where the kinds would be.
        assert_eq!(
            inspector_controls(&model, &il),
            [(C::GetHelper, il.buttons[0])]
        );
        let mut i = input(&lines, rows(&es));
        i.size = size;
        i.row_links = false;
        i.inspector = Some(InspectorIn {
            rect,
            model: &model,
            layout: &il,
            file: "",
            folder: "",
        });
        let t = tree(&i);
        sound(&t);
        let pane = t.find(&Id::Inspector).unwrap();
        named(
            pane,
            Role::Text,
            "MP4 files need FFmpeg, which isn't installed. Use Get FFmpeg to download it.",
        );
        let get = named(&t, Role::Button, "Get FFmpeg");
        assert_eq!(get.id, Id::InspectorControl(C::GetHelper));
        assert_eq!(get.rect, il.buttons[0]);
        assert!(get.states.focusable && get.patterns().invoke);
        // Tab onto it: it has the focus.
        i.focus_button = Some(FocusButton::Inspector(C::GetHelper));
        let t = tree(&i);
        assert_eq!(t.focused().unwrap().id, Id::InspectorControl(C::GetHelper));
    }

    #[test]
    fn several_files_have_more_formats_after_their_shared_ones() {
        use InspectorControl as C;
        let lines = empty_lines();
        let es = [
            entry("a.png", JobState::Idle),
            entry("b.png", JobState::Idle),
            entry("c.png", JobState::Idle),
        ];
        let target = |ext: &str| Target {
            ext: ext.to_owned(),
            why: kiln_core::describe_format(ext),
            estimate: None,
            suggested: false,
        };
        let model = Inspector::Several {
            rows: vec![0, 1, 2],
            count: 3,
            mixed: false,
            smaller: false,
            kinds: "3 images".to_owned(),
            targets: vec![target("jpg"), target("pdf"), target("webp")],
            more: true,
            none_shared: None,
            offered: vec![],
            helper: None,
        };
        let size = (1300.0, 760.0);
        let shape = InspectorShape::Several {
            targets: 3,
            info: None,
            smaller: false,
            more: Some(MORE_W),
            buttons: 0,
        };
        let (rect, il) = inspector_at(size, es.len(), shape);
        let mut i = input(&lines, rows(&es));
        i.size = size;
        i.row_links = false;
        i.inspector = Some(InspectorIn {
            rect,
            model: &model,
            layout: &il,
            file: "",
            folder: "",
        });
        let t = tree(&i);
        sound(&t);
        let more = named(&t, Role::Button, "More formats for 3 images");
        assert_eq!(more.id, Id::InspectorControl(C::More));
        assert_eq!(Some(more.rect), il.more);
        // In reading order: the three formats, then More.
        let pane = t.find(&Id::Inspector).unwrap();
        let order: Vec<&Id> = pane
            .children
            .iter()
            .filter(|c| c.role == Role::Button)
            .map(|c| &c.id)
            .collect();
        assert_eq!(
            order,
            [C::Target(0), C::Target(1), C::Target(2), C::More]
                .map(Id::InspectorControl)
                .iter()
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_inspector_reads_the_rows_own_note_and_reason() {
        let lines = empty_lines();
        let es = [entry("harbour.png", JobState::Idle)];
        let size = (1300.0, 760.0);
        let read = |model: &Inspector, shape: InspectorShape| {
            let (rect, il) = inspector_at(size, es.len(), shape);
            let mut i = input(&lines, rows(&es));
            i.size = size;
            i.row_links = false;
            i.inspector = Some(InspectorIn {
                rect,
                model,
                layout: &il,
                file: "harbour.png",
                folder: "",
            });
            let t = tree(&i);
            sound(&t);
            (t, il)
        };
        // A held result removed by hand: the row's note where the drag-out
        // hint would be, and no Export.
        let gone = Inspector::Done {
            row: 0,
            result: "harbour.webp".to_owned(),
            change: None,
            kept: Kept::Gone,
            note: Some((crate::model::GONE.to_owned(), crate::model::Tone::Crack)),
            options: false,
            place: String::new(),
            smaller: false,
            again: vec![],
        };
        let (t, il) = read(
            &gone,
            InspectorShape::Done {
                info: layout::InfoShape {
                    message_h: 40.0,
                    titled: true,
                    actions: [96.0; 4],
                    count: 0,
                },
                smaller: false,
                again: 0,
            },
        );
        let pane = t.find(&Id::Inspector).unwrap();
        // In the InfoBar, after the result's name, where it is drawn.
        let said = format!("harbour.webp. {}", crate::model::GONE);
        assert_eq!(
            named(pane, Role::Text, &said).rect,
            il.info.as_ref().unwrap().message
        );
        named(pane, Role::Text, crate::inspector::CONVERTED);
        assert!(pane.children.iter().all(|c| !c.name.contains("Drag")));
        // A file ruled out on adding: why, where what it is would be.
        let ruled_out = Inspector::Waiting {
            row: 0,
            facts: vec![("Size", "2.0 MB".to_owned())],
            targets: vec![],
            more: false,
            helper: None,
            reason: Some((
                "Isn't really a DOCX file".to_owned(),
                crate::model::Tone::Crack,
            )),
            smaller: false,
        };
        let (t, il) = read(
            &ruled_out,
            InspectorShape::Waiting {
                what_h: 20.0,
                facts: 1,
                smaller: false,
                targets: 0,
                more: None,
                buttons: 0,
            },
        );
        let pane = t.find(&Id::Inspector).unwrap();
        assert_eq!(
            named(pane, Role::Text, "Isn't really a DOCX file").rect,
            il.line(0).unwrap()
        );
        assert!(pane.children.iter().all(|c| c.role != Role::Button));
    }

    #[test]
    fn a_narrow_window_has_no_inspector_in_the_tree() {
        let lines = empty_lines();
        let es = [entry("harbour.png", JobState::Idle)];
        let t = tree(&input(&lines, rows(&es)));
        assert!(t.find(&Id::Inspector).is_none());
        assert!(t.find(&Id::InspectorPreview).is_none());
        let mut controls = 0;
        t.walk(&mut |n| controls += usize::from(matches!(n.id, Id::InspectorControl(_))));
        assert_eq!(controls, 0);
    }

    #[test]
    fn only_the_controls_the_inspector_places_are_there() {
        use InspectorControl as C;
        // A short inspector cuts targets from the bottom, and More with
        // them: what isn't drawn is neither a Tab stop nor a node.
        let model = harbour_waiting();
        let rect = Rect::new(900.0, 72.0, 300.0, 220.0);
        let il = layout::inspector_layout(
            &rect,
            &InspectorShape::Waiting {
                what_h: 40.0,
                facts: 3,
                smaller: false,
                targets: 3,
                more: Some(MORE_W),
                buttons: 0,
            },
        );
        assert!(il.targets.len() < 3 && il.more.is_none());
        let placed = inspector_controls(&model, &il);
        assert_eq!(placed.len(), il.targets.len());
        for (k, (c, r)) in placed.iter().enumerate() {
            assert_eq!((*c, *r), (C::Target(k), il.targets[k]));
        }
        // Buttons take their slots in order.
        let failed = Inspector::Failed {
            row: 0,
            sentence: "x".to_owned(),
            tone: crate::model::Tone::Crack,
            retry: true,
            details: true,
            title: "Couldn\'t convert this file".to_owned(),
            cancelled: false,
        };
        let fl = layout::inspector_layout(
            &Rect::new(0.0, 0.0, 300.0, 600.0),
            &InspectorShape::Failed {
                info: layout::InfoShape {
                    message_h: 20.0,
                    titled: true,
                    actions: [96.0; 4],
                    count: 2,
                },
            },
        );
        assert_eq!(
            inspector_controls(&failed, &fl),
            [(C::Retry, fl.buttons[0]), (C::ShowDetails, fl.buttons[1])]
        );
        // With no picture shown, the tree has no preview either.
        let lines = empty_lines();
        let es = [entry("harbour.png", JobState::Idle)];
        let mut i = input(&lines, rows(&es));
        i.inspector = Some(InspectorIn {
            rect,
            model: &model,
            layout: &il,
            file: "harbour.png",
            folder: "",
        });
        let t = tree(&i);
        sound(&t);
        assert_eq!(il.hero.h, 0.0);
        assert!(t.find(&Id::InspectorPreview).is_none());
        assert!(t.find(&Id::InspectorControl(C::More)).is_none());
    }

    #[test]
    fn each_state_of_the_inspector_reads_its_words() {
        use InspectorControl as C;
        let lines = empty_lines();
        let es = [entry("harbour.png", JobState::Idle)];
        let size = (1300.0, 760.0);
        let read = |model: &Inspector, shape: InspectorShape, file: &str| {
            let (rect, il) = inspector_at(size, es.len(), shape);
            let mut i = input(&lines, rows(&es));
            i.size = size;
            i.inspector = Some(InspectorIn {
                rect,
                model,
                layout: &il,
                file,
                folder: "C:\\photos\\2026",
            });
            let t = tree(&i);
            sound(&t);
            t.find(&Id::Inspector).unwrap().clone()
        };
        let texts = |pane: &Node| -> Vec<String> {
            pane.children
                .iter()
                .filter(|c| c.role == Role::Text)
                .map(|c| c.name.clone())
                .collect()
        };
        // Nothing selected: the title and the key hint. The mark is no
        // picture of anything.
        let nothing_shape = InspectorShape::Nothing {
            title_h: 40.0,
            hint_h: 50.0,
        };
        let nothing = read(&Inspector::Nothing, nothing_shape, "");
        assert_eq!(
            texts(&nothing),
            [
                crate::inspector::NOTHING_TITLE,
                crate::inspector::NOTHING_HINT
            ]
        );
        // The key hint where it is drawn.
        let (_, il) = inspector_at(size, es.len(), nothing_shape);
        let rect_of = |pane: &Node, name: &str| named(pane, Role::Text, name).rect;
        assert_eq!(
            rect_of(&nothing, crate::inspector::NOTHING_HINT),
            il.line(0).unwrap()
        );
        assert!(il.line(1).is_none());
        assert!(nothing
            .children
            .iter()
            .all(|c| c.id != Id::InspectorPreview));
        // Converting: what it does, how far and how long.
        let converting = Inspector::Converting {
            row: 0,
            percent: Some(38),
            doing: "Converting to WEBP".to_owned(),
            left: Some("About 2 min left".to_owned()),
        };
        let shape = InspectorShape::Converting {
            figure: true,
            left: true,
        };
        let pane = read(&converting, shape, "harbour.png");
        assert_eq!(
            texts(&pane),
            [
                "harbour.png",
                "Converting to WEBP",
                "38%",
                "About 2 min left"
            ]
        );
        // Progress only: Cancel is the row's, so the pane has no button.
        assert!(pane.children.iter().all(|c| c.role != Role::Button));
        let (_, il) = inspector_at(size, es.len(), shape);
        assert_eq!(rect_of(&pane, "Converting to WEBP"), il.line(0).unwrap());
        assert_eq!(rect_of(&pane, "38%"), il.line(1).unwrap());
        assert_eq!(rect_of(&pane, "About 2 min left"), il.line(3).unwrap());
        // An engine that can't say how far along: no figure and no time.
        let unknown = Inspector::Converting {
            row: 0,
            percent: None,
            doing: "Converting to WEBP".to_owned(),
            left: None,
        };
        let shape = InspectorShape::Converting {
            figure: false,
            left: false,
        };
        let pane = read(&unknown, shape, "harbour.png");
        assert_eq!(texts(&pane), ["harbour.png", "Converting to WEBP"]);
        // Done: the InfoBar's title and message, Show in folder, Open,
        // Convert again.
        let target = |ext: &str| crate::inspector::Target {
            ext: ext.to_owned(),
            why: None,
            estimate: None,
            suggested: false,
        };
        let done = Inspector::Done {
            row: 0,
            result: "harbour.webp".to_owned(),
            change: Some("2.4 MB → 410 KB, 83% smaller".to_owned()),
            kept: Kept::InFolder,
            note: None,
            options: false,
            place: String::new(),
            smaller: false,
            again: vec![target("jpg"), target("pdf")],
        };
        let done_shape = InspectorShape::Done {
            info: layout::InfoShape {
                message_h: 60.0,
                titled: true,
                actions: [64.0, 64.0, 0.0, 0.0],
                count: 2,
            },
            smaller: false,
            again: 2,
        };
        let pane = read(&done, done_shape, "harbour.png");
        let message = "harbour.webp. 2.4 MB → 410 KB, 83% smaller.";
        assert_eq!(
            texts(&pane),
            [
                "harbour.png",
                crate::inspector::SAVED,
                message,
                crate::inspector::CONVERT_AGAIN,
            ]
        );
        let (_, il) = inspector_at(size, es.len(), done_shape);
        let bar = il.info.clone().unwrap();
        assert_eq!(rect_of(&pane, crate::inspector::SAVED), bar.title);
        assert_eq!(rect_of(&pane, message), bar.message);
        assert_eq!(
            named(&pane, Role::Button, "Show harbour.webp in folder").rect,
            bar.actions[0]
        );
        assert_eq!(
            named(&pane, Role::Button, "Open harbour.webp").id,
            Id::InspectorControl(C::Open)
        );
        assert_eq!(
            named(&pane, Role::Button, "Convert to PDF").id,
            Id::InspectorControl(C::Target(1))
        );
        // Several sharing nothing: how many, their kinds, why, and each
        // kind to pick.
        let several = Inspector::Several {
            rows: vec![0, 1, 2],
            count: 3,
            mixed: true,
            smaller: false,
            kinds: "2 videos, 1 document".to_owned(),
            targets: vec![],
            none_shared: Some("These files have no format in common.".to_owned()),
            offered: vec![("MP4".to_owned(), 2), ("DOCX".to_owned(), 1)],
            more: false,
            helper: None,
        };
        let pane = read(
            &several,
            InspectorShape::Several {
                targets: 2,
                info: Some(layout::InfoShape {
                    message_h: 20.0,
                    titled: false,
                    actions: [0.0; 4],
                    count: 0,
                }),
                smaller: false,
                more: None,
                buttons: 0,
            },
            "",
        );
        assert_eq!(
            texts(&pane),
            [
                "3 files",
                "2 videos, 1 document",
                "These files have no format in common.",
                crate::inspector::ONE_KIND,
            ]
        );
        named(&pane, Role::Button, "Select only the 2 MP4 files");
        named(&pane, Role::Button, "Select only the DOCX file");
        assert!(pane.children.iter().all(|c| c.id != Id::InspectorPreview));
        // Failed: the file, then its InfoBar with Retry and Show details
        // in it.
        let failed = Inspector::Failed {
            row: 0,
            title: "Couldn't read this PDF".to_owned(),
            sentence: "This PDF looks damaged.".to_owned(),
            tone: crate::model::Tone::Crack,
            retry: false,
            details: true,
            cancelled: false,
        };
        let shape = InspectorShape::Failed {
            info: layout::InfoShape {
                message_h: 20.0,
                titled: true,
                actions: [96.0, 120.0, 0.0, 0.0],
                count: 2,
            },
        };
        let pane = read(&failed, shape, "harbour.png");
        assert_eq!(
            texts(&pane),
            [
                "harbour.png",
                "Couldn't read this PDF",
                "This PDF looks damaged."
            ]
        );
        named(&pane, Role::Button, "Show details for harbour.png");
    }

    #[test]
    fn a_text_the_inspector_has_no_room_for_is_not_read() {
        let lines = empty_lines();
        let es = [entry("harbour.png", JobState::Idle)];
        // An inspector `h` tall, as short windows and the fold leave it.
        let read = |model: &Inspector, shape: InspectorShape, h: f32| {
            let rect = Rect::new(900.0, 72.0, 320.0, h);
            let il = layout::inspector_layout(&rect, &shape);
            let mut i = input(&lines, rows(&es));
            i.inspector = Some(InspectorIn {
                rect,
                model,
                layout: &il,
                file: "harbour.png",
                folder: "",
            });
            let t = tree(&i);
            sound(&t);
            let pane = t.find(&Id::Inspector).unwrap();
            let texts: Vec<String> = pane
                .children
                .iter()
                .filter(|c| c.role == Role::Text)
                .map(|c| c.name.clone())
                .collect();
            // Every text node is where something is drawn: no 0-tall rect.
            assert!(pane.children.iter().all(|c| c.rect.h > 0.0), "{pane:?}");
            (texts, il)
        };
        let converting = Inspector::Converting {
            row: 0,
            percent: Some(38),
            doing: "Converting to WEBP".to_owned(),
            left: Some("About 2 min left".to_owned()),
        };
        let running = InspectorShape::Converting {
            figure: true,
            left: true,
        };
        // Room for the title, what it does and the percentage, but not the
        // time under the bar: the time isn't read, and nor is Cancel.
        // (Without its picture, which goes first.)
        let top = 16.0 + layout::INSPECTOR_TITLE;
        let h = top + 4.0 + 20.0 + 4.0 + layout::INSPECTOR_FIGURE + 4.0 + 4.0 + 10.0 + 16.0;
        let (said, il) = read(&converting, running, h);
        assert_eq!(said, ["harbour.png", "Converting to WEBP", "38%"]);
        assert_eq!(il.lines[3].h, 0.0);
        assert!(il.buttons.is_empty());
        // Room for the title alone: no line is read.
        let (said, il) = read(&converting, running, 16.0 + layout::INSPECTOR_TITLE + 16.0);
        assert_eq!(said, ["harbour.png"]);
        assert!(il.lines.iter().all(|r| r.h == 0.0));
        // Nothing selected, with room for the title but not the key hint
        // under it, and then with room for both.
        let nothing = InspectorShape::Nothing {
            title_h: 40.0,
            hint_h: 50.0,
        };
        let (said, _) = read(&Inspector::Nothing, nothing, 32.0 + 40.0 + 8.0 + 40.0);
        assert_eq!(said, [crate::inspector::NOTHING_TITLE]);
        let (said, _) = read(&Inspector::Nothing, nothing, 32.0 + 40.0 + 8.0 + 50.0);
        assert_eq!(
            said,
            [
                crate::inspector::NOTHING_TITLE,
                crate::inspector::NOTHING_HINT
            ]
        );
    }

    #[test]
    fn requests_for_elements_that_are_gone_are_dropped() {
        let lines = empty_lines();
        let es = [entry("a.png", JobState::Idle)];
        let ask = |id: Id, verb: Verb| Request { id, verb };

        // With Settings open, the list's buttons aren't there to press.
        let mut i = input(&lines, rows(&es));
        i.settings = Some(settings_input(None));
        let open = tree(&i);
        assert!(open.find(&Id::Clear).is_none());
        assert!(!accepts(&open, &ask(Id::Clear, Verb::Invoke)));
        assert!(!accepts(&open, &ask(Id::AddFiles, Verb::Invoke)));
        let theme = Id::Combo(Control::Theme);
        assert!(accepts(&open, &ask(theme.clone(), Verb::Expand(true))));

        // With Settings closed, its controls are gone.
        let closed = tree(&input(&lines, rows(&es)));
        assert!(accepts(&closed, &ask(Id::Clear, Verb::Invoke)));
        assert!(!accepts(&closed, &ask(theme, Verb::Expand(true))));
        let wheel = Id::Toggle(Control::DragWheel);
        assert!(!accepts(&closed, &ask(wheel, Verb::Toggle)));
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
        let lines = empty_lines();
        let es = [
            entry("a.png", JobState::Idle),
            entry(
                "b.png",
                JobState::Running {
                    action: Action::Convert("jpg".into()),
                    fraction: Some(0.5),
                    started: None,
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
        let lines = empty_lines();
        let running = |f: f32| {
            entry(
                "a.mov",
                JobState::Running {
                    action: Action::Convert("mp4".into()),
                    fraction: Some(f),
                    started: None,
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
        let alert = set("Couldn't copy the path.", true);
        for _ in 0..2 {
            assert_eq!(teller.tell([alert.clone()], true), vec![alert.clone()]);
        }
        // An empty status is never said.
        assert!(teller.tell([set("", false)], true).is_empty());
    }
}
