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

## Jobs

- [ ] A running row shows "Converting to WEBM" and a thin celadon line along its bottom that brightens at its leading edge. No spinners anywhere.
- [ ] Hovering or selecting a running row shows a cross; clicking it cancels the job and the row says "Cancelled.".
- [ ] When a job finishes, the output name appears in bone, fading in over about 200 ms.
- [ ] A failed job shows one plain line in brick and, when the engine said more, a "Show details" link. Clicking it, or pressing Space on the row, opens the details over a scrim. Escape, Enter or Close dismisses it.
- [ ] The footer sums up each batch calmly: "Converted 3 files to JPG.", "Couldn't convert the file to FLAC.". No exclamation marks.
- [ ] Clear and Delete cancel running jobs first and leave no partial files behind.

## App icon

- [ ] Explorer, the taskbar and Alt+Tab show the celadon vessel on a soot tile at 16, 32, 48 and 256 px.
