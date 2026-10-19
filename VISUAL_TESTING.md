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

## App icon

- [ ] Explorer, the taskbar and Alt+Tab show the celadon vessel on a soot tile at 16, 32, 48 and 256 px.
