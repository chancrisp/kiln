//! Renders assets/kiln.ico (16, 32, 48 and 256 px) and a 256 px preview PNG
//! from the vessel path in src/icons.rs.
//!
//! cargo run -p kiln-app --example gen_icon

use std::path::PathBuf;

use kiln_app::icons::{app_icon_layout, Cmd, VESSEL_PATH};
use kiln_app::theme::tokens;
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Transform};

fn render(px: u32) -> Pixmap {
    let size = px as f32;
    let l = app_icon_layout(size);
    let mut pm = Pixmap::new(px, px).unwrap();

    let mut paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };

    // The soot tile.
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
    let s = tokens::SOOT;
    paint.set_color_rgba8(s.0, s.1, s.2, 255);
    pm.fill_path(
        &tile.finish().unwrap(),
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );

    // The celadon vessel.
    let mut v = PathBuilder::new();
    let (ox, oy) = l.offset;
    let t = |x: f32, y: f32| (x * l.scale + ox, y * l.scale + oy);
    for c in VESSEL_PATH {
        match *c {
            Cmd::Move(x, y) => {
                let (x, y) = t(x, y);
                v.move_to(x, y)
            }
            Cmd::Line(x, y) => {
                let (x, y) = t(x, y);
                v.line_to(x, y)
            }
            Cmd::Cubic(a, b, c, d, x, y) => {
                let (a, b) = t(a, b);
                let (c, d) = t(c, d);
                let (x, y) = t(x, y);
                v.cubic_to(a, b, c, d, x, y)
            }
            Cmd::Close => v.close(),
        }
    }
    let c = tokens::CELADON;
    paint.set_color_rgba8(c.0, c.1, c.2, 255);
    pm.fill_path(
        &v.finish().unwrap(),
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
        let pm = render(px);
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
