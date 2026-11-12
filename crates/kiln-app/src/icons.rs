//! Kiln's mark, the K on a 24x24 grid, and what the app icon generator
//! (examples/gen_icon.rs) makes from it. The window draws the mark with
//! Direct2D from the same path.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cmd {
    Move(f32, f32),
    Line(f32, f32),
    /// Cubic Bézier: two control points, then the end point.
    Cubic(f32, f32, f32, f32, f32, f32),
    Close,
}

use Cmd::*;

/// Kiln's mark: a K whose upper arm rises into one lick of fire. The
/// stem, then the arm that sweeps up and leans back over the stem, then
/// the straight leg. It spans 4.71..19.29 by 3.25..20.75, the vessel's
/// height, centred on (12, 12). The app icon presses this same shape into
/// a stoneware slab (assets/kiln-seal.svg); in the window it is flat.
pub const MARK_PATH: &[Cmd] = &[
    Move(4.71, 20.75),
    Line(4.71, 7.62),
    Cubic(4.71, 6.38, 5.33, 5.75, 6.58, 5.75),
    Cubic(7.83, 5.75, 8.46, 6.38, 8.46, 7.62),
    Line(8.46, 11.58),
    Cubic(9.71, 11.58, 10.96, 10.75, 11.79, 9.5),
    Cubic(13.04, 7.83, 13.46, 5.33, 14.29, 3.25),
    Cubic(19.29, 7.0, 16.38, 12.0, 11.38, 14.08),
    Line(16.79, 20.75),
    Line(13.04, 20.75),
    Line(9.71, 16.58),
    Line(8.46, 17.83),
    Line(8.46, 20.75),
    Close,
];

/// `svg` without every span from `<!-- name -->` to `<!-- /name -->`, markers
/// included. The app icon's master marks its fine texture and its hand-cut
/// displacement this way, so gen_icon can render a 48 px icon that keeps the
/// relief and loses the detail that would turn to noise. A marker without
/// its partner leaves the text alone from there on.
pub fn without_blocks(svg: &str, name: &str) -> String {
    let open = format!("<!-- {name} -->");
    let close = format!("<!-- /{name} -->");
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    while let Some(start) = rest.find(&open) {
        let Some(end) = rest[start..].find(&close) else {
            break;
        };
        out.push_str(&rest[..start]);
        rest = &rest[start + end + close.len()..];
    }
    out.push_str(rest);
    out
}

/// The app icon's geometry at a given pixel size: the tile's corner radius
/// and the scale and offset that centre the mark on it.
pub struct AppIconLayout {
    pub tile_radius: f32,
    pub scale: f32,
    pub offset: (f32, f32),
}

pub fn app_icon_layout(px: f32) -> AppIconLayout {
    // The mark spans 4.71..19.29 by 3.25..20.75 on the grid, 17.5 units
    // tall like the vessel before it. It fills about 70 percent of the
    // tile, so it reads at 16 px and doesn't crowd 32. Only those two
    // flat sizes come through here; 48 and 256 are the seal from the SVG.
    let scale = px * 0.70 / 17.5;
    let cx = 12.0 * scale;
    let cy = 12.0 * scale;
    AppIconLayout {
        tile_radius: px * 0.22,
        scale,
        offset: (px / 2.0 - cx, px / 2.0 - cy),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn points(path: &[Cmd]) -> Vec<(f32, f32)> {
        path.iter()
            .flat_map(|c| match *c {
                Move(x, y) | Line(x, y) => vec![(x, y)],
                Cubic(a, b, c, d, x, y) => vec![(a, b), (c, d), (x, y)],
                Close => vec![],
            })
            .collect()
    }

    #[test]
    fn the_mark_stays_inside_the_grid_with_room_for_its_line() {
        let margin = crate::theme::fluent::space::ICON_STROKE / 2.0;
        for (x, y) in points(MARK_PATH) {
            assert!(
                (margin..=24.0 - margin).contains(&x) && (margin..=24.0 - margin).contains(&y),
                "the mark has a point at ({x}, {y})"
            );
        }
    }

    #[test]
    fn the_mark_starts_with_a_move_and_is_closed() {
        assert!(matches!(MARK_PATH[0], Move(..)));
        assert_eq!(MARK_PATH.last(), Some(&Close));
    }

    #[test]
    fn without_blocks_cuts_every_marked_span_and_nothing_else() {
        let svg = "<a/>\n<!-- detail -->\n<b/>\n<!-- /detail -->\n<c>\n<!-- detail --><d/><!-- /detail -->\n</c>\n<!-- rough -->\n<e>\n<!-- /rough -->\n";
        assert_eq!(
            without_blocks(svg, "detail"),
            "<a/>\n\n<c>\n\n</c>\n<!-- rough -->\n<e>\n<!-- /rough -->\n"
        );
        assert_eq!(without_blocks(svg, "rough"), "<a/>\n<!-- detail -->\n<b/>\n<!-- /detail -->\n<c>\n<!-- detail --><d/><!-- /detail -->\n</c>\n\n");
        assert_eq!(without_blocks("<a/>", "detail"), "<a/>");
    }

    #[test]
    fn the_mark_stands_on_the_baseline() {
        // The stem's outer edge and the leg's foot pin the K to its grid:
        // 17.5 units tall, centred on (12, 12). A stray edit to the
        // numbers moves one of them.
        let pts = points(MARK_PATH);
        let on = |x: f32, y: f32| {
            pts.iter()
                .any(|(px, py)| (px - x).abs() < 1e-3 && (py - y).abs() < 1e-3)
        };
        assert!(on(4.71, 20.75), "stem foot");
        assert!(on(4.71, 7.62), "stem top");
        assert!(on(16.79, 20.75), "leg foot");
        assert!(on(14.29, 3.25), "flame tip");
    }

    #[test]
    fn app_icon_mark_fits_the_tile() {
        for px in [16.0, 32.0, 48.0, 256.0] {
            let l = app_icon_layout(px);
            for (x, y) in points(MARK_PATH) {
                let (sx, sy) = (x * l.scale + l.offset.0, y * l.scale + l.offset.1);
                assert!(
                    sx > 0.0 && sx < px && sy > 0.0 && sy < px,
                    "{px}: ({sx}, {sy})"
                );
            }
        }
    }
}
