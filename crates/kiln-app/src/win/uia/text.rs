//! The Text pattern, for the Details log: a screen reader reads it by
//! character, word and line, and says where it is as it goes, which a
//! plain Text element's name can't give.
//!
//! The text is the node's name, read from the latest snapshot each time,
//! so a range outlives a new tree and is clamped to the text it finds. How
//! the units fall is `crate::textrange`'s.

use std::sync::{Mutex, Weak};

use windows::core::{implement, Error, Interface, Ref, Result, BOOL, BSTR};
use windows::Win32::Foundation::E_NOTIMPL;
use windows::Win32::System::Com::SAFEARRAY;
use windows::Win32::System::Ole::{SafeArrayCreateVector, SafeArrayDestroy, SafeArrayPutElement};
use windows::Win32::System::Variant::{VARIANT, VT_R8};
use windows::Win32::UI::Accessibility::*;

use super::{gone, none, screen_rect, unknown_array, Element, Element_Impl, Shared};
use crate::textrange::{self, End, Range, Unit};

fn unit_of(unit: TextUnit) -> Unit {
    match unit {
        TextUnit_Character => Unit::Character,
        TextUnit_Word => Unit::Word,
        TextUnit_Line => Unit::Line,
        TextUnit_Paragraph => Unit::Paragraph,
        // A page, a run of one format and the document are all of it.
        _ => Unit::Whole,
    }
}

fn end_of(e: TextPatternRangeEndpoint) -> End {
    if e == TextPatternRangeEndpoint_Start {
        End::Start
    } else {
        End::End
    }
}

fn double_array(values: &[f64]) -> Result<*mut SAFEARRAY> {
    unsafe {
        let psa = SafeArrayCreateVector(VT_R8, 0, values.len() as u32);
        if psa.is_null() {
            return Err(Error::from_hresult(
                windows::Win32::Foundation::E_OUTOFMEMORY,
            ));
        }
        for (i, v) in values.iter().enumerate() {
            let i = i as i32;
            if let Err(e) = SafeArrayPutElement(psa, &i, (v as *const f64).cast()) {
                let _ = SafeArrayDestroy(psa);
                return Err(e);
            }
        }
        Ok(psa)
    }
}

/// A stretch of one element's text.
#[implement(ITextRangeProvider)]
struct TextRange {
    shared: Weak<Shared>,
    key: u64,
    range: Mutex<Range>,
}

impl TextRange {
    fn make(shared: &Weak<Shared>, key: u64, range: Range) -> ITextRangeProvider {
        TextRange {
            shared: shared.clone(),
            key,
            range: Mutex::new(range),
        }
        .into()
    }

    /// The element's text in UTF-16 units, as it is now.
    fn text(&self) -> Result<Vec<u16>> {
        let shared = self.shared.upgrade().ok_or_else(gone)?;
        let snap = shared.lock();
        if !snap.alive {
            return Err(gone());
        }
        let path = snap.index.get(&self.key).ok_or_else(gone)?;
        let node = snap.root.at(path).ok_or_else(gone)?;
        Ok(node.name.encode_utf16().collect())
    }
}

impl TextRange_Impl {
    fn get(&self) -> Range {
        *self.range.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set(&self, r: Range) {
        *self.range.lock().unwrap_or_else(|e| e.into_inner()) = r;
    }

    /// The range, within the text as it is now.
    fn clamped(&self, text: &[u16]) -> Range {
        let (s, e) = self.get();
        let s = s.min(text.len());
        (s, e.clamp(s, text.len()))
    }

    /// Another range of ours, as the client hands it in.
    fn other(&self, r: Ref<'_, ITextRangeProvider>) -> Result<Range> {
        let r = r.ok()?;
        let r = r.cast_object_ref::<TextRange>()?;
        Ok(r.get())
    }
}

impl ITextRangeProvider_Impl for TextRange_Impl {
    fn Clone(&self) -> Result<ITextRangeProvider> {
        Ok(TextRange::make(&self.shared, self.key, self.get()))
    }

    fn Compare(&self, range: Ref<ITextRangeProvider>) -> Result<BOOL> {
        let other = range.ok()?.cast_object_ref::<TextRange>()?;
        Ok((other.key == self.key && other.get() == self.get()).into())
    }

    fn CompareEndpoints(
        &self,
        endpoint: TextPatternRangeEndpoint,
        targetrange: Ref<ITextRangeProvider>,
        targetendpoint: TextPatternRangeEndpoint,
    ) -> Result<i32> {
        let at = |r: Range, e: TextPatternRangeEndpoint| match end_of(e) {
            End::Start => r.0,
            End::End => r.1,
        };
        let (mine, theirs) = (
            at(self.get(), endpoint),
            at(self.other(targetrange)?, targetendpoint),
        );
        Ok(match mine.cmp(&theirs) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        })
    }

    fn ExpandToEnclosingUnit(&self, unit: TextUnit) -> Result<()> {
        let text = self.text()?;
        let r = textrange::expand(&text, self.clamped(&text), unit_of(unit));
        self.set(r);
        Ok(())
    }

    fn FindAttribute(
        &self,
        _attributeid: UIA_TEXTATTRIBUTE_ID,
        _val: &VARIANT,
        _backward: BOOL,
    ) -> Result<ITextRangeProvider> {
        none()
    }

    fn FindText(
        &self,
        text: &BSTR,
        backward: BOOL,
        ignorecase: BOOL,
    ) -> Result<ITextRangeProvider> {
        let all = self.text()?;
        let (s, e) = self.clamped(&all);
        let want: Vec<u16> = text.iter().copied().collect();
        let found =
            textrange::find_units(&all[s..e], &want, backward.as_bool(), ignorecase.as_bool());
        let want_len = want.len();
        match found {
            Some(i) => Ok(TextRange::make(
                &self.shared,
                self.key,
                (s + i, s + i + want_len),
            )),
            None => none(),
        }
    }

    fn GetAttributeValue(&self, _attributeid: UIA_TEXTATTRIBUTE_ID) -> Result<VARIANT> {
        // Nothing is formatted: each attribute is "not supported", which
        // is its own value.
        let unsupported = unsafe { UiaGetReservedNotSupportedValue() }?;
        Ok(VARIANT::from(unsupported))
    }

    fn GetBoundingRectangles(&self) -> Result<*mut SAFEARRAY> {
        let text = self.text()?;
        let (s, e) = self.clamped(&text);
        let shared = self.shared.upgrade().ok_or_else(gone)?;
        let origin = shared.origin();
        let snap = shared.lock();
        let path = snap.index.get(&self.key).ok_or_else(gone)?;
        let node = snap.root.at(path).ok_or_else(gone)?;
        // The window doesn't say where its lines wrap, so a range is
        // boxed by the whole text.
        if s == e || node.states.offscreen {
            return double_array(&[]);
        }
        let r = screen_rect(&snap, node, origin);
        double_array(&[r.left, r.top, r.width, r.height])
    }

    fn GetEnclosingElement(&self) -> Result<IRawElementProviderSimple> {
        Ok(Element::new(self.shared.clone(), self.key).into())
    }

    fn GetText(&self, maxlength: i32) -> Result<BSTR> {
        let text = self.text()?;
        let (s, e) = self.clamped(&text);
        let e = if maxlength >= 0 {
            e.min(s + maxlength as usize)
        } else {
            e
        };
        Ok(BSTR::from_wide(&text[s..e]))
    }

    fn Move(&self, unit: TextUnit, count: i32) -> Result<i32> {
        let text = self.text()?;
        let (r, moved) = textrange::move_range(&text, self.clamped(&text), unit_of(unit), count);
        self.set(r);
        Ok(moved)
    }

    fn MoveEndpointByUnit(
        &self,
        endpoint: TextPatternRangeEndpoint,
        unit: TextUnit,
        count: i32,
    ) -> Result<i32> {
        let text = self.text()?;
        let (r, moved) = textrange::move_end(
            &text,
            self.clamped(&text),
            end_of(endpoint),
            unit_of(unit),
            count,
        );
        self.set(r);
        Ok(moved)
    }

    fn MoveEndpointByRange(
        &self,
        endpoint: TextPatternRangeEndpoint,
        targetrange: Ref<ITextRangeProvider>,
        targetendpoint: TextPatternRangeEndpoint,
    ) -> Result<()> {
        let target = self.other(targetrange)?;
        let to = match end_of(targetendpoint) {
            End::Start => target.0,
            End::End => target.1,
        };
        let (s, e) = self.get();
        // An end moved past the other takes it along.
        self.set(match end_of(endpoint) {
            End::Start => (to, e.max(to)),
            End::End => (s.min(to), to),
        });
        Ok(())
    }

    fn Select(&self) -> Result<()> {
        // The text can't be selected here: it has no caret.
        Err(Error::from_hresult(E_NOTIMPL))
    }

    fn AddToSelection(&self) -> Result<()> {
        Err(Error::from_hresult(E_NOTIMPL))
    }

    fn RemoveFromSelection(&self) -> Result<()> {
        Err(Error::from_hresult(E_NOTIMPL))
    }

    fn ScrollIntoView(&self, _aligntotop: BOOL) -> Result<()> {
        // The dialog's text shows in full, or is cut with an ellipsis.
        Ok(())
    }

    fn GetChildren(&self) -> Result<*mut SAFEARRAY> {
        unknown_array(&[])
    }
}

impl Element_Impl {
    /// The whole text of this element, as a range.
    fn document(&self) -> Result<ITextRangeProvider> {
        let len = self.with(|_, n, _| Ok(n.name.encode_utf16().count()))?;
        Ok(TextRange::make(&self.shared, self.key, (0, len)))
    }
}

/// The text pattern: the whole text is the document, without selection or
/// caret, and every range of it can be asked its text, its bounds and to
/// move by unit.
impl ITextProvider_Impl for Element_Impl {
    fn GetSelection(&self) -> Result<*mut SAFEARRAY> {
        unknown_array(&[])
    }

    fn GetVisibleRanges(&self) -> Result<*mut SAFEARRAY> {
        unknown_array(&[self.document()?.cast()?])
    }

    fn RangeFromChild(
        &self,
        _childelement: Ref<IRawElementProviderSimple>,
    ) -> Result<ITextRangeProvider> {
        self.document()
    }

    fn RangeFromPoint(&self, _point: &UiaPoint) -> Result<ITextRangeProvider> {
        self.document()
    }

    fn DocumentRange(&self) -> Result<ITextRangeProvider> {
        self.document()
    }

    fn SupportedTextSelection(&self) -> Result<SupportedTextSelection> {
        Ok(SupportedTextSelection_None)
    }
}
