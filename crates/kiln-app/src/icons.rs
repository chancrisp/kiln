//! Kiln's icon set: simple geometric paths on a 24x24 grid, drawn with a
//! 1.5 unit stroke and round caps. The same paths render the UI icons with
//! Direct2D and the app icon (.ico) through examples/gen_icon.rs.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cmd {
    Move(f32, f32),
    Line(f32, f32),
    /// Cubic Bézier: two control points, then the end point.
    Cubic(f32, f32, f32, f32, f32, f32),
    Close,
}

use Cmd::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// Outline icons for the UI.
    Stroke,
    /// Solid shapes, such as the app icon's vessel.
    Fill,
}

#[derive(Debug, Clone, Copy)]
pub struct Icon {
    pub name: &'static str,
    pub style: Style,
    pub path: &'static [Cmd],
}

/// A rounded vessel: lip, narrow neck, full belly, flat foot.
pub const VESSEL_PATH: &[Cmd] = &[
    Move(9.0, 3.25),
    Line(15.0, 3.25),
    Cubic(15.4, 3.25, 15.6, 3.9, 15.25, 4.25),
    Line(14.25, 5.25),
    Cubic(14.25, 7.5, 20.0, 9.0, 20.0, 13.75),
    Cubic(20.0, 17.75, 17.5, 20.75, 15.0, 20.75),
    Line(9.0, 20.75),
    Cubic(6.5, 20.75, 4.0, 17.75, 4.0, 13.75),
    Cubic(4.0, 9.0, 9.75, 7.5, 9.75, 5.25),
    Line(8.75, 4.25),
    Cubic(8.4, 3.9, 8.6, 3.25, 9.0, 3.25),
    Close,
];

/// A single flame line: a teardrop with a notch where the flame splits.
pub const FLAME_PATH: &[Cmd] = &[
    Move(12.0, 21.0),
    Cubic(8.5, 21.0, 6.0, 18.5, 6.0, 15.25),
    Cubic(6.0, 11.75, 9.0, 9.5, 10.0, 6.5),
    Cubic(11.5, 8.0, 12.25, 10.0, 12.0, 12.25),
    Cubic(13.25, 11.0, 13.75, 9.75, 14.0, 8.25),
    Cubic(16.5, 10.25, 18.0, 12.75, 18.0, 15.25),
    Cubic(18.0, 18.5, 15.5, 21.0, 12.0, 21.0),
    Close,
];

/// An isometric cube.
pub const CUBE_PATH: &[Cmd] = &[
    Move(12.0, 3.0),
    Line(19.75, 7.5),
    Line(19.75, 16.5),
    Line(12.0, 21.0),
    Line(4.25, 16.5),
    Line(4.25, 7.5),
    Close,
    Move(4.25, 7.5),
    Line(12.0, 12.0),
    Line(19.75, 7.5),
    Move(12.0, 12.0),
    Line(12.0, 21.0),
];

/// Remove / close.
pub const CROSS_PATH: &[Cmd] = &[
    Move(7.0, 7.0),
    Line(17.0, 17.0),
    Move(17.0, 7.0),
    Line(7.0, 17.0),
];

pub const VESSEL: Icon = Icon {
    name: "vessel",
    style: Style::Stroke,
    path: VESSEL_PATH,
};
pub const VESSEL_SOLID: Icon = Icon {
    name: "vessel-solid",
    style: Style::Fill,
    path: VESSEL_PATH,
};
pub const FLAME: Icon = Icon {
    name: "flame",
    style: Style::Stroke,
    path: FLAME_PATH,
};
pub const CUBE: Icon = Icon {
    name: "cube",
    style: Style::Stroke,
    path: CUBE_PATH,
};
pub const CROSS: Icon = Icon {
    name: "cross",
    style: Style::Stroke,
    path: CROSS_PATH,
};

pub const ALL: [Icon; 5] = [VESSEL, VESSEL_SOLID, FLAME, CUBE, CROSS];

/// The app icon's geometry at a given pixel size: the tile's corner radius
/// and the scale and offset that centre the vessel on it.
pub struct AppIconLayout {
    pub tile_radius: f32,
    pub scale: f32,
    pub offset: (f32, f32),
}

pub fn app_icon_layout(px: f32) -> AppIconLayout {
    // The vessel spans 4..20 by 3.25..20.75 on the grid. It fills about
    // 70 percent of the tile, so it reads at 16 px and doesn't crowd 256.
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
    fn icons_stay_inside_the_grid_with_room_for_the_stroke() {
        let margin = crate::theme::shape::ICON_STROKE / 2.0;
        for icon in ALL {
            for (x, y) in points(icon.path) {
                assert!(
                    (margin..=24.0 - margin).contains(&x) && (margin..=24.0 - margin).contains(&y),
                    "{} has a point at ({x}, {y})",
                    icon.name
                );
            }
        }
    }

    #[test]
    fn every_path_starts_with_a_move() {
        for icon in ALL {
            assert!(matches!(icon.path[0], Move(..)), "{}", icon.name);
        }
    }

    #[test]
    fn filled_shapes_are_closed() {
        for icon in ALL.iter().filter(|i| i.style == Style::Fill) {
            assert_eq!(icon.path.last(), Some(&Close), "{}", icon.name);
        }
    }

    #[test]
    fn the_vessel_is_symmetric() {
        let pts = points(VESSEL_PATH);
        for (x, y) in &pts {
            let mirror = 24.0 - x;
            assert!(
                pts.iter()
                    .any(|(mx, my)| (mx - mirror).abs() < 1e-3 && (my - y).abs() < 1e-3),
                "no mirror for ({x}, {y})"
            );
        }
    }

    #[test]
    fn app_icon_vessel_fits_the_tile() {
        for px in [16.0, 32.0, 48.0, 256.0] {
            let l = app_icon_layout(px);
            for (x, y) in points(VESSEL_PATH) {
                let (sx, sy) = (x * l.scale + l.offset.0, y * l.scale + l.offset.1);
                assert!(
                    sx > 0.0 && sx < px && sy > 0.0 && sy < px,
                    "{px}: ({sx}, {sy})"
                );
            }
        }
    }
}
