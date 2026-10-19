# Visual testing

Run this checklist on real Windows before a release, and whenever a change
touches drawing, layout, theme tokens or icons. Automated tests pin the
tokens (`crates/kiln-app/tests/snapshots/theme_tokens.snap`) and the layout
grid, but only eyes can judge the result.

Set scaling in Settings > System > Display > Scale. Check every item at
**100%, 150% and 200%**. With two monitors at different scales, drag the
window between them as well.

## Window

- [ ] Opens in under half a second, with no white flash before the first frame.
- [ ] Title bar is dark (Windows 10 20H1+). On Windows 11 the caption is soot with bone text.
- [ ] The background shows a faint, still grain. It never shimmers or moves.
- [ ] No pure black or pure white anywhere. No orange anywhere.
- [ ] The header shows the celadon vessel and "Kiln" in the bundled serif, not Georgia.
- [ ] Panels have 12 px corners, controls 6 px. Borders are a single 1 px line, never doubled.
- [ ] Text is sharp at every scale, with no blurry bitmap stretching.
- [ ] Resizing never goes below 480 x 360 DIPs, and nothing overlaps at the minimum size.

## Empty state

- [ ] The drop panel fills the body with 16 px padding on every side.
- [ ] Copy reads "Drop files here", "Fire raw files into finished ones." and the privacy line.
- [ ] Dragging files over the window fades in a celadon edge and a faint wash in about 120 ms, and leaving fades them out.
- [ ] Dragging something that isn't a file (text from a browser) shows the "no drop" cursor and no highlight.

## File list

- [ ] Dropping files adds one row each, fading in over about 180 ms. Nothing bounces.
- [ ] Format chips are monospace, uppercase and slightly tracked (PNG, MP4, DOCX).
- [ ] Rows show the name in bone, then the folder, size and number of conversions in stone.
- [ ] A file with no conversions says "No conversions for this type".
- [ ] Dropping the same file twice reports it as already in the list. Dropping a folder reports it as skipped.
- [ ] Hover tints a row; the selected row has the 18% celadon fill, clipped to the panel's rounded corners.
- [ ] Up, Down, Home and End move the selection. Delete removes it. Escape clears it.
- [ ] With more rows than fit, the wheel scrolls and a thin thumb appears on the right.
- [ ] "Clear" shows a soft inner shadow while pressed and empties the list on release.
- [ ] Dropping files on the app's icon in Explorer opens Kiln with them listed.

## Glaze wheel

- [ ] Holding Shift while dragging files over the window pours the convert wheel open at the cursor in about 160 ms. Ctrl opens the tools wheel instead.
- [ ] The wheel stays fully inside the window near every edge and corner.
- [ ] The centre shows "Convert" or "Tools" in the serif, with the source type (PNG, or MIXED) muted below it.
- [ ] Hovering a segment fills it with celadon at 18% and draws a fine celadon arc on its outer edge. Icons and labels keep their colour.
- [ ] A single-item wheel lights the whole ring as one smooth circle.
- [ ] Letting go of the key mid-drag, or pressing Escape, fades the wheel out over about 120 ms.
- [ ] Dropping on an item pulses it once, then the job starts and the wheel fades. Dropping on the centre just adds the files.
- [ ] Enter on a row opens the convert wheel, Ctrl+Enter the tools wheel, and the Apps key or Shift+F10 the convert wheel. Arrows move round it, Enter applies, Escape closes.
- [ ] Tools offered match the file: Merge appears only for two or more files of one type, Crop never for audio.
- [ ] Office types use their own icons in the convert wheel: a page for DOC, DOCX, ODT, RTF and PDF, a sheet for XLSX, XLS, ODS and CSV, and a screen on a stand for PPTX, PPT and ODP.

## Jobs

- [ ] A running row shows "Converting to WEBM" and a thin celadon line along its bottom that brightens at its leading edge. No spinners anywhere.
- [ ] Hovering or selecting a running row shows a cross; clicking it cancels the job and the row says "Cancelled.".
- [ ] When a job finishes, the output name appears in bone, fading in over about 200 ms.
- [ ] A failed job shows one plain line in brick and, when the engine said more, a "Show details" link. Clicking it, or pressing Space on the row, opens the details over a scrim. Escape, Enter or Close dismisses it.
- [ ] The footer sums up each batch calmly: "Converted 3 files to JPG.", "Couldn't convert the file to FLAC.". No exclamation marks.
- [ ] Clear and Delete cancel running jobs first and leave no partial files behind.

## App icon

- [ ] Explorer, the taskbar and Alt+Tab show the celadon vessel on a soot tile at 16, 32, 48 and 256 px.

## File actions

- [ ] Alt while dragging (or Alt+Enter on a row) opens the Actions wheel: Copy path, Move, Open in Terminal.
- [ ] Copy path puts the full path on the clipboard, one per line for several files, and says "Copied the path.".
- [ ] Move asks for a folder, moves the file without replacing anything there ("photo (1).png" if the name is taken), and the row says "Moved to" the folder.
- [ ] Moving across drives works, and a file a job is using can't be moved.
- [ ] Open in Terminal opens Windows Terminal in the file's folder. Without Terminal installed, the footer says so plainly.
- [ ] Pressing and releasing Alt alone doesn't make the next key disappear, and Alt+Enter doesn't beep.

## Settings

- [ ] The dial in the header and Ctrl+, open Settings over a scrim. Escape, Close or a click on the scrim closes it.
- [ ] Tab and Shift+Tab move a celadon focus ring through every control; Enter or Space presses.
- [ ] Output folder: Choose picks a folder and results go there; Reset goes back to "Next to each file".
- [ ] Wheel keys: pressing a key button moves that wheel to the next key and swaps with the wheel that had it. Dragging follows the new keys at once.
- [ ] Engines shows "Found on PATH", "Not installed" or the chosen path for FFmpeg, Pandoc and LibreOffice. LibreOffice installed in Program Files shows its path without being chosen. Choose takes an .exe; jobs use it straight away.
- [ ] Every change is saved to %APPDATA%\Kiln\settings.toml. A broken file starts Kiln with defaults and a footer line saying why.

## Themes

- [ ] With Theme on System, Kiln follows Settings > Personalization > Colors > app mode, and switches live when it changes.
- [ ] Dark and Light override it. The title bar follows the theme.
- [ ] Light mode: bone ground (#F3EFE7), soot text, celadon accent, brick errors. Check the list, every wheel, a failed row, the details dialog and Settings.
- [ ] Nothing in either theme uses pure black, pure white or orange.
