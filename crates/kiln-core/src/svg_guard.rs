//! Limits for drawing SVGs.
//!
//! An SVG can be built to wear Kiln out: elements nested deeper than the
//! drawing code can follow, a canvas of trillions of pixels, or a few
//! effects laid over a canvas so large that drawing them takes hours. Each
//! limit here fails that one job with a plain sentence, and the app lives.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use resvg::usvg::{self, filter::Kind};

use crate::error::{Error, Result};

/// The most elements an SVG may nest, counting the elements a `use` draws.
/// Real drawings stay well under a hundred.
pub(crate) const MAX_SVG_DEPTH: usize = 256;

/// The most pixels a drawing of an SVG may have.
pub(crate) const MAX_SVG_PIXELS: u64 = 50_000_000;

/// The longest side a drawing of an SVG may have, in pixels.
const MAX_SVG_SIDE: f64 = 16_384.0;

/// How long a drawing may take before it is given up. The time is checked
/// between the elements at the top of the drawing.
pub(crate) const SVG_TIME_BUDGET: Duration = Duration::from_secs(60);

/// The most an SVG's effects may cost, as [`filter_cost`] counts.
///
/// A 2,000-pixel square with a blur over all of it costs 17 million, so this
/// leaves it more than a hundred times as much room. Blurring runs at about
/// 40 million of these a second (a 4,000-pixel square took 1.6 s and an
/// 8,000-pixel one 6.5 s, PNG included, in a release build), so this is a
/// minute of blurring, the time budget. The other primitives are weighed
/// against a blur (see [`filter_cost`]); morphology, which reads a square of
/// pixels in a tight loop, was converted in 1.7 s for a 2,000-pixel square
/// with a radius of 10 (2.5 billion by its reads, 79 million as weighed). The
/// hostile `svg-filter.svg` costs 9 trillion.
pub(crate) const MAX_FILTER_COST: u64 = 2_000_000_000;

/// "Nested too deeply": the SVG's elements, or the elements they draw by
/// reference, are more than [`MAX_SVG_DEPTH`] deep, or a `use` draws itself,
/// or an entity holds markup.
pub(crate) fn too_deep() -> Error {
    Error::Unsafe("This SVG is nested too deeply to draw.".to_owned())
}

/// "Effects too heavy": see [`filter_cost`].
pub(crate) fn too_heavy() -> Error {
    Error::Unsafe("This SVG's effects are too heavy to draw.".to_owned())
}

/// "Too slow": the drawing went past [`SVG_TIME_BUDGET`].
pub(crate) fn too_slow() -> Error {
    Error::Unsafe("This SVG took too long to draw.".to_owned())
}

/// How many elements deep the drawing of `xml` goes, counting what an
/// element draws as its children. Besides its child elements that is the
/// element a `use` points at; the mask, clip path, filter, pattern, gradient
/// or marker a `url(#…)` names in a property or in `style`; and what the
/// `href` of a pattern, gradient, text path or `feImage` names.
///
/// Fails when that is more than [`MAX_SVG_DEPTH`], when a `use` draws an
/// element it is inside, or when an entity is declared with markup in its
/// value. Any other reference that comes back to an element it is inside is
/// dropped, as the SVG parser drops it.
///
/// Text that isn't XML gives 0: the SVG parser words that failure.
///
/// The scan keeps its own stack, so a file nested a million deep, or a
/// chain of a million masks, costs memory and no more call depth than a file
/// nested once.
pub(crate) fn nesting_depth(xml: &str) -> Result<usize> {
    // The reader below recurses once per level, and once more for each level
    // an entity's markup holds, so a file too deep for it, or with any such
    // entity, is turned away first.
    let scan = scan_tags(xml);
    if scan.deepest > MAX_SVG_DEPTH || scan.markup_in_entity {
        return Err(too_deep());
    }

    // The parser's own options: what is seen here is what it will see.
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    let Ok(doc) = roxmltree::Document::parse_with_options(xml, options) else {
        return Ok(0);
    };

    // The first element with an id is the one a reference finds.
    let mut ids = HashMap::new();
    for node in doc.descendants().filter(|n| n.is_element()) {
        if let Some(id) = node.attribute("id") {
            ids.entry(id).or_insert(node.id());
        }
    }

    enum Step<'a, 'i> {
        Enter(roxmltree::Node<'a, 'i>),
        Exit(roxmltree::Node<'a, 'i>, Vec<roxmltree::NodeId>),
    }
    let root = doc.root_element();
    let mut heights = HashMap::new();
    // The elements being measured: the path from the root to this one.
    let mut open = HashSet::new();
    let mut steps = vec![Step::Enter(root)];
    while let Some(step) = steps.pop() {
        match step {
            Step::Enter(node) => {
                if heights.contains_key(&node.id()) {
                    continue;
                }
                // A `use` of an element it is inside.
                if !open.insert(node.id()) {
                    return Err(too_deep());
                }
                let drawn: Vec<_> = drawn_by(&doc, &ids, node)
                    .into_iter()
                    .filter(|edge| edge.cycle_is_fatal || !open.contains(&edge.target.id()))
                    .map(|edge| edge.target)
                    .collect();
                steps.push(Step::Exit(node, drawn.iter().map(|n| n.id()).collect()));
                steps.extend(drawn.into_iter().map(Step::Enter));
            }
            Step::Exit(node, drawn) => {
                open.remove(&node.id());
                let tallest = drawn
                    .iter()
                    .filter_map(|id| heights.get(id))
                    .max()
                    .copied()
                    .unwrap_or(0);
                if tallest >= MAX_SVG_DEPTH {
                    return Err(too_deep());
                }
                heights.insert(node.id(), tallest + 1);
            }
        }
    }
    Ok(heights[&root.id()])
}

/// What a scan of the tags of an XML text found.
struct Scan {
    /// The most elements open at once.
    deepest: usize,
    /// Whether an entity is declared with markup in its value.
    markup_in_entity: bool,
}

/// Reads the tags of `xml` the way roxmltree's reader meets them, without a
/// call for each level, to say whether it is safe to hand to that reader.
///
/// roxmltree reads each level of nesting with a call of its own, so a file
/// nested a million deep would overflow the stack inside the reader, before
/// anything was counted. An entity whose value holds markup is read the same
/// way where it is used, however short that is in the text of the file, so
/// any such value is flagged. The prolog is followed as the reader follows
/// it, and a DOCTYPE the reader would refuse stops the scan: the reader
/// refuses it before it reads an element. Text that isn't XML gives whatever
/// its angle brackets add up to.
fn scan_tags(xml: &str) -> Scan {
    let bytes = xml.as_bytes();
    let mut scan = Scan {
        deepest: 0,
        markup_in_entity: false,
    };
    let mut at = if xml.starts_with('\u{feff}') { 3 } else { 0 };

    if starts(xml, at, "<?xml ") {
        // The reader refuses a declaration that never ends.
        let Some(end) = declaration_end(xml, at + 6) else {
            return scan;
        };
        at = end;
    }
    at = skip_misc(xml, at);
    if starts(xml, at, "<!DOCTYPE") {
        let Some((len, markup)) = doctype(&xml[at..]) else {
            return scan;
        };
        if markup {
            scan.markup_in_entity = true;
            return scan;
        }
        at = skip_misc(xml, at + len);
    }

    let mut open = 0usize;
    while let Some(found) = xml[at..].find('<') {
        at += found;
        match markup(&xml[at..]) {
            Markup::Skip(len) => {
                at += len;
                continue;
            }
            // The reader refuses the file here.
            Markup::Refused => break,
            Markup::Tag => {}
        }
        // A tag runs to the first '>' that isn't inside quotes.
        let (mut end, mut quote) = (at + 1, None);
        while end < bytes.len() {
            match (quote, bytes[end]) {
                (Some(q), c) if c == q => quote = None,
                (None, q @ (b'"' | b'\'')) => quote = Some(q),
                (None, b'>') => break,
                _ => {}
            }
            end += 1;
        }
        if bytes.get(at + 1) == Some(&b'/') {
            open = open.saturating_sub(1);
        } else if end > at + 1 && bytes[end - 1] == b'/' {
            scan.deepest = scan.deepest.max(open + 1);
        } else {
            open += 1;
            scan.deepest = scan.deepest.max(open);
        }
        at = (end + 1).min(bytes.len());
    }
    scan
}

/// What starts at an angle bracket in the body of a document.
enum Markup {
    /// A comment, CDATA section or processing instruction, this many bytes long.
    Skip(usize),
    /// Another `<!`, which the reader refuses.
    Refused,
    /// A tag.
    Tag,
}

fn markup(rest: &str) -> Markup {
    let ends = |start: usize, end: &str| {
        Markup::Skip(
            rest[start..]
                .find(end)
                .map_or(rest.len(), |n| start + n + end.len()),
        )
    };
    if rest.starts_with("<!--") {
        ends(4, "-->")
    } else if rest.starts_with("<![CDATA[") {
        ends(9, "]]>")
    } else if rest.starts_with("<?") {
        ends(2, "?>")
    } else if rest.starts_with("<!") {
        Markup::Refused
    } else {
        Markup::Tag
    }
}

/// Whether `xml` has `pat` at byte `at`.
fn starts(xml: &str, at: usize, pat: &str) -> bool {
    xml.get(at..).is_some_and(|rest| rest.starts_with(pat))
}

/// The position after the next `end` at or after `at`, or the end of `xml`.
fn past(xml: &str, at: usize, end: &str) -> usize {
    xml.get(at..)
        .and_then(|rest| rest.find(end))
        .map_or(xml.len(), |n| at + n + end.len())
}

/// The position after the `?>` that ends the XML declaration whose body starts
/// at `at`. Its version, encoding and standalone are quoted strings, and may
/// hold a `?>` of their own, so the first one outside quotes is the end.
fn declaration_end(xml: &str, at: usize) -> Option<usize> {
    let b = xml.as_bytes();
    let mut quote = None;
    for (i, &c) in b.iter().enumerate().skip(at) {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, q @ (b'"' | b'\'')) => quote = Some(q),
            (None, b'?') if b.get(i + 1) == Some(&b'>') => return Some(i + 2),
            _ => {}
        }
    }
    None
}

/// Past any spaces, comments and processing instructions from `at`.
fn skip_misc(xml: &str, mut at: usize) -> usize {
    loop {
        at = skip_spaces(xml.as_bytes(), at);
        if starts(xml, at, "<!--") {
            at = past(xml, at + 4, "-->");
        } else if starts(xml, at, "<?") {
            at = past(xml, at + 2, "?>");
        } else {
            return at;
        }
    }
}

fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r')
}

fn skip_spaces(b: &[u8], mut at: usize) -> usize {
    while b.get(at).is_some_and(|&c| is_space(c)) {
        at += 1;
    }
    at
}

/// Past the spaces from `at`, when there is at least one.
fn spaces(b: &[u8], at: usize) -> Option<usize> {
    b.get(at)
        .is_some_and(|&c| is_space(c))
        .then(|| skip_spaces(b, at))
}

/// Past a name from `at`: up to a space, a quote or a bracket.
fn skip_name(b: &[u8], mut at: usize) -> usize {
    while b
        .get(at)
        .is_some_and(|&c| !is_space(c) && !matches!(c, b'[' | b'>' | b'"' | b'\''))
    {
        at += 1;
    }
    at
}

/// The string in quotes at `at`: the position after it, and what it holds.
fn quoted(s: &str, at: usize) -> Option<(usize, &str)> {
    let quote = *s.as_bytes().get(at).filter(|&&c| c == b'"' || c == b'\'')?;
    let end = s.get(at + 1..)?.find(quote as char)? + at + 1;
    Some((end + 1, &s[at + 1..end]))
}

/// The position after the `SYSTEM "…"` or `PUBLIC "…" "…"` at `at`.
fn external_id(s: &str, at: usize) -> Option<usize> {
    let public = starts(s, at, "PUBLIC");
    if !public && !starts(s, at, "SYSTEM") {
        return None;
    }
    let b = s.as_bytes();
    let (mut end, _) = quoted(s, spaces(b, at + 6)?)?;
    if public {
        end = quoted(s, spaces(b, end)?)?.0;
    }
    Some(end)
}

/// The DOCTYPE at the start of `s`, read as roxmltree reads it: its length,
/// and whether an entity in it is declared with markup in its value. `None`
/// when roxmltree would refuse it.
fn doctype(s: &str) -> Option<(usize, bool)> {
    let b = s.as_bytes();
    let mut at = skip_name(b, spaces(b, 9)?);
    at = skip_spaces(b, at);
    if starts(s, at, "SYSTEM") || starts(s, at, "PUBLIC") {
        at = skip_spaces(b, external_id(s, at)?);
    }
    match b.get(at)? {
        b'>' => return Some((at + 1, false)),
        b'[' => at += 1,
        _ => return None,
    }
    let mut markup = false;
    loop {
        at = skip_spaces(b, at);
        if at >= b.len() {
            return None;
        }
        if starts(s, at, "<!ENTITY") {
            at = spaces(b, at + 8)?;
            if b.get(at) == Some(&b'%') {
                at = spaces(b, at + 1)?;
            }
            at = spaces(b, skip_name(b, at))?;
            match b.get(at)? {
                b'"' | b'\'' => {
                    let (end, value) = quoted(s, at)?;
                    markup |= value.contains('<');
                    at = end;
                }
                _ => {
                    at = skip_spaces(b, external_id(s, at)?);
                    if starts(s, at, "NDATA") {
                        at = skip_name(b, spaces(b, at + 5)?);
                    }
                }
            }
            at = skip_spaces(b, at);
            if b.get(at) != Some(&b'>') {
                return None;
            }
            at += 1;
        } else if starts(s, at, "<!--") {
            at = s.get(at + 4..)?.find("-->")? + at + 4 + 3;
        } else if starts(s, at, "<?") {
            at = s.get(at + 2..)?.find("?>")? + at + 2 + 2;
        } else if b[at] == b']' {
            at = skip_spaces(b, at + 1);
            return (b.get(at) == Some(&b'>')).then_some((at + 1, markup));
        } else if ["<!ELEMENT", "<!ATTLIST", "<!NOTATION"]
            .iter()
            .any(|decl| starts(s, at, decl))
        {
            at = s.get(at..)?.find('>')? + at + 1;
        } else {
            return None;
        }
    }
}

/// An element another element draws inside itself.
struct Edge<'a, 'i> {
    target: roxmltree::Node<'a, 'i>,
    /// Whether coming back to an element it is inside fails the file. That is
    /// so for a `use`; the SVG parser drops any other reference that does.
    cycle_is_fatal: bool,
}

/// The properties whose `url(#id)` draws the element it names.
const REFERENCING: [&str; 8] = [
    "mask",
    "clip-path",
    "filter",
    "fill",
    "stroke",
    "marker-start",
    "marker-mid",
    "marker-end",
];

/// The elements whose `href` draws the element it names.
const LINKING: [&str; 6] = [
    "use",
    "pattern",
    "linearGradient",
    "radialGradient",
    "textPath",
    "feImage",
];

/// What `node` draws inside itself: its child elements, and what its
/// references name.
fn drawn_by<'a, 'i>(
    doc: &'a roxmltree::Document<'i>,
    ids: &HashMap<&str, roxmltree::NodeId>,
    node: roxmltree::Node<'a, 'i>,
) -> Vec<Edge<'a, 'i>> {
    let mut edges: Vec<Edge> = node
        .children()
        .filter(|n| n.is_element())
        .map(|target| Edge {
            target,
            cycle_is_fatal: true,
        })
        .collect();
    let find = |id: &str| ids.get(id).and_then(|id| doc.get_node(*id));

    let name = node.tag_name().name();
    if LINKING.contains(&name) {
        let target = node
            .attribute(("http://www.w3.org/1999/xlink", "href"))
            .or_else(|| node.attribute("href"))
            .and_then(|href| href.trim_start().strip_prefix('#'))
            .and_then(|id| find(id.split(' ').next().unwrap_or(id)));
        if let Some(target) = target {
            edges.push(Edge {
                target,
                cycle_is_fatal: name == "use",
            });
        }
    }
    for attribute in node.attributes() {
        if attribute.name() == "style" || REFERENCING.contains(&attribute.name()) {
            edges.extend(referenced_ids(attribute.value()).filter_map(|id| {
                Some(Edge {
                    target: find(id)?,
                    cycle_is_fatal: false,
                })
            }));
        }
    }
    edges
}

/// The ids that `url(#id)` names in `value`, which is a property or a
/// `style`.
fn referenced_ids(value: &str) -> impl Iterator<Item = &str> {
    value.split("url(").skip(1).filter_map(|after| {
        let id = after
            .trim_start_matches([' ', '\t', '\n', '\r', '"', '\''])
            .strip_prefix('#')?;
        let end = id.find([')', ' ', '\t', '\n', '\r', '"', '\'']);
        Some(&id[..end.unwrap_or(id.len())])
    })
}

/// The pixel size to draw an SVG that is `w` by `h` at: the same shape,
/// rounded, with neither side over 16,384 and no more than
/// [`MAX_SVG_PIXELS`] in all. A drawing within both limits keeps its size.
pub(crate) fn capped_size(w: f32, h: f32) -> (u32, u32) {
    // Not a number, or far past anything drawn: clamped before the sums.
    let tidy = |n: f32| {
        if n.is_nan() {
            1.0
        } else {
            (n as f64).clamp(1.0, 1e12)
        }
    };
    let (mut w, mut h) = (tidy(w), tidy(h));
    let shrink = (MAX_SVG_SIDE / w.max(h)).min(1.0);
    (w, h) = (w * shrink, h * shrink);
    if w * h > MAX_SVG_PIXELS as f64 {
        let shrink = (MAX_SVG_PIXELS as f64 / (w * h)).sqrt();
        (w, h) = (w * shrink, h * shrink);
    }
    let side = |n: f64| n.round().clamp(1.0, MAX_SVG_SIDE) as u32;
    let (mut w, mut h) = (side(w), side(h));
    // Rounding up can pass the limit by a row or a column.
    while w as u64 * h as u64 > MAX_SVG_PIXELS {
        if w >= h {
            w -= 1;
        } else {
            h -= 1;
        }
    }
    (w, h)
}

/// What the effects in `tree` will cost to draw at `scale` times its own
/// size, in pixels worked over.
///
/// For each filtered group that is the filter region's pixels at that scale,
/// once for each primitive in the filter, weighed by how much work a pixel is
/// to that primitive:
///
/// - 3 for a Gaussian blur, and 4 for a drop shadow, which is a blur and the
///   shadow laid on the source;
/// - a thirty-second of the `(2 * radius + 1)` pixels on a side that
///   morphology reads, and at least 1;
/// - the columns times the rows of a convolve matrix;
/// - the octaves of a turbulence, which a file may set to any number;
/// - 1 for the rest.
///
/// Groups inside clip paths, masks and patterns count too.
pub(crate) fn filter_cost(tree: &usvg::Tree, scale: f32) -> u64 {
    cost_under(tree.root(), scale)
}

/// The cost of the filters on `root` and everything inside it. The groups
/// are walked with a list of their own, and only a clip path, mask or pattern
/// inside them is entered by a call.
fn cost_under(root: &usvg::Group, scale: f32) -> u64 {
    let mut total = 0u64;
    let mut groups = vec![root];
    while let Some(group) = groups.pop() {
        total = total.saturating_add(own_cost(group, scale));
        for node in group.children() {
            match node {
                usvg::Node::Group(inner) => groups.push(inner),
                usvg::Node::Text(text) => groups.push(text.flattened()),
                _ => {}
            }
            node.subroots(|subroot| total = total.saturating_add(cost_under(subroot, scale)));
        }
    }
    total
}

/// The cost of the filters on `group` itself.
fn own_cost(group: &usvg::Group, scale: f32) -> u64 {
    let ts = group.abs_transform().post_scale(scale, scale);
    let (sx, sy) = ts.get_scale();
    let mut total = 0u64;
    for filter in group.filters() {
        let Some(region) = filter.rect().transform(ts) else {
            continue;
        };
        let weight: f64 = filter
            .primitives()
            .iter()
            .map(|primitive| match primitive.kind() {
                Kind::GaussianBlur(_) => 3.0,
                // A blur, and the shadow laid over the source.
                Kind::DropShadow(_) => 4.0,
                // Every pixel reads a square of pixels around it, but in a
                // tight loop: a step of it is a thirty-second of a step of a
                // blur.
                Kind::Morphology(m) => {
                    let rx = m.radius_x().get() as f64 * sx as f64;
                    let ry = m.radius_y().get() as f64 * sy as f64;
                    ((2.0 * rx + 1.0) * (2.0 * ry + 1.0) / 32.0).max(1.0)
                }
                // Every pixel reads and sums the pixels under the matrix.
                Kind::ConvolveMatrix(c) => {
                    (c.matrix().columns() as f64 * c.matrix().rows() as f64).max(1.0)
                }
                // Every pixel is worked once for each octave, and the number
                // of octaves is whatever the file says.
                Kind::Turbulence(t) => (t.num_octaves() as f64).max(1.0),
                _ => 1.0,
            })
            .sum();
        let cost = region.width() as f64 * region.height() as f64 * weight;
        // A float past the range of u64 saturates.
        total = total.saturating_add(cost as u64);
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOO_DEEP: &str = "This SVG is nested too deeply to draw.";

    /// `groups` nested groups with `inner` in the innermost.
    fn nested(groups: usize, inner: &str) -> String {
        format!("{}{inner}{}", "<g>".repeat(groups), "</g>".repeat(groups))
    }

    /// Runs `f` on a thread with a 256 KiB stack, which a scan that recursed
    /// a few thousand levels would overflow. (roxmltree's reader recurses once
    /// per level of XML, about a kilobyte each in a debug build, so a document
    /// that is itself deeper than a couple of hundred doesn't fit.)
    fn on_a_small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::Builder::new()
            .stack_size(256 << 10)
            .spawn(f)
            .unwrap()
            .join()
            .expect("the scan finished without overflowing its stack")
    }

    #[test]
    fn depth_counts_nesting() {
        assert_eq!(nesting_depth("<svg><g><g/></g></svg>").unwrap(), 3);
        assert_eq!(nesting_depth("<svg/>").unwrap(), 1);
        // Text and comments between elements add no level.
        assert_eq!(nesting_depth("<svg> <!-- a --><g>x</g> </svg>").unwrap(), 2);
    }

    #[test]
    fn depth_follows_use_chains() {
        // The first group of 200 levels ends in a `use` of a second group of
        // 200 levels: neither is deeper than 256, the drawing is 401.
        let first = nested(199, r##"<use href="#b"/>"##);
        let second = format!(r#"<g id="b">{}</g>"#, nested(199, ""));
        let apart = format!("<svg>{first}</svg>");
        assert_eq!(nesting_depth(&apart).unwrap(), 201, "a missing target");

        let xml = format!("<svg>{first}{second}</svg>");
        let err = nesting_depth(&xml).unwrap_err();
        assert!(matches!(err, Error::Unsafe(_)), "{err:?}");
        assert_eq!(err.to_string(), TOO_DEEP);
    }

    #[test]
    fn a_long_chain_of_uses_is_followed_without_recursion() {
        // Each group draws the next through a `use`: the file is 3 deep and
        // the drawing 40,000.
        let links = 20_000;
        let mut xml = String::from("<svg>");
        for n in 0..links {
            xml += &format!(r##"<g id="g{n}"><use href="#g{}"/></g>"##, n + 1);
        }
        xml += &format!(r#"<g id="g{links}"/></svg>"#);
        let err = on_a_small_stack(move || nesting_depth(&xml)).unwrap_err();
        assert!(matches!(err, Error::Unsafe(_)), "{err:?}");
        assert_eq!(err.to_string(), TOO_DEEP);
    }

    #[test]
    fn xlink_href_is_followed_too() {
        let first = nested(199, r##"<use xlink:href="#b"/>"##);
        let second = format!(r#"<g id="b">{}</g>"#, nested(199, ""));
        let xml =
            format!(r#"<svg xmlns:xlink="http://www.w3.org/1999/xlink">{first}{second}</svg>"#);
        let err = nesting_depth(&xml).unwrap_err();
        assert_eq!(err.to_string(), TOO_DEEP);
    }

    #[test]
    fn a_use_chain_that_stays_shallow_is_counted() {
        // A `use` costs one level and the level of its target.
        let xml = r##"<svg><g id="a"><g><g/></g></g><use href="#a"/></svg>"##;
        assert_eq!(nesting_depth(xml).unwrap(), 5);
    }

    #[test]
    fn a_use_cycle_is_refused() {
        for xml in [
            r##"<g id="a"><use href="#a"/></g>"##,
            r##"<svg><g id="a"><use href="#b"/></g><g id="b"><use href="#a"/></g></svg>"##,
            r##"<svg><use id="u" href="#u"/></svg>"##,
        ] {
            let err = on_a_small_stack(move || nesting_depth(xml)).unwrap_err();
            assert!(matches!(err, Error::Unsafe(_)), "{xml}: {err:?}");
            assert_eq!(err.to_string(), TOO_DEEP, "{xml}");
        }
    }

    #[test]
    fn a_very_deep_file_is_refused_without_recursion() {
        let xml = format!("<svg>{}</svg>", nested(50_000, ""));
        let err = on_a_small_stack(move || nesting_depth(&xml)).unwrap_err();
        assert_eq!(err.to_string(), TOO_DEEP);
    }

    #[test]
    fn exactly_the_limit_is_allowed() {
        let ok = format!("<svg>{}</svg>", nested(MAX_SVG_DEPTH - 1, ""));
        assert_eq!(nesting_depth(&ok).unwrap(), MAX_SVG_DEPTH);
        let over = format!("<svg>{}</svg>", nested(MAX_SVG_DEPTH, ""));
        assert!(nesting_depth(&over).is_err());
    }

    #[test]
    fn markup_that_is_not_an_element_adds_no_depth() {
        let xml = r#"<?xml version="1.0"?><!DOCTYPE svg [
  <!ENTITY ns "http://www.w3.org/2000/svg">
  <!-- it's <g><g><g> -->
]>
<svg xmlns="&ns;"><!-- <g><g> --><![CDATA[<g><g>]]><g a=">" b='x>y'><g/></g></svg>"#;
        assert_eq!(nesting_depth(xml).unwrap(), 3);
    }

    #[test]
    fn scan_tags_counts_tags_without_reading_them() {
        assert_eq!(scan_tags("<a><b/><c><d/></c></a>").deepest, 3);
        assert_eq!(scan_tags("<a></a><b></b>").deepest, 1);
        assert_eq!(scan_tags("</a></b><c>").deepest, 1);
        assert_eq!(scan_tags("<a b=\"<>\"><c d='/>'/></a>").deepest, 2);
        // Missing ends are not a reason to panic.
        for cut in [
            "<a b=\"",
            "<!--",
            "<![CDATA[",
            "<?",
            "<!DOCTYPE a [",
            "<a",
            "<",
        ] {
            let _ = scan_tags(cut);
        }
        assert_eq!(scan_tags("<g>é<g>é</g></g>").deepest, 2);
    }

    #[test]
    fn text_that_is_not_xml_is_left_for_the_parser() {
        // usvg words this failure; the scan has nothing to add.
        assert_eq!(nesting_depth("<svg><g></svg>").unwrap(), 0);
        assert_eq!(nesting_depth("not xml").unwrap(), 0);
    }

    #[test]
    fn capped_size_keeps_the_shape() {
        let (w, h) = capped_size(1_000_000.0, 1_000_000.0);
        assert!(w as u64 * h as u64 <= MAX_SVG_PIXELS, "{w} x {h}");
        assert_eq!(w, h);
        assert!(w > 7000, "{w} x {h} is far under the cap");

        let (w, h) = capped_size(16_000.0, 4_000.0);
        assert!(w as u64 * h as u64 <= MAX_SVG_PIXELS, "{w} x {h}");
        assert!(w.max(h) <= 16_384);
        assert!((w as f64 / 4.0 - h as f64).abs() <= 1.0, "{w} x {h}");

        assert_eq!(capped_size(800.0, 600.0), (800, 600));
    }

    #[test]
    fn capped_size_limits_a_long_thin_drawing() {
        let (w, h) = capped_size(1_000_000.0, 10.0);
        assert_eq!(w, 16_384);
        assert_eq!(h, 1);
        let (w, h) = capped_size(0.2, 0.4);
        assert_eq!((w, h), (1, 1));
    }

    #[test]
    fn capped_size_survives_odd_numbers() {
        assert_eq!(capped_size(f32::NAN, 5.0), (1, 5));
        let (w, h) = capped_size(f32::INFINITY, f32::INFINITY);
        assert!(w as u64 * h as u64 <= MAX_SVG_PIXELS);
    }

    /// The cost of `body` drawn at `scale`, in a 100 x 100 drawing.
    fn cost(body: &str, scale: f32) -> u64 {
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">{body}</svg>"#
        );
        let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).unwrap();
        filter_cost(&tree, scale)
    }

    /// A filter over exactly the 100 x 100 drawing, then a rect that uses it.
    fn filtered(primitives: &str) -> String {
        format!(
            r##"<filter id="f" filterUnits="userSpaceOnUse" x="0" y="0" width="100" height="100">{primitives}</filter>
<rect width="100" height="100" fill="blue" filter="url(#f)"/>"##
        )
    }

    #[test]
    fn the_ceiling_leaves_room_for_an_ordinary_blur() {
        // A 2,000-pixel square with a blur over all of it, as the hostile
        // tests draw: its region is 2,400 pixels square.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="2000" height="2000"><filter id="f"><feGaussianBlur stdDeviation="5"/></filter><rect width="2000" height="2000" filter="url(#f)"/></svg>"##;
        let tree = resvg::usvg::Tree::from_str(svg, &resvg::usvg::Options::default()).unwrap();
        let ordinary = filter_cost(&tree, 1.0);
        assert_eq!(ordinary, 2400 * 2400 * 3);
        assert!(ordinary * 10 <= MAX_FILTER_COST, "{ordinary}");
    }

    #[test]
    fn a_drawing_without_filters_costs_nothing() {
        assert_eq!(cost(r#"<rect width="100" height="100"/>"#, 1.0), 0);
    }

    #[test]
    fn filter_cost_weighs_each_primitive() {
        // 10,000 pixels of region, once per primitive.
        assert_eq!(cost(&filtered(r#"<feFlood/>"#), 1.0), 10_000);
        assert_eq!(
            cost(&filtered(r#"<feGaussianBlur stdDeviation="5"/>"#), 1.0),
            30_000
        );
        assert_eq!(
            cost(
                &filtered(r#"<feFlood/><feGaussianBlur stdDeviation="1"/>"#),
                1.0
            ),
            40_000
        );
    }

    #[test]
    fn filter_cost_follows_the_render_scale() {
        // At 3 times the size the region has 9 times the pixels, and the
        // radius in pixels is 3 times as big: (2 * 6 + 1)^2 / 32 = 169 / 32.
        assert_eq!(
            cost(&filtered(r#"<feMorphology radius="2"/>"#), 3.0),
            475_312
        );
        assert_eq!(cost(&filtered(r#"<feFlood/>"#), 0.5), 2_500);
    }

    #[test]
    fn filter_cost_counts_filters_inside_masks_and_patterns() {
        let body = r##"<filter id="f" filterUnits="userSpaceOnUse" x="0" y="0" width="100" height="100"><feFlood/></filter>
<mask id="m"><rect width="100" height="100" fill="white" filter="url(#f)"/></mask>
<rect width="100" height="100" mask="url(#m)"/>"##;
        assert_eq!(cost(body, 1.0), 10_000);

        let body = r##"<filter id="f" filterUnits="userSpaceOnUse" x="0" y="0" width="100" height="100"><feFlood/></filter>
<pattern id="p" width="100" height="100" patternUnits="userSpaceOnUse"><rect width="100" height="100" filter="url(#f)"/></pattern>
<rect width="100" height="100" fill="url(#p)"/>"##;
        assert_eq!(cost(body, 1.0), 10_000);
    }

    #[test]
    fn the_hostile_effects_are_far_over_the_ceiling() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/hostile/svg-filter.svg");
        let svg = std::fs::read_to_string(path).unwrap();
        let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).unwrap();
        // At the size it would be drawn: 16,000 pixels square, cut to 50 megapixels.
        let (w, _) = capped_size(tree.size().width(), tree.size().height());
        let cost = filter_cost(&tree, w as f32 / tree.size().width());
        assert!(cost > 1000 * MAX_FILTER_COST, "{cost}");
    }

    // ----- Fix round 1 -----

    /// An Illustrator file's DOCTYPE: entities that are text only.
    const ILLUSTRATOR: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<!-- Generator: Adobe Illustrator 16.0.0, SVG Export Plug-In . SVG Version: 6.00 Build 0)  -->
<!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd" [
	<!ENTITY ns_extend "http://ns.adobe.com/Extensibility/1.0/">
	<!ENTITY ns_ai "http://ns.adobe.com/AdobeIllustrator/10.0/">
	<!ENTITY ns_graphs "http://ns.adobe.com/Graphs/1.0/">
	<!ENTITY ns_vars "http://ns.adobe.com/Variables/1.0/">
	<!ENTITY ns_imrep "http://ns.adobe.com/ImageReplacement/1.0/">
	<!ENTITY ns_sfw "http://ns.adobe.com/SaveForWeb/1.0/">
	<!ENTITY ns_custom "http://ns.adobe.com/GenericCustomNamespace/1.0/">
	<!ENTITY ns_adobe_xpath "http://ns.adobe.com/XPath/1.0/">
]>
<svg version="1.1" id="Layer_1" xmlns:x="&ns_extend;" xmlns:i="&ns_ai;" xmlns:graph="&ns_graphs;"
	 xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" x="0px" y="0px"
	 width="64px" height="48px" viewBox="0 0 64 48" xml:space="preserve">
<switch>
	<foreignObject requiredExtensions="&ns_ai;" x="0" y="0" width="1" height="1"/>
	<g i:extraneous="self"><rect width="64" height="48" fill="red"/></g>
</switch>
</svg>"#;

    #[test]
    fn text_entities_still_pass() {
        assert_eq!(nesting_depth(ILLUSTRATOR).unwrap(), 4);
        // The file really is one the SVG parser reads.
        let tree =
            resvg::usvg::Tree::from_str(ILLUSTRATOR, &resvg::usvg::Options::default()).unwrap();
        assert_eq!(tree.size().width(), 64.0);
        // A character reference in an entity is text, not markup.
        let xml = r#"<!DOCTYPE svg [<!ENTITY e "&#60;g&#62;">]><svg>&e;</svg>"#;
        assert_eq!(nesting_depth(xml).unwrap(), 1);
        // An external entity has no value to look at.
        let xml =
            r#"<!DOCTYPE svg [<!ENTITY e SYSTEM "e.xml"><!ENTITY % p PUBLIC "a" "b">]><svg/>"#;
        assert_eq!(nesting_depth(xml).unwrap(), 1);
    }

    #[test]
    fn an_entity_holding_deep_markup_is_refused() {
        // The entity expands to 200,000 nested groups, which the reader would
        // follow with a call for each. On a stack of 256 KiB.
        let deep = "<g>".repeat(200_000) + &"</g>".repeat(200_000);
        let xml = format!(r#"<!DOCTYPE svg [<!ENTITY deep "{deep}">]><svg>&deep;</svg>"#);
        let err = on_a_small_stack(move || nesting_depth(&xml)).unwrap_err();
        assert!(matches!(err, Error::Unsafe(_)), "{err:?}");
        assert_eq!(err.to_string(), TOO_DEEP);
    }

    #[test]
    fn an_entity_holding_any_markup_is_refused() {
        // An open tag in an entity, drawn again and again, nests as deep as the
        // references are many, and none of them is in the text of the file.
        let xml = format!(
            r#"<!DOCTYPE svg [<!ENTITY e '<g>'>]><svg>{}{}</svg>"#,
            "&e;".repeat(1000),
            "</g>".repeat(1000)
        );
        assert_eq!(nesting_depth(&xml).unwrap_err().to_string(), TOO_DEEP);
        for value in ["<g/>", "x<!-- y -->", "<![CDATA[z]]>", "<?pi?>"] {
            for quote in ['"', '\''] {
                let xml = format!("<!DOCTYPE svg [ <!ENTITY % e {quote}{value}{quote}> ]><svg/>",);
                assert!(nesting_depth(&xml).is_err(), "{xml}");
            }
        }
    }

    #[test]
    fn a_processing_instruction_in_the_doctype_does_not_hide_the_document() {
        // The quote in the instruction is not the start of a string, and the
        // 100,000 groups after it are not skipped with it.
        let deep = nested(100_000, "");
        for subset in [
            "<?pi it's ?>",
            "<!-- it's -->",
            "<?pi \"?>",
            "<!ELEMENT a (b)> <!ATTLIST a b CDATA 'x'>",
            "<!ATTLIST a b CDATA \"it's\">",
        ] {
            let xml = format!("<!DOCTYPE svg [{subset}]><svg>{deep}</svg>");
            let err = on_a_small_stack(move || nesting_depth(&xml)).unwrap_err();
            assert_eq!(err.to_string(), TOO_DEEP, "{subset}");
        }
        // And a shallow document with the same prolog is still counted.
        let xml = "<?xml version=\"1.0\"?><!-- a --><?pi b?>\n<!DOCTYPE svg [<?pi it's ?>]><svg><g/></svg><!-- z -->";
        assert_eq!(nesting_depth(xml).unwrap(), 2);
    }

    /// A drawing of `links` links, each link written by `link(n)` as an
    /// element inside `defs` that refers on to link n + 1, then one element
    /// that uses link 0. The file is shallow and the drawing is not.
    fn chain(links: usize, link: impl Fn(usize) -> String, user: &str) -> String {
        let mut xml = String::from(
            r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><defs>"##,
        );
        for n in 0..links {
            xml += &link(n);
        }
        xml += "</defs>";
        xml += user;
        xml += "</svg>";
        xml
    }

    #[test]
    fn a_chain_of_references_is_followed_without_recursion() {
        type Link = Box<dyn Fn(usize) -> String>;
        let cases: Vec<(&str, Link, &str)> = vec![
            (
                "masks",
                Box::new(|n| {
                    format!(
                        r##"<mask id="m{n}"><rect mask="url(#m{})"/></mask>"##,
                        n + 1
                    )
                }),
                r##"<rect mask="url(#m0)"/>"##,
            ),
            (
                "clip paths",
                Box::new(|n| {
                    format!(
                        r##"<clipPath id="m{n}"><rect clip-path="url(#m{})"/></clipPath>"##,
                        n + 1
                    )
                }),
                r##"<rect clip-path="url(#m0)"/>"##,
            ),
            (
                "references in style",
                Box::new(|n| {
                    format!(
                        r##"<mask id="m{n}"><rect style="fill:red; mask : url( '#m{}' )"/></mask>"##,
                        n + 1
                    )
                }),
                r##"<rect style="stroke:blue;mask:url(#m0)"/>"##,
            ),
            (
                "pattern fills",
                Box::new(|n| {
                    format!(
                        r##"<pattern id="m{n}"><rect fill="url(#m{})"/></pattern>"##,
                        n + 1
                    )
                }),
                r##"<rect fill="url(#m0)"/>"##,
            ),
            (
                "pattern strokes",
                Box::new(|n| {
                    format!(
                        r##"<pattern id="m{n}"><rect stroke="url(#m{})"/></pattern>"##,
                        n + 1
                    )
                }),
                r##"<rect stroke="url(#m0)"/>"##,
            ),
            (
                "gradient links",
                Box::new(|n| format!(r##"<linearGradient id="m{n}" xlink:href="#m{}"/>"##, n + 1)),
                r##"<rect fill="url(#m0)"/>"##,
            ),
            (
                "radial gradient links",
                Box::new(|n| format!(r##"<radialGradient id="m{n}" href="#m{}"/>"##, n + 1)),
                r##"<rect fill="url(#m0)"/>"##,
            ),
            (
                "pattern links",
                Box::new(|n| format!(r##"<pattern id="m{n}" xlink:href="#m{}"/>"##, n + 1)),
                r##"<rect fill="url(#m0)"/>"##,
            ),
            (
                "markers",
                Box::new(|n| {
                    format!(
                        r##"<marker id="m{n}"><path marker-end="url(#m{})"/></marker>"##,
                        n + 1
                    )
                }),
                r##"<path marker-start="url(#m0)"/>"##,
            ),
            (
                "filters",
                Box::new(|n| {
                    format!(
                        r##"<filter id="m{n}"><feImage xlink:href="#f{n}"/></filter><g id="f{n}" filter="url(#m{})"/>"##,
                        n + 1
                    )
                }),
                r##"<rect filter="url(#m0)"/>"##,
            ),
            (
                "text paths",
                Box::new(|n| format!(r##"<textPath id="m{n}" xlink:href="#m{}"/>"##, n + 1)),
                r##"<text><textPath href="#m0"/></text>"##,
            ),
        ];
        for (name, link, user) in cases {
            // 20,000 links, on a stack of 256 KiB.
            let xml = chain(20_000, link, user);
            let err = on_a_small_stack(move || nesting_depth(&xml)).unwrap_err();
            assert!(matches!(err, Error::Unsafe(_)), "{name}: {err:?}");
            assert_eq!(err.to_string(), TOO_DEEP, "{name}");
        }
    }

    #[test]
    fn a_short_chain_of_references_is_counted() {
        // rect -> mask -> rect -> mask -> rect: the svg, defs and the masks'
        // rects are siblings, so the deepest is the user, 1 + (mask 2 + mask 2) + ...
        let xml = r##"<svg><defs><mask id="a"><rect mask="url(#b)"/></mask><mask id="b"><rect/></mask></defs><rect mask="url(#a)"/></svg>"##;
        // b: 2, a: 1 + (rect: 1 + b: 2) = 4, the user rect: 1 + a = 5, svg: 6.
        assert_eq!(nesting_depth(xml).unwrap(), 6);
        // A gradient and a clip path, as an ordinary drawing has them.
        let xml = r##"<svg><defs><linearGradient id="g"><stop/></linearGradient><clipPath id="c"><circle/></clipPath></defs><rect fill="url( &quot;#g&quot; )" clip-path="url(#c)"/></svg>"##;
        assert_eq!(nesting_depth(xml).unwrap(), 4);
    }

    #[test]
    fn a_reference_that_comes_back_is_skipped() {
        for xml in [
            r##"<svg><mask id="m"><rect mask="url(#m)"/></mask><rect mask="url(#m)"/></svg>"##,
            r##"<svg><clipPath id="a"><rect clip-path="url(#b)"/></clipPath><clipPath id="b"><rect clip-path="url(#a)"/></clipPath><g clip-path="url(#a)"/></svg>"##,
            r##"<svg><linearGradient id="a" href="#b"/><linearGradient id="b" href="#a"/><rect fill="url(#a)"/></svg>"##,
            r##"<svg><pattern id="p"><rect fill="url(#p)"/></pattern><rect fill="url(#p)"/></svg>"##,
        ] {
            let depth = nesting_depth(xml).unwrap_or_else(|e| panic!("{xml}: {e}"));
            assert!(depth <= 8, "{xml}: {depth}");
        }
        // A `use` that comes back is still refused.
        assert!(nesting_depth(r##"<svg><g id="a"><use href="#a"/></g></svg>"##).is_err());
    }

    #[test]
    fn only_references_that_draw_something_are_followed() {
        // A link to the element it is inside is a link, not a drawing.
        let xml = r##"<svg><g id="a"><a href="#a"><rect/></a><image href="#a"/></g></svg>"##;
        assert_eq!(nesting_depth(xml).unwrap(), 4);
        // A reference to nothing, or to something with no id.
        let xml = r##"<svg><rect fill="url(#nobody)" mask="none" stroke="url()"/></svg>"##;
        assert_eq!(nesting_depth(xml).unwrap(), 2);
    }

    #[test]
    fn filter_cost_weighs_turbulence_by_its_octaves() {
        assert_eq!(
            cost(
                &filtered(r#"<feTurbulence baseFrequency="0.05" numOctaves="4"/>"#),
                1.0
            ),
            40_000
        );
        assert_eq!(
            cost(
                &filtered(r#"<feTurbulence baseFrequency="0.05" numOctaves="100000"/>"#),
                1.0
            ),
            1_000_000_000
        );
        // At least one octave is worked.
        assert_eq!(
            cost(
                &filtered(r#"<feTurbulence baseFrequency="0.05" numOctaves="0"/>"#),
                1.0
            ),
            10_000
        );
    }

    #[test]
    fn filter_cost_weighs_a_convolve_matrix_by_its_size() {
        let nine = "1 ".repeat(9);
        assert_eq!(
            cost(
                &filtered(&format!(
                    r#"<feConvolveMatrix order="3" kernelMatrix="{nine}"/>"#
                )),
                1.0
            ),
            90_000
        );
        let ten = "1 ".repeat(10);
        assert_eq!(
            cost(
                &filtered(&format!(
                    r#"<feConvolveMatrix order="5 2" kernelMatrix="{ten}"/>"#
                )),
                1.0
            ),
            100_000
        );
    }

    #[test]
    fn filter_cost_weighs_a_drop_shadow_at_least_as_much_as_a_blur() {
        let blur = cost(&filtered(r#"<feGaussianBlur stdDeviation="2"/>"#), 1.0);
        let shadow = cost(&filtered(r#"<feDropShadow stdDeviation="2"/>"#), 1.0);
        assert!(shadow >= blur, "{shadow} < {blur}");
        assert_eq!(shadow, 40_000);
    }

    #[test]
    fn filter_cost_weighs_morphology_by_a_thirty_second_of_its_reads() {
        // A radius of 10 reads 21 x 21 pixels for every pixel.
        assert_eq!(
            cost(&filtered(r#"<feMorphology radius="10"/>"#), 1.0),
            137_812
        );
        // The weight of a small radius is never under that of a flood.
        assert_eq!(
            cost(&filtered(r#"<feMorphology radius="1"/>"#), 1.0),
            10_000
        );
    }

    #[test]
    fn a_wide_morphology_on_a_large_drawing_is_allowed() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="2000" height="2000"><filter id="f"><feMorphology radius="10"/></filter><rect width="2000" height="2000" filter="url(#f)"/></svg>"##;
        let tree = resvg::usvg::Tree::from_str(svg, &resvg::usvg::Options::default()).unwrap();
        // 2,400 pixels square, 21 x 21 reads a pixel, a thirty-second as much.
        let cost = filter_cost(&tree, 1.0);
        assert_eq!(cost, 79_380_000);
        assert!(cost * 10 <= MAX_FILTER_COST, "{cost}");
    }

    // ----- Fix round 2 -----

    /// How deep the reader (roxmltree, as usvg runs it) finds `xml`, once its
    /// entities are expanded; `None` when it refuses the file. Only for files
    /// shallow enough for its recursion.
    fn reader_depth(xml: &str) -> Option<usize> {
        let options = roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        };
        let doc = roxmltree::Document::parse_with_options(xml, options).ok()?;
        doc.descendants()
            .filter(|n| n.is_element())
            .map(|n| n.ancestors().filter(|a| a.is_element()).count())
            .max()
    }

    /// Panics unless the scan took in everything the reader reads of `xml`
    /// when the reader accepts it: it counts every level the reader goes down,
    /// or says an entity holds markup. Gives whether the reader accepted it.
    fn assert_counted_if_accepted(xml: &str) -> bool {
        let Some(read) = reader_depth(xml) else {
            return false;
        };
        let scan = scan_tags(xml);
        assert!(
            scan.markup_in_entity || scan.deepest >= read,
            "the reader reads {read} deep and the scan counts {}: {xml:?}",
            scan.deepest
        );
        true
    }

    #[test]
    fn a_declaration_that_holds_the_end_of_itself_does_not_hide_the_document() {
        // `?>` inside the quotes of an encoding is not the end of the
        // declaration. 1,000,000 nested groups follow it, on a stack of 256 KiB.
        let deep = nested(1_000_000, "");
        for declaration in [
            r#"<?xml version="1.0" encoding="?>"?>"#,
            r#"<?xml version='1.0' encoding='?>'?>"#,
            r#"<?xml version="1.0" standalone="?>"?>"#,
            r#"<?xml version="1.0" encoding="it's ?> it's" standalone='no'?>"#,
        ] {
            let xml = format!("{declaration}<!DOCTYPE svg><svg>{deep}</svg>");
            let err = on_a_small_stack(move || nesting_depth(&xml)).unwrap_err();
            assert!(matches!(err, Error::Unsafe(_)), "{declaration}: {err:?}");
            assert_eq!(err.to_string(), TOO_DEEP, "{declaration}");
        }
    }

    #[test]
    fn a_declaration_that_holds_the_end_of_itself_does_not_hide_an_entity() {
        for declaration in [
            r#"<?xml version="1.0" encoding="?>"?>"#,
            r#"<?xml version='1.0' encoding='?>' standalone="?>"?>"#,
        ] {
            let xml = format!(
                r#"{declaration}<!DOCTYPE svg [<!ENTITY e "<g>">]><svg>&e;&e;</g></g></svg>"#
            );
            let err = on_a_small_stack(move || nesting_depth(&xml)).unwrap_err();
            assert_eq!(err.to_string(), TOO_DEEP, "{declaration}");
        }
        // The declaration read as it is written: the file is counted.
        let xml = r#"<?xml version="1.0" encoding="?>" standalone="no"?><svg><g/></svg>"#;
        assert_eq!(nesting_depth(xml).unwrap(), 2);
    }

    #[test]
    fn a_doctype_that_is_not_one_the_reader_accepts_stops_the_count() {
        // The reader stops at the first of these, before it reads an element.
        for xml in [
            "<!DOCTYPE svg [ junk ]><svg><g/></svg>",
            "<!DOCTYPE svg [ <!ENTITY e 5> ]><svg><g/></svg>",
            "<!DOCTYPE svg [ <!ENTITY e \"x\" ]><svg><g/></svg>",
            "<!DOCTYPE",
            "<!DOCTYPE svg PUBLIC \"a\"",
            "<!DOCTYPE svg [",
            "<!DOCTYPE svg [ <!ENTITY",
            "<svg><!DOCTYPE svg [<!ENTITY e \"<g>\">]></svg>",
            "<?xml version=\"1.0\" encoding=\"?>",
            "<?xml version=\"1.0\"",
            "<?xml encoding=\"x\"?><svg/>",
            "<!DOCTYPE svg><!DOCTYPE svg><svg/>",
        ] {
            assert_eq!(reader_depth(xml), None, "the reader accepts {xml:?}");
            let _ = nesting_depth(xml);
        }
        // Every cut of a real header: whatever the reader accepts is counted.
        let mut accepted = 0;
        for end in 0..=ILLUSTRATOR.len() {
            accepted += usize::from(assert_counted_if_accepted(&ILLUSTRATOR[..end]));
            let _ = nesting_depth(&ILLUSTRATOR[..end]);
        }
        assert_eq!(accepted, 1, "only the whole header is a file");
    }

    #[test]
    fn a_file_the_reader_accepts_is_counted() {
        for xml in [
            "\u{feff}<svg><g/></svg>",
            "\u{feff}<?xml version=\"1.0\"?><svg><g><g/></g></svg>",
            "<?xml version=\"1.0\" ?>\n<!-- a --><?pi?> <!DOCTYPE svg >\n<?pi b?><svg><g/></svg>",
            "<!DOCTYPE svg SYSTEM \"a[b>c.dtd\" [ <!ENTITY a \"x>y\"> ]><svg><g/></svg>",
            "<!DOCTYPE svg PUBLIC \"a'b\" 'c\"d' []><svg><g><g/></g></svg>",
            "<!DOCTYPE svg [<!ATTLIST a b CDATA 'x'>\n<!NOTATION n SYSTEM 'x'>]><svg/>",
            "<!DOCTYPE svg [<!ENTITY n SYSTEM \"f\" NDATA gif>]><svg/>",
            "<svg><!--<g><g>--><![CDATA[<g>]]><?pi '?><g a='\"' b=\"'\"/></svg>",
        ] {
            assert!(
                assert_counted_if_accepted(xml),
                "the reader refuses {xml:?}"
            );
            assert!(nesting_depth(xml).unwrap() >= 1, "{xml:?}");
        }
        assert_eq!(nesting_depth("\u{feff}<svg><g/></svg>").unwrap(), 2);
    }

    /// A small deterministic random source.
    struct Dice(u64);

    impl Dice {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % n as u64) as usize
        }

        fn pick(&mut self, items: &[&'static str]) -> &'static str {
            items[self.below(items.len())]
        }
    }

    /// Some content for inside an element, `depth` levels from running out.
    fn random_content(dice: &mut Dice, depth: usize, out: &mut String) {
        for _ in 0..dice.below(3) {
            match dice.below(if depth == 0 { 6 } else { 9 }) {
                0 => out.push_str("<!-- <g><g> it's -->"),
                1 => out.push_str("<![CDATA[<g><g> \"]]>"),
                2 => out.push_str("<?pi '\" <g> ?>"),
                3 => out.push_str(dice.pick(&["text &amp; &#60; text", "&a;", "&b;", "&e;"])),
                4 => out.push_str(dice.pick(&["<g a=\">\" b='x\"y'/>", "<g />", "<g\n/>"])),
                5 => out.push_str("<g a='/' b=\"x'/>\" ></g>"),
                _ => {
                    let tag = dice.pick(&["g", "svg:g", "g  ", "rect"]);
                    let name = tag.trim_end();
                    out.push_str(&format!("<{tag}>"));
                    random_content(dice, depth - 1, out);
                    out.push_str(&format!("</{name}>"));
                }
            }
        }
    }

    /// A file that may or may not be one the reader accepts, built from the
    /// parts of a prolog that are easy to get wrong.
    fn random_file(dice: &mut Dice) -> String {
        let mut xml = String::new();
        if dice.below(8) == 0 {
            xml.push('\u{feff}');
        }
        xml.push_str(dice.pick(&[
            "",
            "",
            "<?xml version=\"1.0\"?>",
            "<?xml version='1.0' encoding='UTF-8'?>",
            "<?xml version=\"1.0\" encoding=\"?>\"?>",
            "<?xml version=\"1.0\" encoding='a?>b' standalone=\"no\"?>",
            "<?xml version=\"1.0\" encoding=\"it's\"?>",
            "<?xml version=\"1.0\" standalone='\"'?>",
            "<?xml version=\"1.0\" encoding=\"?>\" standalone=\"?>\"?>",
            "<?xml   version = \"1.0\"  ?>",
            "<?xml version=\"1.0\" encoding=\"?>",
            "<?xml version=\"1.0\"?",
            "<?xml encoding=\"x\"?>",
        ]));
        let misc = [
            "",
            "",
            " ",
            "\n",
            "<!-- c -->",
            "<!-- it's \"q\" -->",
            "<?pi?>",
            "<?pi it's ?>",
            "<?pi \"?>",
            "<!--  <g><g>  -->",
            "<?pi <g> ?>",
        ];
        for _ in 0..dice.below(3) {
            xml.push_str(dice.pick(&misc));
        }
        match dice.below(4) {
            0 => {}
            1 => xml.push_str(dice.pick(&[
                "<!DOCTYPE svg>",
                "<!DOCTYPE svg >",
                "<!DOCTYPE  svg SYSTEM \"a.dtd\">",
                "<!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"http://x/y.dtd\">",
                "<!DOCTYPE svg SYSTEM \"a.dtd\" []>",
                "<!DOCTYPE svg SYSTEM \"a[b>c.dtd\">",
                "<!DOCTYPE>",
                "<!DOCTYPE svg PUBLIC \"a\">",
                "<!DOCTYPE svg [ ]>",
            ])),
            _ => {
                let declarations = [
                    "<!ENTITY a \"text\">",
                    "<!ENTITY b 'te\"xt'>",
                    "<!ENTITY  c   \"x\"  >",
                    "<!ENTITY % p \"x\">",
                    "<!ENTITY s SYSTEM \"f.xml\">",
                    "<!ENTITY q PUBLIC \"a\" \"b\">",
                    "<!ENTITY n SYSTEM \"f\" NDATA gif>",
                    "<!ENTITY m \"<g><g/></g>\">",
                    "<!ENTITY e '<g>'>",
                    "<!ENTITY t \"a>b\">",
                    "<!ELEMENT a (b)>",
                    "<!ATTLIST a b CDATA \"x\">",
                    "<!ATTLIST a b CDATA \">\">",
                    "<!NOTATION n SYSTEM \"x\">",
                    "<!-- it's -->",
                    "<?pi it's ?>",
                    "<?pi \"?>",
                    " ",
                    "\n",
                    "<!ENTITY",
                    "junk",
                ];
                xml.push_str(dice.pick(&["<!DOCTYPE svg [", "<!DOCTYPE svg SYSTEM \"a.dtd\" ["]));
                for _ in 0..dice.below(5) {
                    xml.push_str(dice.pick(&declarations));
                }
                xml.push_str(dice.pick(&["]>", "]>", "] >", "]"]));
            }
        }
        for _ in 0..dice.below(3) {
            xml.push_str(dice.pick(&misc));
        }
        xml.push_str("<svg xmlns:svg=\"http://www.w3.org/2000/svg\">");
        random_content(dice, 5, &mut xml);
        xml.push_str("</svg>");
        for _ in 0..dice.below(2) {
            xml.push_str(dice.pick(&misc));
        }
        xml
    }

    /// Builds `rounds` random files from `seed`, and checks the scan counts
    /// whatever the reader reads of each. Gives how many the reader accepted,
    /// and how many of those declare an entity.
    fn fuzz(seed: u64, rounds: usize) -> (usize, usize) {
        let mut dice = Dice(seed);
        let (mut accepted, mut with_entities) = (0, 0);
        for _ in 0..rounds {
            let xml = random_file(&mut dice);
            if assert_counted_if_accepted(&xml) {
                accepted += 1;
                with_entities += usize::from(xml.contains("<!ENTITY"));
            }
        }
        (accepted, with_entities)
    }

    #[test]
    fn the_scan_counts_whatever_the_reader_reads_of_random_prologs() {
        let (accepted, with_entities) = fuzz(0x2545_F491_4F6C_DD1D, 30_000);
        // The files are not all refused, and the entities are in some of them.
        assert!(accepted > 3_000, "{accepted} accepted");
        assert!(with_entities > 300, "{with_entities} with entities");
    }

    /// The same, for two million files: `cargo test -p kiln-core --lib
    /// fuzz_at_length -- --ignored --nocapture`.
    #[test]
    #[ignore = "about 15 s in a debug build"]
    fn fuzz_at_length() {
        for seed in 1..=8u64 {
            let (accepted, with_entities) = fuzz(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15), 250_000);
            eprintln!("seed {seed}: {accepted} accepted, {with_entities} with entities");
        }
    }
}
