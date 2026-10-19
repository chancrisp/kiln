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

/// A picture: frame, hills and a sun.
pub const IMAGE_PATH: &[Cmd] = &[
    Move(4.5, 5.0),
    Line(19.5, 5.0),
    Line(19.5, 19.0),
    Line(4.5, 19.0),
    Close,
    Move(6.5, 17.0),
    Line(10.5, 11.5),
    Line(13.5, 14.75),
    Line(15.5, 12.5),
    Line(17.5, 17.0),
    // The sun: an octagon. At this size it reads as a circle, and short
    // straight segments stroke cleanly on every Direct2D implementation.
    Move(17.0, 8.75),
    Line(16.56, 9.81),
    Line(15.5, 10.25),
    Line(14.44, 9.81),
    Line(14.0, 8.75),
    Line(14.44, 7.69),
    Line(15.5, 7.25),
    Line(16.56, 7.69),
    Close,
];

/// Settings: a kiln's temperature dial with its needle.
pub const DIAL_PATH: &[Cmd] = &[
    Move(12.0, 4.75),
    Cubic(16.0, 4.75, 19.25, 8.0, 19.25, 12.0),
    Cubic(19.25, 16.0, 16.0, 19.25, 12.0, 19.25),
    Cubic(8.0, 19.25, 4.75, 16.0, 4.75, 12.0),
    Cubic(4.75, 8.0, 8.0, 4.75, 12.0, 4.75),
    Close,
    Move(12.0, 12.0),
    Line(15.25, 8.75),
    Move(7.5, 16.5),
    Line(8.5, 15.5),
    Move(16.5, 16.5),
    Line(15.5, 15.5),
];

/// Copy path: a sheet in front of another.
pub const COPY_PATH: &[Cmd] = &[
    Move(9.0, 9.0),
    Line(19.0, 9.0),
    Line(19.0, 19.0),
    Line(9.0, 19.0),
    Close,
    Move(5.0, 15.0),
    Line(5.0, 5.0),
    Line(15.0, 5.0),
];

/// Move: an arrow going into a folder.
pub const MOVE_PATH: &[Cmd] = &[
    Move(3.5, 6.0),
    Line(9.5, 6.0),
    Line(11.5, 8.5),
    Line(20.5, 8.5),
    Line(20.5, 19.0),
    Line(3.5, 19.0),
    Close,
    Move(8.0, 13.75),
    Line(15.0, 13.75),
    Move(12.5, 11.25),
    Line(15.0, 13.75),
    Line(12.5, 16.25),
];

/// Open in Terminal: a window with a prompt.
pub const TERMINAL_PATH: &[Cmd] = &[
    Move(3.5, 5.0),
    Line(20.5, 5.0),
    Line(20.5, 19.0),
    Line(3.5, 19.0),
    Close,
    Move(7.0, 9.5),
    Line(10.0, 12.0),
    Line(7.0, 14.5),
    Move(12.0, 15.0),
    Line(16.5, 15.0),
];

/// Sound: five bars of a level meter.
pub const AUDIO_PATH: &[Cmd] = &[
    Move(6.0, 10.0),
    Line(6.0, 14.0),
    Move(9.0, 7.0),
    Line(9.0, 17.0),
    Move(12.0, 4.5),
    Line(12.0, 19.5),
    Move(15.0, 8.0),
    Line(15.0, 16.0),
    Move(18.0, 10.5),
    Line(18.0, 13.5),
];

/// Moving picture: a camera body and lens.
pub const VIDEO_PATH: &[Cmd] = &[
    Move(3.5, 6.5),
    Line(15.5, 6.5),
    Line(15.5, 17.5),
    Line(3.5, 17.5),
    Close,
    Move(15.5, 10.5),
    Line(20.5, 7.5),
    Line(20.5, 16.5),
    Line(15.5, 13.5),
];

/// A page with a folded corner.
pub const DOCUMENT_PATH: &[Cmd] = &[
    Move(6.5, 3.5),
    Line(14.0, 3.5),
    Line(18.0, 7.5),
    Line(18.0, 20.5),
    Line(6.5, 20.5),
    Close,
    Move(14.0, 3.5),
    Line(14.0, 7.5),
    Line(18.0, 7.5),
    Move(9.5, 12.0),
    Line(15.0, 12.0),
    Move(9.5, 15.5),
    Line(15.0, 15.5),
];

/// A spreadsheet: a table with a header row and a label column.
pub const SHEET_PATH: &[Cmd] = &[
    Move(3.5, 4.5),
    Line(20.5, 4.5),
    Line(20.5, 19.5),
    Line(3.5, 19.5),
    Close,
    Move(3.5, 9.5),
    Line(20.5, 9.5),
    Move(3.5, 14.5),
    Line(20.5, 14.5),
    Move(9.5, 4.5),
    Line(9.5, 19.5),
];

/// Slides: a screen with two lines of text, on a stand.
pub const SLIDES_PATH: &[Cmd] = &[
    Move(3.5, 4.0),
    Line(20.5, 4.0),
    Line(20.5, 15.0),
    Line(3.5, 15.0),
    Close,
    Move(7.0, 8.0),
    Line(13.0, 8.0),
    Move(7.0, 11.0),
    Line(17.0, 11.0),
    Move(12.0, 15.0),
    Line(12.0, 17.5),
    Move(8.5, 20.5),
    Line(12.0, 17.5),
    Line(15.5, 20.5),
];

/// Compress: two arrows meeting at a line.
pub const COMPRESS_PATH: &[Cmd] = &[
    Move(12.0, 3.5),
    Line(12.0, 9.5),
    Move(9.0, 6.5),
    Line(12.0, 9.5),
    Line(15.0, 6.5),
    Move(12.0, 20.5),
    Line(12.0, 14.5),
    Move(9.0, 17.5),
    Line(12.0, 14.5),
    Line(15.0, 17.5),
    Move(5.0, 12.0),
    Line(19.0, 12.0),
];

/// Crop: two interlocking corners.
pub const CROP_PATH: &[Cmd] = &[
    Move(7.0, 3.5),
    Line(7.0, 17.0),
    Line(20.5, 17.0),
    Move(3.5, 7.0),
    Line(17.0, 7.0),
    Line(17.0, 20.5),
];

/// Trim: brackets closing in on a line.
pub const TRIM_PATH: &[Cmd] = &[
    Move(8.0, 5.0),
    Line(5.0, 5.0),
    Line(5.0, 19.0),
    Line(8.0, 19.0),
    Move(16.0, 5.0),
    Line(19.0, 5.0),
    Line(19.0, 19.0),
    Line(16.0, 19.0),
    Move(9.5, 12.0),
    Line(14.5, 12.0),
];

/// Split: one line becoming two.
pub const SPLIT_PATH: &[Cmd] = &[
    Move(12.0, 3.5),
    Line(12.0, 10.5),
    Line(6.0, 20.5),
    Move(12.0, 10.5),
    Line(18.0, 20.5),
];

/// Merge: two lines becoming one.
pub const MERGE_PATH: &[Cmd] = &[
    Move(6.0, 3.5),
    Line(12.0, 13.5),
    Line(12.0, 20.5),
    Move(18.0, 3.5),
    Line(12.0, 13.5),
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

/// Subtitles: a screen with two lines of captions along the bottom.
pub const SUBTITLE_PATH: &[Cmd] = &[
    Move(3.5, 5.0),
    Line(20.5, 5.0),
    Line(20.5, 19.0),
    Line(3.5, 19.0),
    Close,
    Move(7.0, 12.5),
    Line(12.0, 12.5),
    Move(14.5, 12.5),
    Line(17.0, 12.5),
    Move(7.0, 15.5),
    Line(15.5, 15.5),
];

/// More: three dots in a row, drawn as small octagons so they stroke
/// cleanly.
pub const MORE_PATH: &[Cmd] = &[
    Move(7.0, 12.0),
    Line(6.56, 13.06),
    Line(5.5, 13.5),
    Line(4.44, 13.06),
    Line(4.0, 12.0),
    Line(4.44, 10.94),
    Line(5.5, 10.5),
    Line(6.56, 10.94),
    Close,
    Move(13.5, 12.0),
    Line(13.06, 13.06),
    Line(12.0, 13.5),
    Line(10.94, 13.06),
    Line(10.5, 12.0),
    Line(10.94, 10.94),
    Line(12.0, 10.5),
    Line(13.06, 10.94),
    Close,
    Move(20.0, 12.0),
    Line(19.56, 13.06),
    Line(18.5, 13.5),
    Line(17.44, 13.06),
    Line(17.0, 12.0),
    Line(17.44, 10.94),
    Line(18.5, 10.5),
    Line(19.56, 10.94),
    Close,
];

const fn stroke(name: &'static str, path: &'static [Cmd]) -> Icon {
    Icon {
        name,
        style: Style::Stroke,
        path,
    }
}

pub const ALL: [Icon; 22] = [
    VESSEL,
    VESSEL_SOLID,
    FLAME,
    CUBE,
    CROSS,
    stroke("image", IMAGE_PATH),
    stroke("audio", AUDIO_PATH),
    stroke("video", VIDEO_PATH),
    stroke("document", DOCUMENT_PATH),
    stroke("sheet", SHEET_PATH),
    stroke("slides", SLIDES_PATH),
    stroke("subtitle", SUBTITLE_PATH),
    stroke("compress", COMPRESS_PATH),
    stroke("crop", CROP_PATH),
    stroke("trim", TRIM_PATH),
    stroke("split", SPLIT_PATH),
    stroke("merge", MERGE_PATH),
    stroke("copy", COPY_PATH),
    stroke("move", MOVE_PATH),
    stroke("terminal", TERMINAL_PATH),
    stroke("dial", DIAL_PATH),
    stroke("more", MORE_PATH),
];

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
