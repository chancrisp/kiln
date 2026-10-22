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
   Nothing bounces. With Windows' Animation effects off, every pour, slide
   and fade is instant and progress lines stop breathing; progress still
   shows.

## Colour

Code asks for a role (`palette.accent`), never a raw token.

| Token | Dark | Role |
|---|---|---|
| kiln-soot | `#1C1B19` | window ground |
| kiln-clay | `#2A2825` | panels and surfaces |
| kiln-ash | `#3B3834` | fills: progress track, chips, a hovered button |
| kiln-flint | `#7C756B` | outlines of buttons and controls, panel borders, rules between rows |
| kiln-bone | `#EDE6DA` | primary text |
| kiln-stone | `#A89E8F` | secondary text and labels |
| kiln-celadon | `#7FB5A0` | the single accent |
| kiln-brick-light | `#D99D93` | errors only, used sparingly |
| kiln-stone-light | `#C4BCB0` | muted text on a selected or hovered row |

Light mode inverts the neutrals, keeps celadon for washes and fills, and
deepens the colours that must read on its pale ground.

| Role | Light |
|---|---|
| ground (bone ground) | `#F3EFE7` |
| surface (slip) | `#E8E2D7` |
| line (bisque), a fill | `#D6CEC1` |
| edge (taupe), outlines and rules | `#837A6E` |
| text (soot) | `#1C1B19` |
| muted text (umber) | `#5C5449` |
| muted text on a wash (umber deep) | `#544C42` |
| strong accent (celadon deep) | `#467966` |
| errors (brick deep) | `#984335` |

- The strong accent draws every celadon line that must be seen: the focus
  ring, a chosen segment's edge, a selected row's edge, progress and the
  suggestion dot. In dark it is celadon itself.
- Muted text reaches 5.5:1 on the panel and on a selected or hovered row,
  all other text 4.5:1, and every cue and every button, panel and row outline
  3:1, in both themes. The outlines have their own colour, apart from the
  fills the same neutral serves. `theme::contrast_report` lists each pair, and a test holds it.
- The theme follows the Windows app mode unless Settings says Dark or Light.
- Under a Windows contrast theme Kiln draws in the theme's own colours
  (window, window text, highlight, highlight text, grey text, hot light).
  Washes, glows, grain and shadows go; outlines and solid fills stand in.

Never use orange, pure black (`#000000`) or pure white (`#FFFFFF`). The token
test enforces this.

## Type

| Use | Face | Size |
|---|---|---|
| App name, wheel centre, dialog titles | Fraunces SemiBold (bundled, Latin subset, 32 KB, OFL) | 22 |
| Empty-state headline | Segoe UI Variable, falling back to Segoe UI | 16 |
| UI text | Segoe UI Variable, falling back to Segoe UI | 14 |
| Captions, status, secondary lines | Segoe UI Variable, falling back to Segoe UI | 13 |
| Format codes | Cascadia Mono, falling back to Consolas; uppercase, 0.06em tracking | 13 |
| The wheel's group names | Segoe UI Variable, falling back to Segoe UI | 12 |

- The scale is 12, 13, 14, 16, 22 and 32. Use at most four sizes in any one view. 12 is only the wheel's group names, which have to fit a ring band 64 DIPs wide.
- The serif appears in exactly three places: the app name, the wheel's centre label and dialog titles.
- Format codes (PNG, MP4, DOCX) always use the monospace style. This is a signature element, so use it wherever a format is shown.

## Shape and space

- Everything sits on a 4 px grid.
  - Padding inside panels is always 16 px.
  - Rows are 56 px tall, controls 32 px, and the header 56 px.
  - Content runs in a column at most 880 px wide, centred in a wider window, so a row's link is never far from its name. Below that width it fills the window.
- Corners are 6 px on controls and 12 px on panels. The wheel is circular.
- Borders are 1 px in kiln-flint (taupe in light) and drawn inside their shape, so neighbours never double up. A segmented control has one outer border and hairline dividers.
- The keyboard focus ring is a 1.5 px strong-celadon line drawn 3 px outside the control. In the list it sits 2 px inside the focused row. It shows after keyboard input, stays when Escape clears the selection, and hides after a click in the list until the next key.
- A selected row has the celadon wash and a 1 px strong-celadon edge on its left.
- Layout is in DIPs, and Direct2D scales it. The layout tests check the grid at several window sizes.
- Windows' Text size zooms the whole layout, type and all, from 1.0 to 2.25. The smallest window grows with it. On a screen too small for the zoomed window, the zoom gives way so nothing overlaps.
- Small targets are at least 24 DIPs: a row's "Show details" or "Export" link has its own 24 DIP target on the second line, apart from the row's.

## Icons

- Icons sit on a 24 × 24 grid and use a 1.5 px stroke with round caps and joins.
- They are built from simple geometric shapes and drawn as vector paths in [`icons.rs`](crates/kiln-app/src/icons.rs). There is no icon font.
- Keep curves generous. A Bézier smaller than about 2 units strokes poorly in some Direct2D implementations, so draw tiny circles as short polygons.
- Current set: vessel, vessel-solid, flame, cube, cross, image, audio, video, subtitle, document, sheet, slides, compress, crop, trim, split, merge, copy, move, terminal, dial, help, more.
- The app icon is the solid vessel in celadon on a soot tile with rounded corners, at 16, 32, 48 and 256 px. Regenerate it with `cargo run -p kiln-app --example gen_icon`.

## The glaze wheel

- **Opening.** The wheel opens at the pointer while a modifier is held during a drag.
  - By default Shift opens Convert, Ctrl opens Tools and Alt opens Actions. Settings can reassign them, one key per wheel.
  - From the keyboard, Enter (with the modifier) opens it in the middle of the list.
- **Size.** The outer radius is 136 DIPs and the inner radius 56. The pointer still counts as on an item up to 48 DIPs past the edge. The wheel always stays 8 DIPs inside the window.
- **Pour.** The wheel opens over 160 ms, scaling from 55% to full size with opacity, like glaze spreading from the pointer.
- **Items.** Items are segments clockwise from the top. Each shows its icon, then its label. Format codes are in monospace and tool names in UI text.
- **Centre.** It shows the wheel's name in the serif, the key that opens it below in muted UI text, then the source type in muted monospace (PNG, or MIXED). The title reads `Convert · Shift`, broken across the two lines, and follows the wheel keys.
- **Other ways in.** Right-clicking a row, the Menu key or Shift+F10 opens a native Windows menu: the wheels that have anything for the row, then Export… and Open, Cancel or Retry as the row allows, Show in folder for one file (its saved result, or else the file itself) and Copy path for any number, then Remove. Items that don't apply are left out. A wheel chosen there pours over the row.
- **Hover.** Hovering fills the segment with celadon at 18% and draws a 1.5 px celadon arc on its outer edge. The icon and label keep their colour.
- **Escape.** Escape, or releasing the key mid-drag, fades the wheel out over 120 ms.
- **Choosing.** Dropping on an item, clicking it or pressing Enter pulses it once over 180 ms. Then the job starts and the wheel fades.
- **Contents.** The wheel shows only what applies:
  - The formats every dropped file can become.
  - The tools that fit them. Merge needs two or more files of one type.
- **More.** The convert wheel holds at most eight targets, the ones people reach for first. When there are more, the ninth segment is **More** (three dots).
  - Pointing at More pours an outer ring from behind the dish over 160 ms, scaling from 66% with opacity. Pointing at another inner segment draws it back over 120 ms.
  - The ring runs from 144 to 208 DIPs. Its targets are grouped by kind, each group led by a heading: the kind's icon in muted stone with its name under it in muted caption text (Images, Video, Audio, Documents, Sheets, Slides, Subtitles). A heading takes two and a half slots, a code one, with a hairline divider only where a group starts. A click on a heading does nothing, and the wheel stays open. Codes are monospace, without icons.
  - The 8 DIP groove between the dish and the ring is solid ground, so nothing behind shows through.
  - The centre reads `MP4 → AVI` while a target is lit, so the pair is always spelled out.
- **Names.** While a convert target is lit, in the wheel or the More ring, the footer says what it is in place of the generic hint: `WEBP: a small image for the web`, `WV: WavPack, lossless audio`. The code is upper-cased and the description comes from `describe_format` in kiln-core: lower case after the colon unless it opens with a proper name, no full stop, at most 40 characters. With nothing lit, the footer goes back to what it said. `wheel::target_label` composes the line, and the options sheet reuses it.
- **Options.** Tab or right-click on a target opens its options sheet instead of converting. While a target with options is lit, the centre shows an underlined **Options… (Tab)** in place of the key, and clicking it does the same. Outside a drag, moving the pointer into the centre keeps the target lit, so the link can be reached. A drop on the centre still converts nothing.
- **Suggestion.** When Kiln has a suggestion for the files, the suggested segment gets a 4 px celadon dot centred 6 px above its icon. Places never change.
  - When the suggestion is in More, the More segment and the suggestion's slot in the outer ring both get the dot. In the ring, where codes have no icon, the dot sits 6 px above the code.
  - When facts arrive while the wheel is open, the dot fades in over 160 ms. Nothing moves or pulses.
  - Opened from the keyboard, the wheel starts on the suggestion. When the suggestion is in More, it starts on More, and Enter opens the ring with the suggestion lit.
  - While nothing is lit, the footer reads "Suggested: JPG. " followed by the reason, as in "Suggested: JPG. A photo. JPG keeps the look at about 2 MB." While the suggested target itself is lit, its name comes first: "JPG: a photo that opens anywhere · Suggested: JPG. A photo. JPG keeps the look at about 2 MB." A line too long for the footer ends in "…". With More lit and the suggestion inside it, the footer keeps the suggestion alone, since More is not a target.
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

- One panel with three tabs: **General**, **Quality** and **Helper apps**. The tabs are a segmented control. The chosen tab's fill slides to it over 160 ms, and the page crossfades over 120 ms. Every page keeps the panel's height, so switching never moves it.
- Every stepper and segmented group has its own label. When two share a line, the second sits in its own labelled column.
- Changes save straight away. There is no Apply button.
- Labels say what they change: **Sound quality**, **Sharpness (DPI)**, **Keys to hold while dropping**, **Folder shown first when saving** (where the save dialog opens). The suggestions row has **Reset suggestions**, spoken as "Reset learned suggestions". A label too long for its column wraps onto a second line, and every label fits at the smallest window.
- On **Helper apps**, each app's name has a muted line under it saying what it's for, as in "E-books.". A missing app gets a **Get it** link that opens its official download page in the browser; if the browser can't open, the panel's note line says so. Kiln itself never downloads anything.

## The file list

- Each row shows the file's name, and below it a muted second line: `Converts to 12 formats · 3.1 MB · folder`. The folder is the last part of its path.
- Once Kiln has looked inside the file, the second line says what it is, what Kiln suggests, the size and the folder, in that order, as in `4032 × 3024 photo · Suggested: JPG, about 2 MB · 3.1 MB · folder`. The new text fades in over 160 ms and stays muted.
- With no suggestion, the description takes the place of "Converts to N formats" and nothing else is added.
- Hovering a row shows a tooltip: the whole line when the row cuts it short, then the file's full folder path on a line of its own.

## Starting a conversion

- **Convert is always in sight.** With files listed, the footer has a **Convert** button left of Add files, and a file nothing has been done to has a **Convert** link at the right end of its row. Both open the same format choice for the selection (the link selects its row first). The wheel stays the quick way: Enter, Shift/Ctrl/Alt+Enter, the held-key drags and the right-click menu all work as before.
- **Where the choice opens.** From the button it pours from the button's top edge; from a row's link or from Enter it pours from the right end of the row, so the file's name stays readable. The drag route and the right-click menu are unchanged.
- **Never an empty wheel.** Before a wheel opens, `wheel::offer` decides what it can offer. One kind of file, or several kinds with a format in common, opens the choice. Several kinds with nothing in common do not: the footer says `These files have no format in common: MP4 (3), DOCX (2). Select one kind at a time.` (the first four kinds, then "N more"). One kind Kiln has nothing for reads `Kiln can't convert XYZ files.`
- **When Convert can't be used.** With nothing selected, or with a selected file that is still converting, the button is quiet (muted label, no hover), is left out of Tab, and says why in its tooltip and, if pressed, in the footer: `Select a file to convert.` / `A selected file is still converting. Wait for it, or cancel it.` A file that is converting can't be started a second time from the button, the link, Enter or the menu.
- **The drop tip.** The first plain drop that adds files (no Shift, Ctrl or Alt held) shows `Tip: hold Shift, Ctrl or Alt as you drop.` in the footer, with the keys as set in Settings. It shows once on this PC (`drop_tip_shown` in settings.toml) and stands until something replaces it.

## Honest offers

- **Only what can work is offered.** Kiln looks for the helper apps (FFmpeg, Pandoc, LibreOffice, Calibre) when the first file is added and again each time Settings closes. The format choice lists only formats whose helpers are there; a row's line counts the same way.
- **A missing helper is said on the row.** A file whose formats all need a missing helper reads `Needs FFmpeg to convert` and its link reads **Get FFmpeg** (the helper's own download page; Kiln never downloads anything). A file that has some formats and misses others reads `Converts to 10 formats, 2 more need FFmpeg`. Trying to open the choice for such a file says `MP4 files need FFmpeg, which isn't installed. Use Get FFmpeg on the file's row.` Once per run the footer also says `FFmpeg isn't installed, so some files can't be converted yet. Use Get FFmpeg on their rows.`
- **Files are looked at when added.** The first 1 KB of the file is checked against the signature every valid file of that format carries (`kiln_core::check`). Only formats with such a signature are judged (PNG, JPG, GIF, BMP, TIFF, WEBP, ICO, WAV, PDF, DOCX/XLSX/PPTX/ODT/ODS/ODP/EPUB, RTF); text, media containers and legacy Office files are never rejected. A file with zero bytes reads `Empty file (0 bytes)`; one that isn't what its name says reads `Isn't really a DOCX file`; one Windows won't let Kiln read reads `Can't read this file. Check it isn't open in another app.`; an extension Kiln doesn't know reads `Unknown type (.XYZ)`. None of them offers a conversion or a link. They are still listed, so the person can see why.
- **No false success.** Before an engine runs, the same check runs again, so a DOCX of random bytes fails with the damaged-file sentence instead of LibreOffice treating it as text and writing a PDF.
- **Folders.** Dropping or adding a folder adds its files and those of the folders directly inside it, but not hidden or system items and not folders two levels down. `Added 12 files from Holiday.` is followed by `Skipped 2 items: Thumbs.db (hidden), old (folder inside a folder).` (three names, then "and N more"). More than 200 files from folders asks first: `That's 1240 files. Add them all?` Drop, Add files, Ctrl+V, Open with and Send to all go through the same step.

## Keyboard and assistive tools

- **Adding files puts the keyboard on them.** After Add files, Ctrl+V, a drop, Open with or Send to, the list shows the *first* new row, selected, with the keyboard on it, so Enter converts instead of reopening the dialog. (It used to jump to the last row.)
- **Tab follows the reading order:** Help, Settings, the list, then the footer (Convert while it can be used, Add files, Export all, Clear), and round. The empty window goes Help, Settings, Add files. In Settings the page tabs are one Tab stop: Left and Right move between them, Tab leaves the strip, and Shift+Tab comes back to the page that is showing.
- **The list handles Page Up and Page Down** (a screenful less one row; with Shift they extend the selection), and Home and End (Ctrl+Home and Ctrl+End too).
- **Remove can be undone.** Removing two or more rows says `500 files removed.` with an **Undo** link at the right of the footer line (Ctrl+Z does the same, and there is always room for it). Undo brings the rows back where they were, selected; a job that was stopped by the removal comes back as a file waiting to be converted, and a result that had been saved or moved stays so. The link goes when the footer says something else.
- **A row is read as labelled parts**, each a sentence: `photo.png. 64 by 48 photo. Suggested JPG, about 4.2 KB. File size 9.3 KB. Folder files.` A running row reads what it is doing (its progress is a separate range value); a failed row reads its whole sentence. A row's Cancel, Retry, Export, Show details and Show in folder buttons carry its file name (`Cancel photo.png`).
- **Help lines that are scrolled out of its panel are reported off screen** to assistive tools, so Narrator doesn't read what can't be seen.

## Progress and results

- **Progress** is a 4 px line along the bottom of the job row, filled with celadon. Its leading edge glows slightly brighter, like a glaze taking heat. This is the only gradient in Kiln. While a job runs, the glow breathes slowly over 2.4 s, like coals. When the helper app can't report progress, the whole line holds a low glow that breathes the same way. There are no spinners.
- **A running row says how far.** Its second line reads `Converting to WEBM  ·  38%  ·  about 2 min`. The time left shows once the job has run 3 seconds and is at least 2% done, rounded as a person would say it ("a few seconds", "about 40 sec", "about 2 min", "about 1 h 20 min"). A helper that can't say how far reads `Converting to WEBM  ·  Working…`. The row's accessible name keeps to what it is doing; the percent is the progress bar's range value (0 to 100) and the row's status.
- **Cancel is a word.** A running row has a **Cancel** button at the right end of its second line, always visible, where its other links sit; the hover cross is gone. While the selection holds a file that is converting, the footer's Convert button reads **Cancel** instead (tooltip `Cancel the converting files (Space)`), is in the Tab order, and stops those files; the footer adds `Space cancels the selected file.` Cancelling leaves the original untouched and the row offers Retry.
- **A batch counts.** While jobs run, the footer reads `12 of 50 done.` Each job that finishes while others still run is said once, with the count: `photo.jpg is ready. 13 of 50 done.` The last one gives the batch summary.
- **Finishing while away.** The taskbar button shows the work's progress (a pulse while no helper can say how far). When the last job ends and Kiln is not in front, the button flashes until Kiln is brought forward (not when Windows has animations off) and a notification names the result (`photo.jpg is ready.`, `12 files are ready.`, `3 ready, 2 couldn't be converted.`); clicking it brings Kiln forward with that row selected. Cancelling gives no notice.
- **Success:** the row settles and `Done, not saved yet:` with the output name appears in bone, fading in over 200 ms, as in `Done, not saved yet: photo.jpg`. There's no sound or confetti. The result waits inside Kiln until it's exported.
- **Export:** a held row has an "Export" link where a failed row has "Show details", and Ctrl+S does the same. One dialog serves one result or many: a folder picker titled `Save photo.jpg to` or `Save 3 files to`. A single result can still be saved under a name of its own with **Export as…** in the row menu. Once saved, the line reads `Saved next to the original: photo.jpg` or `Saved to <folder>.`, its link reads **Show in folder**, and hovering shows the full path (`Saved to C:\Users\me\Pictures`). Hovering a held row also says that it can be dragged to a folder. The footer's **Export all (3)** sits beside Clear while anything is held. A failed save is one line in brick that names the folder, not its path: `Couldn't save to Locked. Choose another folder.`; the result stays held and the link reads **Choose folder**.
- **Save next to the original** (Settings > General, off by default). Results are held in Kiln until saved, as before. With the setting on, each result is saved beside its file as soon as it is ready, under a free name (`photo (1).jpg`; nothing is ever overwritten), so there is nothing unsaved and closing asks nothing. The first time a result is ready, Kiln asks once, `Save finished files next to the originals from now on?`, and remembers the answer (Settings changes it; closing the question without answering asks again at the next result). If saving fails, the result stays held. With it off the close prompt (`2 results aren't saved yet`: Export all, Discard, Cancel) stays as it was.
- **Drag out:** dragging a held row into a folder moves its result there, and its line reads `Saved where you dropped it.` A drop that only copies leaves it held.
- **Failure:** the row shows one plain sentence in brick that says what happened and what to do next, and never a file path, a library's words or the helper's name. "This file looks damaged, or isn't really an MP4. Try opening it in another app." names a cause Kiln recognises (video, audio, images, documents all use it). A helper that stopped, or any cause Kiln doesn't recognise, reads "The conversion stopped before it finished. Try again." ("The job stopped…" for a tool). Kiln's own plain sentences, such as "The PDF has no pages.", are kept. The file's own name never picks the sentence. A line too long for the row ends in "…". The path and the helper's own words wait in Show details.
- **Retry** is a link beside Show details (or alone, at the right) on a row that was cancelled, or that failed in a way trying again can fix: a stopped helper, a file another app had open, a full drive. A damaged file or a missing helper would fail the same way, so they get no Retry. Retry runs again what the row was last asked to do, with the current settings; a merge, which takes several files, has none. The right-click menu offers it too.
- **Show details** appears when the helper app said something or the line was cut short. It opens a dialog titled "What FFmpeg said" ("What Kiln said" for Kiln's own words), with the row's whole sentence first and then the app's output. The text scrolls with the wheel or touchpad, the arrows, Page Up, Page Down, Home and End. **Copy** copies all of it and reads "Copied" for 1.2 s, or "Couldn't copy" if the clipboard refused. Focus starts on Copy, and Escape or Enter closes the dialog.

## Help and tooltips

- A **?** button sits in the header beside Settings. It, or F1 anywhere but a panel or dialog, opens help over a scrim, drawn like the details dialog: the ways in (drop, Add files, paste, right-click, holding a key while dropping), how results wait until they're exported, a table of every keyboard shortcut and, last, **About Kiln**: what Kiln does, in a line, and that nothing is uploaded. The shortcuts follow the keys chosen in Settings.
- The text scrolls with the wheel or touchpad, the arrows, Page Up, Page Down, Home and End. Focus sits on Close. Escape, Enter, a click outside or Close dismisses it.
- Icon-only controls have native Windows tooltips: "Settings (Ctrl+,)", "Help (F1)", "Convert the selected files (Enter)", "Add files (Ctrl+O)", "Export all (Ctrl+Shift+S)", a row's "Cancel" and "Export (Ctrl+S)". There are none while a wheel, panel or dialog is up.

## Voice

- Be short, calm and specific: "Converted 3 files to JPG.", not "Success! 🎉".
- Never use exclamation marks.
- Menus say "convert". "Fire" appears only in the tagline, "Fire raw files into finished ones.", at the end of Help; the empty window keeps one instruction.
- Error messages name the problem and the next step: "Needs FFmpeg, which isn't installed. Settings > Helper apps says where to get it."
- Say what happened, not how Kiln feels about it: "Moved 2 files to Pictures.", "Cancelled." A footer line that says something failed also says where the reason is: "Couldn't convert the file to FLAC. The row says why."
- Never put a file path or a library's words on a row or in the footer; they belong in Show details.
- Use sentence case everywhere, with full stops on full sentences in the footer and row lines.

### Glossary

One word for each idea, in rows, menus, tooltips, dialogs and screen-reader names. Add a word here before using a new one.

| Say | Means | Not |
|---|---|---|
| Convert | turn a file into another format | change, transform, process |
| Format | a kind of file: PNG, MP4 (not "type" or "conversion", except "type" for a file's own kind) | codec, target |
| Tool | Compress, Crop, Trim, Split, Merge | action (an Alt wheel item is a "file action") |
| File action | Copy path, Move, Open in Terminal | tool |
| Helper app | FFmpeg, Pandoc, LibreOffice, Calibre, as a group; name the app only where the person must install it | engine, backend, converter |
| Cancel | end a running job (the row's button, its menu item, its screen-reader name) | stop, abort |
| Remove | take a row out of the list (the files are untouched) | delete |
| Clear | remove every row | reset |
| Retry | run the row's last job again | redo |
| Export | copy a held result out of Kiln | save (except "Saved to ..." once it has been exported) |
| Held | a result waiting in Kiln, not exported yet | pending |

In the "N results aren't saved yet" prompt, "Stop and close/clear/remove" keeps "Stop" because that dialog's own Cancel means "never mind".

## Clean room

Kiln is designed from its own requirements only. Don't reference, decompile or
copy the source, binaries, assets, icons or visual design of any other
product, commercial or open source, when working on Kiln.
