# Look and motion: design

Date: 2026-10-23. Status: the design was approved in conversation, section
by section. This written spec still needs review.

This is the first of six pieces that make Kiln feel like a finished product.
The others follow it, each with its own spec, and are drawn with what this
one builds:

1. **Look and motion** (this spec)
2. Kiln's own right-click menus
3. Formats explained: what each format is and what it's good for, and a
   Format guide
4. A first-run welcome
5. Recent conversions
6. Presets

Builds on the phase 3 redesign (`docs/ux/phase3/redesign-spec.md`, kept out
of git by the owner's choice), which made converting visible, honest and
finishable, and left "a different visual identity" out of scope. The mockups the owner chose from are the private
page "Kiln Look Studies" (Study B and the Motion section).

## Goal

Kiln works, but it looks hollow: flat grey panels, a list of names with no
pictures, and one colour for everything. After this piece:

- Every file shows a preview, so a list of files is recognisable at a glance.
- Each kind of file has its own glaze colour.
- The selected file has a panel that says what it is and what Kiln can turn
  it into.
- Surfaces have depth, and the app moves with weight.

All of this keeps Kiln's identity (soot, bone and celadon, the Fraunces
name, stoneware) and its speed.

## Decisions made

| Question | Decision |
|---|---|
| Visual direction | A richer Kiln identity, not Fluent and not a new identity. |
| Layout | Study B: the list and an inspector panel. Below 760 DIPs wide the panel folds away and the list stands alone (Study A). |
| Colour | A glaze colour per kind of file. Celadon stays the one accent for focus, selection and buttons. |
| Motion | Expressive but quick: 120 to 300 ms, eased out, no bounce, and all of it off with Windows' Animation effects off. |
| Order of the six pieces | Look and motion first, then menus, formats explained, welcome, recent conversions, presets. |

## Layout

### Main window

- **Header:** unchanged. The vessel, "Kiln" in Fraunces, Help and Settings.
- **Body:** at a window 760 DIPs or wider, the panel holds two columns:
  - the file list, filling what's left
  - the inspector, 300 DIPs wide, with a 1 px rule between them

  Narrower, the inspector is hidden and the list fills the panel. The
  content column's existing 880 DIP cap is lifted to 1180 DIPs, so that the
  list keeps at least 880 when the inspector shows.
- **Footer:** the status line on the left. On the right: **Convert**
  (primary, with its key shown), then Add files and Clear.

### Rows

Rows stay 56 DIPs tall. From left to right:

- a 34 DIP preview with 6 DIP corners
- the format chip in the file's glaze
- the file name, with a second line under it, in 13 px muted text
- the row's own action link at the right edge (Convert…, Cancel, or Save),
  as the phase 3 redesign has it. While the inspector shows, the link is
  dropped and the inspector carries the actions.

The second line says one of:

- **Waiting:** the facts and the suggestion, for example
  "1600 × 1200 photo · 2.4 MB · as WEBP about 410 KB". The suggestion is in
  the glaze colour.
- **Converting:** "Converting to WEBM · 38% · about 2 min".
- **Done:** "Done, not saved yet · harbour.webp" or "Saved · harbour.webp".
- **Failed:** the phase 3 failure sentence.

A converting row has a glaze-tinted wash fading from the left, and its heat
line along the bottom edge.

### Inspector

The inspector shows the selection. It has its own layout for each case:

- **One file, waiting**
  - a large preview, 268 × 120 DIPs, cropped to fill
  - the chip and the file name (15 px semibold)
  - what the format is: today's one-line description from `describe_format`.
    Piece 3 adds "good for…".
  - facts: size, then pixels or length, then folder
  - **Turn it into:** up to three targets, then a "More formats…" link that
    opens the format choice.
    - Each target shows its chip, what it's for (from `describe_format`), and
      its estimated size where Kiln has an estimate.
    - The suggested target comes first, with the suggestion dot.
    - Then come the two targets most often picked for this kind (learned
      suggestions). Failing those, the first targets in the registry's order.
    - Clicking a target converts the file. Each target is a button for the
      keyboard and assistive tools.
- **One file, converting:** the preview with the heat rising over it, the
  percentage and time left in large figures, and a Cancel button.
- **One file, done:**
  - The result's name, and its size before and after ("2.4 MB → 410 KB,
    83% smaller").
  - Buttons: Save (or Show in folder once saved).
  - A line saying the result can be dragged out.
- **One file, failed:** the failure sentence, the next step, and Retry and
  Show details.
- **Several files:** a stack of up to three previews, and the kinds in
  words ("3 images, 1 video").
  - When they share formats, the shared ones are shown as targets.
  - When they share none, the inspector shows the phase 3 explanation and
    offers each kind.
- **Nothing selected:** the vessel, "Select a file to see what Kiln can do
  with it", the ways in, and "Everything converts on this PC. Nothing is
  uploaded."

### Empty window

- A larger vessel drawing in celadon line work.
- "Drop files here" and the Add files button, as now.
- Under them, the four glazes as small tiles naming the kinds Kiln reads:
  images, video, audio and documents.
- The modifier-key hint and the privacy line stay. The tagline already
  moved to About.

## Look

### Glazes

| Kind | Glaze | Dark | Light |
|---|---|---|---|
| Images | Celadon | `#7FB5A0` | `#467966` |
| Video | Chun blue | `#8FA6C9` | `#4D6489` |
| Audio | Ki-seto yellow | `#C8BA83` | `#756835` |
| Documents (including ebooks, spreadsheets, slides, subtitles) | Lavender jun | `#B3A5CF` | `#6A5B8C` |
| Anything else | Stone | `#A89E8F` | `#6E665B` |

- Ki-seto yellow sits at hue 48°, saturation 0.38, so the token test
  doesn't read it as orange.
- Text drawn in a glaze uses that glaze's ink: the glaze mixed toward the
  theme's text colour just enough to reach 4.5:1. The ink colours are in
  the implementation plan and in `DESIGN.md`.
- A glaze appears only in these places:
  - format chips: text in the glaze's ink, an opaque fill of the glaze at
    16% over the surface, and a 35% outline
  - the preview's fallback tile
  - a file's heat and its converting wash
  - the suggestion text on a row
  - the inspector's top wash
- Glazes are theme tokens with roles (`palette.glaze(kind)`). The token
  snapshot changes on purpose.
- Chip text reaches at least 4.5:1 on its own fill, on the panel and on a
  selected row, in both themes. `theme::contrast_report` lists the pairs
  and a test holds them.
- Under a Windows contrast theme, glazes go and the chips use the theme's
  text and outline colours.
- No orange, no pure black, no pure white: the token test still holds.

### Depth

- Each surface level gets:
  - a 1 px top highlight (bone at 5% in dark, white at 60% in light)
  - one soft shadow under it: 0 10 30, spread −18, in a shadow colour per
    theme
- The levels are panels, the footer's primary button, sheets, the wheel
  and (in piece 2) menus.
- The window ground keeps its grain, and adds one faint celadon glow at the
  top centre (6%). It's pre-rendered with the grain and never animates.
- Pressed controls keep their inner shadow.
- `DESIGN.md`'s "Flat surfaces have no drop shadows" becomes "Each surface
  level has one soft shadow; nothing else does".

### Previews

- Previews come from Windows' thumbnail service (`IShellItemImageFactory`),
  the same one Explorer uses. It runs offline, and covers images, video
  frames, PDFs and Office files where a handler is installed.
  - Kiln asks for thumbnails only (`SIIGBF_THUMBNAILONLY`), so it never
    waits on an icon.
  - Requests run on one worker thread, nearest-visible first. The thread
    posts each finished bitmap to the window, and a preview fades in over
    180 ms.
  - Bitmaps are cached by path and modified time, at the sizes in use (34,
    and 268 × 120 for the inspector). The cache is capped at 64 MB, least
    recently used first.
  - A file that changes on disk gets a new preview. A removed row's
    preview is dropped.
- **Fallback, when there is no thumbnail:** the glaze tile, with the kind's
  icon from `icons.rs` drawn in the glaze. Audio always uses this tile,
  with a waveform icon (new).
- The preview is decoration: a row's name for assistive tools doesn't
  change, and the inspector's large preview is named "Preview of
  harbour.png".

### Type and icons

- The inspector's file name uses 15 px semibold, which adds 15 to the type
  scale. Everything else stays as `DESIGN.md` has it, still with at most
  four sizes in one view.
- New icons, on the 24 grid with a 1.5 px stroke: waveform, page, film,
  picture, and the larger empty-state vessel.

## Motion

### The engine

- A small animation module in the platform-free part of the app crate.
  - It holds eased values keyed by what they animate (a row, the
    selection, the inspector, the wheel, a button).
  - Each value is a pure function of its start time, duration, easing and
    the current time, so tests can drive time directly.
- Easing is one ease-out curve (cubic-bezier .2, .8, .2, 1) for entrances
  and moves, and ease-in-out for the heat's breathing. Nothing overshoots.
- The window's frame timer (`ANIM_TIMER`) runs at 16 ms only while a value
  is moving, drops to the existing 50 ms heat rate when only heat is
  breathing, and stops when nothing moves. An idle Kiln uses no CPU.
- With Windows' Animation effects off (`theme::animations()` false), every
  value is at its end at once and heat stops breathing. Progress still
  shows.
- The current `RowAnim` (appear, settle, facts) moves into the engine
  rather than living beside it.

### Catalogue

| Moment | Motion | Time |
|---|---|---|
| Rows added | Each row rises 10 DIPs and fades in. The delay grows by 45 ms a row, up to a 400 ms total, so 500 rows don't crawl. | 260 ms |
| Selection moves | The selected-row wash and its edge glide to the new row. | 160 ms |
| Inspector content changes | Old content fades out as new content fades in, rising 6 DIPs. | 180 ms |
| Preview arrives | Fades in. | 180 ms |
| Converting | The heat line glows in the glaze and breathes (opacity 1 to 0.65). Its length follows progress, eased. | 2.4 s cycle |
| Finished | The row flashes in its glaze at 32% and cools to its resting colour. The check mark fades in on the preview. | 900 ms |
| Failed | The row shakes 2 DIPs, left then right, once (a crack), and its text turns to the error colour. | 120 ms |
| Wheel opens | Scales from 0.86 and turns from −8° as it fades in. The segments follow 18 ms apart. | 200 ms |
| Sheets (Settings, Help, options) | Slide up 12 DIPs and fade in, with the scrim fading in behind. | 180 ms |
| Buttons | Hover lifts the fill. Press sinks it with the inner shadow. | 120 ms |
| Inspector folds away at the breakpoint | Slides out to the right as the list widens. | 220 ms |

### Accessibility

- Every inspector control has a name and role in UI Automation:
  - targets are buttons named "Convert to WEBP, for websites, about 410 KB"
  - facts are text
  - Cancel, Save, Retry and Show details are named with the file
- Tab order: header, list, inspector, footer. The inspector's targets are
  one Tab stop each.
- Motion never carries meaning alone: a finished row also says "Done", and
  a failed one says why.
- Text size: the inspector zooms with the rest of the layout. The fold
  point is measured in zoomed DIPs, so at a large text size the inspector
  folds sooner.

## Speed

These hold the phase 3 baselines:

- The window is visible by 100 ms, with no white first frame.
- A key reaches the screen in under 35 ms (median) with 4 conversions
  running.
- 500 files are listed in under 0.3 s. Previews arrive afterwards, without
  slowing the list.
- The UI thread replies in under 3 ms (p95) while previews load.
- Memory with 500 rows and their previews stays under 160 MB working set.

## Testing

- **Model tests** (platform-free):
  - the inspector's content for each case: one file waiting, converting,
    done and failed; several files sharing formats or none; nothing
    selected
  - target order and the three-target limit
  - the fold point at 760 zoomed DIPs
- **Theme tests:** the glaze tokens in both themes, the chip contrast
  pairs, contrast-theme fallbacks, and the token snapshot.
- **Engine tests:** values at chosen times for each easing, the stagger cap
  for 500 rows, everything at its end with animations off, and the timer
  stopping when nothing moves.
- **Layout tests:** the grid at several window sizes on both sides of the
  fold, and nothing overlapping at 480 × 360.
- **Preview cache tests:** the size cap, eviction order, and a change of
  modified time asking again.
- **UIA tests:** the inspector's names and roles, and the Tab order.
- **The phase 3 harness:** `sc01` (start-up flash), `sc50` (500 files),
  `sc61` (latency) and `sc90` (keyboard task) rerun against the speed limits
  above.
- **Owner checks:**
  - the look at 125%, 150% and 200%
  - High Contrast
  - Text size
  - Animation effects off
  - a first look in both themes

## Documents

- `DESIGN.md`:
  - the glazes and their roles
  - the depth rule
  - the type scale's 15
  - the new icons
  - the motion catalogue (replacing the "120 to 180 ms" line)
  - the inspector
- `VISUAL_TESTING.md`: the fold point, previews, and motion with
  animations off.
- README: one line saying the list shows previews and a panel that
  describes the selected file.
- CHANGELOG: one line under the next version.

## Out of scope

- Kiln's own right-click menus (piece 2). The Windows menu stays until then.
- Longer format descriptions, common uses and the Format guide (piece 3).
  The inspector uses today's one-line descriptions until then.
- The welcome, recent conversions and presets (pieces 4 to 6).
- Real audio waveforms, video scrubbing and previews made by Kiln's own
  decoders. Windows' thumbnails and the glaze tile cover this piece.
- Mica, the system accent colour and Fluent controls.
- New formats or changes to conversion.
