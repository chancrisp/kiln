# Look and Motion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Kiln's window gains file previews, a glaze colour per kind of file,
an inspector panel for the selection, surfaces with depth, and motion with
weight, while keeping its identity and its speed.

**Architecture:**
- Every decision that can be a function lives in the platform-free half of
  `kiln-app`, with its tests:
  - glazes and depth in `theme.rs`
  - the motion engine in a new `anim.rs`
  - the inspector's content in a new `inspector.rs`
  - the preview cache and its helpers in a new `previews.rs`
  - geometry in `layout.rs`
  - names in `a11y.rs`
- The Windows half (`src/win`) only draws, hit-tests and runs a new
  thumbnail worker, `win/thumbs.rs`.

**Tech Stack:** Rust; `windows` 0.62 (Direct2D, DirectWrite, Shell
`IShellItemImageFactory`, GDI `GetDIBits`); no new crates.

**Spec:** `docs/superpowers/specs/2026-10-23-look-and-motion-design.md`

## Global Constraints

- **Branch:** `look-and-motion`.
  - Never push, and never merge without the owner's yes.
  - Commits use the owner's git identity, with no AI attribution or
    trailers.
- **Architecture:** the platform-free half (model, layout, theme, a11y,
  wording) holds every decision that can be a function. Add no framework
  and no new crate.
- **Identity:** soot, bone and celadon, Fraunces in its three places, the
  grain.
  - No orange, no pure black (`#000000`), no pure white (`#FFFFFF`). The
    token tests enforce this.
  - Celadon stays the one accent for focus, selection and buttons.
- **Glaze base colours:**

  | Kind | Dark | Light |
  |---|---|---|
  | Images | `#7FB5A0` | `#467966` |
  | Video | `#8FA6C9` | `#4D6489` |
  | Audio | `#C8BA83` | `#756835` |
  | Documents | `#B3A5CF` | `#6A5B8C` |
  | Other | `#A89E8F` | `#6E665B` |

  Documents include spreadsheets, presentations, subtitles and ebooks.
- **Glaze ink colours**, for text drawn in a glaze:

  | Kind | Dark | Light |
  |---|---|---|
  | Images | `#83B7A2` | `#395D4F` |
  | Video | `#99ADCB` | `#445775` |
  | Audio | `#C8BA83` | `#61562F` |
  | Documents | `#B3A5CF` | `#5C4F77` |
  | Other | `#B4AA9C` | `#5B554C` |

- **Chip:** an opaque fill of the glaze at 16% over the surface, a 1 px
  outline of the glaze at 35%, and text in the glaze ink.
- **Sizes and timings:**
  - Fold point: 760 zoomed DIPs.
  - Inspector: 300 DIPs wide, with a 1 px rule.
  - Content cap: 1180 DIPs with the inspector; 880 without.
  - Row preview: 34 DIPs, 6 DIP corners.
  - Inspector preview: 268 × 120 DIPs.
- **Depth:**
  - A 1 px top highlight: bone at 5% in dark, bone ground at 60% in light.
  - One soft shadow: y 10, blur 30, spread −18.
  - The ground keeps its grain and gains a celadon glow at 6%, top centre.
- **Motion:** every value is given in Task 2's catalogue.
  - Easing is the existing cubic `motion::ease_out`. Heat breathes on the
    existing cosine.
  - With Windows' Animation effects off, every value is at its end at once.
- **Speed:**
  - The window is visible by 100 ms, with no white first frame.
  - A key reaches the screen in under 35 ms (median) with 4 conversions
    running.
  - 500 files are listed in under 0.3 s.
  - The UI thread replies in under 3 ms (p95) while previews load.
  - Memory stays under 160 MB with 500 rows.
  - The preview cache is capped at 64 MB.
- **Gates:**
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test -p kiln-core`, with `%LOCALAPPDATA%\Pandoc` on PATH
  - `cargo test -p kiln-app`
- **Line endings:** the repo stores LF. Keep each file's endings as they
  are.

## Rulings made while planning (from the code, against the spec)

- **Ki-seto yellow** is `#C8BA83` / `#756835`, not the spec's `#CDB67E` /
  `#7A6630`. The spec's colours fail the existing `no_orange_anywhere` test
  (hue 43°, saturation 0.44). Hue 48° at saturation 0.38 passes.
- **Glaze text uses an ink tone.** In light the base glazes reach only 2.7
  to 3.9:1 on a chip fill or a selected row. Each ink is its glaze mixed
  toward the theme's text colour just enough to reach 4.6:1 on the chip
  fill, the surface, the selected wash and the hovered row.
- **Easing:** the existing cubic ease-out stands in for cubic-bezier
  (.2, .8, .2, 1). It's the same feel, and needs no curve solver.
- **Icons:** the spec's "waveform, page, film, picture" are the existing
  `audio`, `document`, `video` and `image` paths in `icons.rs`. The
  empty-state vessel is `VESSEL_PATH` drawn at 72 DIPs. No new icon paths.
- **Light highlight:** bone ground (`#F3EFE7`) at 60%, because the
  no-white rule forbids white.

## Review Focus

1. **JPEG thumbnails with no alpha.** Windows hands many JPEG and video
   thumbnails back with every alpha byte at 0. Drawn premultiplied, they
   would vanish or come out black. Expect a normal picture. Pinned in
   Task 5 by `an_all_clear_bitmap_is_made_opaque`.
2. **Rows removed, cleared or undone while previews are in flight.** A late
   preview must land on its own file's row, or nowhere, never on the row
   that took its index. Pinned in Task 5 by
   `a_preview_for_a_file_no_longer_listed_is_dropped`, because results are
   keyed by path and modified time, not by row.
3. **Holding an arrow key through 500 rows.** The inspector must keep up,
   and must not queue 500 large-preview requests or stack 500 cross-fades.
   Pinned in Task 5 by `request_order_puts_the_selection_first_and_drops_stale_heroes`
   and in Task 2 by `a_crossfade_restarted_midway_starts_from_where_it_was`.
4. **A short, wide window** (for example 1200 × 360). The inspector clips
   its content without drawing over the footer, and every control it keeps
   is whole, so nothing invisible can be clicked or tabbed to. Pinned in
   Task 4 by `a_short_inspector_drops_what_does_not_fit_whole`.
5. **A move to a monitor with another scale.** Previews are asked for again
   at the new pixel size rather than blurred or stretched. Pinned in Task 5
   by `the_cache_key_carries_the_pixel_size`.

---

### Task 1: Glazes and depth in the theme

**Files:**
- Modify: `crates/kiln-app/src/theme.rs`
- Modify: `crates/kiln-app/tests/snapshots/theme_tokens.snap` (regenerated)
- Modify: `DESIGN.md` (Colour, Type, Principles 2 and 3)

**Interfaces:**
- Produces:
  - `pub enum Glaze { Image, Video, Audio, Document, Other }`
    (`Debug, Clone, Copy, PartialEq, Eq, Hash`), with
    `pub const ALL: [Glaze; 5]`
  - `pub fn glaze_of(kind: Option<kiln_core::Kind>) -> Glaze`.
    Spreadsheet, Presentation, Subtitle and Document all map to `Document`;
    `None` maps to `Other`.
  - New `Palette` fields:
    - `glazes: [Rgb; 5]` and `glaze_inks: [Rgb; 5]`, indexed in `Glaze::ALL`
      order
    - `shadow: Rgb` and `shadow_alpha: f32`
    - `highlight: Rgb` and `highlight_alpha: f32`
  - New `Palette` methods:
    - `pub fn glaze(&self, g: Glaze) -> Rgb`
    - `pub fn glaze_ink(&self, g: Glaze) -> Rgb`
    - `pub fn chip_fill(&self, g: Glaze) -> Rgb`, which returns
      `over(glaze, 0.16, surface)`
  - `pub mod effects` gains:
    - `CHIP_FILL: f32 = 0.16`
    - `CHIP_OUTLINE: f32 = 0.35`
    - `GROUND_GLOW: f32 = 0.06`
    - `ROW_WASH: f32 = 0.09`, the converting row's wash at its left edge
  - `pub mod depth` holds:
    - `pub const OFFSET_Y: f32 = 10.0`, `BLUR: f32 = 30.0`,
      `SPREAD: f32 = -18.0`
    - `pub fn shadow_layers() -> Vec<(f32, f32)>`, where each entry is
      (grow in DIPs, alpha share)
  - `type_scale::INSPECTOR_TITLE: f32 = 15.0`, added to `ALL`

- [ ] **Step 1: Write the failing tests** in `theme.rs`'s test module:

```rust
#[test]
fn every_kind_has_a_glaze() {
    use kiln_core::Kind::*;
    assert_eq!(glaze_of(Some(Image)), Glaze::Image);
    assert_eq!(glaze_of(Some(Video)), Glaze::Video);
    assert_eq!(glaze_of(Some(Audio)), Glaze::Audio);
    for k in [Document, Spreadsheet, Presentation, Subtitle] {
        assert_eq!(glaze_of(Some(k)), Glaze::Document);
    }
    assert_eq!(glaze_of(None), Glaze::Other);
}

#[test]
fn glazes_are_the_designed_colours() {
    assert_eq!(DARK.glaze(Glaze::Audio), Rgb::hex(0xC8BA83));
    assert_eq!(LIGHT.glaze_ink(Glaze::Image), Rgb::hex(0x395D4F));
    assert_eq!(DARK.glaze(Glaze::Image), tokens::CELADON);
}

#[test]
fn glaze_ink_reads_on_every_ground_it_meets() {
    for p in [&DARK, &LIGHT] {
        let wash = over(p.accent, effects::HOVER_FILL, p.surface);
        let hover = over(p.line, effects::ROW_HOVER, p.surface);
        for g in Glaze::ALL {
            let ink = p.glaze_ink(g);
            for bg in [p.chip_fill(g), p.surface, wash, hover] {
                assert!(contrast(ink, bg) >= 4.5, "{} {:?} on {}", p.name, g, bg.to_hex());
            }
        }
    }
}

#[test]
fn a_contrast_theme_has_no_glazes() {
    let p = theme_from_system(sys); // the SysColors the existing contrast-theme test builds
    for g in Glaze::ALL {
        assert_eq!(p.glaze(g), p.text);
        assert_eq!(p.glaze_ink(g), p.text);
    }
}

#[test]
fn shadow_layers_fade_out_and_sum_to_one() {
    let l = depth::shadow_layers();
    assert!(l.len() >= 6);
    assert!(l.windows(2).all(|w| w[0].0 < w[1].0 && w[0].1 >= w[1].1));
    let sum: f32 = l.iter().map(|x| x.1).sum();
    assert!((sum - 1.0).abs() < 1e-3);
    assert!(l.last().unwrap().0 <= depth::BLUR / 2.0 + depth::SPREAD.abs());
}
```

  Also extend `contrast_report` with these pairs, so the existing "every
  pair passes" test covers them:
  - "Glaze ink on its chip fill"
  - "Glaze ink on the panel"
  - "Glaze ink on selected row"
  - "Glaze ink on hovered row"

  That's 20 pairs per theme, each at `TEXT`.

- [ ] **Step 2: Run them and see them fail.**
  - Run: `cargo test -p kiln-app theme`
  - Expected: compile errors for `Glaze`, `glaze_of` and `depth`.

- [ ] **Step 3: Implement.**
  - Add the glaze and ink colours from Global Constraints to `tokens` as
    named constants (`GLAZE_VIDEO`, `GLAZE_VIDEO_DEEP`, `INK_VIDEO`, and
    so on; images reuse `CELADON` / `CELADON_DEEP`).
  - Add to `tokens`: `SHADOW_DARK = #0E0D0C` (alpha 0.45) and
    `SHADOW_LIGHT = #3C2D1E` (alpha 0.18).
  - The highlights are `BONE` at 0.05 in dark and `BONE_GROUND` at 0.60 in
    light.
  - `theme_from_system` sets every glaze and ink to `window_text`, the
    shadow and highlight alphas to 0, and `ROW_WASH` doesn't apply there.
  - `shadow_layers`: 8 layers, grow `i * (BLUR / 2 + SPREAD) / 7` for `i`
    in 0..8, clamped at 0, with alpha shares from a Gaussian
    (σ = BLUR / 6), normalised to sum to 1.
  - `describe()` prints the new tokens and palette fields, so the snapshot
    shows them.
- [ ] **Step 4: Regenerate the snapshot and run the tests.**
  - Run: `KILN_UPDATE_SNAPSHOTS=1 cargo test -p kiln-app --test theme_snapshot`
  - Then run: `cargo test -p kiln-app theme`
  - Expected: PASS. That includes `no_orange_anywhere` and
    `no_pure_black_or_white` over the new tokens.
- [ ] **Step 5: Update `DESIGN.md`.**
  - Colour gets a "Glazes" table with base and ink colours, where each
    glaze appears (chips, previews' fallback tile, heat and converting
    wash, the suggestion text, the inspector's top wash), and the
    contrast-theme fallback.
  - Principle 2 becomes "two neutrals, one accent, and a glaze per kind of
    file".
  - Principle 3's "Flat surfaces have no drop shadows" becomes "Each
    surface level has one soft shadow; nothing else does".
  - Type adds 15 for the inspector's file name.
- [ ] **Step 6: Run the gates, then commit.**

```bash
git add crates/kiln-app/src/theme.rs crates/kiln-app/tests/snapshots/theme_tokens.snap DESIGN.md
git commit -m "Look: glazes, depth and the inspector's type size in the theme"
```

---

### Task 2: The motion engine

**Files:**
- Create: `crates/kiln-app/src/anim.rs`, and add `pub mod anim;` to
  `crates/kiln-app/src/lib.rs`
- Modify: `crates/kiln-app/src/win/mod.rs`. Remove its private `Tween`,
  `eased` and `within`, and use `anim::` with `Instant::now()`. Behaviour is
  unchanged in this task.

**Interfaces:**
- Produces, in `anim`:
  - **Tween:** `pub struct Tween`, the existing type moved here.
    - `pub fn new(v: f32, now: Instant) -> Tween`
    - `pub fn value(&self, now: Instant) -> f32`
    - `pub fn set(&mut self, to: f32, ms: u32, now: Instant, animations: bool)`
    - `pub fn running(&self, now: Instant) -> bool`
  - **Timing helpers:**
    - `pub fn eased(start: Instant, ms: u32, now: Instant, animations: bool) -> f32`
    - `pub fn within(start: Instant, ms: u32, now: Instant, animations: bool) -> bool`
  - **Catalogue constants:**
    - `POUR_MS = 260`, `POUR_STAGGER_MS = 45`, `POUR_STAGGER_CAP_MS = 400`,
      `POUR_RISE = 10.0`
    - `GLIDE_MS = 160`
    - `CROSSFADE_MS = 180`, `CROSSFADE_RISE = 6.0`
    - `PREVIEW_MS = 180`
    - `COOL_MS = 900`, `COOL_PEAK = 0.32`
    - `CRACK_MS = 120`, `CRACK_DX = 2.0`
    - `BLOOM_MS = 200`, `BLOOM_SCALE = 0.86`, `BLOOM_TURN_DEG = -8.0`
    - `SEGMENT_STAGGER_MS = 18`, `SEGMENT_SCALE = 0.94`
    - `SHEET_MS = 180`, `SHEET_RISE = 12.0`
    - `PRESS_MS = 120`
    - `FOLD_MS = 220`
  - **Catalogue functions.** Each takes `elapsed_ms: f32` and
    `animations: bool`:
    - `pub fn pour(elapsed_ms, order: usize, animations) -> (f32, f32)`
      gives (alpha, dy). The delay is
      `min(order * POUR_STAGGER_MS, POUR_STAGGER_CAP_MS)`.
    - `pub fn cool(elapsed_ms, animations) -> f32`: the glaze wash alpha,
      from `COOL_PEAK` down to 0.
    - `pub fn crack(elapsed_ms, animations) -> f32`: dx, going 0, −2, +2,
      0 in thirds.
    - `pub fn bloom(elapsed_ms, animations) -> (f32, f32, f32)` gives
      (alpha, scale, turn in degrees).
    - `pub fn segment(elapsed_ms, i: usize, animations) -> (f32, f32)`
      gives (alpha, scale), delayed `40 + i * SEGMENT_STAGGER_MS`.
    - `pub fn rise(elapsed_ms, ms: u32, distance: f32, animations) -> (f32, f32)`
      gives (alpha, dy). The crossfade, sheets and previews use it.
  - **Frame timer:** `pub fn frame_ms(moving: bool, burning: bool, animations: bool) -> Option<u32>`
    gives `Some(16)` while moving, `Some(50)` while only burning with
    animations on, else `None`.

- [ ] **Step 1: Write the failing tests** in `anim.rs`:

```rust
fn t0() -> Instant { Instant::now() }
fn at(t: Instant, ms: u64) -> Instant { t + Duration::from_millis(ms) }

#[test]
fn a_tween_eases_to_its_target_and_stops() {
    let t = t0();
    let mut w = Tween::new(0.0, t);
    w.set(1.0, 200, t, true);
    assert!(w.value(at(t, 100)) > 0.5 && w.value(at(t, 100)) < 1.0);
    assert_eq!(w.value(at(t, 200)), 1.0);
    assert!(w.running(at(t, 199)) && !w.running(at(t, 200)));
}

#[test]
fn a_crossfade_restarted_midway_starts_from_where_it_was() {
    let t = t0();
    let mut w = Tween::new(0.0, t);
    w.set(1.0, CROSSFADE_MS, t, true);
    let mid = w.value(at(t, 60));
    w.set(0.0, CROSSFADE_MS, at(t, 60), true);
    assert!((w.value(at(t, 60)) - mid).abs() < 1e-4);
}

#[test]
fn rows_pour_with_a_capped_stagger() {
    assert_eq!(pour(0.0, 0, true), (0.0, POUR_RISE));
    assert_eq!(pour(POUR_MS as f32, 0, true), (1.0, 0.0));
    // Row 499 starts no later than the cap.
    let end = (POUR_STAGGER_CAP_MS + POUR_MS) as f32;
    assert_eq!(pour(end, 499, true), (1.0, 0.0));
    assert_eq!(pour(0.0, 499, false), (1.0, 0.0));
}

#[test]
fn a_crack_shakes_once_and_settles() {
    assert_eq!(crack(0.0, true), 0.0);
    assert!(crack(CRACK_MS as f32 / 3.0, true) < -1.9);
    assert!(crack(2.0 * CRACK_MS as f32 / 3.0, true) > 1.9);
    assert_eq!(crack(CRACK_MS as f32, true), 0.0);
    assert_eq!(crack(40.0, false), 0.0);
}

#[test]
fn cooling_starts_at_the_peak_and_ends_clear() {
    assert!((cool(0.0, true) - COOL_PEAK).abs() < 1e-6);
    assert_eq!(cool(COOL_MS as f32, true), 0.0);
    assert_eq!(cool(0.0, false), 0.0);
}

#[test]
fn the_wheel_blooms_from_small_and_turned() {
    assert_eq!(bloom(0.0, true), (0.0, BLOOM_SCALE, BLOOM_TURN_DEG));
    assert_eq!(bloom(BLOOM_MS as f32, true), (1.0, 1.0, 0.0));
    assert_eq!(segment(40.0 + 3.0 * SEGMENT_STAGGER_MS as f32, 3, true).0, 0.0);
    assert_eq!(bloom(0.0, false), (1.0, 1.0, 0.0));
}

#[test]
fn the_frame_timer_runs_only_when_needed() {
    assert_eq!(frame_ms(true, false, true), Some(16));
    assert_eq!(frame_ms(false, true, true), Some(50));
    assert_eq!(frame_ms(false, true, false), None);
    assert_eq!(frame_ms(false, false, true), None);
}
```

- [ ] **Step 2: Run them and see them fail.**
  - Run: `cargo test -p kiln-app anim`
  - Expected: compile error, because there's no `anim` module.
- [ ] **Step 3: Implement `anim.rs`.**
  - It builds on `theme::motion::{ease_out, progress}`.
  - With `animations` false, every function returns its end value.
  - Then move `win/mod.rs` onto `anim::Tween`, `anim::eased` and
    `anim::within`, passing `Instant::now()` and `motion::animations()`.
  - Replace the timer choice in `animate()` and the `WM_TIMER` arm with
    `anim::frame_ms`.
- [ ] **Step 4: Run the tests.**
  - Run: `cargo test -p kiln-app`
  - Expected: PASS, including the existing `sync_kind` timer tests.
- [ ] **Step 5: Run the gates, then commit.**

```bash
git add crates/kiln-app/src/anim.rs crates/kiln-app/src/lib.rs crates/kiln-app/src/win/mod.rs
git commit -m "Motion: one engine, driven by time, with the catalogue's values"
```

---

### Task 3: What the inspector says

**Files:**
- Modify: `crates/kiln-core/src/suggest.rs` (`Habits::ranked`)
- Create: `crates/kiln-app/src/inspector.rs`, and add `pub mod inspector;`
  to `lib.rs`
- Modify: `crates/kiln-app/src/model.rs`:
  - `FocusButton` gains `Inspector(InspectorControl)`
  - `tab_stop` gains the inspector's stops
  - add `Entry::detail_parts`

**Interfaces:**
- Consumes (existing, unchanged):
  - `model::{Entry, FileList, JobState, human_size, time_left, left_text}`
  - `kiln_core::{describe_format, suggest::about, Kind}`
  - `wheel::{offer_sentence, Offer, WheelKind}`
  - `a11y::percent`
- Produces:
  - In `kiln_core::suggest`: `impl Habits { pub fn ranked(&self, from: &str) -> Vec<String> }`
    lists targets picked from `from`, the most picked first, with ties in
    alphabetical order. Case and a leading dot in `from` are ignored, as in
    `record`.
  - In `inspector`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InspectorControl { Target(usize), More, Cancel, Save, ShowInFolder, Retry, ShowDetails }

#[derive(Debug, Clone, PartialEq)]
pub struct Target { pub ext: String, pub why: Option<&'static str>, pub estimate: Option<u64>, pub suggested: bool }

#[derive(Debug, Clone, PartialEq)]
pub enum Inspector {
    Nothing,
    Waiting { row: usize, what: Option<&'static str>, facts: Vec<(&'static str, String)>, targets: Vec<Target>, more: bool },
    Converting { row: usize, percent: Option<u32>, left: Option<String> },
    Done { row: usize, result: String, change: Option<String>, saved: bool },
    Failed { row: usize, sentence: String, retry: bool },
    Several { rows: Vec<usize>, kinds: String, targets: Vec<Target>, none_shared: Option<String> },
}

pub const MAX_TARGETS: usize = 3;
pub struct Lookups<'a> { pub kind_of: &'a dyn Fn(&Entry) -> Option<Kind>, pub size_of: &'a dyn Fn(&Path) -> Option<u64> }
pub fn inspect(files: &FileList, look: &Lookups, habits: &Habits) -> Inspector;
impl Inspector { pub fn controls(&self) -> Vec<InspectorControl>; }        // reading order
impl Target { pub fn name(&self) -> String; }                              // "Convert to WEBP, a small image for the web, about 410 KB"
pub fn kinds_line(kinds: &[Option<Kind>]) -> String;                       // "3 images, 1 video"
pub fn change_line(before: u64, after: u64) -> String;                     // "2.4 MB → 410 KB, 83% smaller"
```

  - In `model`:
    - `pub fn tab_stop(at, back, listed, export_all, convert, inspector: &[InspectorControl]) -> Option<FocusButton>`.
      The order is Help, Settings, the list, the inspector's controls,
      then the footer.
    - `impl Entry { pub fn detail_parts(&self) -> (String, Option<String>) }`
      gives the row's facts and its suggestion ("as WEBP about 410 KB"), so
      the renderer can draw the suggestion in glaze ink. Joined with
      " · ", the two parts equal `detail()`.

- [ ] **Step 1: Write the failing tests.**
  - In `suggest.rs`: `ranked_lists_the_most_picked_first`. Record
    png→jpg ×3, png→avif ×5 and png→gif ×3, then assert that
    `ranked("PNG") == ["avif", "gif", "jpg"]` and `ranked("mp4")` is empty.
  - In `inspector.rs`, using `model`'s test row style (build `Entry`
    directly, `FileList::default()`, `insert`, `select_only`):

```rust
#[test]
fn one_waiting_file_offers_the_suggestion_then_habits_then_the_rest() {
    // harbour.png, 2.4 MB, 1600 × 1200, targets jpg webp avif gif pdf,
    // suggested WEBP about 410 KB, habits png→avif ×3.
    let i = inspect(&list, &look, &habits);
    let Inspector::Waiting { what, facts, targets, more, .. } = i else { panic!() };
    assert_eq!(what, Some("a lossless image with transparency"));
    assert_eq!(facts[0], ("Size", "2.4 MB".to_owned()));
    assert_eq!(facts[1], ("Pixels", "1600 × 1200".to_owned()));
    assert_eq!(facts.last().unwrap().0, "Folder");
    let exts: Vec<_> = targets.iter().map(|t| t.ext.as_str()).collect();
    assert_eq!(exts, ["webp", "avif", "jpg"]);
    assert!(targets[0].suggested && !targets[1].suggested);
    assert_eq!(targets[0].estimate, Some(410_000));
    assert!(more);
    assert_eq!(targets[0].name(), "Convert to WEBP, a small image for the web, about 410 KB");
}

#[test]
fn a_converting_file_shows_its_percent() { /* Running, fraction 0.38 → percent Some(38) */ }

#[test]
fn a_done_file_shows_before_and_after() {
    assert_eq!(change_line(2_400_000, 410_000), "2.4 MB → 410 KB, 83% smaller");
    assert_eq!(change_line(1_000_000, 1_120_000), "1 MB → 1.1 MB, 12% larger");
    assert_eq!(change_line(1_000_000, 1_004_000), "1 MB → 1 MB, about the same size");
    // Done row, held: result is the output's file name, saved false;
    // size_of gives the after size.
}

#[test]
fn a_failed_file_says_why_and_offers_retry_when_it_may_help() { /* sentence == e.line().0; retry == e.retryable() */ }

#[test]
fn several_files_name_their_kinds_most_first() {
    use Kind::*;
    assert_eq!(kinds_line(&[Some(Image), Some(Video), Some(Image), Some(Image)]), "3 images, 1 video");
    assert_eq!(kinds_line(&[Some(Audio), None]), "1 audio file, 1 other file");
}

#[test]
fn several_files_with_nothing_shared_explain_it() {
    // an MP4 and a DOCX with disjoint targets
    // → targets empty, none_shared == offer_sentence(Convert, Mixed(..))
}

#[test]
fn nothing_selected_is_nothing() { assert_eq!(inspect(&FileList::default(), &look, &habits), Inspector::Nothing); }

#[test]
fn controls_follow_the_reading_order() {
    // Waiting with 3 targets and more → [Target(0), Target(1), Target(2), More]
    // Converting → [Cancel]; Done held → [Save]; Done saved → [ShowInFolder];
    // Failed retryable → [Retry, ShowDetails]
}
```

  - In `model.rs` tests:
    - `tab_runs_header_list_inspector_footer`: with
      `inspector = [Target(0), More]`, Tab from the list goes to
      `Inspector(Target(0))`, then `Inspector(More)`, then `Convert`.
      Shift+Tab from `Convert` goes back to `Inspector(More)`.
    - `detail_parts_split_the_suggestion_off`: for a row with a convert
      suggestion, the second part starts with "as ", and the two parts
      joined with " · " equal `detail()`.
  - Update every existing `tab_stop` call in tests and in `win/mod.rs` to
    pass `&[]` for now. Task 8 passes the real list.

- [ ] **Step 2: Run them and see them fail.**
  - Run: `cargo test -p kiln-core ranked` and `cargo test -p kiln-app inspector`
  - Expected: compile errors.
- [ ] **Step 3: Implement.**
  - **Waiting:** target order is:
    1. the suggestion, when it's `Suggested::Convert(ext)` and `ext` is in
       `targets`
    2. `habits.ranked(code)` among the targets
    3. the rest of `targets` in order

    Take the first `MAX_TARGETS`; `more` is true when there are more. A
    non-suggested target's estimate is `facts.estimates.get(ext)`.
  - **Facts:**
    - Size: `human_size(e.size)`
    - Pixels: `"{w} × {h}"` from `facts.picture`
    - Length: from `facts.media.duration`, in the format `describe`
      already uses
    - Folder: `e.folder_name()`

    Size and Folder are always there; Pixels and Length only when known.
  - **Converting:** `percent` comes from `a11y::percent(fraction)`; `left`
    from `time_left` and `left_text`.
  - **Done:** `result` is the first output's file name. `change` is
    `change_line(e.size, size_of(output))` when the size is known.
    `saved` is `saved.is_some()`.
  - **Several:** `rows` is the first 3 selected. `kinds` comes from
    `kind_of`, with the singular and plural words in the test.
  - **Shared targets:** the intersection in the first row's order, the
    first 3, none suggested. When it's empty, `none_shared` is
    `offer_sentence(WheelKind::Convert, &Offer::Mixed(codes))`, where
    codes are (code, count) most first.
- [ ] **Step 4: Run the tests and the gates.** Expected: PASS.
- [ ] **Step 5: Commit**

```bash
git add crates/kiln-core/src/suggest.rs crates/kiln-app/src/inspector.rs crates/kiln-app/src/lib.rs crates/kiln-app/src/model.rs crates/kiln-app/src/win/mod.rs
git commit -m "Inspector: what it says for each selection, and its place in the Tab order"
```

---

### Task 4: Geometry: the split, the fold, row previews and the inspector

**Files:**
- Modify: `crates/kiln-app/src/layout.rs`

**Interfaces:**
- Produces:
  - **Constants:**
    - `INSPECTOR_WIDTH = 300.0` and `INSPECTOR_FROM = 760.0`
    - `MAX_CONTENT_WIDTH_WIDE = 1180.0`. `MAX_CONTENT_WIDTH` stays 880.
    - `PREVIEW_ROW = 34.0`, `PREVIEW_RADIUS = 6.0` and
      `HERO = (268.0, 120.0)`
    - `TARGET_HEIGHT = 36.0` and `FACT_HEIGHT = 18.0`
  - **Layout fields:** `Layout` gains `pub list: Rect` (the list's part of
    the panel) and `pub inspector: Option<Rect>`. `rows` take `list`'s
    width.
  - **Fold:**
    - `pub fn inspector_fits(w: f32) -> bool` is `w >= INSPECTOR_FROM`.
    - `pub fn layout_with(w, h, rows, shown: f32) -> Layout`, where
      `shown` runs 0 to 1. The inspector is `INSPECTOR_WIDTH * shown` wide
      and slides in from the right edge. With rows 0, or `shown` 0, it's
      `None`.
    - `layout(w, h, rows)` becomes
      `layout_with(w, h, rows, inspector_fits(w) as u8 as f32)`.
    - `w` is in zoomed DIPs, as `layout()` already receives it, so a large
      Text size folds the inspector sooner.
    - The column cap is `MAX_CONTENT_WIDTH_WIDE` when `shown > 0`, else
      `MAX_CONTENT_WIDTH`.
  - **Rows:**
    - `pub fn row_preview(row: &Rect) -> Rect` sits at `x + 12`, centred
      vertically, `PREVIEW_ROW` square.
    - The chip and text start 12 DIPs after the preview.
  - **The inspector's own layout:**

```rust
pub enum InspectorShape { Nothing, Waiting { what_h: f32, facts: usize, targets: usize, more: bool },
    Converting, Done { change: bool }, Failed { sentence_h: f32, retry: bool }, Several { targets: usize, none_h: f32 } }
pub struct InspectorLayout { pub hero: Rect, pub title: Rect, pub body: Rect, pub facts: Vec<Rect>,
    pub label: Option<Rect>, pub targets: Vec<Rect>, pub more: Option<Rect>, pub buttons: Vec<Rect> }
pub fn inspector_layout(r: &Rect, shape: &InspectorShape) -> InspectorLayout;
```

  - **Inspector stacking**, inside `r.inset(16.0)`, top down:
    - hero `HERO`
    - 12 gap, then title 24
    - 8 gap, then body: `what_h`, `sentence_h` or `none_h`; in Converting
      it holds the 40 DIP percentage line and a 20 DIP time line
    - 12 gap, then facts at `FACT_HEIGHT` each
    - 12 gap, then the "Turn it into" label at 16
    - 6 gap, then targets at `TARGET_HEIGHT` with 6 between
    - 6 gap, then More at 24
    - 12 gap, then buttons at `BUTTON_HEIGHT`, left to right, 8 apart, each
      96 wide
    - Any part that doesn't fit whole inside `r.inset(16.0)` is left out,
      as are the parts after it.
  - **Empty state:** `EmptyState` gains `pub glazes: [Rect; 4]`, four
    64 × 64 tiles, 12 apart and centred, between the privacy line and the
    bottom. The vessel grows to 72 × 72. Update `PARTS` so everything still
    centres.

- [ ] **Step 1: Write the failing tests** in `layout.rs`:

```rust
#[test]
fn the_inspector_shows_from_760_and_takes_300() {
    let narrow = layout(759.0, 560.0, 3);
    assert!(narrow.inspector.is_none());
    assert_eq!(narrow.list, narrow.panel);
    let wide = layout(760.0, 560.0, 3);
    let i = wide.inspector.unwrap();
    assert_eq!(i.w, INSPECTOR_WIDTH);
    assert_eq!(i.right(), wide.panel.right());
    assert_eq!(wide.list.w + 1.0 + i.w, wide.panel.w);
    assert_eq!(wide.rows[0].w, wide.list.w);
}

#[test]
fn a_wide_window_caps_the_content_at_1180() {
    let l = layout(2000.0, 900.0, 3);
    assert_eq!(l.body.w, MAX_CONTENT_WIDTH_WIDE);
    assert!(l.list.w >= 879.0);
}

#[test]
fn half_shown_slides_from_the_right() {
    let l = layout_with(1000.0, 600.0, 3, 0.5);
    assert_eq!(l.inspector.unwrap().w, INSPECTOR_WIDTH * 0.5);
}

#[test]
fn the_empty_window_has_no_inspector() { assert!(layout(1000.0, 600.0, 0).inspector.is_none()); }

#[test]
fn the_row_preview_sits_at_the_left_on_the_grid() {
    let row = Rect::new(16.0, 72.0, 600.0, ROW_HEIGHT);
    let p = row_preview(&row);
    assert_eq!((p.x, p.w, p.h), (28.0, PREVIEW_ROW, PREVIEW_ROW));
    assert_eq!(p.y - row.y, row.bottom() - p.bottom());
}

#[test]
fn a_short_inspector_drops_what_does_not_fit_whole() {
    let r = Rect::new(900.0, 72.0, 300.0, 260.0);
    let l = inspector_layout(&r, &InspectorShape::Waiting { what_h: 40.0, facts: 3, targets: 3, more: true });
    let inner = r.inset(16.0);
    for t in l.targets.iter().chain(l.more.iter()).chain(l.facts.iter()) {
        assert!(t.y >= inner.y && t.bottom() <= inner.bottom());
    }
    assert!(l.targets.len() < 3);
}

#[test]
fn the_empty_window_shows_four_glaze_tiles_centred() {
    let e = empty_state(&Rect::new(16.0, 72.0, 688.0, 380.0));
    let span = e.glazes[3].right() - e.glazes[0].x;
    assert!((e.glazes[0].x - 16.0 - (688.0 - span) / 2.0).abs() < 1.0);
    assert_eq!(e.vessel.w, 72.0);
}
```

  The existing grid tests (several window sizes, nothing overlapping at
  480 × 360) keep passing and extend to 1200 × 360 and 1600 × 900.

- [ ] **Step 2: Run them and see them fail.**
  - Run: `cargo test -p kiln-app layout`
  - Expected: compile errors for the new names.
- [ ] **Step 3: Implement** the signatures above. `win/mod.rs` keeps
  calling `layout()`; Task 9 moves it to `layout_with`.
- [ ] **Step 4: Run the tests and the gates.** Expected: PASS.
- [ ] **Step 5: Commit**

```bash
git add crates/kiln-app/src/layout.rs
git commit -m "Layout: the inspector, its fold, row previews and the empty window's glazes"
```

---

### Task 5: The preview cache

**Files:**
- Create: `crates/kiln-app/src/previews.rs`, and add `pub mod previews;`
  to `lib.rs`

**Interfaces:**
- Produces:

```rust
pub const CACHE_CAP: usize = 64 * 1024 * 1024;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)] pub enum PreviewSize { Row, Hero }
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PreviewKey { pub path: PathBuf, pub modified: Option<SystemTime>, pub size: PreviewSize, pub pixels: (u32, u32) }
impl PreviewKey { pub fn id(&self) -> u64; }                 // stable hash, the renderer's bitmap key
pub struct Pixels { pub width: u32, pub height: u32, pub bgra: Vec<u8> }   // premultiplied, top-down
pub enum Preview { Ready(Arc<Pixels>), None }               // None: Windows has no thumbnail; draw the glaze tile
pub struct PreviewCache { /* LRU by bytes */ }
impl PreviewCache {
    pub fn new(cap: usize) -> Self;
    pub fn get(&mut self, key: &PreviewKey, now: Instant) -> Option<(&Preview, Instant)>; // and when it arrived
    pub fn put(&mut self, key: PreviewKey, p: Preview, now: Instant);
    pub fn pending(&mut self, key: &PreviewKey) -> bool;    // true once per key until put: ask the worker once
    pub fn keep_paths(&mut self, listed: &dyn Fn(&Path) -> bool); // drops entries and pendings for paths no longer listed
    pub fn bytes(&self) -> usize;
}
pub fn pixel_size(size: PreviewSize, px_per_dip: f32) -> (u32, u32); // Row 34², Hero 268×120, × scale, rounded up
pub fn request_order(selected: Option<usize>, visible: Range<usize>, total: usize) -> Vec<(usize, PreviewSize)>;
pub fn cover_source(img: (u32, u32), dst: (f32, f32)) -> (f32, f32, f32, f32); // centred crop that fills dst
pub fn fix_alpha(bgra: &mut [u8]);                           // all alpha 0 → 255
```

- [ ] **Step 1: Write the failing tests:**

```rust
#[test]
fn the_cache_holds_its_cap_and_drops_the_least_recently_used() {
    // cap 3 × 100 bytes; put a, b, c; get a; put d → b is gone, a stays.
}

#[test]
fn the_cache_key_carries_the_pixel_size() {
    assert_eq!(pixel_size(PreviewSize::Row, 1.0), (34, 34));
    assert_eq!(pixel_size(PreviewSize::Row, 1.25), (43, 43));
    assert_eq!(pixel_size(PreviewSize::Hero, 2.0), (536, 240));
    // same path and time at two sizes → two keys, two ids
}

#[test]
fn a_changed_file_is_asked_for_again() {
    // same path, modified t1 then t2 → different keys; pending(t2) is true
}

#[test]
fn pending_asks_once() {
    // pending(k) true, then false, until put(k)
}

#[test]
fn a_preview_for_a_file_no_longer_listed_is_dropped() {
    // put a and b; keep_paths(|p| p == a) → get(b) is None, bytes counts a only
}

#[test]
fn request_order_puts_the_selection_first_and_drops_stale_heroes() {
    let o = request_order(Some(250), 240..252, 500);
    assert_eq!(o[0], (250, PreviewSize::Hero));
    assert_eq!(o[1], (250, PreviewSize::Row));
    assert!(o[2..14].iter().all(|(i, s)| (240..252).contains(i) && *s == PreviewSize::Row));
    assert_eq!(o.iter().filter(|(_, s)| *s == PreviewSize::Hero).count(), 1);
    assert!(o.len() <= 2 + 12 + 24);  // visible, then 12 below and 12 above
}

#[test]
fn cover_source_crops_the_centre() {
    assert_eq!(cover_source((400, 300), (268.0, 120.0)), (0.0, 60.5, 400.0, 179.0)); // rounded to 0.5
    assert_eq!(cover_source((34, 34), (34.0, 34.0)), (0.0, 0.0, 34.0, 34.0));
}

#[test]
fn an_all_clear_bitmap_is_made_opaque() {
    let mut px = vec![10, 20, 30, 0, 40, 50, 60, 0];
    fix_alpha(&mut px);
    assert_eq!(px, [10, 20, 30, 255, 40, 50, 60, 255]);
    let mut keep = vec![10, 20, 30, 0, 40, 50, 60, 128];
    fix_alpha(&mut keep);
    assert_eq!(keep[3], 0);
}
```

- [ ] **Step 2: Run them and see them fail.**
  - Run: `cargo test -p kiln-app previews`
  - Expected: compile error.
- [ ] **Step 3: Implement.**
  - LRU with a `VecDeque` of keys and a `HashMap`. Bytes count
    `bgra.len()`; `Preview::None` counts 64.
  - `request_order`:
    1. the selection's Hero, then its Row
    2. the visible rows, top down
    3. then up to 12 below the visible range, then up to 12 above it

    No duplicates.
  - `cover_source` scales `dst` to fit inside `img`, centres it, and
    rounds each value to 0.5.
- [ ] **Step 4: Run the tests and the gates.** Expected: PASS.
- [ ] **Step 5: Commit**

```bash
git add crates/kiln-app/src/previews.rs crates/kiln-app/src/lib.rs
git commit -m "Previews: a capped cache, the order they're asked for, and the crop"
```

---

### Task 6: Windows thumbnails

**Files:**
- Create: `crates/kiln-app/src/win/thumbs.rs`, with `mod thumbs;` in
  `win/mod.rs`
- Modify: `crates/kiln-app/src/win/mod.rs`. Add a `WM_KILN_PREVIEW`
  message on the next free `WM_APP + n` (check what's taken), a
  `PreviewCache` on the window, and the request and arrival handling.

**Interfaces:**
- Consumes: `previews::{PreviewKey, Pixels, Preview, PreviewCache, pixel_size, request_order, fix_alpha, CACHE_CAP}` (Task 5).
- Produces:
  - `pub fn fetch(path: &Path, pixels: (u32, u32)) -> Option<Pixels>`
    - It calls `SHCreateItemFromParsingName`, then
      `IShellItemImageFactory::GetImage` with
      `SIZE { cx: max(w, h), cy: max(w, h) }` and `SIIGBF_THUMBNAILONLY`.
    - It reads the pixels top-down with `GetDIBits`
      (`BITMAPINFOHEADER` with a negative height, 32 bpp), calls
      `fix_alpha`, and deletes the `HBITMAP`.
    - It returns `None` on any error.
  - `pub struct Thumbs`:
    - `pub fn start(post: PostTarget) -> Thumbs` starts one worker thread
      with COM apartment-threaded.
    - `pub fn ask(&self, keys: Vec<PreviewKey>)` replaces the worker's
      queue with `keys`, in order.
  - The worker posts each `(PreviewKey, Preview)` to `WM_KILN_PREVIEW`
    through `PostTarget::post`.

  How the window uses it:
  - **When it asks:** after layout or scroll, a selection change, files
    added, and a DPI change. It builds keys from `request_order`, reading
    `modified` from `std::fs::metadata`. Keys that `cache.pending` lets
    through go to `thumbs.ask`.
  - **When a preview arrives:** `cache.put(key, preview, Instant::now())`,
    then `invalidate()`, but only if the path is still listed and its
    modified time still matches. Otherwise the preview is dropped.
  - **When rows are removed, cleared or restored:**
    `cache.keep_paths(...)`.
  - **Never:** a call to the Shell on the UI thread.

- [ ] **Step 1: Write the failing test** in `thumbs.rs`, under
  `#[cfg(test)]`:
  - `a_png_gives_a_thumbnail_the_right_size`:
    - Write a 64 × 48 PNG with `tiny_skia::Pixmap::save_png` (a
      dev-dependency) into a `tempfile` folder.
    - `fetch(&path, (34, 34))` must be `Some(px)`, with
      `px.width <= 34 && px.height <= 34`,
      `px.bgra.len() == (px.width * px.height * 4) as usize`, and some
      alpha byte non-zero.
  - `a_missing_file_gives_none`: `fetch(Path::new("C:\\no\\such\\file.png"), (34, 34))`
    is `None`.

  The tests initialise COM themselves.
- [ ] **Step 2: Run them and see them fail.**
  - Run: `cargo test -p kiln-app thumbs`
  - Expected: compile error.
- [ ] **Step 3: Implement** `fetch`, the worker, and the window's wiring
  above. The worker checks the queue between items, so a newer `ask`
  replaces stale keys at once.
- [ ] **Step 4: Run the tests and the gates.** Expected: PASS.
- [ ] **Step 5: Smoke check (controller).**
  - Use the scratch-profile pattern from
    `docs/ux/phase3/scripts/kiln-ux-lib.ps1`. That folder is untracked,
    but it's present on this PC.
  - Start the release build on the phase 3 awkward files, plus 500 PNGs.
  - Check that the UI thread reply holds p95 < 3 ms, measured with
    `SendMessageTimeout WM_NULL`, while previews load.
  - Check that the working set stays under 160 MB.
- [ ] **Step 6: Commit**

```bash
git add crates/kiln-app/src/win/thumbs.rs crates/kiln-app/src/win/mod.rs
git commit -m "Previews: Windows thumbnails on a worker, cached and dropped with their rows"
```

---

### Task 7: Draw the look

**Files:**
- Modify: `crates/kiln-app/src/win/render.rs`
- Modify: `crates/kiln-app/src/win/mod.rs`. Fill the new `View` fields,
  and drop the row link from hit-testing while the inspector shows.

**Interfaces:**
- Consumes:
  - `theme::{Glaze, glaze_of, effects::{CHIP_FILL, CHIP_OUTLINE, GROUND_GLOW, ROW_WASH}, depth::shadow_layers}`
    (Task 1)
  - `layout::{row_preview, Layout.list, Layout.inspector, EmptyState.glazes}`
    (Task 4)
  - `previews::{Preview, Pixels, cover_source, PreviewSize}` (Task 5)
  - `Entry::detail_parts` (Task 3)
- Produces, in `View`:
  - `pub glaze: &'a dyn Fn(usize) -> Glaze`, the glaze of row `i`, from
    `glaze_of(kind_of(entry))`
  - `pub preview: &'a dyn Fn(usize, PreviewSize) -> Option<(Arc<Pixels>, u64, f32)>`,
    giving the pixels, the key id and the fade (0 to 1)
  - `pub row_links: bool`, false while the inspector shows

  The renderer keeps a `HashMap<u64, ID2D1Bitmap>`. It builds a bitmap on
  first draw (`CreateBitmap`, premultiplied BGRA), drops it when its id
  hasn't been drawn for 120 frames, and clears the map on device loss.

What the renderer draws:
- **Rows:**
  - The preview goes in `row_preview`, clipped to `PREVIEW_RADIUS`, drawn
    with `cover_source` at the preview's fade. With no pixels, or
    `Preview::None`, it draws the glaze tile: a `chip_fill` of the glaze,
    with the kind's icon (`wheel::icon_for_kind`) in the glaze at 0.75
    scale.
  - The chip is an opaque `chip_fill`, with a 1 px glaze outline at
    `CHIP_OUTLINE` and text in `glaze_ink`.
  - The second line draws `detail_parts().0` in the muted tone, then
    " · " and `.1` in `glaze_ink`.
  - A converting row has a horizontal wash: the glaze at `ROW_WASH` at the
    left, fading to clear at 60% of the row. Its heat line uses the glaze,
    with the existing breath.
  - Without `row_links`, the row draws no link and `row_part` gets
    `link: false`.
- **Depth:** the panel, the footer's primary button, sheets and the wheel
  each draw `shadow_layers()` as rounded rectangles in `p.shadow`, offset
  `OFFSET_Y`, and a 1 px top highlight in `p.highlight`. In a contrast
  theme, neither is drawn.
- **Ground glow:** an `ID2D1RadialGradientBrush`, a device resource made
  with the grain, centred at (width / 2, −0.1 × height), with radii 1.2w
  and 0.7h. It runs from celadon at `GROUND_GLOW` to clear, drawn over the
  ground under the grain. It isn't drawn in a contrast theme.
- **Empty window:** `VESSEL_PATH` at 72 DIPs in celadon with a 2 DIP
  stroke, and the four glaze tiles from `EmptyState.glazes`: the kind's
  icon over a `chip_fill`, each with its word (Images, Video, Audio,
  Documents) in 12 px glaze ink under it. The tagline stays out.

- [ ] **Step 1: Write the visual checklist.** This task only draws, so it
  adds no unit test: Tasks 1, 4 and 5 test every value it uses, and step 3
  checks the result. Extend `VISUAL_TESTING.md`'s checklist, which is the
  controller's visual gate here, with:
    - previews in rows
    - glaze chips in both themes
    - the converting wash
    - depth on the panel
    - the empty window's tiles
- [ ] **Step 2: Implement the drawing above.**
  - `cargo build -p kiln-app --release` must succeed.
  - `cargo test -p kiln-app` must pass.
- [ ] **Step 3: Visual check (controller).**
  - Run the release build with a scratch profile and the phase 3 awkward
    files, in both themes and at the minimum size.
  - Take screenshots with `PrintWindow` from the phase 3 harness.
  - Check them against Global Constraints and `VISUAL_TESTING.md`.
  - Rerun `sc01-startup-flash.ps1`: no white, visible by 100 ms.
- [ ] **Step 4: Run the gates, then commit.**

```bash
git add crates/kiln-app/src/win/render.rs crates/kiln-app/src/win/mod.rs crates/kiln-app/src/model.rs VISUAL_TESTING.md
git commit -m "Look: previews, glaze chips, depth and the empty window"
```

---

### Task 8: The inspector on screen, by pointer, keyboard and Narrator

**Files:**
- Modify: `crates/kiln-app/src/win/render.rs` (a new `InspectorView`)
- Modify: `crates/kiln-app/src/win/mod.rs` (state, hit-testing, actions,
  Tab, focus)
- Modify: `crates/kiln-app/src/a11y.rs` (`Input.inspector`, ids, the tree)
- Modify: `crates/kiln-app/src/win/uia.rs` only if a new role needs
  mapping

**Interfaces:**
- Consumes:
  - `inspector::{inspect, Inspector, InspectorControl, Lookups, Target}`
    (Task 3)
  - `layout::{inspector_layout, InspectorShape, InspectorLayout}` (Task 4)
  - `model::tab_stop(.., inspector)` (Task 3)
  - previews at `PreviewSize::Hero` (Tasks 5 and 6)
- Produces:
  - **In `a11y`:**
    - `pub struct InspectorIn<'a> { pub rect: Rect, pub model: &'a Inspector, pub layout: &'a InspectorLayout, pub file: &'a str }`
    - `Input.inspector: Option<InspectorIn<'a>>`
    - `Id::Inspector`, `Id::InspectorControl(InspectorControl)` and
      `Id::InspectorPreview`
    - `pub fn inspector_name(c: InspectorControl, model: &Inspector, file: &str) -> String`
  - **Names:**

    | Control | Name |
    |---|---|
    | Pane | "Selected file" |
    | Preview | "Preview of harbour.png" |
    | Target | `Target::name()` |
    | More | "More formats for harbour.png" |
    | Cancel | "Cancel harbour.png" |
    | Save | "Save harbour.webp" |
    | Show in folder | "Show harbour.webp in folder" |
    | Retry | "Retry harbour.png" |
    | Show details | "Show details for harbour.png" |

    The texts (what, facts, change, sentence) are Text nodes.

What the window does:
- **Building it:** the window builds `Inspector` with `inspect()`
  whenever the selection, a row's job, facts or outputs change. It builds
  `InspectorLayout` from `InspectorShape`, measuring `what_h` and
  `sentence_h` with the renderer's text measure at the body width, at
  13 px, at most 4 lines.
- **Drawing it:**
  - the hero preview, clipped to 8 DIP corners
  - the chip and the file name at `INSPECTOR_TITLE`, semibold
  - the body in 13 px muted text
  - facts as label (muted) and value (text), right-aligned with tabular
    figures
  - "Turn it into" as a 12 px muted label
  - each target as a 6 DIP rounded row: chip, why (muted), and the
    estimate. The suggested target gets a celadon edge at 55%, a celadon
    wash at 9%, and the suggestion dot.
  - More as a link
  - buttons in the existing button face
  - Converting: the percentage in 32 px Segoe UI Variable semibold with
    tabular figures (Fraunces stays in its three places), and the time in
    13 px muted. The hero gets the glaze heat rising from the bottom to
    `percent` height, with the breath.
  - Done: a celadon check on the hero, then the result and the change
    line.
  - Several: up to three row previews stacked in the hero's place, each
    offset 12 DIPs right and 8 down from the one behind it. Then the
    kinds line as the title, and the shared targets or the `none_shared`
    sentence.
  - Nothing: the vessel at 48 DIPs in the hero's place. Then the title
    "Select a file to see what Kiln can do with it", and the body with the
    empty window's key hint and its privacy line ("Everything converts on
    this PC. Nothing is uploaded.").
  - A top wash of the selection's glaze at 7%, fading out over 40% of the
    inspector's height.
- **Actions:**
  - `Target(i)` runs `start(Action::Convert(ext), paths, None)` for the
    inspected rows, and records the pick in habits the way a wheel choice
    does.
  - `More` opens the format choice the footer's Convert opens.
  - Cancel, Save, ShowInFolder, Retry and ShowDetails call the existing
    row handlers for that row.
  - Enter or Space on a focused control does the same.
- **Tab and focus:**
  - Tab follows `tab_stop` with `Inspector::controls()`.
  - The focus ring is drawn 3 DIPs outside the focused control.
  - When the inspector's content changes, a focus that no longer exists
    falls back to the list.
- **The tree:** with the inspector shown, the tree has a "Selected file"
  pane after the list, with the nodes above.

- [ ] **Step 1: Write the failing tests** in `a11y.rs`:

```rust
#[test]
fn the_inspector_is_a_pane_after_the_list_with_named_controls() {
    // Input with a Waiting inspector for harbour.png, targets webp (suggested, 410 KB), avif, jpg, more.
    let t = tree(&input);
    let pane = t.find(&Id::Inspector).unwrap();
    assert_eq!(pane.name, "Selected file");
    let first = t.find(&Id::InspectorControl(InspectorControl::Target(0))).unwrap();
    assert_eq!(first.role, Role::Button);
    assert_eq!(first.name, "Convert to WEBP, a small image for the web, about 410 KB");
    assert_eq!(t.find(&Id::InspectorPreview).unwrap().name, "Preview of harbour.png");
    // The pane comes after the list among the root's children.
}

#[test]
fn inspector_buttons_are_named_with_their_file() {
    assert_eq!(inspector_name(InspectorControl::Cancel, &converting, "interview.mp4"), "Cancel interview.mp4");
    assert_eq!(inspector_name(InspectorControl::Save, &done_held, "voice memo.m4a"), "Save voice memo.mp3");
}

#[test]
fn a_narrow_window_has_no_inspector_in_the_tree() { /* Input.inspector None → find(Id::Inspector) is None */ }
```

- [ ] **Step 2: Run them and see them fail.**
  - Run: `cargo test -p kiln-app a11y`
  - Expected: compile errors.
- [ ] **Step 3: Implement** the tree, the drawing, hit-testing, actions,
  Tab and focus described above.
- [ ] **Step 4: Run the tests and the gates.** Expected: PASS.
- [ ] **Step 5: UIA check (controller).**
  - Rerun `sc10-chrome.ps1` and `sc90-keyboard-task.ps1`.
  - The pane and its named buttons appear in the dump.
  - The main task by keyboard still completes. One added Tab stop is
    allowed, and `sc90`'s expected step count is updated in its log if
    so.
  - Enter on the first target converts.
- [ ] **Step 6: Commit**

```bash
git add crates/kiln-app/src/win/render.rs crates/kiln-app/src/win/mod.rs crates/kiln-app/src/a11y.rs crates/kiln-app/src/win/uia.rs
git commit -m "Inspector: drawn, clickable, on the Tab path and named for Narrator"
```

---

### Task 9: Motion, everywhere

**Files:**
- Modify: `crates/kiln-app/src/win/mod.rs` (state per moment)
- Modify: `crates/kiln-app/src/win/render.rs` (transforms and opacities)

**Interfaces:**
- Consumes: the `anim` catalogue (Task 2), `layout_with` (Task 4),
  previews' arrival time (Task 5).

What moves:

| Moment | State on the window | What the renderer applies |
|---|---|---|
| Rows added | `RowState.order: usize`, the index within its add batch | `pour(elapsed, order)` |
| Selection moves (single selection to single selection) | `selection_y: Tween` | the wash and edge drawn at the tweened y; a multi-selection draws at once |
| Inspector content | `inspector_since: Instant`, and `inspector_prev: Option<(Inspector, InspectorLayout)>` kept for `CROSSFADE_MS / 2` | the old content at alpha `1 - 2t`; the new one via `rise(.., CROSSFADE_MS, CROSSFADE_RISE)` |
| Preview arrives | the arrival time from `PreviewCache::get` | the bitmap's alpha `rise(.., PREVIEW_MS, 0.0).0` |
| Finished | `RowState.settled`, whose length becomes `COOL_MS` | the glaze wash at `cool(elapsed)`, and the check mark's alpha |
| Failed | `RowState.cracked: Option<Instant>`, set when the job turns `Failed` | the row content offset by `crack(elapsed)` |
| Wheel opens | the existing `opened` | `bloom()` for the disc; `segment(.., i)` per segment |
| Sheets | the existing `opened` | `rise(.., SHEET_MS, SHEET_RISE)`, with the scrim's alpha by the same t |
| Buttons | `button_hover: HashMap<FocusButton, Tween>`, and one for pressed | the fill moves `PRIMARY_HOVER` by hover; press uses the inner shadow at the pressed amount, over `PRESS_MS` |
| Fold | `inspector_shown: Tween`, set to `inspector_fits(w)` over `FOLD_MS` | `layout_with(w, h, rows, shown)` everywhere the window calls `layout()` |

- `moving()` includes every row above.
- `frame_ms` drives the timer.
- With `motion::animations()` false, every row in the table is at its end.

- [ ] **Step 1: Write the failing tests** in `win/mod.rs`'s test module.
  They're pure, on the helpers the window uses:
  - `moving_stops_when_every_motion_has_ended`:
    - Build the window's motion state as a plain struct, with
      `fn moving_at(&self, now: Instant) -> bool`, extracted for the test.
    - It's true 100 ms after an add.
    - It's false at `POUR_STAGGER_CAP_MS + POUR_MS` after the add.
    - It's false at once with animations off.
  - `the_fold_follows_the_width`: starting from 1.0, after a resize to
    700 wide the target is 0.0, reached at `FOLD_MS`.
- [ ] **Step 2: Implement the table above.**
  - `cargo test -p kiln-app` must pass.
  - The existing `sync_kind` and timer tests must still pass.
- [ ] **Step 3: Motion check (controller).**
  - Run a release build with a scratch profile.
  - Add 30 files, convert one, fail one (a 0-byte PNG), open the wheel,
    open Settings, and resize across 760.
  - Record frames with the phase 3 harness's screen polling. The timings
    must match the catalogue within one frame.
  - Rerun `sc61-load-latency.ps1`: a key reaches the screen in under
    35 ms (median) with 4 conversions running.
  - The owner checks with Animation effects off.
- [ ] **Step 4: Run the gates, then commit.**

```bash
git add crates/kiln-app/src/win/mod.rs crates/kiln-app/src/win/render.rs
git commit -m "Motion: pour, glide, cross-fade, cool, crack, bloom, sheets, buttons and the fold"
```

---

### Task 10: Documents and the speed check

**Files:**
- Modify: `DESIGN.md`, `VISUAL_TESTING.md`, `README.md`, `CHANGELOG.md`

- [ ] **Step 1: Update `DESIGN.md`.**
  - Principle 4 and a new "Motion" section with the catalogue table, which
    replaces "Animations last 120 to 180 ms".
  - "The inspector": what it shows per case, the fold, and its Tab
    position.
  - "Previews": Windows thumbnails, the glaze tile fallback, and the cache
    cap.
  - Shape and space: the 1180 cap with the inspector, the row preview, and
    the hero size.
  - Icons: note that the kinds' icons double as preview fallbacks, and the
    72 DIP empty-state vessel.
- [ ] **Step 2: Update `VISUAL_TESTING.md`** with the fold point at 760,
  previews, glazes in both themes and High Contrast, and motion with
  Animation effects off.
- [ ] **Step 3: README and CHANGELOG.**
  - README, under the features: "The file list shows a preview of every
    file, and a panel that describes the selected file and what Kiln can
    turn it into."
  - CHANGELOG, under the next version: "A new look: previews, a colour for
    each kind of file, a panel for the selected file, and smoother motion."
- [ ] **Step 4: Speed check (controller).**
  - Rerun `sc01`, `sc50`, `sc61` and `sc90` from
    `docs/ux/phase3/scripts`.
  - Record the numbers against Global Constraints' speed limits in the
    task report.
  - Any miss goes back to its task as a finding.
- [ ] **Step 5: Run the gates, then commit.**

```bash
git add DESIGN.md VISUAL_TESTING.md README.md CHANGELOG.md
git commit -m "Look and motion: design notes, visual checks, README and CHANGELOG"
```
