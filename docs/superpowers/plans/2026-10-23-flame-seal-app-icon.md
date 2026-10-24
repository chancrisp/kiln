# Flame Seal App Icon Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Kiln's mark becomes the flame K potter's seal: the K whose upper
arm is one lick of fire, pressed into a slab of dark stoneware with celadon
glaze pooled in the impression. It replaces the vessel everywhere, in
`kiln.ico`, the header, the empty-state hero and the inspector, so there is
one mark, and it is regenerated from code like the vessel was.

**Decisions already made (2026-10-23):**

- The icon is rendered from an SVG master with resvg, which draws the grain,
  displacement and inner-shadow filters the seal depends on. resvg 0.45 is
  already in `Cargo.lock` through `kiln-core`.
- One mark everywhere: the vessel goes, in the icon and inside the app.
- The `.ico` and the in-app mark sit square. The two-degree stamped tilt and
  the incised frame line belong only to the brand seal, for the README banner
  and marketing.

**Sources, as drawn on the design canvas:**

| What | Where it is now | Where it goes |
|---|---|---|
| Icon master, straight, full bleed | `C:\Users\chanm\AppData\Local\Temp\claude\C--kiln\569a6718-e9b1-49cb-914a-77fdf0d9928d\scratchpad\logo\k-seal-icon.svg` | `crates/kiln-app/assets/kiln-seal.svg` |
| Brand seal, tilted, with frame line | `C:\Users\chanm\AppData\Local\Temp\claude\C--kiln\569a6718-e9b1-49cb-914a-77fdf0d9928d\scratchpad\logo\k-seal-brand.svg` | `docs/readme/kiln-seal.svg` |
| The flat K path on the 24 grid | Task 3 below | `crates/kiln-app/src/icons.rs` |

Copy the two SVG files with the file tool, not a shell, and read them before
committing: the scratchpad is wiped with the session.

**Architecture:**

- `gen_icon` keeps its shape: one example, four sizes, one `.ico`, one
  256 px preview PNG. What changes is how each size is drawn.
  - **256 and 48** come from the SVG master through resvg. The master has
    a `<!-- detail -->` … `<!-- /detail -->` block holding grain, mottling,
    speckle and the hand-cut displacement. At 48 px that block is cut from
    the text before parsing, so the small icon keeps only the slab's edge
    light and the K's inner shadow.
  - **32 and 16** stay vector fills in tiny-skia from the `Cmd` path, as
    the vessel was, so the small sizes are pixel-crisp: at 32 the K takes
    the glaze gradient, at 16 flat celadon on an ash tile.
- The in-app mark is the same K as a `Cmd` path on the 24 grid, filled in
  the header and the inspector, line work in the empty-state hero. The
  seal's relief is for the icon; inside the window the mark is flat, like
  every other icon.
- One tiny-skia. resvg 0.45 depends on tiny-skia 0.11 and the example on
  0.12, and their `Pixmap` types are different. The example uses
  `resvg::tiny_skia` for everything and the direct `tiny-skia` dev-dependency
  goes.

**Tech stack:** Rust, resvg 0.45 (dev-dependency, `default-features = false`),
the `ico` crate, Direct2D in `win/render.rs`.

---

## Task 1: The SVG masters land in the repo

**Files:**
- Create: `crates/kiln-app/assets/kiln-seal.svg`
- Create: `docs/readme/kiln-seal.svg`

- [ ] Copy the icon master and the brand seal from the scratchpad paths in the
      table above. Read each before copying.
- [ ] In `kiln-seal.svg`, wrap the detail layers in markers so Task 2 can
      strip them for 48 px:

```xml
<!-- detail -->
<rect x="4" y="4" width="112" height="112" rx="25" filter="url(#mottle)" opacity="0.4"/>
<rect x="4" y="4" width="112" height="112" rx="25" filter="url(#grain)" opacity="0.25"/>
<!-- /detail -->
```

      and give the `roughK` group its own markers, `<!-- rough -->` …
      `<!-- /rough -->`, around the opening `<g filter="url(#roughK)">` and
      its closing `</g>`. The speckle `<use>` inside the K goes inside the
      detail block too. Keep the `deboss` and `edge` filters outside both
      blocks: they are the relief that 48 px keeps.
- [ ] Open both files in a browser once and compare with the canvas. The
      master must have no `<text>`, no `<image>` and no `xlink:href` to
      anything outside itself, so resvg needs no fonts and no file access.

## Task 2: `gen_icon` renders the seal

**Files:**
- Modify: `crates/kiln-app/Cargo.toml` (dev-dependencies)
- Modify: `crates/kiln-app/examples/gen_icon.rs`

- [ ] Dev-dependencies: add
      `resvg = { version = "0.45", default-features = false }`, remove
      `tiny-skia = "0.12"`. Keep `ico = "0.6"`. Run `cargo tree -p kiln-app
      -e dev -i tiny-skia` and confirm only 0.11 remains in the example's
      graph.
- [ ] `render_svg(px)`: read `assets/kiln-seal.svg` to a `String`; for
      `px < 128` remove every span from `<!-- detail -->` to
      `<!-- /detail -->` and from `<!-- rough -->` to `<!-- /rough -->`
      (the rough block leaves its inner content in place, only the `<g>` and
      `</g>` lines go); parse with
      `resvg::usvg::Tree::from_str(&text, &usvg::Options::default())`;
      render into a `resvg::tiny_skia::Pixmap` of `px × px` with
      `Transform::from_scale(px / 120.0, px / 120.0)`.
- [ ] `render_flat(px)`: the existing tile-and-path code, drawing
      `icons::MARK_PATH` (Task 3) through `app_icon_layout`. At 32 the K is
      filled with a `LinearGradient` from `#A6D3C0` through `#7FB5A0` to
      `#4F826E`, top right to bottom left, on an ash `#3B3834` tile. At 16
      flat celadon on the same tile. The tile radius stays `px * 0.22`.
- [ ] `main`: 256 and 48 through `render_svg`, 32 and 16 through
      `render_flat`. The demultiply loop and the `.ico` writing stay as they
      are. Save the 256 preview as before.
- [ ] Run `cargo run -p kiln-app --example gen_icon`. Open
      `assets/kiln-256.png`. Check: the slab fills the square with a
      3 percent margin, the K sits centred and reads as pressed in, the
      glaze is palest at the flame tip and deepest at the junction, there
      is no orange, no pure black and no pure white anywhere in the image
      (sample a few pixels; the deboss flood is `#12201A`, the edge light
      `#FFFDF8` at 30 percent over the slab).
- [ ] Check the 48 entry by extracting it: a quick `ico` read in a scratch
      test, or open `kiln.ico` in a viewer that lists sizes. It must show
      edge light and inner shadow and no grain.

## Task 3: The mark as a `Cmd` path

**Files:**
- Modify: `crates/kiln-app/src/icons.rs`

- [ ] Add `MARK_PATH` beside `VESSEL_PATH`. This is the flame K on the
      24 grid, 17.5 units tall like the vessel, centred on (12, 12), its
      two quadratics raised to cubics:

```rust
/// The flame K: the stem, then the arm that rises into one lick of fire,
/// then the leg. Spans 4.71..19.29 by 3.25..20.75.
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
```

- [ ] Add `MARK` (`Style::Stroke`, name `"mark"`) and `MARK_SOLID`
      (`Style::Fill`, name `"mark-solid"`) to the icon list and to `ALL`.
      Remove `VESSEL`, `VESSEL_SOLID` and `VESSEL_PATH` once Task 4 has moved
      every draw site. Delete `the_vessel_is_symmetric`: the K is not.
- [ ] Rename `app_icon_vessel_fits_the_tile` to `app_icon_mark_fits_the_tile`
      over `MARK_PATH`. `app_icon_layout` needs no change: the mark is 17.5
      tall, centred on (12, 12), like the vessel was. Update its comment.
- [ ] Add a test that `MARK_PATH`'s stem is vertical and the leg's foot sits
      on the vessel's old baseline: the two `Line` endpoints at `y == 20.75`
      and `x == 4.71` both exist. It guards against a stray edit to the
      numbers.
- [ ] `cargo test -p kiln-app icons`.

## Task 4: The app draws the mark

**Files:**
- Modify: `crates/kiln-app/src/win/render.rs`
- Modify: `crates/kiln-app/src/layout.rs`
- Modify: `crates/kiln-app/src/inspector.rs` (if it names the vessel)

- [ ] Header, `render.rs:1211`: `"vessel-solid"` becomes `"mark-solid"`.
- [ ] Empty-state hero, `render.rs:1265`: `"vessel"` becomes `"mark"`. It
      stays celadon line work at 72 DIPs with the 2 px line, as
      VISUAL_TESTING.md describes. The constant `VESSEL_LINE` (`render.rs:82`)
      becomes `MARK_LINE`; it also sets the glaze tiles' line, so the rename
      touches `render.rs:1301`.
- [ ] Inspector with nothing selected (`layout.rs:1390`, the 48 DIP vessel):
      find its draw site with `grep -n '"vessel' crates/kiln-app/src -r` and
      switch it to `"mark-solid"`.
- [ ] Rename the layout field `vessel` to `mark` (`layout.rs:565–652`,
      `1843`, `2500`, and the asserts at `3224` and `3233`). The a11y name at
      `layout.rs:1843` becomes `"mark"`.
- [ ] `cargo test -p kiln-app`. Then `cargo run -p kiln-app` and look at the
      header, the empty window and the inspector with nothing selected, in
      dark and light, and once under a Windows contrast theme (the mark
      follows the theme's text colour like every icon).

## Task 5: Docs follow

**Files:**
- Modify: `DESIGN.md`
- Modify: `VISUAL_TESTING.md`
- Modify: `README.md` (banner only, see below)

- [ ] `DESIGN.md:155` (the icon set): `vessel, vessel-solid` becomes
      `mark, mark-solid`.
- [ ] `DESIGN.md:156`: describe the icon. Something like: "The app icon is
      the flame K pressed into a slab of dark stoneware, celadon glaze pooled
      in the impression, rendered from `crates/kiln-app/assets/kiln-seal.svg`
      at 256 and 48 px and as a flat fill at 32 and 16 px. Regenerate it with
      `cargo run -p kiln-app --example gen_icon`. The brand seal in
      `docs/readme/kiln-seal.svg` is the same mark tilted two degrees with an
      incised frame line; it is for the README and marketing, never the
      icon."
- [ ] Principle 1 (one metaphor) gains the seal: "The mark is a maker's
      seal pressed into clay." Nothing else in the principles changes.
- [ ] `VISUAL_TESTING.md:18`: "the celadon mark". `:99`: "the mark in celadon
      line work". `:107`: "the flame K seal on dark stoneware at 256 and 48,
      the flat K on an ash tile at 32 and 16".
- [ ] The README banner (`docs/readme/banner.png`) shows the vessel. Leave
      it for a separate change; note it in the commit message so it is not
      forgotten.

## Task 6: Verify the shipped icon

- [ ] `cargo build -p kiln-app --release`. The `.rc` picks up the new
      `kiln.ico` through `build.rs`.
- [ ] Pin the exe to the taskbar and look at it at 100 and 200 percent
      scaling; open its folder in Explorer at Large icons and Details; press
      Alt+Tab. Check the 16 px K is legible and the 256 one shows the relief.
- [ ] `packaging/package.ps1` copies `kiln.ico` by name; no change there,
      but run it once and confirm the MSI's icon is the seal.

## Acceptance

- One mark in `icons.rs`, no vessel left anywhere in `crates/`.
- `cargo run -p kiln-app --example gen_icon` produces a four-size `.ico`
  whose 256 and 48 entries are the seal and whose 32 and 16 are flat, and
  the 256 preview PNG matches the canvas within the renderer's grain.
- The header, hero and inspector show the K; the hero in line work.
- DESIGN.md and VISUAL_TESTING.md describe the seal.
- No orange, pure black or pure white in any rendered size.

## Out of scope

- The README banner and screenshots in `docs/screenshots` that show the
  vessel.
- A hand-drawn relief for 32 px. If the flat gradient K reads as too plain
  at 32, revisit with a single offset shadow fill in tiny-skia.
