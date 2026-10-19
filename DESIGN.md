# Designing for Kiln

Kiln is a kiln: the place where raw material is fired into something
finished. Every surface, icon, animation and line of copy should feel like
part of the same object, quiet and tactile, like well-made stoneware. Read this
before changing anything a person can see. The values below live in
[`crates/kiln-app/src/theme.rs`](crates/kiln-app/src/theme.rs). A snapshot
test (`crates/kiln-app/tests/snapshots/theme_tokens.snap`) fails when any of
them changes, so a palette change is always a deliberate one.

## Principles

1. **One metaphor, used consistently.** The menu is a glaze wheel and
   progress is a temperature line. A finished file cools into place, and an
   error is a crack. Don't add other metaphors: no rockets, sparkles or mascots.
2. **Restraint.** The UI uses two neutrals and one accent. The accent marks
   only the active selection, keyboard focus, the primary action and
   progress. There are no gradients except the progress glow.
3. **Material, not decoration.** Surfaces look like matte stoneware.
   - The window ground carries one faint grain at about 3%. It is
     pre-rendered and never animates.
   - Pressed controls get a soft inner shadow.
   - Flat surfaces have no drop shadows.
4. **Motion with weight.** Animations last 120 to 180 ms and ease out.
   Nothing bounces.

## Colour

Code asks for a role (`palette.accent`), never a raw token.

| Token | Dark | Role |
|---|---|---|
| kiln-soot | `#1C1B19` | window ground |
| kiln-clay | `#2A2825` | panels and surfaces |
| kiln-ash | `#3B3834` | borders, dividers, disabled fills |
| kiln-bone | `#EDE6DA` | primary text |
| kiln-stone | `#9A8F80` | secondary text and labels |
| kiln-celadon | `#7FB5A0` | the single accent |
| kiln-brick | `#B5503F` | errors only, used sparingly |

Light mode inverts the neutrals and keeps celadon and brick.

| Role | Light |
|---|---|
| ground (bone ground) | `#F3EFE7` |
| surface (slip) | `#E8E2D7` |
| line (bisque) | `#D6CEC1` |
| text (soot) | `#1C1B19` |
| muted text (umber) | `#6B6257` |

The theme follows the Windows app mode unless Settings says Dark or Light.

Never use orange, pure black (`#000000`) or pure white (`#FFFFFF`). The token
test enforces this.

## Type

| Use | Face | Size |
|---|---|---|
| App name, wheel centre, dialog titles | Fraunces SemiBold (bundled, Latin subset, 32 KB, OFL) | 22 |
| Empty-state headline | Segoe UI Variable, falling back to Segoe UI | 16 |
| UI text | Segoe UI Variable, falling back to Segoe UI | 14 |
| Captions, status, secondary lines | Segoe UI Variable, falling back to Segoe UI | 12 |
| Format codes | Cascadia Mono, falling back to Consolas; uppercase, 0.06em tracking | 12 |

- The scale is 12, 14, 16, 22 and 32. Use at most four sizes in any one view.
- The serif appears in exactly three places: the app name, the wheel's centre label and dialog titles.
- Format codes (PNG, MP4, DOCX) always use the monospace style. This is a signature element, so use it wherever a format is shown.

## Shape and space

- Everything sits on a 4 px grid.
  - Padding inside panels is always 16 px.
  - Rows are 56 px tall, controls 32 px, and the header 56 px.
- Corners are 6 px on controls and 12 px on panels. The wheel is circular.
- Borders are 1 px in kiln-ash and drawn inside their shape, so neighbours never double up. A segmented control has one outer border and hairline dividers.
- The keyboard focus ring is a 1.5 px celadon line drawn 3 px outside the control.
- Layout is in DIPs, and Direct2D scales it. The layout tests check the grid at several window sizes.

## Icons

- Icons sit on a 24 × 24 grid and use a 1.5 px stroke with round caps and joins.
- They are built from simple geometric shapes and drawn as vector paths in [`icons.rs`](crates/kiln-app/src/icons.rs). There is no icon font.
- Keep curves generous. A Bézier smaller than about 2 units strokes poorly in some Direct2D implementations, so draw tiny circles as short polygons.
- Current set: vessel, vessel-solid, flame, cube, cross, image, audio, video, document, sheet, slides, compress, crop, trim, split, merge, copy, move, terminal, dial.
- The app icon is the solid vessel in celadon on a soot tile with rounded corners, at 16, 32, 48 and 256 px. Regenerate it with `cargo run -p kiln-app --example gen_icon`.

## The glaze wheel

- **Opening.** The wheel opens at the pointer while a modifier is held during a drag.
  - By default Shift opens Convert, Ctrl opens Tools and Alt opens Actions. Settings can reassign them, one key per wheel.
  - From the keyboard, Enter (with the modifier) opens it in the middle of the list.
- **Size.** The outer radius is 136 DIPs and the inner radius 56. The pointer still counts as on an item up to 48 DIPs past the edge. The wheel always stays 8 DIPs inside the window.
- **Pour.** The wheel opens over 160 ms, scaling from 55% to full size with opacity, like glaze spreading from the pointer.
- **Items.** Items are segments clockwise from the top. Each shows its icon, then its label. Format codes are in monospace and tool names in UI text.
- **Centre.** It shows the wheel's name in the serif, with the source type below in muted monospace (PNG, or MIXED).
- **Hover.** Hovering fills the segment with celadon at 18% and draws a 1.5 px celadon arc on its outer edge. The icon and label keep their colour.
- **Escape.** Escape, or releasing the key mid-drag, fades the wheel out over 120 ms.
- **Choosing.** Dropping on an item, clicking it or pressing Enter pulses it once over 180 ms. Then the job starts and the wheel fades.
- **Contents.** The wheel shows only what applies:
  - The formats every dropped file can become.
  - The tools that fit them. Merge needs two or more files of one type.

## Progress and results

- **Progress** is a 2 px line along the bottom of the job row, filled with celadon. Its leading edge glows slightly brighter, like a glaze taking heat. This is the only gradient in Kiln. While a job runs, the glow breathes slowly over 2.4 s, like coals. When the engine can't report progress, the whole line holds a low glow that breathes the same way. There are no spinners.
- **Success:** the row settles and the output name appears in bone, fading in over 200 ms. There's no sound or confetti.
- **Failure:** the row shows one plain line in brick, the first line the engine printed with log tags removed. When there's more, a "Show details" link opens the full engine output.

## Voice

- Be short, calm and specific: "Converted 3 files to JPG.", not "Success! 🎉".
- Never use exclamation marks.
- Menus say "convert". "Fire" appears only in the onboarding tagline, "Fire raw files into finished ones."
- Error messages name the problem and the next step: "FFmpeg isn't installed. Get it from Settings > Engines."
- Say what happened, not how Kiln feels about it: "Couldn't convert the file to FLAC.", "Moved 2 files to Pictures.", "Cancelled."
- Use sentence case everywhere, with full stops on full sentences in the footer and row lines.

## Clean room

Kiln is designed from its own requirements only. Don't reference, decompile or
copy the source, binaries, assets, icons or visual design of any other
product, commercial or open source, when working on Kiln.
