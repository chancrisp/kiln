//! Kiln's design tokens. Every colour, size and duration the app draws with
//! is defined here once and referenced everywhere else. DESIGN.md explains
//! the reasoning; tests/snapshots/theme_tokens.snap pins the values.

/// An sRGB colour. Alpha is applied where the colour is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub const fn hex(v: u32) -> Self {
        Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
    }

    pub fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.0, self.1, self.2)
    }

    /// Components in 0.0..=1.0, for Direct2D.
    pub fn unit(self) -> [f32; 3] {
        [
            self.0 as f32 / 255.0,
            self.1 as f32 / 255.0,
            self.2 as f32 / 255.0,
        ]
    }

    /// Win32 COLORREF (0x00BBGGRR), for DWM title bar colours.
    pub fn colorref(self) -> u32 {
        (self.2 as u32) << 16 | (self.1 as u32) << 8 | self.0 as u32
    }
}

/// The named tokens from the spec.
pub mod tokens {
    use super::Rgb;

    pub const SOOT: Rgb = Rgb::hex(0x1C1B19);
    pub const CLAY: Rgb = Rgb::hex(0x2A2825);
    pub const ASH: Rgb = Rgb::hex(0x3B3834);
    pub const BONE: Rgb = Rgb::hex(0xEDE6DA);
    pub const STONE: Rgb = Rgb::hex(0x9A8F80);
    pub const CELADON: Rgb = Rgb::hex(0x7FB5A0);
    pub const BRICK: Rgb = Rgb::hex(0xB5503F);

    // Light mode inverts the neutrals.
    pub const BONE_GROUND: Rgb = Rgb::hex(0xF3EFE7);
    pub const SLIP: Rgb = Rgb::hex(0xE8E2D7);
    pub const BISQUE: Rgb = Rgb::hex(0xD6CEC1);
    pub const UMBER: Rgb = Rgb::hex(0x6B6257);
}

/// What each colour is for. Code asks for roles, never raw tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub name: &'static str,
    /// Window background.
    pub ground: Rgb,
    /// Panels and surfaces.
    pub surface: Rgb,
    /// Borders, dividers, disabled fills.
    pub line: Rgb,
    pub text: Rgb,
    pub text_muted: Rgb,
    /// The single accent: selection, focus, primary action, progress.
    pub accent: Rgb,
    /// Text drawn on an accent fill.
    pub on_accent: Rgb,
    /// Errors only.
    pub crack: Rgb,
    /// Colour of the light specks in the grain texture, and their peak
    /// opacity. The dark specks use `ground`'s opposite.
    pub grain_light: Rgb,
    pub grain_dark: Rgb,
}

pub const DARK: Palette = Palette {
    name: "dark",
    ground: tokens::SOOT,
    surface: tokens::CLAY,
    line: tokens::ASH,
    text: tokens::BONE,
    text_muted: tokens::STONE,
    accent: tokens::CELADON,
    on_accent: tokens::SOOT,
    crack: tokens::BRICK,
    grain_light: tokens::BONE,
    grain_dark: tokens::SOOT,
};

pub const LIGHT: Palette = Palette {
    name: "light",
    ground: tokens::BONE_GROUND,
    surface: tokens::SLIP,
    line: tokens::BISQUE,
    text: tokens::SOOT,
    text_muted: tokens::UMBER,
    accent: tokens::CELADON,
    on_accent: tokens::SOOT,
    crack: tokens::BRICK,
    grain_light: tokens::BONE_GROUND,
    grain_dark: tokens::UMBER,
};

/// Type scale in DIPs. A single view uses at most four of these.
pub mod type_scale {
    pub const CAPTION: f32 = 12.0;
    pub const BODY: f32 = 14.0;
    pub const LEAD: f32 = 16.0;
    pub const TITLE: f32 = 22.0;
    pub const DISPLAY: f32 = 32.0;
    pub const ALL: [f32; 5] = [CAPTION, BODY, LEAD, TITLE, DISPLAY];

    /// Letter spacing for format codes, as a fraction of the font size.
    pub const CODE_TRACKING_EM: f32 = 0.06;
}

/// Font families. The serif is bundled; the others come with Windows.
pub mod fonts {
    /// The bundled display serif (Fraunces, OFL), used only for the app
    /// name, the wheel's centre label and dialog titles.
    pub const DISPLAY_FAMILY: &str = "Fraunces 72pt SuperSoft SemiBold";
    pub const DISPLAY_TTF: &[u8] = include_bytes!("../assets/fonts/fraunces-kiln.ttf");
    pub const DISPLAY_FALLBACK: &str = "Georgia";

    /// UI text: the first family Windows has wins.
    pub const UI: [&str; 2] = ["Segoe UI Variable Text", "Segoe UI"];
    /// Format codes.
    pub const MONO: [&str; 2] = ["Cascadia Mono", "Consolas"];
}

/// Shape and spacing in DIPs.
pub mod shape {
    pub const GRID: f32 = 4.0;
    pub const PANEL_PADDING: f32 = 16.0;
    pub const RADIUS_CONTROL: f32 = 6.0;
    pub const RADIUS_PANEL: f32 = 12.0;
    pub const BORDER: f32 = 1.0;
    pub const ICON_GRID: f32 = 24.0;
    pub const ICON_STROKE: f32 = 1.5;
}

/// Motion. Everything eases out; nothing bounces.
pub mod motion {
    pub const QUICK_MS: u32 = 120;
    pub const SETTLE_MS: u32 = 180;
    pub const CONFIRM_MS: u32 = 200;

    /// One slow breath of a running progress line's heat.
    pub const HEAT_PERIOD_MS: u32 = 2400;

    /// Cubic ease-out: fast start, gentle stop, never past 1.
    pub fn ease_out(t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        1.0 - (1.0 - t).powi(3)
    }

    /// How hot a running progress line burns, `ms` into the job: rises
    /// from 0 to 1 and falls back over each `HEAT_PERIOD_MS`, smoothly,
    /// like a glaze taking heat. Never outside 0 to 1.
    pub fn heat(ms: f32) -> f32 {
        let phase = (ms / HEAT_PERIOD_MS as f32).fract();
        0.5 - 0.5 * (std::f32::consts::TAU * phase).cos()
    }
}

/// Effects with fixed strengths.
pub mod effects {
    /// Opacity of the accent fill on a hovered or selected item.
    pub const HOVER_FILL: f32 = 0.18;
    /// Opacity of the accent wash on a drop zone while a drag is over it.
    pub const DROP_WASH: f32 = 0.06;
    /// Peak opacity of the background grain (spec: 2 to 4 percent).
    pub const GRAIN_OPACITY: f32 = 0.03;
    /// Side of the pre-rendered grain tile, in pixels.
    pub const GRAIN_TILE: u32 = 128;
}

/// A deterministic noise tile for the window grain, premultiplied BGRA.
/// Rendered once at startup and tiled; it never animates.
pub fn grain_tile(p: &Palette) -> Vec<u8> {
    let side = effects::GRAIN_TILE as usize;
    let mut out = Vec::with_capacity(side * side * 4);
    let mut state: u32 = 0x4B49_4C4E; // "KILN"
    for _ in 0..side * side {
        // xorshift32: cheap and stable across platforms.
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        let r = state as f32 / u32::MAX as f32;
        let (c, strength) = if r < 0.5 {
            (p.grain_dark, (0.5 - r) * 2.0)
        } else {
            (p.grain_light, (r - 0.5) * 2.0)
        };
        let a = strength * effects::GRAIN_OPACITY;
        let [cr, cg, cb] = c.unit();
        let px = |v: f32| (v * a * 255.0).round() as u8;
        out.extend_from_slice(&[px(cb), px(cg), px(cr), (a * 255.0).round() as u8]);
    }
    out
}

/// Every token as text, for the snapshot test.
pub fn describe() -> String {
    let mut s = String::new();
    let tok = [
        ("kiln-soot", tokens::SOOT),
        ("kiln-clay", tokens::CLAY),
        ("kiln-ash", tokens::ASH),
        ("kiln-bone", tokens::BONE),
        ("kiln-stone", tokens::STONE),
        ("kiln-celadon", tokens::CELADON),
        ("kiln-brick", tokens::BRICK),
        ("kiln-bone-ground", tokens::BONE_GROUND),
        ("kiln-slip", tokens::SLIP),
        ("kiln-bisque", tokens::BISQUE),
        ("kiln-umber", tokens::UMBER),
    ];
    s.push_str("[tokens]\n");
    for (name, c) in tok {
        s.push_str(&format!("{name} = {}\n", c.to_hex()));
    }
    for p in [DARK, LIGHT] {
        s.push_str(&format!("\n[palette.{}]\n", p.name));
        for (role, c) in [
            ("ground", p.ground),
            ("surface", p.surface),
            ("line", p.line),
            ("text", p.text),
            ("text_muted", p.text_muted),
            ("accent", p.accent),
            ("on_accent", p.on_accent),
            ("crack", p.crack),
            ("grain_light", p.grain_light),
            ("grain_dark", p.grain_dark),
        ] {
            s.push_str(&format!("{role} = {}\n", c.to_hex()));
        }
    }
    s.push_str("\n[type]\n");
    s.push_str(&format!("scale = {:?}\n", type_scale::ALL));
    s.push_str(&format!(
        "code_tracking_em = {}\n",
        type_scale::CODE_TRACKING_EM
    ));
    s.push_str(&format!("display = {:?}\n", fonts::DISPLAY_FAMILY));
    s.push_str(&format!("ui = {:?}\n", fonts::UI));
    s.push_str(&format!("mono = {:?}\n", fonts::MONO));
    s.push_str("\n[shape]\n");
    s.push_str(&format!(
        "grid = {}\npanel_padding = {}\nradius_control = {}\nradius_panel = {}\nborder = {}\nicon_grid = {}\nicon_stroke = {}\n",
        shape::GRID,
        shape::PANEL_PADDING,
        shape::RADIUS_CONTROL,
        shape::RADIUS_PANEL,
        shape::BORDER,
        shape::ICON_GRID,
        shape::ICON_STROKE
    ));
    s.push_str("\n[motion]\n");
    s.push_str(&format!(
        "quick_ms = {}\nsettle_ms = {}\nconfirm_ms = {}\n",
        motion::QUICK_MS,
        motion::SETTLE_MS,
        motion::CONFIRM_MS
    ));
    s.push_str("\n[effects]\n");
    s.push_str(&format!(
        "hover_fill = {}\ndrop_wash = {}\ngrain_opacity = {}\ngrain_tile = {}\n",
        effects::HOVER_FILL,
        effects::DROP_WASH,
        effects::GRAIN_OPACITY,
        effects::GRAIN_TILE
    ));
    s
}

#[cfg(test)]
mod tests {

    #[test]
    fn heat_breathes_smoothly_within_range() {
        use super::motion::{heat, HEAT_PERIOD_MS};
        let period = HEAT_PERIOD_MS as f32;
        assert!(heat(0.0).abs() < 1e-6);
        assert!((heat(period / 2.0) - 1.0).abs() < 1e-6);
        assert!(heat(period).abs() < 1e-4);
        let mut last = heat(0.0);
        for step in 1..=480 {
            let h = heat(step as f32 * period / 240.0);
            assert!((0.0..=1.0).contains(&h));
            assert!((h - last).abs() < 0.02, "jumped at step {step}");
            last = h;
        }
    }
    use super::*;

    fn all_colours() -> Vec<Rgb> {
        let mut v = vec![];
        for p in [DARK, LIGHT] {
            v.extend([
                p.ground,
                p.surface,
                p.line,
                p.text,
                p.text_muted,
                p.accent,
                p.on_accent,
                p.crack,
                p.grain_light,
                p.grain_dark,
            ]);
        }
        v
    }

    /// Hue in degrees and HSL saturation.
    fn hue_sat(c: Rgb) -> (f32, f32) {
        let [r, g, b] = c.unit();
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let l = (max + min) / 2.0;
        let d = max - min;
        if d == 0.0 {
            return (0.0, 0.0);
        }
        let s = d / (1.0 - (2.0 * l - 1.0).abs());
        let h = if max == r {
            60.0 * (((g - b) / d) % 6.0)
        } else if max == g {
            60.0 * ((b - r) / d + 2.0)
        } else {
            60.0 * ((r - g) / d + 4.0)
        };
        ((h + 360.0) % 360.0, s)
    }

    #[test]
    fn no_pure_black_or_white() {
        for c in all_colours() {
            assert_ne!(c, Rgb(0, 0, 0));
            assert_ne!(c, Rgb(255, 255, 255));
        }
    }

    #[test]
    fn no_orange_anywhere() {
        for c in all_colours() {
            let (h, s) = hue_sat(c);
            assert!(
                !((15.0..=45.0).contains(&h) && s > 0.4),
                "{} reads as orange (hue {h:.0}, saturation {s:.2})",
                c.to_hex()
            );
        }
    }

    #[test]
    fn both_palettes_share_the_one_accent() {
        assert_eq!(DARK.accent, tokens::CELADON);
        assert_eq!(LIGHT.accent, tokens::CELADON);
        assert_eq!(DARK.crack, tokens::BRICK);
    }

    #[test]
    fn light_mode_inverts_the_neutrals() {
        assert_eq!(LIGHT.ground, Rgb::hex(0xF3EFE7));
        assert_eq!(LIGHT.text, tokens::SOOT);
    }

    /// WCAG relative luminance contrast.
    fn contrast(a: Rgb, b: Rgb) -> f32 {
        let lum = |c: Rgb| {
            let f = |v: f32| {
                if v <= 0.03928 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            let [r, g, b] = c.unit();
            0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b)
        };
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn text_is_readable_on_every_surface() {
        for p in [DARK, LIGHT] {
            for bg in [p.ground, p.surface] {
                assert!(contrast(p.text, bg) >= 7.0, "{} text", p.name);
                assert!(contrast(p.text_muted, bg) >= 4.5, "{} muted text", p.name);
            }
            assert!(
                contrast(p.on_accent, p.accent) >= 4.5,
                "{} on accent",
                p.name
            );
        }
    }

    #[test]
    fn spacing_sits_on_the_grid() {
        for v in [shape::PANEL_PADDING, shape::RADIUS_PANEL, shape::ICON_GRID] {
            assert_eq!(v % shape::GRID, 0.0, "{v}");
        }
    }

    #[test]
    fn motion_is_short_and_never_overshoots() {
        assert!((120..=180).contains(&motion::QUICK_MS));
        assert!((120..=180).contains(&motion::SETTLE_MS));
        let mut last = 0.0;
        for i in 0..=100 {
            let v = motion::ease_out(i as f32 / 100.0);
            assert!(v >= last && v <= 1.0);
            last = v;
        }
        assert_eq!(motion::ease_out(1.0), 1.0);
    }

    #[test]
    fn grain_is_faint_and_stable() {
        let a = grain_tile(&DARK);
        assert_eq!(a, grain_tile(&DARK));
        assert_eq!(
            a.len(),
            (effects::GRAIN_TILE * effects::GRAIN_TILE * 4) as usize
        );
        let max_alpha = a.chunks(4).map(|px| px[3]).max().unwrap();
        assert!(max_alpha as f32 / 255.0 <= 0.04 + 1e-3);
        assert!(max_alpha as f32 / 255.0 >= 0.02);
    }

    #[test]
    fn display_font_subset_is_small() {
        assert!(fonts::DISPLAY_TTF.len() < 120 * 1024);
        assert_eq!(&fonts::DISPLAY_TTF[..4], &[0, 1, 0, 0], "TrueType outlines");
    }
}
