//! What a frame would otherwise make again and again: the text layouts of
//! labels and measured words and the rounded clips of panels and pictures.
//! (The layers they are drawn through are Direct2D's own: `push_layer`
//! gives it none and it keeps one.)
//!
//! Each cache is bounded and drops the least recently used entry when it is
//! full. The renderer clears them when the device is lost and when the
//! scale, the Text size or the theme changes (`Renderer::clear_caches`),
//! though a layout or a geometry made from a factory doesn't depend on any
//! of those: nothing a change could leave wrong is kept.

use std::collections::HashMap;
use std::hash::Hash;

use windows::core::{Interface, Result};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;

use crate::win::text::Style;

/// The most text layouts kept: a window of rows, its inspector and a wheel
/// draw a few hundred different words between them.
pub(in crate::win) const LAYOUTS: usize = 512;

/// The most rounded clips kept: the panel, the pictures, the inspector's
/// hero and the sheet, at the sizes a window has had lately.
pub(in crate::win) const CLIPS: usize = 64;

/// One entry of an `Lru`, linked to the entries used just before and after
/// it.
struct Node<K, V> {
    key: K,
    value: V,
    /// Towards the most recently used.
    newer: Option<usize>,
    /// Towards the least recently used.
    older: Option<usize>,
}

/// A map holding at most `cap` entries. A new entry in a full map drops the
/// one used longest ago. Looking up, inserting and dropping take the same
/// time however many entries it holds: the entries sit in a list from the
/// most to the least recently used.
pub(in crate::win) struct Lru<K, V> {
    cap: usize,
    nodes: Vec<Node<K, V>>,
    at: HashMap<K, usize>,
    newest: Option<usize>,
    oldest: Option<usize>,
}

impl<K: Hash + Eq + Clone, V> Lru<K, V> {
    pub(in crate::win) fn new(cap: usize) -> Self {
        Lru {
            cap: cap.max(1),
            nodes: Vec::new(),
            at: HashMap::new(),
            newest: None,
            oldest: None,
        }
    }

    /// Take node `i` out of the list.
    fn unlink(&mut self, i: usize) {
        let (newer, older) = (self.nodes[i].newer, self.nodes[i].older);
        match newer {
            Some(n) => self.nodes[n].older = older,
            None => self.newest = older,
        }
        match older {
            Some(o) => self.nodes[o].newer = newer,
            None => self.oldest = newer,
        }
    }

    /// Put node `i`, which is in no list, at the newest end.
    fn link_newest(&mut self, i: usize) {
        self.nodes[i].newer = None;
        self.nodes[i].older = self.newest;
        match self.newest {
            Some(n) => self.nodes[n].newer = Some(i),
            None => self.oldest = Some(i),
        }
        self.newest = Some(i);
    }

    /// The entry for `key`, which is now the most recently used.
    pub(in crate::win) fn get(&mut self, key: &K) -> Option<&V> {
        let i = *self.at.get(key)?;
        if self.newest != Some(i) {
            self.unlink(i);
            self.link_newest(i);
        }
        Some(&self.nodes[i].value)
    }

    /// Keep `value` for `key`, dropping the least recently used entry first
    /// if the map is full and `key` is new.
    pub(in crate::win) fn insert(&mut self, key: K, value: V) {
        if let Some(&i) = self.at.get(&key) {
            self.nodes[i].value = value;
            if self.newest != Some(i) {
                self.unlink(i);
                self.link_newest(i);
            }
            return;
        }
        let i = if self.nodes.len() >= self.cap {
            // Full: the oldest node is reused for the new entry.
            let i = self.oldest.expect("a full map has an oldest entry");
            self.unlink(i);
            let old = std::mem::replace(&mut self.nodes[i].key, key.clone());
            self.at.remove(&old);
            self.nodes[i].value = value;
            i
        } else {
            self.nodes.push(Node {
                key: key.clone(),
                value,
                newer: None,
                older: None,
            });
            self.nodes.len() - 1
        };
        self.at.insert(key, i);
        self.link_newest(i);
    }

    #[cfg(test)]
    pub(in crate::win) fn len(&self) -> usize {
        self.nodes.len()
    }

    pub(in crate::win) fn clear(&mut self) {
        self.nodes.clear();
        self.at.clear();
        self.newest = None;
        self.oldest = None;
    }
}

/// What a text layout is made from: the words, the format (by the address
/// of its COM object, which the formats' owner keeps alive), the box, by
/// the bits of its sides, and what kind of text it is: a type-ramp
/// style's (whose figures and tracking aren't in the format) or a glyph.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(in crate::win) struct LayoutKey {
    text: Box<str>,
    format: usize,
    width: u32,
    height: u32,
    kind: Laying,
    /// Runs set in another weight, as UTF-16 start and length: a layout
    /// with them is never the plain text's.
    runs: Box<[(u32, u32)]>,
}

/// What a layout adds to its format's text (`LayoutKey`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Laying {
    Style(Style),
    Glyph,
}

impl LayoutKey {
    /// Text in a type-ramp style, all of it in the style's weight.
    #[cfg(test)]
    pub(in crate::win) fn styled(
        text: &str,
        format: &IDWriteTextFormat,
        width: f32,
        height: f32,
        style: Style,
    ) -> Self {
        Self::styled_runs(text, format, width, height, style, &[])
    }

    /// Text in a type-ramp style with `runs` (UTF-16 start and length)
    /// set strong.
    pub(in crate::win) fn styled_runs(
        text: &str,
        format: &IDWriteTextFormat,
        width: f32,
        height: f32,
        style: Style,
        runs: &[(u32, u32)],
    ) -> Self {
        LayoutKey {
            runs: runs.into(),
            ..Self::of(text, format, width, height, Laying::Style(style))
        }
    }

    /// An icon font's glyph.
    pub(in crate::win) fn glyph(
        text: &str,
        format: &IDWriteTextFormat,
        width: f32,
        height: f32,
    ) -> Self {
        Self::of(text, format, width, height, Laying::Glyph)
    }

    fn of(text: &str, format: &IDWriteTextFormat, width: f32, height: f32, kind: Laying) -> Self {
        LayoutKey {
            text: text.into(),
            format: format.as_raw() as usize,
            width: width.to_bits(),
            height: height.to_bits(),
            kind,
            runs: Box::new([]),
        }
    }
}

/// A laid-out text and how wide it runs, trailing whitespace counted.
#[derive(Clone)]
pub(in crate::win) struct Laid {
    pub(in crate::win) layout: IDWriteTextLayout,
    pub(in crate::win) width: f32,
}

/// The text layouts made so far (`LayoutKey`).
pub(in crate::win) type Layouts = Lru<LayoutKey, Laid>;

pub(in crate::win) fn layouts() -> Layouts {
    Lru::new(LAYOUTS)
}

/// What a rounded clip is made from: the bits of its rectangle and radii.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(in crate::win) struct ClipKey([u32; 6]);

impl ClipKey {
    pub(in crate::win) fn new(r: &D2D1_ROUNDED_RECT) -> Self {
        ClipKey([
            r.rect.left.to_bits(),
            r.rect.top.to_bits(),
            r.rect.right.to_bits(),
            r.rect.bottom.to_bits(),
            r.radiusX.to_bits(),
            r.radiusY.to_bits(),
        ])
    }
}

/// The rounded clips made so far (`ClipKey`).
pub(in crate::win) type Clips = Lru<ClipKey, ID2D1Geometry>;

pub(in crate::win) fn clips() -> Clips {
    Lru::new(CLIPS)
}

/// The geometry of rounded rectangle `r`: the one made before if it is
/// kept, else a new one, which is kept.
pub(in crate::win) fn rounded_clip(
    cache: &std::cell::RefCell<Clips>,
    factory: &ID2D1Factory,
    r: &D2D1_ROUNDED_RECT,
) -> Result<ID2D1Geometry> {
    let key = ClipKey::new(r);
    if let Some(g) = cache.borrow_mut().get(&key) {
        return Ok(g.clone());
    }
    // SAFETY: a plain geometry made from this thread's factory.
    let geometry: ID2D1Geometry = unsafe { factory.CreateRoundedRectangleGeometry(r)? }.into();
    cache.borrow_mut().insert(key, geometry.clone());
    Ok(geometry)
}

/// Start drawing into a layer limited to `bounds`, by `mask` if there is
/// one, at `opacity`. Direct2D keeps the layer: none is made or held here.
/// End it with `PopLayer`.
pub(in crate::win) fn push_layer(
    rt: &ID2D1RenderTarget,
    bounds: D2D_RECT_F,
    mask: Option<&ID2D1Geometry>,
    opacity: f32,
) -> Result<()> {
    let dc: ID2D1DeviceContext = rt.cast()?;
    let params = D2D1_LAYER_PARAMETERS1 {
        contentBounds: bounds,
        geometricMask: std::mem::ManuallyDrop::new(mask.cloned()),
        maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
        maskTransform: windows_numerics::Matrix3x2::identity(),
        opacity,
        opacityBrush: std::mem::ManuallyDrop::new(None),
        layerOptions: D2D1_LAYER_OPTIONS1_NONE,
    };
    // SAFETY: a plain push on this thread's device context, in a BeginDraw;
    // the layer is Direct2D's own (none given), and the mask is released
    // after the call, which takes its own reference.
    unsafe { dc.PushLayer(&params, None) };
    drop(std::mem::ManuallyDrop::into_inner(params.geometricMask));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::win::text::Text;

    #[test]
    fn a_full_map_drops_the_entry_used_longest_ago() {
        let mut lru = Lru::new(3);
        for n in 0..3 {
            lru.insert(n, n * 10);
        }
        // 0 is used again, so 1 is now the oldest.
        assert_eq!(lru.get(&0), Some(&0));
        lru.insert(3, 30);
        assert_eq!(lru.len(), 3);
        assert_eq!(lru.get(&1), None, "the least recently used went");
        assert_eq!(lru.get(&0), Some(&0));
        assert_eq!(lru.get(&2), Some(&20));
        assert_eq!(lru.get(&3), Some(&30));
    }

    #[test]
    fn a_map_never_holds_more_than_its_bound() {
        let mut lru = Lru::new(LAYOUTS);
        for n in 0..LAYOUTS * 3 {
            lru.insert(n, ());
            assert!(lru.len() <= LAYOUTS);
        }
        assert_eq!(lru.len(), LAYOUTS);
        // The newest are the ones kept.
        assert!(lru.get(&(LAYOUTS * 3 - 1)).is_some());
        assert!(lru.get(&0).is_none());
        let mut clips = Lru::new(CLIPS);
        for n in 0..CLIPS + 1 {
            clips.insert(n, ());
        }
        assert_eq!(clips.len(), CLIPS);
        assert!(clips.get(&0).is_none(), "the oldest of 65 went");
        assert!(clips.get(&1).is_some());
    }

    #[test]
    fn replacing_an_entry_drops_nothing() {
        let mut lru = Lru::new(2);
        lru.insert("a", 1);
        lru.insert("b", 2);
        lru.insert("a", 3);
        assert_eq!(lru.len(), 2);
        assert_eq!(lru.get(&"a"), Some(&3));
        assert_eq!(lru.get(&"b"), Some(&2));
    }

    #[test]
    fn clearing_forgets_everything() {
        let mut lru = Lru::new(4);
        lru.insert(1, 1);
        lru.clear();
        assert_eq!(lru.len(), 0);
        assert_eq!(lru.get(&1), None);
    }

    #[test]
    fn a_layout_is_keyed_by_words_format_and_box() {
        use crate::win::text::{Ramp, Style};
        let text = Text::new().unwrap();
        let caption = text.format(Style::new(Ramp::Caption), 1.0).unwrap();
        let body = text.format(Style::new(Ramp::Body), 1.0).unwrap();
        let style = Style::new(Ramp::Caption);
        let key =
            |s: &str, f: &IDWriteTextFormat, w: f32, h: f32| LayoutKey::styled(s, f, w, h, style);
        let base = key("Done", &caption, 120.0, 20.0);
        assert_eq!(base, key("Done", &caption, 120.0, 20.0));
        assert_ne!(base, key("Done!", &caption, 120.0, 20.0));
        assert_ne!(base, key("Done", &body, 120.0, 20.0));
        assert_ne!(base, key("Done", &caption, 121.0, 20.0));
        assert_ne!(base, key("Done", &caption, 120.0, 21.0));
        // The box is told apart by its bits.
        assert_ne!(
            key("Done", &caption, 0.0, 20.0),
            key("Done", &caption, -0.0, 20.0)
        );
    }

    #[test]
    fn a_styled_layout_is_keyed_by_its_style_too() {
        use crate::win::text::{Ramp, Style};
        let text = Text::new().unwrap();
        let caption = text.format(Style::new(Ramp::Caption), 1.0).unwrap();
        let f = &caption;
        let body = Style::new(Ramp::Body);
        let base = LayoutKey::styled("Done", f, 120.0, 20.0, body);
        assert_eq!(base, LayoutKey::styled("Done", f, 120.0, 20.0, body));
        // The same words, format and box in another style, or as a code or
        // a glyph, are other layouts.
        assert_ne!(base, LayoutKey::styled("Done", f, 120.0, 20.0, body.code()));
        assert_ne!(
            base,
            LayoutKey::styled("Done", f, 120.0, 20.0, body.centered())
        );
        assert_ne!(
            base,
            LayoutKey::styled("Done", f, 120.0, 20.0, Style::new(Ramp::Title))
        );
        // Runs set strong make another layout; the same runs, the same.
        let strong =
            |runs: &[(u32, u32)]| LayoutKey::styled_runs("Done", f, 120.0, 20.0, body, runs);
        assert_eq!(base, strong(&[]));
        assert_ne!(base, strong(&[(0, 2)]));
        assert_eq!(strong(&[(0, 2)]), strong(&[(0, 2)]));
        assert_ne!(strong(&[(0, 2)]), strong(&[(1, 2)]));
        assert_ne!(base, LayoutKey::glyph("Done", f, 120.0, 20.0));
    }

    #[test]
    fn a_clip_is_keyed_by_its_rectangle_and_radii() {
        let r = |l: f32, rx: f32| D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: l,
                top: 2.0,
                right: 30.0,
                bottom: 40.0,
            },
            radiusX: rx,
            radiusY: rx,
        };
        assert_eq!(ClipKey::new(&r(1.0, 8.0)), ClipKey::new(&r(1.0, 8.0)));
        assert_ne!(ClipKey::new(&r(1.0, 8.0)), ClipKey::new(&r(1.5, 8.0)));
        assert_ne!(ClipKey::new(&r(1.0, 8.0)), ClipKey::new(&r(1.0, 6.0)));
    }
}
