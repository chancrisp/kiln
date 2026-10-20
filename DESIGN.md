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
   only the active selection, keyboard focus, the primary action,
   progress and the suggestion dot. There are no gradients except the progress glow.
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
- Current set: vessel, vessel-solid, flame, cube, cross, image, audio, video, subtitle, document, sheet, slides, compress, crop, trim, split, merge, copy, move, terminal, dial, more.
- The app icon is the solid vessel in celadon on a soot tile with rounded corners, at 16, 32, 48 and 256 px. Regenerate it with `cargo run -p kiln-app --example gen_icon`.

## The glaze wheel

- **Opening.** The wheel opens at the pointer while a modifier is held during a drag.
  - By default Shift opens Convert, Ctrl opens Tools and Alt opens Actions. Settings can reassign them, one key per wheel.
  - From the keyboard, Enter (with the modifier) opens it in the middle of the list.
- **Size.** The outer radius is 136 DIPs and the inner radius 56. The pointer still counts as on an item up to 48 DIPs past the edge. The wheel always stays 8 DIPs inside the window.
- **Pour.** The wheel opens over 160 ms, scaling from 55% to full size with opacity, like glaze spreading from the pointer.
- **Items.** Items are segments clockwise from the top. Each shows its icon, then its label. Format codes are in monospace and tool names in UI text.
- **Centre.** It shows the wheel's name in the serif, the key that opens it below in muted UI text, then the source type in muted monospace (PNG, or MIXED). The title reads `Convert · Shift`, broken across the two lines, and follows the wheel keys.
- **Other ways in.** Right-clicking a row, the Menu key or Shift+F10 opens a native Windows menu: the wheels that have anything for the row, then Export… and Open or Show in folder, then Remove. Items that don't apply are left out. A wheel chosen there pours over the row.
- **Hover.** Hovering fills the segment with celadon at 18% and draws a 1.5 px celadon arc on its outer edge. The icon and label keep their colour.
- **Escape.** Escape, or releasing the key mid-drag, fades the wheel out over 120 ms.
- **Choosing.** Dropping on an item, clicking it or pressing Enter pulses it once over 180 ms. Then the job starts and the wheel fades.
- **Contents.** The wheel shows only what applies:
  - The formats every dropped file can become.
  - The tools that fit them. Merge needs two or more files of one type.
- **More.** The convert wheel holds at most eight targets, the ones people reach for first. When there are more, the ninth segment is **More** (three dots).
  - Pointing at More pours an outer ring from behind the dish over 160 ms, scaling from 66% with opacity. Pointing at another inner segment draws it back over 120 ms.
  - The ring runs from 144 to 208 DIPs. Its targets are grouped by kind, each group led by a heading: the kind's icon in muted stone with its name under it in muted caption text (Images, Video, Audio, Documents, Sheets, Slides, Subtitles). A heading takes two and a half slots, a code one, with a hairline divider only where a group starts. Codes are monospace, without icons.
  - The 8 DIP groove between the dish and the ring is solid ground, so nothing behind shows through.
  - The centre reads `MP4 → AVI` while a target is lit, so the pair is always spelled out.
- **Names.** While a convert target is lit, in the wheel or the More ring, the footer says what it is in place of the generic hint: `WEBP: a small image for the web`, `WV: WavPack, lossless audio`. The code is upper-cased and the description comes from `describe_format` in kiln-core: lower case after the colon unless it opens with a proper name, no full stop, at most 40 characters. With nothing lit, the footer goes back to what it said. `wheel::target_label` composes the line, and the options sheet reuses it.
- **Options.** Tab or right-click on a target opens its options sheet instead of converting.
- **Suggestion.** When Kiln has a suggestion for the files, the suggested segment gets a 4 px celadon dot centred 6 px above its icon. Places never change.
  - When the suggestion is in More, the More segment and the suggestion's slot in the outer ring both get the dot. In the ring, where codes have no icon, the dot sits 6 px above the code.
  - When facts arrive while the wheel is open, the dot fades in over 160 ms. Nothing moves or pulses.
  - Opened from the keyboard, the wheel starts on the suggestion. When the suggestion is in More, it starts on More, and Enter opens the ring with the suggestion lit.
  - While nothing is lit, or while the suggested segment is lit, the footer reads "Suggested: JPG. " followed by the reason, as in "Suggested: JPG. A photo. JPG keeps the look at about 2 MB."
  - The Tools wheel marks Compress, Merge or Crop the same way.

## The options sheet

- A panel centred over a scrim. It rises 8 px and fades in over 160 ms, and fades out over 120 ms.
- **Title** in the serif: "Convert to MOV", or "Convert 3 files to MOV".
- **Format** below it in muted caption text: the target as the wheel's footer names it, as in `MOV: QuickTime video from Apple`.
- **Context** below that in muted UI text: the file and what it is, as in `long take.mp4 · 1280 × 720 · 25 fps · 1:30`. While the file is read, it says "Reading the file…".
- **Rows** show only the settings that change this pair's result, so a PNG to JPG sheet has two rows and an MP3 to OGG sheet one. Each value is a segmented control (Small, High, Best) or a stepper.
- A **stepper** shows its value between two chevrons. Clicking the left or right end, or Left and Right on the keyboard, steps through choices that fit the file. A 720p clip never offers 1080p.
- **Result** at the bottom left: the size it will come out at and a rough file size, as in `1280 × 720 · About 25 MB`. It updates as values change. Kiln leaves the file size out where it can't be estimated well.
- **Convert** is the primary button, on the right, with Cancel beside it. Opened from the keyboard, focus starts on Convert, so Tab then Enter converts with the defaults.

## Settings

- One panel with three tabs: **General**, **Quality** and **Engines**. The tabs are a segmented control. The chosen tab's fill slides to it over 160 ms, and the page crossfades over 120 ms. Every page keeps the panel's height, so switching never moves it.
- Every stepper and segmented group has its own label. When two share a line, the second sits in its own labelled column.
- Changes save straight away. There is no Apply button.
- On **Engines**, each engine's name has a muted line under it saying what it's for, as in "E-books.". A missing engine gets a **Get it** link that opens its official download page in the browser; if the browser can't open, the panel's note line says so. Kiln itself never downloads anything.

## The file list

- Each row shows the file's name, and below it a muted second line: `folder · 3.1 MB · Converts to 12 formats`.
- Once Kiln has looked inside the file, the second line says what it is and what Kiln suggests, as in `folder · 4032 × 3024 photo · 3.1 MB · Suggested: JPG, about 2 MB`. The new text fades in over 160 ms and stays muted.
- With no suggestion, the description takes the place of "Converts to N formats" and nothing else is added.

## Progress and results

- **Progress** is a 2 px line along the bottom of the job row, filled with celadon. Its leading edge glows slightly brighter, like a glaze taking heat. This is the only gradient in Kiln. While a job runs, the glow breathes slowly over 2.4 s, like coals. When the engine can't report progress, the whole line holds a low glow that breathes the same way. There are no spinners.
- **Success:** the row settles and `Ready:` with the output name appears in bone, fading in over 200 ms, as in `Ready: photo.jpg`. There's no sound or confetti. The result waits inside Kiln until it's exported.
- **Export:** a held row has an "Export" link where a failed row has "Show details", and Ctrl+S does the same. Once exported, the line reads `Saved to <folder>.`, and clicking it shows the file in Explorer. The footer's **Export all (3)** sits beside Clear while anything is held. A failed export is one line in brick: `Couldn't save to D:\Locked. Pick another folder.`
- **Failure:** the row shows one plain sentence in brick, with a next step where Kiln recognises the failure: "This file looks damaged, or isn't really an MP4. Try opening it in another app." Otherwise it says what the engine couldn't do, such as "FFmpeg couldn't convert the file." or, for a tool, "FFmpeg couldn't trim the file." The file's own name never picks the sentence. A line too long for the row ends in "…".
- **Show details** appears when the engine said something or the line was cut short. It opens a dialog titled "What FFmpeg said" ("What Kiln said" for Kiln's own words), with the row's whole sentence first and then the engine's output. The text scrolls with the wheel or touchpad, the arrows, Page Up, Page Down, Home and End. **Copy** copies all of it and reads "Copied" for 1.2 s, or "Couldn't copy" if the clipboard refused. Focus starts on Copy, and Escape or Enter closes the dialog.

## Voice

- Be short, calm and specific: "Converted 3 files to JPG.", not "Success! 🎉".
- Never use exclamation marks.
- Menus say "convert". "Fire" appears only in the onboarding tagline, "Fire raw files into finished ones."
- Error messages name the problem and the next step: "Needs FFmpeg, which isn't installed. Settings > Engines says where to get it."
- Say what happened, not how Kiln feels about it: "Couldn't convert the file to FLAC.", "Moved 2 files to Pictures.", "Cancelled."
- Use sentence case everywhere, with full stops on full sentences in the footer and row lines.

## Clean room

Kiln is designed from its own requirements only. Don't reference, decompile or
copy the source, binaries, assets, icons or visual design of any other
product, commercial or open source, when working on Kiln.
