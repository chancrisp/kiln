//! UI Automation: serves the tree `a11y::tree` builds to screen readers,
//! voice control and other assistive technology.
//!
//! UI Automation calls the providers on its own threads. They answer from
//! a snapshot of the tree, swapped in by the UI thread after each change,
//! and never touch `App`. What a client asks an element to do is posted to
//! the window as `WM_KILN_A11Y`, and the UI thread does it the way a click
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
    HWND, LPARAM, LRESULT, POINT, VARIANT_FALSE, VARIANT_TRUE, WPARAM,
};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Com::SAFEARRAY;
use windows::Win32::System::Ole::{SafeArrayCreateVector, SafeArrayDestroy, SafeArrayPutElement};
use windows::Win32::System::Variant::{VARIANT, VT_BOOL, VT_BSTR, VT_I4, VT_UNKNOWN};
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::a11y::{Id, Neighbour, Node, Request, Role, Toward, Verb};

/// Posted by a provider when a client asks an element to do something;
/// lparam is a `Box<Request>` the window takes.
pub const WM_KILN_A11Y: u32 = WM_APP + 10;

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
        let raw = Box::into_raw(Box::new(Request { id, verb }));
        let posted = unsafe {
            PostMessageW(
                Some(self.hwnd()),
                WM_KILN_A11Y,
                WPARAM(0),
                LPARAM(raw as isize),
            )
        };
        if posted.is_err() {
            drop(unsafe { Box::from_raw(raw) });
            return Err(gone());
        }
        Ok(())
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
    Focus(u64),
    Rows(u64),
    Status(String, bool),
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
    pub fn new(hwnd: HWND) -> Self {
        Host {
            shared: Arc::new(Shared {
                hwnd: hwnd.0 as isize,
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

    /// The root provider, for WM_GETOBJECT.
    pub fn root(&mut self) -> IRawElementProviderSimple {
        self.active = true;
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
        {
            let mut snap = self.shared.lock();
            if !snap.alive {
                return Vec::new();
            }
            let same = snap.root == root && snap.unit_px == unit_px && snap.has_focus == has_focus;
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
        if let Some(key) = new_focus {
            tell(Kind::Focus(key));
        }
        if let (true, Some(key)) = (new_rows, list) {
            tell(Kind::Rows(key));
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

    /// The window is closing: every element goes, and UI Automation lets
    /// go of the providers.
    pub fn shutdown(&mut self, hwnd: HWND) {
        {
            let mut snap = self.shared.lock();
            snap.alive = false;
            snap.index.clear();
            snap.root.children.clear();
        }
        unsafe {
            UiaReturnRawElementProvider(hwnd, WPARAM(0), LPARAM(0), None);
            let _ = UiaDisconnectAllProviders();
        }
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
        // A pane that says it's a dialog: there's no dialog control type.
        Role::Dialog => UIA_PaneControlTypeId,
    }
}

/// An automation id that stays the same from run to run. A row's holds a
/// hash of its path rather than the path.
fn automation_id(id: &Id) -> String {
    match id {
        Id::Row(_) => format!("Row.{:016x}", id.key()),
        Id::RowCancel(_) => format!("RowCancel.{:016x}", id.key()),
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
    IScrollItemProvider
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
            UIA_ScrollItemPatternId => self.pattern::<IScrollItemProvider>(p.scroll_item),
            _ => none(),
        }
    }

    fn GetPropertyValue(&self, id: UIA_PROPERTY_ID) -> Result<VARIANT> {
        self.with(|snap, n, _| {
            let s = n.states;
            Ok(match id {
                UIA_NamePropertyId => var_str(&n.name),
                UIA_ControlTypePropertyId => var_i4(control_type(n.role).0),
                UIA_LocalizedControlTypePropertyId if n.role == Role::Dialog => var_str("dialog"),
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

impl IValueProvider_Impl for Element_Impl {
    fn SetValue(&self, val: &PCWSTR) -> Result<()> {
        let value = unsafe { val.to_string() }.unwrap_or_default();
        self.ask(Verb::SetValue(value), true)
    }

    fn Value(&self) -> Result<BSTR> {
        self.with(|_, n, _| Ok(BSTR::from(n.value.as_deref().unwrap_or_default())))
    }

    fn IsReadOnly(&self) -> Result<BOOL> {
        self.with(|_, n, _| Ok(n.states.disabled.into()))
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
