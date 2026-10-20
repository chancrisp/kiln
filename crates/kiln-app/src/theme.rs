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
    /// Brick lightened, same hue, so errors read on dark surfaces and on
    /// a selected row (4.5:1). The original brick, #B5503F, gave 2.1:1.
    pub const BRICK_LIGHT: Rgb = Rgb::hex(0xD99D93);
    /// Stone lightened, same hue: muted text on a selected or hovered row.
    pub const STONE_LIGHT: Rgb = Rgb::hex(0xB3AB9F);

    // Light mode inverts the neutrals.
    pub const BONE_GROUND: Rgb = Rgb::hex(0xF3EFE7);
    pub const SLIP: Rgb = Rgb::hex(0xE8E2D7);
    pub const BISQUE: Rgb = Rgb::hex(0xD6CEC1);
    pub const UMBER: Rgb = Rgb::hex(0x6B6257);
    /// Celadon darkened, same hue: on light surfaces the accent that marks
    /// something by itself (focus, chosen edges, the dot, progress) needs
    /// 3:1, which the shared celadon can't give.
    pub const CELADON_DEEP: Rgb = Rgb::hex(0x467966);
    /// Brick darkened, same hue, for errors on light surfaces.
    pub const BRICK_DEEP: Rgb = Rgb::hex(0x984335);
    /// Umber darkened, same hue: muted text on a selected or hovered row.
    pub const UMBER_DEEP: Rgb = Rgb::hex(0x645C52);
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
    /// Muted text on a selected or hovered row, whose wash sits between
    /// the surface and the text.
    pub muted_on_wash: Rgb,
    /// The single accent: selection washes and the primary action.
    pub accent: Rgb,
    /// The accent where it marks something on its own: focus rings, chosen
    /// edges, the selected row's edge, the suggestion dot and progress.
    /// At least 3:1 on everything it sits on.
    pub accent_strong: Rgb,
    /// Text drawn on an accent fill.
    pub on_accent: Rgb,
    /// Errors only.
    pub crack: Rgb,
    /// Colour of the light specks in the grain texture, and their peak
    /// opacity. The dark specks use `ground`'s opposite.
    pub grain_light: Rgb,
    pub grain_dark: Rgb,
    /// Links such as "Show details": muted in Kiln's own themes, the
    /// system's hyperlink colour under a contrast theme.
    pub link: Rgb,
    /// Drawn with a Windows contrast theme's colours: no washes, glows,
    /// shadows or grain, only solid fills and outlines.
    pub high_contrast: bool,
}

pub const DARK: Palette = Palette {
    name: "dark",
    ground: tokens::SOOT,
    surface: tokens::CLAY,
    line: tokens::ASH,
    text: tokens::BONE,
    text_muted: tokens::STONE,
    muted_on_wash: tokens::STONE_LIGHT,
    accent: tokens::CELADON,
    accent_strong: tokens::CELADON,
    on_accent: tokens::SOOT,
    crack: tokens::BRICK_LIGHT,
    grain_light: tokens::BONE,
    grain_dark: tokens::SOOT,
    link: tokens::STONE,
    high_contrast: false,
};

pub const LIGHT: Palette = Palette {
    name: "light",
    ground: tokens::BONE_GROUND,
    surface: tokens::SLIP,
    line: tokens::BISQUE,
    text: tokens::SOOT,
    text_muted: tokens::UMBER,
    muted_on_wash: tokens::UMBER_DEEP,
    accent: tokens::CELADON,
    accent_strong: tokens::CELADON_DEEP,
    on_accent: tokens::SOOT,
    crack: tokens::BRICK_DEEP,
    grain_light: tokens::BONE_GROUND,
    grain_dark: tokens::UMBER,
    link: tokens::UMBER,
    high_contrast: false,
};

/// The colours a Windows contrast theme sets, from `GetSysColor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SysColors {
    pub window: Rgb,
    pub window_text: Rgb,
    pub highlight: Rgb,
    pub highlight_text: Rgb,
    pub gray_text: Rgb,
    pub hotlight: Rgb,
}

/// The palette for a Windows contrast theme: the person's own colours,
/// each in the role Windows gives it. Window text stands in for muted text
/// and errors, since a contrast theme has no colour for either; gray text
/// draws borders and dividers; the highlight marks selection and progress.
pub fn theme_from_system(c: SysColors) -> Palette {
    Palette {
        name: "high-contrast",
        ground: c.window,
        surface: c.window,
        line: c.gray_text,
        text: c.window_text,
        text_muted: c.window_text,
        muted_on_wash: c.window_text,
        accent: c.highlight,
        accent_strong: c.highlight,
        on_accent: c.highlight_text,
        crack: c.window_text,
        grain_light: c.window,
        grain_dark: c.window,
        link: c.hotlight,
        high_contrast: true,
    }
}

/// From `a` toward `b` by `t`, per channel: a blend of two solid colours,
/// rounded half to even as the Phase 1 audit did.
pub fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round_ties_even() as u8;
    Rgb(l(a.0, b.0), l(a.1, b.1), l(a.2, b.2))
}

/// `fg` drawn at `alpha` over `bg`, as Direct2D composites it.
pub fn over(fg: Rgb, alpha: f32, bg: Rgb) -> Rgb {
    mix(bg, fg, alpha)
}

/// WCAG 2.x relative luminance.
pub fn luminance(c: Rgb) -> f32 {
    let f = |v: f32| {
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let [r, g, b] = c.unit();
    0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b)
}

/// WCAG 2.x contrast ratio, 1 to 21.
pub fn contrast(a: Rgb, b: Rgb) -> f32 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// What a pair must reach: 4.5:1 for text, 3:1 for a cue that isn't text.
pub const TEXT: Option<f32> = Some(4.5);
pub const NON_TEXT: Option<f32> = Some(3.0);

/// One measured pair: what it is, foreground, background (composited
/// where it's a wash), the ratio, and what it needs. `None` means no
/// threshold applies, and the name says why.
pub type ContrastPair = (&'static str, Rgb, Rgb, f32, Option<f32>);

/// Every pair in the Phase 1 contrast table (docs/ux/phase1/
/// accessibility.md), named as it names them, plus the cues added since,
/// measured from `p` as `win/render.rs` draws Kiln's own themes. Washes
/// are composited on what they sit on first.
pub fn contrast_report(p: &Palette) -> Vec<ContrastPair> {
    use effects::*;
    let wash = over(p.accent, HOVER_FILL, p.surface);
    let hover = over(p.line, ROW_HOVER, p.surface);
    let pulse = over(p.accent, HOVER_FILL + PULSE_PEAK, p.surface);
    let primary_hover = mix(p.accent, p.text, PRIMARY_HOVER);
    let glow = mix(p.accent_strong, p.text, GLOW_MIN);
    let breath = mix(p.accent_strong, p.text, BREATH_SWING);
    let underline = over(p.text_muted, UNDERLINE, p.surface);
    let thumb = over(p.text_muted, THUMB, p.surface);
    let (s, g, muted, mow, strong) = (
        p.surface,
        p.ground,
        p.text_muted,
        p.muted_on_wash,
        p.accent_strong,
    );
    let pairs: Vec<(&'static str, Rgb, Rgb, Option<f32>)> = vec![
        ("Body text: file name, row label", p.text, s, TEXT),
        ("Body text on selected row", p.text, wash, TEXT),
        ("Muted second line (12 px caption)", muted, s, TEXT),
        ("Muted second line on selected row", mow, wash, TEXT),
        ("Muted second line on hovered row", mow, hover, TEXT),
        ("Empty-state tagline and hint (muted)", muted, s, TEXT),
        ("Footer status line (muted, 12 px)", muted, g, TEXT),
        ("Header wordmark (text)", p.text, g, TEXT),
        ("Format chip code (text on line fill)", p.text, p.line, TEXT),
        ("Wheel labels and icons (text on dish)", p.text, s, TEXT),
        ("Wheel label on lit segment", p.text, wash, TEXT),
        ("Wheel label at pulse peak", p.text, pulse, TEXT),
        ("Wheel centre source line (muted on ground)", muted, g, TEXT),
        ("Outer-ring group icon (muted)", muted, s, NON_TEXT),
        ("Settings row labels (muted)", muted, s, TEXT),
        ("Unselected tab label (muted)", muted, s, TEXT),
        ("Tab label, hovered", p.text, p.line, TEXT),
        ("Selected tab or segment label", p.text, wash, TEXT),
        ("Button label (Clear, Close, Choose)", p.text, s, TEXT),
        ("Button label, hovered", p.text, p.line, TEXT),
        (
            "Disabled \"Forget picks\" (exempt: disabled)",
            muted,
            s,
            None,
        ),
        ("Primary Convert label", p.on_accent, p.accent, TEXT),
        (
            "Primary Convert label, hovered",
            p.on_accent,
            primary_hover,
            TEXT,
        ),
        ("Error line (crack red, 12 px)", p.crack, s, TEXT),
        ("Error line on selected row", p.crack, wash, TEXT),
        ("Error line on hovered row", p.crack, hover, TEXT),
        ("\"Show details\" link (muted, 12 px)", p.link, s, TEXT),
        ("\"Show details\" link on selected row", mow, wash, TEXT),
        ("Details dialog engine output (muted mono)", muted, s, TEXT),
        ("Gear icon (muted on header ground)", muted, g, NON_TEXT),
        ("Cancel × (muted on row)", muted, s, NON_TEXT),
        ("Cancel × on selected row", mow, wash, NON_TEXT),
        ("Focus ring (accent, 1.5 DIP) vs panel", strong, s, NON_TEXT),
        ("Focus ring on a selected row", strong, wash, NON_TEXT),
        (
            "Selected-row wash vs unselected row (advisory: the row's edge carries it)",
            wash,
            s,
            None,
        ),
        (
            "Selected-row edge (accent, 1 DIP) vs its wash",
            strong,
            wash,
            NON_TEXT,
        ),
        (
            "Selected-row edge (accent, 1 DIP) vs surface",
            strong,
            s,
            NON_TEXT,
        ),
        (
            "Chosen segment edge (accent) vs surface",
            strong,
            s,
            NON_TEXT,
        ),
        (
            "Lit wheel segment wash vs dish (advisory: the arc carries it)",
            wash,
            s,
            None,
        ),
        (
            "Lit wheel segment arc (accent) vs dish",
            strong,
            s,
            NON_TEXT,
        ),
        (
            "Suggestion dot (accent, 4 DIP) vs dish",
            strong,
            s,
            NON_TEXT,
        ),
        ("Suggestion dot on lit segment", strong, wash, NON_TEXT),
        (
            "Progress fill (accent) vs track (line)",
            strong,
            p.line,
            NON_TEXT,
        ),
        ("Progress fill (accent) vs row surface", strong, s, NON_TEXT),
        (
            "Progress fill (accent) on selected row",
            strong,
            wash,
            NON_TEXT,
        ),
        (
            "Progress track (line) vs row surface (advisory: the fill carries progress)",
            p.line,
            s,
            None,
        ),
        ("Progress glow head vs track", glow, p.line, NON_TEXT),
        (
            "Unknown-length progress, dim phase",
            strong,
            p.line,
            NON_TEXT,
        ),
        (
            "Unknown-length progress, bright phase",
            breath,
            p.line,
            NON_TEXT,
        ),
        (
            "Button and control border (line) vs surface (advisory: labels identify buttons)",
            p.line,
            s,
            None,
        ),
        ("Link underline (muted 80%)", underline, s, NON_TEXT),
        ("Scroll thumb (muted 80%)", thumb, s, NON_TEXT),
    ];
    pairs
        .into_iter()
        .map(|(name, fg, bg, need)| (name, fg, bg, contrast(fg, bg), need))
        .collect()
}

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

    /// The heat a progress line holds when Windows' Animation effects are
    /// off: it shows, but doesn't breathe.
    pub const STILL_HEAT: f32 = 0.0;

    static ANIMATE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

    /// Follow Windows' Animation effects setting
    /// (`SPI_GETCLIENTAREAANIMATION`). Every window in the process shares
    /// it, as Windows does.
    pub fn set_animations(on: bool) {
        ANIMATE.store(on, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn animations() -> bool {
        ANIMATE.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// How long a pour, slide or fade of `ms` lasts: all of it with
    /// animations on, none with them off.
    pub fn duration(ms: u32, animations: bool) -> u32 {
        if animations {
            ms
        } else {
            0
        }
    }

    /// `duration` under the current setting.
    pub fn ms(ms: u32) -> u32 {
        duration(ms, animations())
    }

    /// How far, 0 to 1 and eased, a motion of `ms` is after `elapsed_ms`.
    /// One of no length is already done.
    pub fn progress(elapsed_ms: f32, ms: u32) -> f32 {
        if ms == 0 {
            return 1.0;
        }
        ease_out(elapsed_ms / ms as f32)
    }

    /// The heat of a running line `ms` into its breath, or the still heat
    /// with animations off.
    pub fn breath(ms: f32, animations: bool) -> f32 {
        if animations {
            heat(ms)
        } else {
            STILL_HEAT
        }
    }

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
    /// Opacity of the line fill on a hovered row.
    pub const ROW_HOVER: f32 = 0.5;
    /// How much brighter a chosen wheel item's wash gets at its pulse's
    /// peak.
    pub const PULSE_PEAK: f32 = 0.3;
    /// How far a hovered primary button moves toward the text colour.
    pub const PRIMARY_HOVER: f32 = 0.12;
    /// How far a progress line's leading glow moves toward the text
    /// colour at its coolest; the breath adds up to `BREATH_SWING`.
    pub const GLOW_MIN: f32 = 0.2;
    pub const BREATH_SWING: f32 = 0.25;
    /// Opacity of a link's underline.
    pub const UNDERLINE: f32 = 0.8;
    /// Opacity of a scroll thumb: 3:1 on the surface.
    pub const THUMB: f32 = 0.8;
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
        ("kiln-brick-light", tokens::BRICK_LIGHT),
        ("kiln-stone-light", tokens::STONE_LIGHT),
        ("kiln-bone-ground", tokens::BONE_GROUND),
        ("kiln-slip", tokens::SLIP),
        ("kiln-bisque", tokens::BISQUE),
        ("kiln-umber", tokens::UMBER),
        ("kiln-celadon-deep", tokens::CELADON_DEEP),
        ("kiln-brick-deep", tokens::BRICK_DEEP),
        ("kiln-umber-deep", tokens::UMBER_DEEP),
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
            ("muted_on_wash", p.muted_on_wash),
            ("accent", p.accent),
            ("accent_strong", p.accent_strong),
            ("on_accent", p.on_accent),
            ("crack", p.crack),
            ("grain_light", p.grain_light),
            ("grain_dark", p.grain_dark),
            ("link", p.link),
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
    s.push_str(&format!(
        "row_hover = {}\npulse_peak = {}\nprimary_hover = {}\nglow_min = {}\nbreath_swing = {}\nunderline = {}\nthumb = {}\n",
        effects::ROW_HOVER,
        effects::PULSE_PEAK,
        effects::PRIMARY_HOVER,
        effects::GLOW_MIN,
        effects::BREATH_SWING,
        effects::UNDERLINE,
        effects::THUMB
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
                p.muted_on_wash,
                p.accent,
                p.accent_strong,
                p.on_accent,
                p.crack,
                p.grain_light,
                p.grain_dark,
                p.link,
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
        assert_eq!(DARK.accent_strong, tokens::CELADON);
    }

    #[test]
    fn contrast_fixes_keep_their_hue() {
        // Each colour darkened or lightened to pass keeps the hue of the
        // token it came from, within rounding.
        for (from, to) in [
            (tokens::CELADON, tokens::CELADON_DEEP),
            (Rgb::hex(0xB5503F), tokens::BRICK_LIGHT),
            (Rgb::hex(0xB5503F), tokens::BRICK_DEEP),
            (tokens::STONE, tokens::STONE_LIGHT),
            (tokens::UMBER, tokens::UMBER_DEEP),
        ] {
            let (h0, _) = hue_sat(from);
            let (h1, _) = hue_sat(to);
            assert!(
                (h0 - h1).abs() < 3.0,
                "{} -> {}",
                from.to_hex(),
                to.to_hex()
            );
        }
    }

    #[test]
    fn light_mode_inverts_the_neutrals() {
        assert_eq!(LIGHT.ground, Rgb::hex(0xF3EFE7));
        assert_eq!(LIGHT.text, tokens::SOOT);
    }

    #[test]
    fn contrast_maths_matches_the_phase_1_table() {
        // Pairs the Phase 1 audit measured, before this change.
        let close = |a: f32, b: f32| (a - b).abs() < 0.01;
        assert!(close(contrast(tokens::BONE, tokens::CLAY), 11.85));
        assert!(close(contrast(tokens::STONE, tokens::CLAY), 4.63));
        assert!(close(contrast(tokens::CELADON, tokens::SLIP), 1.81));
        // The selected row's wash, composited as Direct2D draws it.
        assert_eq!(
            over(tokens::CELADON, effects::HOVER_FILL, tokens::CLAY),
            Rgb::hex(0x39413B)
        );
        assert_eq!(
            over(tokens::CELADON, effects::HOVER_FILL, tokens::SLIP),
            Rgb::hex(0xD5DACD)
        );
        assert_eq!(over(tokens::ASH, 0.5, tokens::CLAY), Rgb::hex(0x32302C));
        assert!(close(
            contrast(Rgb::hex(0xB5503F), Rgb::hex(0x39413B)),
            2.10
        ));
    }

    #[test]
    fn every_pair_reaches_its_threshold() {
        for p in [DARK, LIGHT] {
            let report = contrast_report(&p);
            assert!(report.len() >= 45, "{}", report.len());
            let mut failed = vec![];
            for (name, fg, bg, ratio, need) in &report {
                if let Some(need) = need {
                    if ratio < need {
                        failed.push(format!(
                            "{} {name}: {} on {} is {ratio:.2}, needs {need}",
                            p.name,
                            fg.to_hex(),
                            bg.to_hex()
                        ));
                    }
                }
            }
            assert!(failed.is_empty(), "{}", failed.join("\n"));
        }
    }

    #[test]
    fn the_report_names_each_pair_once_and_covers_the_failures() {
        let report = contrast_report(&LIGHT);
        let names: Vec<_> = report.iter().map(|r| r.0).collect();
        for (i, n) in names.iter().enumerate() {
            assert!(!names[i + 1..].contains(n), "{n} twice");
        }
        // The rows Phase 1 failed are all measured and all have a bar.
        for failed in [
            "Muted second line on selected row",
            "Muted second line on hovered row",
            "Error line (crack red, 12 px)",
            "Error line on selected row",
            "\"Show details\" link on selected row",
            "Focus ring (accent, 1.5 DIP) vs panel",
            "Chosen segment edge (accent) vs surface",
            "Lit wheel segment arc (accent) vs dish",
            "Suggestion dot (accent, 4 DIP) vs dish",
            "Suggestion dot on lit segment",
            "Progress fill (accent) vs track (line)",
            "Unknown-length progress, dim phase",
            "Unknown-length progress, bright phase",
            "Scroll thumb (muted 80%)",
        ] {
            let row = report.iter().find(|r| r.0 == failed);
            assert!(row.is_some_and(|r| r.4.is_some()), "{failed}");
        }
    }

    #[test]
    fn a_contrast_theme_takes_the_system_colours() {
        // High Contrast Black and White as Windows ships them.
        let black = SysColors {
            window: Rgb::hex(0x000000),
            window_text: Rgb::hex(0xFFFFFF),
            highlight: Rgb::hex(0x1AEBFF),
            highlight_text: Rgb::hex(0x000000),
            gray_text: Rgb::hex(0x3FF23F),
            hotlight: Rgb::hex(0xFFFF00),
        };
        let p = theme_from_system(black);
        assert!(p.high_contrast);
        assert_eq!((p.ground, p.surface), (black.window, black.window));
        assert_eq!(
            (p.text, p.text_muted),
            (black.window_text, black.window_text)
        );
        assert_eq!(p.muted_on_wash, black.window_text);
        assert_eq!(p.crack, black.window_text);
        assert_eq!(p.line, black.gray_text);
        assert_eq!(
            (p.accent, p.accent_strong),
            (black.highlight, black.highlight)
        );
        assert_eq!(p.on_accent, black.highlight_text);
        assert_eq!(p.link, black.hotlight);
        // No grain shows: both its specks are the window colour.
        assert_eq!((p.grain_light, p.grain_dark), (black.window, black.window));

        let white = SysColors {
            window: Rgb::hex(0xFFFFFF),
            window_text: Rgb::hex(0x000000),
            highlight: Rgb::hex(0x37006E),
            highlight_text: Rgb::hex(0xFFFFFF),
            gray_text: Rgb::hex(0x600000),
            hotlight: Rgb::hex(0x00009F),
        };
        let q = theme_from_system(white);
        assert_eq!(q.text, white.window_text);
        assert_eq!(q.accent_strong, white.highlight);
        assert_ne!(p, q, "a new contrast theme is a new palette");
        const { assert!(!DARK.high_contrast && !LIGHT.high_contrast) };
    }

    #[test]
    fn animation_off_makes_every_motion_instant() {
        use super::motion::*;
        for ms in [QUICK_MS, SETTLE_MS, CONFIRM_MS, 160] {
            assert_eq!(duration(ms, true), ms);
            assert_eq!(duration(ms, false), 0);
        }
        // A motion of no length is done at once, even at time zero.
        assert_eq!(progress(0.0, 0), 1.0);
        assert_eq!(progress(0.0, duration(SETTLE_MS, false)), 1.0);
        assert_eq!(progress(0.0, SETTLE_MS), 0.0);
        assert_eq!(progress(SETTLE_MS as f32, SETTLE_MS), 1.0);
        // The breath holds still; with animations on it moves.
        let period = HEAT_PERIOD_MS as f32;
        for t in [0.0, period / 4.0, period / 2.0] {
            assert_eq!(breath(t, false), STILL_HEAT);
        }
        assert_ne!(breath(0.0, true), breath(period / 2.0, true));
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
