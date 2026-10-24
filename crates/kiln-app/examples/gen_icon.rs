//! Renders assets/kiln.ico (16, 32, 48 and 256 px) and a 256 px preview PNG.
//!
//! 256 and 48 px come from assets/kiln-seal.svg through resvg: the flame K
//! pressed into a slab of dark stoneware, celadon glaze pooled in the
//! impression. At 48 the grain, mottling, speckle and hand-cut displacement
//! are cut out of the SVG text first, so the small icon keeps the slab's
//! edge light and the K's inner shadow and nothing that would turn to noise.
//! 32 and 16 px are flat fills of icons::MARK_PATH, pixel-crisp: the glaze
//! gradient at 32, plain celadon at 16, both on an ash tile.
//!
//! cargo run -p kiln-app --example gen_icon

use std::path::PathBuf;

use kiln_app::icons::{app_icon_layout, without_blocks, Cmd, MARK_PATH};
use kiln_app::theme::tokens;
use resvg::tiny_skia::{
    Color, FillRule, GradientStop, LinearGradient, Paint, PathBuilder, Pixmap, Point, SpreadMode,
    Transform,
};
use resvg::usvg;

const MASTER: &str = include_str!("../assets/kiln-seal.svg");

/// From this size up the icon is the seal rendered from the SVG master;
/// below it, a flat fill of the mark.
const SEAL_FROM: u32 = 48;
/// From this size up the seal keeps its grain, mottling, speckle and
/// hand-cut displacement; below it they would turn to noise and go.
const DETAIL_FROM: u32 = 128;

/// The seal from the SVG master. Below `DETAIL_FROM` the detail blocks go.
fn render_seal(px: u32) -> Pixmap {
    let text = if px < DETAIL_FROM {
        without_blocks(&without_blocks(MASTER, "detail"), "rough")
    } else {
        MASTER.to_string()
    };
    let tree = usvg::Tree::from_str(&text, &usvg::Options::default())
        .expect("assets/kiln-seal.svg parses");
    let mut pm = Pixmap::new(px, px).unwrap();
    let s = px as f32 / tree.size().width();
    resvg::render(&tree, Transform::from_scale(s, s), &mut pm.as_mut());
    pm
}

/// The flat mark on an ash tile, from the same path the window draws.
fn render_flat(px: u32) -> Pixmap {
    let size = px as f32;
    let l = app_icon_layout(size);
    let mut pm = Pixmap::new(px, px).unwrap();

    let mut paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };

    // The ash tile.
    let mut tile = PathBuilder::new();
    let r = l.tile_radius;
    let k = 0.5523 * r;
    tile.move_to(r, 0.0);
    tile.line_to(size - r, 0.0);
    tile.cubic_to(size - r + k, 0.0, size, r - k, size, r);
    tile.line_to(size, size - r);
    tile.cubic_to(size, size - r + k, size - r + k, size, size - r, size);
    tile.line_to(r, size);
    tile.cubic_to(r - k, size, 0.0, size - r + k, 0.0, size - r);
    tile.line_to(0.0, r);
    tile.cubic_to(0.0, r - k, r - k, 0.0, r, 0.0);
    tile.close();
    let a = tokens::ASH;
    paint.set_color_rgba8(a.0, a.1, a.2, 255);
    pm.fill_path(
        &tile.finish().unwrap(),
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );

    // The mark.
    let (ox, oy) = l.offset;
    let t = |x: f32, y: f32| (x * l.scale + ox, y * l.scale + oy);
    let mut m = PathBuilder::new();
    for c in MARK_PATH {
        match *c {
            Cmd::Move(x, y) => {
                let (x, y) = t(x, y);
                m.move_to(x, y)
            }
            Cmd::Line(x, y) => {
                let (x, y) = t(x, y);
                m.line_to(x, y)
            }
            Cmd::Cubic(a, b, c, d, x, y) => {
                let (a, b) = t(a, b);
                let (c, d) = t(c, d);
                let (x, y) = t(x, y);
                m.cubic_to(a, b, c, d, x, y)
            }
            Cmd::Close => m.close(),
        }
    }
    if px >= 32 {
        // The glaze: palest at the flame tip, deepest at the leg, running
        // from the top right of the mark to its bottom left as the seal's
        // gradient does.
        let (x0, y0) = t(13.46, 3.25);
        let (x1, y1) = t(9.08, 20.75);
        paint.shader = LinearGradient::new(
            Point::from_xy(x0, y0),
            Point::from_xy(x1, y1),
            vec![
                GradientStop::new(0.0, Color::from_rgba8(0xA6, 0xD3, 0xC0, 255)),
                GradientStop::new(0.45, Color::from_rgba8(0x7F, 0xB5, 0xA0, 255)),
                GradientStop::new(1.0, Color::from_rgba8(0x4F, 0x82, 0x6E, 255)),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        )
        .unwrap();
    } else {
        let c = tokens::CELADON;
        paint.set_color_rgba8(c.0, c.1, c.2, 255);
    }
    pm.fill_path(
        &m.finish().unwrap(),
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
    pm
}

fn main() {
    let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets");
    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    for px in [16, 32, 48, 256] {
        let pm = if px >= SEAL_FROM {
            render_seal(px)
        } else {
            render_flat(px)
        };
        // tiny-skia stores premultiplied RGBA; ICO wants straight alpha.
        let rgba: Vec<u8> = pm
            .pixels()
            .iter()
            .flat_map(|p| {
                let c = p.demultiply();
                [c.red(), c.green(), c.blue(), c.alpha()]
            })
            .collect();
        let image = ico::IconImage::from_rgba_data(px, px, rgba);
        dir.add_entry(ico::IconDirEntry::encode(&image).unwrap());
        if px == 256 {
            pm.save_png(assets.join("kiln-256.png")).unwrap();
        }
    }
    let file = std::fs::File::create(assets.join("kiln.ico")).unwrap();
    dir.write(file).unwrap();
}
