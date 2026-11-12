//! UI Automation: serves the tree `a11y::tree` builds to screen readers,
//! voice control and other assistive technology.
//!
//! UI Automation calls the providers on its own threads. They answer from
//! a snapshot of the tree, swapped in by the UI thread after each change,
//! and never touch `App`. What a client asks an element to do is sent to
//! the window on its channel (`channel::Event::A11y`), and the UI thread
//! does it the way a click
//! or a key would, if the element is still there. Events are raised once
//! the window is done with `App`, as a client can call straight back in.

#![allow(non_upper_case_globals)]

use std::collections::HashMap;
use std::mem::ManuallyDrop;
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use windows::core::{
    implement, Error, IUnknown, IUnknownImpl, Interface, Result, BOOL, BSTR, PCWSTR,
};
use windows::Win32::Foundation::{
    E_INVALIDARG, HWND, LPARAM, LRESULT, POINT, VARIANT_FALSE, VARIANT_TRUE, WPARAM,
};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Com::SAFEARRAY;
use windows::Win32::System::Ole::{SafeArrayCreateVector, SafeArrayDestroy, SafeArrayPutElement};
use windows::Win32::System::Variant::{
    VariantClear, VARIANT, VT_BOOL, VT_BSTR, VT_I4, VT_R8, VT_UNKNOWN,
};
use windows::Win32::UI::Accessibility::*;

use super::channel::{self, Wake};
use crate::a11y::{Id, Neighbour, Node, Request, Role, Toward, Verb};

mod text;

/// WM_GETOBJECT's lparam when UI Automation asks for the root.
pub const ROOT_OBJECT_ID: i32 = UiaRootObjectId;

/// The tree as it stands, with where to find each node.
struct Snapshot {
    root: Node,
    /// Each node's place below the root, by key.
    index: HashMap<u64, Vec<usize>>,
    /// Pixels per layout unit.
    unit_px: f32,
    /// Whether the window has the keyboard focus.
    has_focus: bool,
    /// False once the window is closing: every element is then gone.
    alive: bool,
}

struct Shared {
    hwnd: isize,
    snap: Mutex<Snapshot>,
    /// The way to the window, for what clients ask: it takes the request,
    /// or says the window won't hear it.
    post: Box<dyn Fn(Request) -> bool + Send + Sync>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Snapshot> {
        self.snap.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn hwnd(&self) -> HWND {
        HWND(self.hwnd as *mut core::ffi::c_void)
    }

    /// The window's client area's top left, in screen pixels.
    fn origin(&self) -> (f64, f64) {
        let mut p = POINT::default();
        let _ = unsafe { ClientToScreen(self.hwnd(), &mut p) };
        (f64::from(p.x), f64::from(p.y))
    }

    /// Ask the window to do something. Never called with the lock held.
    fn post(&self, id: Id, verb: Verb) -> Result<()> {
        if (self.post)(Request { id, verb }) {
            Ok(())
        } else {
            Err(gone())
        }
    }
}

/// What the window last told clients, so events go out only on change.
#[derive(Default)]
struct Said {
    focus: Option<u64>,
    rows: Vec<u64>,
}

/// Something to tell listening clients, raised by `raise` once the window
/// is done with `App`.
pub struct Event {
    shared: Weak<Shared>,
    kind: Kind,
}

enum Kind {
    /// A modal dialog appeared (`UIA_Window_WindowOpenedEventId`).
    Opened(u64),
    /// The right-click menu appeared (`UIA_MenuOpenedEventId`).
    MenuOpened(u64),
    /// It went (`UIA_MenuClosedEventId`), told from the window, as the
    /// menu's own element is gone.
    MenuClosed,
    Focus(u64),
    Rows(u64),
    Status(String, bool),
    /// A property of the element with this key changed.
    Changed(u64, Change),
}

/// A property clients follow, from what it was to what it is.
#[derive(Debug, Clone, PartialEq)]
enum Change {
    /// A slider's step (`UIA_RangeValueValuePropertyId`).
    Range(f64, f64),
    /// A slider's value text (`UIA_ValueValuePropertyId`).
    Value(String, String),
    /// A disclosure opened or closed
    /// (`UIA_ExpandCollapseExpandCollapseStatePropertyId`).
    Expanded(bool, bool),
    /// A ToggleSwitch flipped (`UIA_ToggleToggleStatePropertyId`).
    Toggled(bool, bool),
}

/// What changed from `old` to `new` that clients are told of: a slider's
/// step and value as it moves, a ComboBox's or text box's value, a
/// disclosure opening or closing, and a ToggleSwitch flipping. Only
/// elements in both trees count; one that came or went is a structure
/// change, not a property's.
fn changes(old: &Node, old_index: &HashMap<u64, Vec<usize>>, new: &Node) -> Vec<(u64, Change)> {
    let mut out = Vec::new();
    for (key, path) in new.paths() {
        let Some(n) = new.at(&path) else {
            continue;
        };
        let valued = matches!(n.role, Role::Slider | Role::ComboBox | Role::Edit);
        if !valued && n.states.expanded.is_none() && n.states.toggled.is_none() {
            continue;
        }
        let Some(was) = old_index.get(&key).and_then(|p| old.at(p)) else {
            continue;
        };
        if n.role == Role::Slider {
            if let (Some(a), Some(b)) = (was.range, n.range) {
                if a != b {
                    out.push((key, Change::Range(a, b)));
                }
            }
        }
        if valued && was.value != n.value {
            let text = |v: &Option<String>| v.clone().unwrap_or_default();
            out.push((key, Change::Value(text(&was.value), text(&n.value))));
        }
        if let (Some(a), Some(b)) = (was.states.expanded, n.states.expanded) {
            if a != b {
                out.push((key, Change::Expanded(a, b)));
            }
        }
        if let (Some(a), Some(b)) = (was.states.toggled, n.states.toggled) {
            if a != b {
                out.push((key, Change::Toggled(a, b)));
            }
        }
    }
    out
}

/// The modal dialogs in `new` that weren't in the tree `old_index` maps:
/// a prompt or Details opening, told as a window opened, as WinUI's
/// ContentDialog is, so a screen reader reads it out whole.
fn opened(old_index: &HashMap<u64, Vec<usize>>, new: &Node) -> Vec<u64> {
    new.paths()
        .into_iter()
        .filter(|(key, path)| {
            !old_index.contains_key(key)
                && new
                    .at(path)
                    .is_some_and(|n| n.role == Role::Dialog && n.states.modal)
        })
        .map(|(key, _)| key)
        .collect()
}

/// The menus in `new` that weren't in `old`, and whether one in `old` has
/// gone: the right-click menu opening or closing, told as WinUI's flyout
/// is, so a screen reader knows a menu has the keyboard.
fn menus(old: &Node, new: &Node) -> (Vec<u64>, bool) {
    let keys = |t: &Node| -> Vec<u64> {
        t.paths()
            .into_iter()
            .filter(|(_, path)| t.at(path).is_some_and(|n| n.role == Role::Menu))
            .map(|(key, _)| key)
            .collect()
    };
    let (was, now) = (keys(old), keys(new));
    let opened = now.iter().filter(|k| !was.contains(k)).copied().collect();
    (opened, was.iter().any(|k| !now.contains(k)))
}

/// The window's side: owns the snapshot and raises events.
pub struct Host {
    shared: Arc<Shared>,
    /// A client has asked for the tree, so it's worth keeping up to date.
    active: bool,
    said: Said,
}

fn gone() -> Error {
    Error::from_hresult(windows::core::HRESULT(UIA_E_ELEMENTNOTAVAILABLE as i32))
}

fn not_enabled() -> Error {
    Error::from_hresult(windows::core::HRESULT(UIA_E_ELEMENTNOTENABLED as i32))
}

/// A null answer: S_OK with nothing written.
fn none<T>() -> Result<T> {
    Err(Error::empty())
}

impl Host {
    pub fn new(hwnd: HWND, wake: Wake) -> Self {
        Host::with_post(hwnd, move |r| wake.send(channel::Event::A11y(r)))
    }

    /// A host for a window that hears its clients' requests its own way:
    /// `post` hands one to the window, and says whether it will hear it.
    pub fn with_post(hwnd: HWND, post: impl Fn(Request) -> bool + Send + Sync + 'static) -> Self {
        Host {
            shared: Arc::new(Shared {
                hwnd: hwnd.0 as isize,
                post: Box::new(post),
                snap: Mutex::new(Snapshot {
                    root: Node::new(Id::Root, Role::Window, "Kiln", Default::default()),
                    index: HashMap::new(),
                    unit_px: 1.0,
                    has_focus: false,
                    alive: true,
                }),
            }),
            active: false,
            said: Said::default(),
        }
    }

    /// Whether any client has asked for the tree yet.
    pub fn active(&self) -> bool {
        self.active
    }

    /// The root provider, for WM_GETOBJECT: from now on clients have the
    /// tree.
    pub fn root(&mut self) -> IRawElementProviderSimple {
        self.active = true;
        self.provider()
    }

    /// A root provider, without telling the host a client has it. It
    /// reads the tree's shared copy only, so it can be handed to a client
    /// while `App` is borrowed.
    pub fn provider(&self) -> IRawElementProviderSimple {
        Root {
            shared: Arc::downgrade(&self.shared),
        }
        .into()
    }

    /// Swap in a new tree, unless it's the one clients already have, and
    /// return what to tell them: where the focus went, rows added or
    /// removed. With `announce` false, as when the first client connects,
    /// nothing is told: the tree is simply there. The status is told apart,
    /// by `status`, never held back with the tree.
    pub fn update(
        &mut self,
        root: Node,
        unit_px: f32,
        has_focus: bool,
        announce: bool,
    ) -> Vec<Event> {
        let focus = root.focused().map(|n| n.key).filter(|_| has_focus);
        let rows: Vec<u64> = root
            .find(&Id::List)
            .map(|l| l.children.iter().map(|c| c.key).collect())
            .unwrap_or_default();
        let list = root.find(&Id::List).map(|l| l.key);
        let unit_px = unit_px.max(0.01);
        let changed;
        let appeared;
        let menu;
        {
            let mut snap = self.shared.lock();
            if !snap.alive {
                return Vec::new();
            }
            let same = snap.root == root && snap.unit_px == unit_px && snap.has_focus == has_focus;
            (changed, appeared, menu) = if same || !announce {
                (Vec::new(), Vec::new(), (Vec::new(), false))
            } else {
                (
                    changes(&snap.root, &snap.index, &root),
                    opened(&snap.index, &root),
                    menus(&snap.root, &root),
                )
            };
            if !same {
                snap.index = root.paths().into_iter().collect();
                snap.root = root;
                snap.unit_px = unit_px;
                snap.has_focus = has_focus;
            }
        }
        let new_focus = (focus != self.said.focus).then_some(focus).flatten();
        let new_rows = rows != self.said.rows;
        self.said.focus = focus;
        self.said.rows = rows;
        if !announce {
            return Vec::new();
        }
        let shared = Arc::downgrade(&self.shared);
        let mut events = Vec::new();
        let mut tell = |kind| {
            events.push(Event {
                shared: shared.clone(),
                kind,
            })
        };
        for key in appeared {
            tell(Kind::Opened(key));
        }
        let (menu_opened, menu_closed) = menu;
        if menu_closed {
            tell(Kind::MenuClosed);
        }
        for key in menu_opened {
            tell(Kind::MenuOpened(key));
        }
        if let Some(key) = new_focus {
            tell(Kind::Focus(key));
        }
        if let (true, Some(key)) = (new_rows, list) {
            tell(Kind::Rows(key));
        }
        for (key, change) in changed {
            tell(Kind::Changed(key, change));
        }
        events
    }

    /// A status line to announce, an alert as an action that didn't
    /// happen.
    pub fn status(&self, text: String, alert: bool) -> Event {
        Event {
            shared: Arc::downgrade(&self.shared),
            kind: Kind::Status(text, alert),
        }
    }

    /// The window is closing: every element goes. UI Automation lets go
    /// of the providers once `App` is let go ([`disconnect`]).
    pub fn shutdown(&mut self) {
        let mut snap = self.shared.lock();
        snap.alive = false;
        snap.index.clear();
        snap.root.children.clear();
    }
}

/// A small window of its own has closed: UI Automation lets go of its
/// providers, and only its own. Called with no borrow of its state held.
pub fn disconnect_window(hwnd: HWND) {
    // SAFETY: a plain call for this thread's own window; no provider is
    // handed over.
    unsafe {
        UiaReturnRawElementProvider(hwnd, WPARAM(0), LPARAM(0), None);
    }
}

/// The window has closed: UI Automation lets go of its providers. Called
/// with no borrow of `App` held, as it can deliver the window's messages
/// while a client is still asking.
pub fn disconnect(hwnd: HWND) {
    // SAFETY: plain calls for this thread's own window; no provider is
    // handed over.
    unsafe {
        UiaReturnRawElementProvider(hwnd, WPARAM(0), LPARAM(0), None);
        let _ = UiaDisconnectAllProviders();
    }
}

/// Tell listening clients what `Host::update` found. Called with no
/// borrow of `App` held, as a client may call straight back in.
pub fn raise(events: Vec<Event>) {
    if events.is_empty() || !unsafe { UiaClientsAreListening() }.as_bool() {
        return;
    }
    for Event { shared, kind } in events {
        match kind {
            Kind::Opened(key) => {
                let el: IRawElementProviderSimple = Element::new(shared, key).into();
                let _ = unsafe { UiaRaiseAutomationEvent(&el, UIA_Window_WindowOpenedEventId) };
            }
            Kind::MenuOpened(key) => {
                let el: IRawElementProviderSimple = Element::new(shared, key).into();
                let _ = unsafe { UiaRaiseAutomationEvent(&el, UIA_MenuOpenedEventId) };
            }
            Kind::MenuClosed => {
                let root: IRawElementProviderSimple = Root { shared }.into();
                let _ = unsafe { UiaRaiseAutomationEvent(&root, UIA_MenuClosedEventId) };
            }
            Kind::Focus(key) => {
                let el: IRawElementProviderSimple = Element::new(shared, key).into();
                let _ = unsafe { UiaRaiseAutomationEvent(&el, UIA_AutomationFocusChangedEventId) };
            }
            Kind::Rows(key) => {
                let el: IRawElementProviderSimple = Element::new(shared, key).into();
                let mut id = runtime_id(key);
                let _ = unsafe {
                    UiaRaiseStructureChangedEvent(
                        &el,
                        StructureChangeType_ChildrenInvalidated,
                        id.as_mut_ptr(),
                        id.len() as i32,
                    )
                };
            }
            Kind::Changed(key, change) => {
                let el: IRawElementProviderSimple = Element::new(shared, key).into();
                let (id, mut old, mut new) = match change {
                    Change::Range(a, b) => (UIA_RangeValueValuePropertyId, var_r8(a), var_r8(b)),
                    Change::Value(a, b) => (UIA_ValueValuePropertyId, var_str(&a), var_str(&b)),
                    Change::Expanded(a, b) => (
                        UIA_ExpandCollapseExpandCollapseStatePropertyId,
                        var_i4(expand_state(Some(a)).0),
                        var_i4(expand_state(Some(b)).0),
                    ),
                    Change::Toggled(a, b) => (
                        UIA_ToggleToggleStatePropertyId,
                        var_i4(toggle_state(Some(a)).0),
                        var_i4(toggle_state(Some(b)).0),
                    ),
                };
                let _ = unsafe { UiaRaiseAutomationPropertyChangedEvent(&el, id, &old, &new) };
                // The strings are ours to free.
                unsafe {
                    let _ = VariantClear(&mut old);
                    let _ = VariantClear(&mut new);
                }
            }
            Kind::Status(text, alert) => {
                let root: IRawElementProviderSimple = Root { shared }.into();
                let kind = if alert {
                    NotificationKind_ActionAborted
                } else {
                    NotificationKind_ActionCompleted
                };
                let _ = unsafe {
                    UiaRaiseNotificationEvent(
                        &root,
                        kind,
                        NotificationProcessing_MostRecent,
                        &BSTR::from(text),
                        &BSTR::from("kiln.status"),
                    )
                };
            }
        }
    }
}

/// `[UiaAppendRuntimeId, high, low]`: unique within the window, which
/// UI Automation makes unique across windows.
fn runtime_id(key: u64) -> [i32; 3] {
    [UiaAppendRuntimeId as i32, (key >> 32) as i32, key as i32]
}

fn int_array(values: &[i32]) -> Result<*mut SAFEARRAY> {
    unsafe {
        let psa = SafeArrayCreateVector(VT_I4, 0, values.len() as u32);
        if psa.is_null() {
            return Err(Error::from_hresult(
                windows::Win32::Foundation::E_OUTOFMEMORY,
            ));
        }
        for (i, v) in values.iter().enumerate() {
            let i = i as i32;
            if let Err(e) = SafeArrayPutElement(psa, &i, (v as *const i32).cast()) {
                let _ = SafeArrayDestroy(psa);
                return Err(e);
            }
        }
        Ok(psa)
    }
}

fn unknown_array(items: &[IUnknown]) -> Result<*mut SAFEARRAY> {
    unsafe {
        let psa = SafeArrayCreateVector(VT_UNKNOWN, 0, items.len() as u32);
        if psa.is_null() {
            return Err(Error::from_hresult(
                windows::Win32::Foundation::E_OUTOFMEMORY,
            ));
        }
        for (i, item) in items.iter().enumerate() {
            let i = i as i32;
            // SafeArrayPutElement AddRefs what it stores, and
            // SafeArrayDestroy releases it.
            if let Err(e) = SafeArrayPutElement(psa, &i, item.as_raw()) {
                let _ = SafeArrayDestroy(psa);
                return Err(e);
            }
        }
        Ok(psa)
    }
}

fn var_i4(v: i32) -> VARIANT {
    let mut var = VARIANT::default();
    unsafe {
        (*var.Anonymous.Anonymous).vt = VT_I4;
        (*var.Anonymous.Anonymous).Anonymous.lVal = v;
    }
    var
}

fn var_r8(v: f64) -> VARIANT {
    let mut var = VARIANT::default();
    unsafe {
        (*var.Anonymous.Anonymous).vt = VT_R8;
        (*var.Anonymous.Anonymous).Anonymous.dblVal = v;
    }
    var
}

fn var_bool(v: bool) -> VARIANT {
    let mut var = VARIANT::default();
    unsafe {
        (*var.Anonymous.Anonymous).vt = VT_BOOL;
        (*var.Anonymous.Anonymous).Anonymous.boolVal = if v { VARIANT_TRUE } else { VARIANT_FALSE };
    }
    var
}

fn var_str(s: &str) -> VARIANT {
    let mut var = VARIANT::default();
    unsafe {
        (*var.Anonymous.Anonymous).vt = VT_BSTR;
        // The caller frees it.
        (*var.Anonymous.Anonymous).Anonymous.bstrVal = ManuallyDrop::new(BSTR::from(s));
    }
    var
}

/// A disclosure's state as UI Automation says it; anything that doesn't
/// open is a leaf.
fn expand_state(expanded: Option<bool>) -> ExpandCollapseState {
    match expanded {
        Some(true) => ExpandCollapseState_Expanded,
        Some(false) => ExpandCollapseState_Collapsed,
        None => ExpandCollapseState_LeafNode,
    }
}

/// What an element's range value says.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Range {
    value: f64,
    min: f64,
    max: f64,
    small: f64,
    large: f64,
    read_only: bool,
}

/// A slider's range is its steps, 0 to the last, one step a small change
/// and the way to the next mark a large one; it can be set unless
/// disabled. Anything else with a range is a progress bar: a percent,
/// read only.
fn range_of(n: &Node) -> Range {
    let value = n.range.unwrap_or(0.0);
    match n.role {
        Role::Slider => Range {
            value,
            min: 0.0,
            max: n.range_max.unwrap_or(0.0),
            small: 1.0,
            large: n.range_large.unwrap_or(1.0),
            read_only: n.states.disabled,
        },
        _ => Range {
            value,
            min: 0.0,
            max: 100.0,
            small: 1.0,
            large: 10.0,
            read_only: true,
        },
    }
}

/// The step a range value set to `v` lands on: the nearest. `None` when
/// the element can't be set, or `v` is outside its range or no number.
fn snap_step(n: &Node, v: f64) -> Option<usize> {
    let r = range_of(n);
    if r.read_only || !v.is_finite() {
        return None;
    }
    let step = v.round();
    (step >= r.min && step <= r.max).then_some(step as usize)
}

fn control_type(role: Role) -> UIA_CONTROLTYPE_ID {
    match role {
        Role::Window => UIA_WindowControlTypeId,
        Role::Button => UIA_ButtonControlTypeId,
        Role::Hyperlink => UIA_HyperlinkControlTypeId,
        Role::Text => UIA_TextControlTypeId,
        Role::Group => UIA_GroupControlTypeId,
        Role::List => UIA_ListControlTypeId,
        Role::ListItem => UIA_ListItemControlTypeId,
        Role::Menu => UIA_MenuControlTypeId,
        Role::MenuItem => UIA_MenuItemControlTypeId,
        Role::Tab => UIA_TabControlTypeId,
        Role::TabItem => UIA_TabItemControlTypeId,
        Role::Spinner => UIA_SpinnerControlTypeId,
        Role::RadioButton => UIA_RadioButtonControlTypeId,
        Role::ProgressBar => UIA_ProgressBarControlTypeId,
        Role::Slider => UIA_SliderControlTypeId,
        // A dialog is a Window (IsDialog says it's a dialog), as UI
        // Automation's control type for one with the Window pattern.
        Role::Dialog => UIA_WindowControlTypeId,
        Role::Image => UIA_ImageControlTypeId,
        Role::ComboBox => UIA_ComboBoxControlTypeId,
        Role::Edit => UIA_EditControlTypeId,
    }
}

/// A ToggleSwitch's state as UI Automation says it.
fn toggle_state(on: Option<bool>) -> ToggleState {
    match on {
        Some(true) => ToggleState_On,
        _ => ToggleState_Off,
    }
}

/// Whether an element's value can't be set: a slider's moves by its
/// range, a ComboBox's by picking a choice, and nothing disabled can be.
/// A spinner's and a text box's can.
fn value_read_only(n: &Node) -> bool {
    n.states.disabled || matches!(n.role, Role::Slider | Role::ComboBox)
}

/// Why a value can't be set on `n`, if it can't: disabled, it isn't
/// enabled; read only (a slider's or ComboBox's), it is an operation it
/// doesn't do; a text box's that the box would refuse is an invalid
/// argument, rather than an Ok that sets nothing.
fn value_refusal(n: &Node, value: &str) -> Option<Error> {
    if n.states.disabled {
        Some(not_enabled())
    } else if value_read_only(n) {
        Some(Error::from_hresult(windows::core::HRESULT(
            UIA_E_INVALIDOPERATION as i32,
        )))
    } else if n.text_rule.is_some_and(|r| !r.allows(value)) {
        Some(Error::from_hresult(E_INVALIDARG))
    } else {
        None
    }
}

/// The control type said for an element where it isn't its type's own:
/// a dialog, and a ToggleSwitch (a button that toggles), as WinUI says.
fn localized_type(n: &Node) -> Option<&'static str> {
    match n.role {
        Role::Dialog => Some("dialog"),
        Role::Button if n.states.toggled.is_some() => Some("toggle switch"),
        _ => None,
    }
}

/// An automation id that stays the same from run to run. A row's holds a
/// hash of its path rather than the path.
fn automation_id(id: &Id) -> String {
    match id {
        Id::Row(_) => format!("Row.{:016x}", id.key()),
        Id::RowCancel(_) => format!("RowCancel.{:016x}", id.key()),
        Id::RowProgress(_) => format!("RowProgress.{:016x}", id.key()),
        Id::RowRetry(_) => format!("RowRetry.{:016x}", id.key()),
        Id::RowOptions(_) => format!("RowOptions.{:016x}", id.key()),
        Id::RowLink(_) => format!("RowLink.{:016x}", id.key()),
        _ => format!("{id:?}"),
    }
}

fn screen_rect(snap: &Snapshot, node: &Node, origin: (f64, f64)) -> UiaRect {
    if node.states.offscreen {
        return UiaRect::default();
    }
    let k = f64::from(snap.unit_px);
    let r = node.rect;
    UiaRect {
        left: origin.0 + f64::from(r.x) * k,
        top: origin.1 + f64::from(r.y) * k,
        width: f64::from(r.w) * k,
        height: f64::from(r.h) * k,
    }
}

/// The window itself. Its name, type and bounds come from the HWND's own
/// provider; this adds the children.
#[implement(
    IRawElementProviderSimple,
    IRawElementProviderFragment,
    IRawElementProviderFragmentRoot
)]
struct Root {
    shared: Weak<Shared>,
}

impl Root_Impl {
    /// The window's side, while the window is open.
    fn shared(&self) -> Result<Arc<Shared>> {
        let shared = self.shared.upgrade().ok_or_else(gone)?;
        if !shared.lock().alive {
            return Err(gone());
        }
        Ok(shared)
    }

    fn child(&self, last: bool) -> Result<IRawElementProviderFragment> {
        let shared = self.shared()?;
        let key = {
            let snap = shared.lock();
            let c = &snap.root.children;
            let n = if last { c.last() } else { c.first() };
            n.map(|n| n.key)
        };
        match key {
            Some(key) => Ok(Element::new(self.shared.clone(), key).into()),
            None => none(),
        }
    }
}

impl IRawElementProviderSimple_Impl for Root_Impl {
    fn ProviderOptions(&self) -> Result<ProviderOptions> {
        Ok(ProviderOptions_ServerSideProvider)
    }

    fn GetPatternProvider(&self, _id: UIA_PATTERN_ID) -> Result<IUnknown> {
        none()
    }

    fn GetPropertyValue(&self, id: UIA_PROPERTY_ID) -> Result<VARIANT> {
        self.shared()?;
        Ok(match id {
            UIA_AutomationIdPropertyId => var_str("Kiln"),
            _ => VARIANT::default(),
        })
    }

    fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
        let shared = self.shared()?;
        unsafe { UiaHostProviderFromHwnd(shared.hwnd()) }
    }
}

impl IRawElementProviderFragment_Impl for Root_Impl {
    fn Navigate(&self, direction: NavigateDirection) -> Result<IRawElementProviderFragment> {
        match direction {
            NavigateDirection_FirstChild => self.child(false),
            NavigateDirection_LastChild => self.child(true),
            // The host HWND's provider answers for parent and siblings.
            _ => none(),
        }
    }

    fn GetRuntimeId(&self) -> Result<*mut SAFEARRAY> {
        // The host HWND's provider gives the root its id.
        Ok(std::ptr::null_mut())
    }

    fn BoundingRectangle(&self) -> Result<UiaRect> {
        // The host HWND's provider gives the window's bounds.
        Ok(UiaRect::default())
    }

    fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
        Ok(std::ptr::null_mut())
    }

    fn SetFocus(&self) -> Result<()> {
        Ok(())
    }

    fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
        Ok(self.to_interface())
    }
}

impl IRawElementProviderFragmentRoot_Impl for Root_Impl {
    fn ElementProviderFromPoint(&self, x: f64, y: f64) -> Result<IRawElementProviderFragment> {
        let shared = self.shared()?;
        let origin = shared.origin();
        let key = {
            let snap = shared.lock();
            let k = f64::from(snap.unit_px);
            let (ux, uy) = (((x - origin.0) / k) as f32, ((y - origin.1) / k) as f32);
            let n = snap.root.hit(ux, uy);
            (n.id != Id::Root).then_some(n.key)
        };
        match key {
            Some(key) => Ok(Element::new(self.shared.clone(), key).into()),
            None => Ok(self.to_interface()),
        }
    }

    fn GetFocus(&self) -> Result<IRawElementProviderFragment> {
        let shared = self.shared()?;
        let key = shared.lock().root.focused().map(|n| n.key);
        match key {
            Some(key) => Ok(Element::new(self.shared.clone(), key).into()),
            None => none(),
        }
    }
}

/// Any element below the window, found by its key in the latest tree.
#[implement(
    IRawElementProviderSimple,
    IRawElementProviderFragment,
    IInvokeProvider,
    ISelectionProvider,
    ISelectionItemProvider,
    IValueProvider,
    IRangeValueProvider,
    IScrollItemProvider,
    IExpandCollapseProvider,
    IToggleProvider,
    IWindowProvider,
    ITextProvider
)]
struct Element {
    shared: Weak<Shared>,
    key: u64,
}

impl Element {
    fn new(shared: Weak<Shared>, key: u64) -> Self {
        Element { shared, key }
    }
}

impl Element_Impl {
    /// Answer from this element's node in the latest tree, or say it's
    /// gone. `f` runs with the lock held: it must not call out.
    fn with<T>(&self, f: impl FnOnce(&Snapshot, &Node, &[usize]) -> Result<T>) -> Result<T> {
        let shared = self.shared.upgrade().ok_or_else(gone)?;
        let snap = shared.lock();
        if !snap.alive {
            return Err(gone());
        }
        let path = snap.index.get(&self.key).ok_or_else(gone)?;
        let node = snap.root.at(path).ok_or_else(gone)?;
        f(&snap, node, path)
    }

    fn other(&self, key: u64) -> Element {
        Element::new(self.shared.clone(), key)
    }

    /// Post what a client asked for, once it's known the element can.
    fn ask(&self, verb: Verb, enabled_only: bool) -> Result<()> {
        let id = self.with(|_, n, _| {
            if enabled_only && n.states.disabled {
                return Err(not_enabled());
            }
            Ok(n.id.clone())
        })?;
        let shared = self.shared.upgrade().ok_or_else(gone)?;
        shared.post(id, verb)
    }

    fn pattern<I: Interface>(&self, has: bool) -> Result<IUnknown>
    where
        Self: windows::core::ComObjectInterface<I>,
    {
        if has {
            Ok(self.to_interface::<I>().cast()?)
        } else {
            none()
        }
    }
}

impl IRawElementProviderSimple_Impl for Element_Impl {
    fn ProviderOptions(&self) -> Result<ProviderOptions> {
        Ok(ProviderOptions_ServerSideProvider)
    }

    fn GetPatternProvider(&self, id: UIA_PATTERN_ID) -> Result<IUnknown> {
        let p = self.with(|_, n, _| Ok(n.patterns()))?;
        match id {
            UIA_InvokePatternId => self.pattern::<IInvokeProvider>(p.invoke),
            UIA_SelectionPatternId => self.pattern::<ISelectionProvider>(p.selection),
            UIA_SelectionItemPatternId => self.pattern::<ISelectionItemProvider>(p.selection_item),
            UIA_ValuePatternId => self.pattern::<IValueProvider>(p.value),
            UIA_RangeValuePatternId => self.pattern::<IRangeValueProvider>(p.range_value),
            UIA_ScrollItemPatternId => self.pattern::<IScrollItemProvider>(p.scroll_item),
            UIA_ExpandCollapsePatternId => {
                self.pattern::<IExpandCollapseProvider>(p.expand_collapse)
            }
            UIA_TogglePatternId => self.pattern::<IToggleProvider>(p.toggle),
            UIA_WindowPatternId => self.pattern::<IWindowProvider>(p.window),
            UIA_TextPatternId => self.pattern::<ITextProvider>(p.text),
            _ => none(),
        }
    }

    fn GetPropertyValue(&self, id: UIA_PROPERTY_ID) -> Result<VARIANT> {
        self.with(|snap, n, _| {
            let s = n.states;
            Ok(match id {
                UIA_NamePropertyId => var_str(&n.name),
                UIA_ControlTypePropertyId => var_i4(control_type(n.role).0),
                UIA_LocalizedControlTypePropertyId if localized_type(n).is_some() => {
                    var_str(localized_type(n).unwrap_or_default())
                }
                UIA_AutomationIdPropertyId => var_str(&automation_id(&n.id)),
                UIA_HelpTextPropertyId => match &n.help {
                    Some(h) => var_str(h),
                    None => VARIANT::default(),
                },
                UIA_ItemStatusPropertyId => match &n.status {
                    Some(st) => var_str(st),
                    None => VARIANT::default(),
                },
                UIA_IsKeyboardFocusablePropertyId => var_bool(s.focusable && !s.disabled),
                UIA_HasKeyboardFocusPropertyId => var_bool(s.focused && snap.has_focus),
                UIA_IsEnabledPropertyId => var_bool(!s.disabled),
                UIA_IsOffscreenPropertyId => var_bool(s.offscreen),
                UIA_IsDialogPropertyId => var_bool(n.role == Role::Dialog),
                UIA_LiveSettingPropertyId if n.live => var_i4(Polite.0),
                UIA_AcceleratorKeyPropertyId => match n.shortcut {
                    Some(k) => var_str(k),
                    None => VARIANT::default(),
                },
                UIA_ClassNamePropertyId => var_str("Kiln"),
                _ => VARIANT::default(),
            })
        })
    }

    fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
        none()
    }
}

impl IRawElementProviderFragment_Impl for Element_Impl {
    fn Navigate(&self, direction: NavigateDirection) -> Result<IRawElementProviderFragment> {
        let to = match direction {
            NavigateDirection_Parent => Toward::Parent,
            NavigateDirection_NextSibling => Toward::Next,
            NavigateDirection_PreviousSibling => Toward::Previous,
            NavigateDirection_FirstChild => Toward::FirstChild,
            NavigateDirection_LastChild => Toward::LastChild,
            _ => return none(),
        };
        match self.with(|snap, _, path| Ok(snap.root.neighbour(path, to)))? {
            Neighbour::Root => Ok(Root {
                shared: self.shared.clone(),
            }
            .into()),
            Neighbour::Node(key) => Ok(self.other(key).into()),
            Neighbour::Nothing => none(),
        }
    }

    fn GetRuntimeId(&self) -> Result<*mut SAFEARRAY> {
        let shared = self.shared.upgrade().ok_or_else(gone)?;
        if !shared.lock().alive {
            return Err(gone());
        }
        int_array(&runtime_id(self.key))
    }

    fn BoundingRectangle(&self) -> Result<UiaRect> {
        let shared = self.shared.upgrade().ok_or_else(gone)?;
        let origin = shared.origin();
        self.with(|snap, n, _| Ok(screen_rect(snap, n, origin)))
    }

    fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
        Ok(std::ptr::null_mut())
    }

    fn SetFocus(&self) -> Result<()> {
        self.ask(Verb::Focus, true)
    }

    fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
        Ok(Root {
            shared: self.shared.clone(),
        }
        .into())
    }
}

impl IInvokeProvider_Impl for Element_Impl {
    fn Invoke(&self) -> Result<()> {
        self.ask(Verb::Invoke, true)
    }
}

impl ISelectionProvider_Impl for Element_Impl {
    fn GetSelection(&self) -> Result<*mut SAFEARRAY> {
        let keys: Vec<u64> = self.with(|_, n, _| {
            Ok(n.children
                .iter()
                .filter(|c| c.states.selected)
                .map(|c| c.key)
                .collect())
        })?;
        let items: Vec<IUnknown> = keys
            .into_iter()
            .map(|k| {
                let el: IRawElementProviderSimple = self.other(k).into();
                el.into()
            })
            .collect();
        unknown_array(&items)
    }

    fn CanSelectMultiple(&self) -> Result<BOOL> {
        self.with(|_, n, _| Ok((n.role == Role::List).into()))
    }

    fn IsSelectionRequired(&self) -> Result<BOOL> {
        // Tabs and segmented groups always have one chosen; the list can
        // have none.
        self.with(|_, n, _| Ok((n.role != Role::List).into()))
    }
}

impl ISelectionItemProvider_Impl for Element_Impl {
    fn Select(&self) -> Result<()> {
        self.ask(Verb::Select, true)
    }

    fn AddToSelection(&self) -> Result<()> {
        self.ask(Verb::AddToSelection, true)
    }

    fn RemoveFromSelection(&self) -> Result<()> {
        self.ask(Verb::RemoveFromSelection, true)
    }

    fn IsSelected(&self) -> Result<BOOL> {
        self.with(|_, n, _| Ok(n.states.selected.into()))
    }

    fn SelectionContainer(&self) -> Result<IRawElementProviderSimple> {
        let parent = self.with(|snap, _, path| {
            Ok(path
                .split_last()
                .and_then(|(_, p)| snap.root.at(p))
                .filter(|p| p.patterns().selection)
                .map(|p| p.key))
        })?;
        match parent {
            Some(key) => Ok(self.other(key).into()),
            None => none(),
        }
    }
}

/// A spinner's value is its text, set by naming a choice; a text box's is
/// its text, set as typed. A slider's value is what its step means in
/// words and a ComboBox's its choice, read only: they move by their range
/// value and their choices.
impl IValueProvider_Impl for Element_Impl {
    fn SetValue(&self, val: &PCWSTR) -> Result<()> {
        let value = unsafe { val.to_string() }.unwrap_or_default();
        if let Some(e) = self.with(|_, n, _| Ok(value_refusal(n, &value)))? {
            return Err(e);
        }
        self.ask(Verb::SetValue(value), true)
    }

    fn Value(&self) -> Result<BSTR> {
        self.with(|_, n, _| Ok(BSTR::from(n.value.as_deref().unwrap_or_default())))
    }

    fn IsReadOnly(&self) -> Result<BOOL> {
        self.with(|_, n, _| Ok(value_read_only(n).into()))
    }
}

/// A progress bar's percent, read only, or a slider's step (`range_of`).
impl IRangeValueProvider_Impl for Element_Impl {
    fn SetValue(&self, val: f64) -> Result<()> {
        let step = self.with(|_, n, _| {
            if range_of(n).read_only {
                // Progress is read, never set; a disabled slider waits.
                return Err(not_enabled());
            }
            snap_step(n, val).ok_or_else(|| Error::from_hresult(E_INVALIDARG))
        })?;
        self.ask(Verb::SetStep(step), true)
    }

    fn Value(&self) -> Result<f64> {
        self.with(|_, n, _| Ok(range_of(n).value))
    }

    fn IsReadOnly(&self) -> Result<BOOL> {
        self.with(|_, n, _| Ok(range_of(n).read_only.into()))
    }

    fn Maximum(&self) -> Result<f64> {
        self.with(|_, n, _| Ok(range_of(n).max))
    }

    fn Minimum(&self) -> Result<f64> {
        self.with(|_, n, _| Ok(range_of(n).min))
    }

    fn LargeChange(&self) -> Result<f64> {
        self.with(|_, n, _| Ok(range_of(n).large))
    }

    fn SmallChange(&self) -> Result<f64> {
        self.with(|_, n, _| Ok(range_of(n).small))
    }
}

/// Advanced: open or closed, as `States::expanded` says.
impl IExpandCollapseProvider_Impl for Element_Impl {
    fn Expand(&self) -> Result<()> {
        self.ask(Verb::Expand(true), true)
    }

    fn Collapse(&self) -> Result<()> {
        self.ask(Verb::Expand(false), true)
    }

    fn ExpandCollapseState(&self) -> Result<ExpandCollapseState> {
        self.with(|_, n, _| Ok(expand_state(n.states.expanded)))
    }
}

/// A ToggleSwitch: on or off, as `States::toggled` says.
impl IToggleProvider_Impl for Element_Impl {
    fn Toggle(&self) -> Result<()> {
        self.ask(Verb::Toggle, true)
    }

    fn ToggleState(&self) -> Result<ToggleState> {
        self.with(|_, n, _| Ok(toggle_state(n.states.toggled)))
    }
}

/// A modal dialog inside the window: it says it is modal and stays as it
/// is; closing it is Cancel.
impl IWindowProvider_Impl for Element_Impl {
    fn SetVisualState(&self, _state: WindowVisualState) -> Result<()> {
        Err(Error::from_hresult(windows::core::HRESULT(
            UIA_E_INVALIDOPERATION as i32,
        )))
    }

    fn Close(&self) -> Result<()> {
        self.ask(Verb::Close, false)
    }

    fn WaitForInputIdle(&self, _milliseconds: i32) -> Result<BOOL> {
        Ok(true.into())
    }

    fn CanMaximize(&self) -> Result<BOOL> {
        Ok(false.into())
    }

    fn CanMinimize(&self) -> Result<BOOL> {
        Ok(false.into())
    }

    fn IsModal(&self) -> Result<BOOL> {
        self.with(|_, n, _| Ok(n.states.modal.into()))
    }

    fn WindowVisualState(&self) -> Result<WindowVisualState> {
        Ok(WindowVisualState_Normal)
    }

    fn WindowInteractionState(&self) -> Result<WindowInteractionState> {
        Ok(WindowInteractionState_ReadyForUserInteraction)
    }

    fn IsTopmost(&self) -> Result<BOOL> {
        Ok(true.into())
    }
}

impl IScrollItemProvider_Impl for Element_Impl {
    fn ScrollIntoView(&self) -> Result<()> {
        self.ask(Verb::ScrollIntoView, false)
    }
}

/// The answer to WM_GETOBJECT for the UI Automation root.
pub fn answer(
    hwnd: HWND,
    wparam: WPARAM,
    lparam: LPARAM,
    root: IRawElementProviderSimple,
) -> LRESULT {
    unsafe { UiaReturnRawElementProvider(hwnd, wparam, lparam, &root) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Rect, SheetControl};
    use crate::tune_sheet::TuneSheet;
    use kiln_media::Target;

    #[test]
    fn the_new_controls_have_their_control_types() {
        assert_eq!(control_type(Role::ComboBox), UIA_ComboBoxControlTypeId);
        assert_eq!(control_type(Role::Edit), UIA_EditControlTypeId);
        // A dialog is a Window to UI Automation, with IsDialog set.
        assert_eq!(control_type(Role::Dialog), UIA_WindowControlTypeId);
        assert_eq!(toggle_state(Some(true)), ToggleState_On);
        assert_eq!(toggle_state(Some(false)), ToggleState_Off);
        assert_eq!(toggle_state(None), ToggleState_Off);
        let mut t = Node::new(Id::AddFiles, Role::Button, "Start", Rect::default());
        assert_eq!(localized_type(&t), None);
        t.states.toggled = Some(false);
        assert_eq!(localized_type(&t), Some("toggle switch"));
        let d = Node::new(Id::Details, Role::Dialog, "Details", Rect::default());
        assert_eq!(localized_type(&d), Some("dialog"));
    }

    #[test]
    fn a_text_box_can_be_set_and_a_combo_box_cannot() {
        let mut edit = Node::new(Id::AddFiles, Role::Edit, "Size", Rect::default());
        assert!(!value_read_only(&edit));
        edit.states.disabled = true;
        assert!(value_read_only(&edit));
        let combo = Node::new(Id::AddFiles, Role::ComboBox, "Theme", Rect::default());
        assert!(value_read_only(&combo));
        let spin = Node::new(Id::AddFiles, Role::Spinner, "Size", Rect::default());
        assert!(!value_read_only(&spin));
        let slider = Node::new(Id::AddFiles, Role::Slider, "Sound", Rect::default());
        assert!(value_read_only(&slider));
    }

    #[test]
    fn a_value_is_refused_with_the_error_that_says_why() {
        let code = |n: &Node, v: &str| value_refusal(n, v).map(|e| e.code().0 as u32);
        let mut edit = Node::new(Id::AddFiles, Role::Edit, "Size", Rect::default());
        edit.text_rule = Some(crate::controls::TextRule {
            max_len: 3,
            digits: true,
            point: false,
        });
        assert_eq!(code(&edit, "15"), None);
        // One the box itself would refuse: not digits, or too long.
        assert_eq!(code(&edit, "1a5"), Some(E_INVALIDARG.0 as u32));
        assert_eq!(code(&edit, "1500"), Some(E_INVALIDARG.0 as u32));
        // Disabled: not enabled.
        edit.states.disabled = true;
        assert_eq!(code(&edit, "15"), Some(UIA_E_ELEMENTNOTENABLED));
        // Read only: an operation it doesn't do.
        let combo = Node::new(Id::AddFiles, Role::ComboBox, "Theme", Rect::default());
        assert_eq!(code(&combo, "Light"), Some(UIA_E_INVALIDOPERATION));
        let slider = Node::new(Id::AddFiles, Role::Slider, "Sound", Rect::default());
        assert_eq!(code(&slider, "V0"), Some(UIA_E_INVALIDOPERATION));
        // A spinner names its choice; the model decides.
        let spin = Node::new(Id::AddFiles, Role::Spinner, "Size", Rect::default());
        assert_eq!(code(&spin, "1080p"), None);
    }

    #[test]
    fn a_modal_dialog_that_appears_is_told_as_a_window_opened() {
        let tree = |ask: bool, settings: bool| {
            let mut root = Node::new(Id::Root, Role::Window, "Kiln", Rect::default());
            if settings {
                root.children.push(Node::new(
                    Id::Settings,
                    Role::Dialog,
                    "Settings",
                    Rect::default(),
                ));
            }
            if ask {
                let mut d = Node::new(
                    Id::Ask,
                    Role::Dialog,
                    "Save before closing?",
                    Rect::default(),
                );
                d.states.modal = true;
                root.children.push(d);
            }
            root
        };
        let index = |t: &Node| -> HashMap<u64, Vec<usize>> { t.paths().into_iter().collect() };
        let none = tree(false, false);
        assert_eq!(opened(&index(&none), &tree(true, false)), [Id::Ask.key()]);
        // Already open: told once only.
        let open = tree(true, false);
        assert!(opened(&index(&open), &tree(true, false)).is_empty());
        // A panel that isn't modal isn't a window opening.
        assert!(opened(&index(&none), &tree(false, true)).is_empty());
    }

    #[test]
    fn a_menu_that_appears_or_goes_is_told_as_a_menu_opened_or_closed() {
        let tree = |menu: bool| {
            let mut root = Node::new(Id::Root, Role::Window, "Kiln", Rect::default());
            if menu {
                let item = Node::new(Id::RowMenuItem(0), Role::MenuItem, "Open", Rect::default());
                let mut menu = Node::new(Id::RowMenu, Role::Menu, "", Rect::default());
                menu.children.push(item);
                root.children.push(menu);
            }
            root
        };
        let index = |t: &Node| -> HashMap<u64, Vec<usize>> { t.paths().into_iter().collect() };
        let (none, up) = (tree(false), tree(true));
        assert_eq!(menus(&none, &up), (vec![Id::RowMenu.key()], false));
        // Still up: nothing told.
        assert_eq!(menus(&up, &up), (vec![], false));
        // Gone: told closed.
        assert_eq!(menus(&up, &none), (vec![], true));
        // A menu isn't a window opening.
        assert!(opened(&index(&none), &up).is_empty());
    }

    #[test]
    fn a_flip_a_new_value_and_a_choice_are_told() {
        let tree = |on: bool, text: &str, choice: &str| {
            let mut root = Node::new(Id::Root, Role::Window, "Kiln", Rect::default());
            let mut t = Node::new(Id::AddFiles, Role::Button, "Start", Rect::default());
            t.states.toggled = Some(on);
            let mut e = Node::new(Id::Clear, Role::Edit, "Size", Rect::default());
            e.value = Some(text.to_owned());
            let mut c = Node::new(Id::Status, Role::ComboBox, "Theme", Rect::default());
            c.value = Some(choice.to_owned());
            root.children = vec![t, e, c];
            root
        };
        let old = tree(false, "1", "Dark");
        let index: HashMap<u64, Vec<usize>> = old.paths().into_iter().collect();
        assert!(changes(&old, &index, &tree(false, "1", "Dark")).is_empty());
        let got = changes(&old, &index, &tree(true, "15", "Light"));
        assert_eq!(
            got,
            vec![
                (Id::AddFiles.key(), Change::Toggled(false, true)),
                (Id::Clear.key(), Change::Value("1".into(), "15".into())),
                (
                    Id::Status.key(),
                    Change::Value("Dark".into(), "Light".into())
                ),
            ]
        );
    }

    /// The slider's node as the tree builds it from `ts`.
    fn slider(ts: &TuneSheet) -> Node {
        let mut n = Node::new(
            Id::SheetControl(SheetControl::Tune(0)),
            Role::Slider,
            "Sound quality",
            Rect::default(),
        );
        n.range = Some(ts.at as f64);
        n.range_max = Some(ts.steps.len().saturating_sub(1) as f64);
        n.range_large = Some(ts.large_change() as f64);
        n
    }

    #[test]
    fn range_value_maps_to_steps() {
        let info = kiln_core::MediaInfo {
            duration: Some(221.0),
            has_audio: true,
            channels: Some(2),
            rate: Some(44_100),
            ..Default::default()
        };
        let mut ts = TuneSheet::new(Target::Mp3, &info, &kiln_core::Quality::default());
        let n = slider(&ts);
        let r = range_of(&n);
        // Index along the steps, settable, one step at a time.
        assert_eq!(r.value, ts.at as f64);
        assert_eq!((r.min, r.max, r.small), (0.0, 18.0, 1.0));
        assert!(!r.read_only);
        // From V2, a large change is the distance to V0.
        assert_eq!(r.large, (ts.marks[2] - ts.at) as f64);
        // SetValue snaps to the nearest step.
        let to = snap_step(&n, 7.4).unwrap();
        ts.set_at(to);
        assert_eq!(ts.at, 7);
        assert_eq!(snap_step(&n, 7.6), Some(8));
        assert_eq!(snap_step(&n, 18.0), Some(18));
        // Out of range, or not a number: refused.
        assert_eq!(snap_step(&n, 18.6), None);
        assert_eq!(snap_step(&n, -0.6), None);
        assert_eq!(snap_step(&n, f64::NAN), None);
        // A progress bar keeps its read-only percent.
        let mut bar = Node::new(Id::Root, Role::ProgressBar, "Progress", Rect::default());
        bar.range = Some(45.0);
        let r = range_of(&bar);
        assert_eq!((r.value, r.min, r.max, r.large), (45.0, 0.0, 100.0, 10.0));
        assert!(r.read_only);
        assert_eq!(snap_step(&bar, 50.0), None);
    }

    #[test]
    fn the_video_slider_ranges_over_its_steps() {
        use crate::tune_sheet::VideoSheet;
        use kiln_media::video::{VideoCodec, VideoTarget};
        let info = kiln_core::MediaInfo {
            duration: Some(42.0),
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
            video_codec: Some(VideoCodec::H264),
            video_kbps: Some(6000),
            ..Default::default()
        };
        let mut ts = VideoSheet::new(VideoTarget::Mp4, &info, &kiln_core::Quality::default());
        let mut n = Node::new(
            Id::SheetControl(SheetControl::Tune(0)),
            Role::Slider,
            "Video quality",
            Rect::default(),
        );
        n.range = Some(ts.at as f64);
        n.range_max = Some(ts.steps.len().saturating_sub(1) as f64);
        n.range_large = Some(ts.large_change() as f64);
        let r = range_of(&n);
        // Opened on Original, the last of sixteen steps; a large change
        // goes back to Best.
        assert_eq!((r.value, r.min, r.max, r.small), (15.0, 0.0, 15.0, 1.0));
        assert_eq!(r.large, 3.0);
        assert!(!r.read_only);
        // SetValue snaps to a step: 9 is High.
        let to = snap_step(&n, 9.3).unwrap();
        ts.set_at(to);
        assert_eq!(ts.at, ts.marks[1]);
        assert_eq!(snap_step(&n, 15.6), None);
    }

    #[test]
    fn moving_the_slider_tells_clients() {
        let dialog = |at: f64, value: &str, open: bool| {
            let mut s = Node::new(
                Id::SheetControl(SheetControl::Tune(0)),
                Role::Slider,
                "Sound quality",
                Rect::default(),
            );
            s.range = Some(at);
            s.value = Some(value.to_owned());
            let mut a = Node::new(
                Id::SheetControl(SheetControl::Advanced),
                Role::Button,
                "More options",
                Rect::default(),
            );
            a.states.expanded = Some(open);
            let mut root = Node::new(Id::Root, Role::Window, "Kiln", Rect::default());
            root.children = vec![s, a];
            root
        };
        let old = dialog(14.0, "V2", false);
        let index: HashMap<u64, Vec<usize>> = old.paths().into_iter().collect();
        let slider = Id::SheetControl(SheetControl::Tune(0)).key();
        let advanced = Id::SheetControl(SheetControl::Advanced).key();
        assert_eq!(changes(&old, &index, &old), vec![]);
        assert_eq!(
            changes(&old, &index, &dialog(18.0, "V0", true)),
            vec![
                (slider, Change::Range(14.0, 18.0)),
                (slider, Change::Value("V2".into(), "V0".into())),
                (advanced, Change::Expanded(false, true)),
            ]
        );
    }

    /// The Details log through the interfaces a screen reader calls: its
    /// document, and a range moved by line and by word.
    #[test]
    fn the_details_log_is_read_by_line_and_word_through_the_text_pattern() {
        let log = "ffmpeg: no such file\r\nError opening input\nretry later";
        let mut text = Node::new(
            Id::DetailsText,
            Role::Text,
            log,
            Rect::new(0.0, 0.0, 200.0, 60.0),
        );
        text.states.offscreen = false;
        let mut other = Node::new(Id::DetailsLead, Role::Text, "Lead", Rect::default());
        other.states.offscreen = false;
        let mut root = Node::new(Id::Root, Role::Window, "Kiln", Rect::default());
        root.children = vec![other, text];
        let mut host = Host::with_post(HWND::default(), |_| true);
        host.update(root, 2.0, true, false);
        let element = |id: Id| -> IRawElementProviderSimple {
            Element::new(Arc::downgrade(&host.shared), id.key()).into()
        };
        // Only the log has the pattern.
        let lead = element(Id::DetailsLead);
        assert!(unsafe { lead.GetPatternProvider(UIA_TextPatternId) }.is_err());
        let provider: ITextProvider =
            unsafe { element(Id::DetailsText).GetPatternProvider(UIA_TextPatternId) }
                .and_then(|p| p.cast())
                .expect("the log has the Text pattern");
        assert_eq!(
            unsafe { provider.SupportedTextSelection() }.unwrap(),
            SupportedTextSelection_None
        );
        let doc = unsafe { provider.DocumentRange() }.unwrap();
        assert_eq!(unsafe { doc.GetText(-1) }.unwrap().to_string(), log);
        assert_eq!(unsafe { doc.GetText(7) }.unwrap().to_string(), "ffmpeg:");
        // A line at a time, from the start.
        let line = unsafe { doc.Clone() }.unwrap();
        unsafe {
            line.MoveEndpointByRange(
                TextPatternRangeEndpoint_End,
                &line,
                TextPatternRangeEndpoint_Start,
            )
        }
        .unwrap();
        unsafe { line.ExpandToEnclosingUnit(TextUnit_Line) }.unwrap();
        assert_eq!(
            unsafe { line.GetText(-1) }.unwrap().to_string(),
            "ffmpeg: no such file\r\n"
        );
        assert_eq!(unsafe { line.Move(TextUnit_Line, 1) }.unwrap(), 1);
        assert_eq!(
            unsafe { line.GetText(-1) }.unwrap().to_string(),
            "Error opening input\n"
        );
        // And a word at a time.
        unsafe { line.ExpandToEnclosingUnit(TextUnit_Word) }.unwrap();
        assert_eq!(unsafe { line.GetText(-1) }.unwrap().to_string(), "Error ");
        assert_eq!(unsafe { line.Move(TextUnit_Word, 2) }.unwrap(), 2);
        assert_eq!(unsafe { line.GetText(-1) }.unwrap().to_string(), "input\n");
        assert_eq!(unsafe { line.Move(TextUnit_Word, 9) }.unwrap(), 2);
        assert_eq!(unsafe { line.GetText(-1) }.unwrap().to_string(), "later");
        // Where it is, in the window's pixels: the element's, at 2 a unit.
        let boxes = unsafe { line.GetBoundingRectangles() }.unwrap();
        let (lo, hi) = unsafe {
            (
                windows::Win32::System::Ole::SafeArrayGetLBound(boxes, 1).unwrap(),
                windows::Win32::System::Ole::SafeArrayGetUBound(boxes, 1).unwrap(),
            )
        };
        assert_eq!(hi - lo + 1, 4);
        let _ = unsafe { SafeArrayDestroy(boxes) };
        // Ranges compare, and a text that is found is a range.
        let again = unsafe { doc.Clone() }.unwrap();
        assert!(unsafe { doc.Compare(&again) }.unwrap().as_bool());
        assert!(!unsafe { doc.Compare(&line) }.unwrap().as_bool());
        let found = unsafe { doc.FindText(&BSTR::from("INPUT"), false, true) }.unwrap();
        assert_eq!(unsafe { found.GetText(-1) }.unwrap().to_string(), "input");
        assert!(unsafe { doc.FindText(&BSTR::from("INPUT"), false, false) }.is_err());
        // The element it is in.
        let owner = unsafe { found.GetEnclosingElement() }.unwrap();
        assert_eq!(
            unsafe { owner.GetPropertyValue(UIA_NamePropertyId) }.unwrap(),
            var_str(log)
        );
    }
}
