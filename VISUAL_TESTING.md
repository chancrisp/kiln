# Visual testing

Run this checklist on real Windows before a release, and whenever a change
touches drawing, layout, theme tokens or icons. Automated tests pin the
tokens (`crates/kiln-app/tests/snapshots/theme_tokens.snap`) and the layout
grid, but only eyes can judge the result.

Set scaling in Settings > System > Display > Scale. Check every item at
**100%, 150% and 200%**. With two monitors at different scales, drag the
window between them as well.

## Window

- [ ] Opens at once (the window is up within 100 ms), with no white flash before the first frame.
- [ ] Title bar is dark (Windows 10 20H1+). On Windows 11 the caption is soot with bone text.
- [ ] The background shows a faint, still grain. It never shimmers or moves.
- [ ] No pure black or pure white anywhere. No orange anywhere.
- [ ] The header shows the celadon mark, the flame K, and "Kiln" in the bundled serif, not Georgia.
- [ ] Panels have 12 px corners, controls 6 px. Borders are a single 1 px line, never doubled.
- [ ] Text is sharp at every scale, with no blurry bitmap stretching.
- [ ] Resizing never goes below 480 x 360 DIPs, and nothing overlaps at the minimum size.

## Empty state

- [ ] The drop panel fills the body with 16 px padding on every side.
- [ ] Copy reads, in order: "Drop files here", "Or add them with the button below, or Ctrl+O.", an **Add files** button, the key hint, the privacy line, and last, muted, "Fire raw files into finished ones."
- [ ] **Add files** and Ctrl+O open the Windows Open dialog with multi-select. Picked files are listed as a drop would list them. The next time, it opens in the folder files were last added from.
- [ ] Copying files in File Explorer and pressing Ctrl+V in Kiln lists them. A copied folder is reported as skipped, as a dropped one is. With no files on the clipboard, the footer reads "There are no files to paste."
- [ ] In the empty window, a status such as "There are no files to paste." or "Skipped 1 item that isn't a file." shows as one muted line under the **Add files** button. The group doesn't move when it appears.
- [ ] Dragging files over the window fades in a celadon edge and a faint wash in about 120 ms, and leaving fades them out.
- [ ] Dragging something that isn't a file (text from a browser) shows the "no drop" cursor and no highlight.

## File list

- [ ] Dropping files adds one row each, rising a little and fading in over about a quarter of a second, one after another. Adding 500 files doesn't crawl: every row is in place within about two thirds of a second. Nothing bounces.
- [ ] Format chips are monospace, uppercase and slightly tracked (PNG, MP4, DOCX).
- [ ] Rows show the preview, the chip, the name in bone, then in stone the number of conversions (or what the file is), the size and the folder's last part. With a suggestion, it comes last in the file's glaze ink ("as JPG about 890 KB") and the folder gives way to it.
- [ ] Hovering a row shows a native tooltip: the whole line when the row cuts it short, then the full folder path on its own line. A running row's Cancel says "Cancel" and a held row's Export link says "Export (Ctrl+S)".
- [ ] A file with no conversions says "No formats for this type".
- [ ] Dropping the same file twice reports it as already in the list. Dropping a folder reports it as skipped.
- [ ] Hover tints a row; every selected row has the 18% celadon fill, clipped to the panel's rounded corners.
- [ ] Up, Down, Home and End move the selection to one row. With Shift they extend it from where it started. Delete removes it. Escape clears it.
- [ ] A click selects one row, Ctrl-click adds or takes away a row, Shift-click selects a range, and Ctrl+A selects every row.
- [ ] With more than one row selected, the footer reads "3 selected", over the idle hint and over a finished batch's or export's summary, which comes back when one row or none is selected. "Working on…", "Saving…" and lines about something that went wrong still show instead.
- [ ] Space follows a row's link only with one row selected. Ctrl or Shift with a double-click on a held row selects, and doesn't open the result.
- [ ] Enter, Delete, Ctrl+S and the right-click menu act on every selected row. Removing several rows asks once, counting all of them. Ctrl+S on several held rows asks for one folder.
- [ ] Right-clicking a row outside the selection selects only that row first. Right-clicking a selected row keeps the selection. With two or more PDFs selected, Tools… offers Merge.
- [ ] With more rows than fit, the wheel scrolls and a thin thumb appears on the right.
- [ ] "Clear" shows a soft inner shadow while pressed and empties the list on release.
- [ ] Dropping files on the app's icon in Explorer opens Kiln with them listed.
- [ ] With Kiln already open, dropping more files on the icon adds them to that window and brings it forward. No second window opens.
- [ ] The empty list's hint names the current wheel keys, and changes when they're reassigned in Settings. It isn't cut off at the smallest window size.
- [ ] With files listed, the footer has an **Add files** button left of Export all and Clear, the same height and corners as Clear, and a **Convert** button left of Add files.
- [ ] Convert is quiet (muted label, no hover) with nothing selected, and a press says "Select a file to convert." It is also quiet while a selected file is converting. Select a file and it comes to life; pressing it opens the format choice beside the button.
- [ ] In a window narrower than 760 px, a file nothing has been done to has a **Convert** link at the right end of its row; it selects the row and opens the format choice beside the row, with the file's name still readable. Enter on a selected row opens the same.
- [ ] Selecting an MP4 and a DOCX and pressing Convert (or Enter) opens no wheel: the footer says "These files have no format in common: …" and Ctrl+A on many mixed files says the same.
- [ ] The first plain drop that adds files shows "Tip: hold Shift, Ctrl or Alt as you drop." in the footer. A second drop, and any drop after restarting, does not.
- [ ] Tab goes from the list to Convert (while enabled), then Add files.
- [ ] With FFmpeg hidden from Kiln, an MP4 reads "Needs FFmpeg to convert" with a **Get FFmpeg** link that opens the download page, and the footer says once that FFmpeg isn't installed. Opening the choice for it explains the same. Installing it (or choosing it in Settings) and closing Settings brings its formats back without re-adding the file.
- [ ] An empty file reads "Empty file (0 bytes)", a DOCX made of random bytes reads "Isn't really a DOCX file", and a file with an unknown extension reads "Unknown type (.XYZ)". None has a Convert link or a wheel. A real DOCX, a PNG with trailing data and a text file renamed .txt are not rejected.
- [ ] Dropping a folder of photos adds them: "Added 12 files from Holiday." Hidden items and folders two levels down are named as skipped. A folder with more than 200 files asks "Add them all?" first.
- [ ] "Added 2 files." stays two seconds, then the footer reads "Right-click a file, or select it and press Enter, to convert it." while nothing is running, without moving the pointer. Where that doesn't fit, as in the smallest window, it reads "Right-click a file to convert it." A line that says files were already listed or skipped stays.
- [ ] Right-clicking a row opens a Windows menu: Convert…, Tools… and File actions… with their keys, then Export… and Open on a held row or Show in folder on a saved one, then Remove. Items that don't apply are missing, not greyed.
- [ ] The Menu key or Shift+F10 opens the same menu at the selected row's bottom left. Right-clicking an empty part of the list offers Add files… and, with files on the clipboard, Paste.

## Glaze wheel

- [ ] Holding Shift while dragging files over the window opens the convert wheel at the cursor in about 200 ms: it grows and turns slightly into place as it fades in, and its segments follow one after another. Ctrl opens the tools wheel instead.
- [ ] The wheel stays fully inside the window near every edge and corner.
- [ ] The centre shows "Convert" or "Tools" in the serif, the key that opens it ("Shift", "Ctrl") muted below, then the source type (PNG, or MIXED). The key follows Settings.
- [ ] Hovering a segment fills it with celadon at 18% and draws a fine celadon arc on its outer edge. Icons and labels keep their colour.
- [ ] A single-item wheel lights the whole ring as one smooth circle.
- [ ] Letting go of the key mid-drag, or pressing Escape, fades the wheel out over about 120 ms.
- [ ] Dropping on an item pulses it once, then the job starts and the wheel fades. Dropping on the centre just adds the files.
- [ ] With a target that has options lit (opened from a row, not mid-drag), the centre shows an underlined "Options… (Tab)" in place of the key. Moving the pointer onto it keeps the target lit, it brightens under the pointer, and a click opens the options sheet, as Tab does.
- [ ] Enter on a row opens the convert wheel and Ctrl+Enter the tools wheel. A wheel chosen from the right-click menu opens over the row. Arrows move round it, Enter applies, Escape closes.
- [ ] Tools offered match the file: Merge appears only for two or more files of one type, Crop never for audio.
- [ ] Office types use their own icons in the convert wheel: a page for DOC, DOCX, ODT, RTF and PDF, a sheet for XLSX, XLS, ODS and CSV, and a screen on a stand for PPTX, PPT and ODP.

## Jobs

- [ ] A running row shows "Converting to WEBM" and a thin line in the file's glaze along its bottom that brightens at its leading edge. The glow breathes slowly; nothing flickers. No spinners anywhere.
- [ ] A running row shows a **Cancel** button at the right of its second line without hovering; clicking it cancels the job and the row says "Cancelled." with a Retry link. There is no cross.
- [ ] A running row reads "Converting to WEBM  ·  38%  ·  about 2 min" (the time appears after a few seconds), or "Converting to WEBM  ·  Working…" when no progress is known. The progress line is 4 px thick.
- [ ] With a running file selected, the footer's button reads Cancel, can be reached with Tab and pressed with Enter or Space, and its tooltip says "Cancel the converting files (Space)". The footer reads "3 of 50 done. Space cancels the selected file."
- [ ] While work runs, the taskbar button shows progress. With Kiln behind another window when the last job ends, the taskbar button flashes and a notification says "photo.jpg is ready."; clicking it brings Kiln forward on that row. With Windows animations off there is no flash.
- [ ] When a job finishes, the row flashes in its glaze and cools over about a second as the output name settles in, in bone. A failed row shakes once, slightly, as its line turns brick.
- [ ] A failed job shows one plain line in brick and, when the helper app said more, a "Show details" link. Clicking it, or pressing Space on the row, opens the details over a scrim. Escape, Enter or Close dismisses it.
- [ ] A failed row never shows a file path or the helper's own words, and ends with a next step ("Try opening it in another app.", "Try again."). Where trying again can help, or after a cancel, a Retry link sits beside Show details (or alone) and runs the job again; a damaged file gets none. The right-click menu has Retry too.
- [ ] The footer sums up each batch calmly: "Converted 3 files to JPG.", "Couldn't convert the file to FLAC. The row says why.". No exclamation marks.
- [ ] Clear and Delete cancel running jobs first and leave no partial files behind.

## Look

- [ ] **Previews in rows.** Each row starts with a 34 px square preview with 6 px corners: a photo, a video frame or a PDF page, cropped to fill, never stretched. It fades in as it arrives. A file with no thumbnail (an MP3, an empty or broken file, an unknown type) shows its glaze tile: the glaze's chip colour with the kind's icon in the glaze, and a plain tile for a type Kiln doesn't know. A PNG with transparent or half-transparent parts shows on its tile with no bright fringe or halo, in dark and light.
- [ ] **Glaze chips in both themes.** Chips follow the file's kind: celadon for images, blue for video, yellow for audio, lavender for documents (spreadsheets, slides and subtitles too), stone for anything else. Each is an opaque tint with a fine outline of its glaze and its code in the glaze's ink, readable on the panel, a hovered row and a selected row. A row's suggestion ("as WEBP about 410 KB") is in the same ink, after the muted facts. Under a contrast theme chips are outlined in the theme's colours, with no tint.
- [ ] **The converting wash.** A converting row has a faint wash of its glaze at the left, gone by 60% of the way across, and its heat line along the bottom is in the glaze, its leading edge breathing brighter. Under a contrast theme the line is the highlight colour, with no wash and no glow.
- [ ] **Depth on the panel.** The list panel (and the empty window's panel) casts one soft shadow, darker below than at the sides, and has a 1 px light along its top edge. Settings, Help, the details dialog, the options sheet, the wheel and the footer's celadon Convert button do the same. The ground has a faint celadon glow at the top centre under the grain. A contrast theme has no shadow, light or glow.
- [ ] **The empty window's tiles.** The empty window shows the mark in celadon line work (72 px, 2 px line) and, under the privacy line, four tiles: Images, Video, Audio and Documents, each its glaze with its icon and its name under it in the glaze's ink. In a window too short for them (480 x 360) the tiles are left out and nothing overlaps.
- [ ] **Links and the inspector.** In a window 760 px wide or more (with files listed), rows show no action link in any state: no Convert, Export, Show details or Show in folder, no Cancel on a converting row, and an exported row's "Saved to" line isn't underlined or clickable. Narrator finds none of them either, and Retry, where a row has it, sits at the row's right edge, where Narrator places it too. Space on a selected row follows no link then, but still cancels a converting one (the footer's Cancel). Narrower, rows show their links as before. The scroll thumb stays at the list's right edge.
- [ ] **Convert's key.** While the footer's button reads Convert it shows the word and, after it, "Enter" in a fine keycap, the two centred together. It reads plain "Cancel" while a selected file converts, and plain muted "Convert" while disabled. In the smallest window (480 x 360) the keycap is left out so the short idle hint still fits beside the buttons. Narrator reads the button as "Convert".
- [ ] **One text column.** The file names and second lines of PNG, MP4, DOCX and XLSX rows all start at the same x; only a longer code (five letters or more) pushes its own row's text right.
- [ ] **Audio keeps its tile.** An MP3 or WAV row always shows its yellow tile with the waveform, even when the file has album art.
- [ ] **The fold at 760.** With files listed, narrow the window past 760 DIPs (950 pixels at 125%): the inspector slides out to the right over about a fifth of a second as the list widens, and the rows' links come back. Widening brings it back the same way. The header and footer never jump. At a larger Text size it folds in a wider window. Minimising and restoring the window leaves it where it was.
- [ ] **The inspector, case by case.** One file waiting: its preview, chip and name, what the format is, Size, Pixels or Length, and Folder, then under "Turn it into" up to three formats, the suggestion first with its dot and "≈ 890 KB", then "More formats…". Converting: the glaze's heat rising over the preview, the percentage in large figures, the time left, and Cancel. Done: the result's name, its size before and after, a check on the preview, and Save, which becomes Show in folder once saved. Failed: the sentence in brick, Retry where it helps, and Show details. Several files: up to three stacked previews and "1 video, 1 document"; with no format in common, the footer's sentence and "Select one kind", each kind's button selecting only its files. Nothing selected: the mark, the hint and the privacy line.
- [ ] **The inspector on the keyboard.** Tab goes from the list through the inspector's formats, More formats… and its buttons, then the footer. Narrator reads the panel as "Selected file", a format as "Convert to JPG, a photo that opens anywhere, about 890 KB", and the preview as "Preview of harbour.png".
- [ ] **Previews keep up.** In a list of 500 files, previews fill in nearest first as you scroll, and the list never stutters while they load. The inspector's preview is 268 x 120, cropped to fill, never stretched; with no thumbnail it is the glaze tile with a larger icon.
- [ ] **Glazes in both themes.** In dark and in light, the inspector's top wash, the heat over its preview and its formats' chips take the selected file's glaze, and all its text stays readable. A video, an image, an MP3 and a DOCX side by side are blue, celadon, yellow and lavender.
- [ ] **Glazes under High Contrast.** Under a Windows contrast theme there are no tints, washes, glows or shadows: chips and preview tiles are outlined in the theme's colours, every kind is the same colour (the code tells them apart), the inspector has no wash, and a converting file's heat in the inspector is a line in the highlight colour.

## Motion

- [ ] Rows pour in: each rises a little and fades in, one after another. Up and Down glide the selection's wash and edge to the next row; a long jump doesn't glide.
- [ ] Selecting another file cross-fades the inspector, the new content rising a little; a preview fades in when it arrives.
- [ ] The wheel blooms open, its segments following, and fades out quickly when it closes; More's ring pours open and draws back. Settings, Help and the options sheet slide up and fade in over the scrim.
- [ ] Buttons ease their hover and press: the header's, the footer's, the inspector's, Settings', Help's Close, the details dialog's and the options sheet's.
- [ ] A finished row flashes in its glaze and cools; a failed one cracks, a small shake left then right, once. A converting row's heat breathes slowly.
- [ ] **Animation effects off.** Turn off Settings > Accessibility > Visual effects > Animation effects while Kiln is open. Rows appear at once and in place, the selection moves without gliding, the inspector's content changes with no fade and it folds and unfolds at once, and previews appear without fading. The wheel, the More ring, Settings, Help and the options sheet open and close at once, with no pulse on a chosen item. Buttons change at once. A finished row doesn't flash and a failed one doesn't shake. Progress lines hold still but still show how far, and the taskbar button doesn't flash when the last job ends behind another window. Turning it back on brings the motion back without restarting Kiln.

## App icon

- [ ] Explorer, the taskbar and Alt+Tab show the flame K seal on dark stoneware at 256 and 48 px and the flat K on an ash tile at 32 and 16 px.

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
- [ ] Every button, key button, theme segment and the dialog's Close show a soft inner shadow while held, and act only when released over them.
- [ ] Check for updates says "Checking for updates…", then the result in one calm line beside the button. With no network it says "Couldn't reach GitHub.". When a newer version exists, the button reads "Get the update" and opens the releases page.
- [ ] Folder shown first when saving: Choose picks a folder and save dialogs open there; Reset goes back to "Last export folder".
- [ ] Labels read "Sound quality", "Sharpness (DPI)" and "Keys to hold while dropping" (the last two on two lines), and the tabs "General", "Quality" and "Helper apps". None is cut off at the smallest window size.
- [ ] Keys to hold while dropping: pressing a key button moves that wheel to the next key and swaps with the wheel that had it. Dragging follows the new keys at once.
- [ ] Helper apps shows "Found on PATH", "Not installed" or the chosen path for FFmpeg, Pandoc and LibreOffice. LibreOffice installed in Program Files shows its path without being chosen. Choose takes an .exe; jobs use it straight away.
- [ ] Every change is saved to %APPDATA%\Kiln\settings.toml. A broken file starts Kiln with defaults and a footer line saying why.

## Help and tooltips

- [ ] The ? beside the dial, or F1, opens help over a scrim. F1 does nothing while Settings, an options sheet or the details dialog is up, and closes an open wheel.
- [ ] Help lists the ways in, that nothing is saved until you export, and every shortcut, following the keys chosen in Settings. At the smallest window it scrolls with the wheel, arrows, Page Up, Page Down, Home and End, with a thin thumb.
- [ ] Close has the focus ring. Escape, Enter, Close or a click on the scrim closes help.
- [ ] Hovering the dial, the ?, Add files and Export all shows "Settings (Ctrl+,)", "Help (F1)", "Add files (Ctrl+O)" and "Export all (Ctrl+Shift+S)". No tooltips show while a wheel, panel or dialog is up. They are sized right after moving to a monitor with another scale.

## Themes

- [ ] With Theme on System, Kiln follows Settings > Personalization > Colors > app mode, and switches live when it changes.
- [ ] Dark and Light override it. The title bar follows the theme.
- [ ] Light mode: bone ground (#F3EFE7), soot text, celadon accent, brick errors. Check the list, every wheel, a failed row, the details dialog and Settings.
- [ ] Nothing in either theme uses pure black, pure white or orange.
- [ ] Settings > General has "Save results next to the original", On or Off, off by default. The first time a result is ready, Kiln asks once whether to save next to the original from now on; either answer is remembered, and the setting shows it.
- [ ] With it on, a converted file appears beside the original under a free name (`photo (1).jpg` when `photo.jpg` exists, never overwriting), the row reads "Saved next to the original: photo (1).jpg" with a **Show in folder** link, hovering shows the full path, and closing Kiln asks nothing.
- [ ] With it on and a folder that can't be written (read-only or full), the result stays held, the row reads "Couldn't save to <folder>. Choose another folder." with a **Choose folder** link, and closing Kiln asks about the unsaved result.
- [ ] With it off, a finished row reads "Done, not saved yet: photo.jpg"; hovering it says it can be dragged to a folder. Ctrl+S on one result and on several both open a folder picker ("Save photo.jpg to", "Save 3 files to"). "Export as…" in the row menu saves one result under a new name.
- [ ] Add files by keyboard (Ctrl+O), choose files, and the list shows the first new row, selected, with the focus ring on it; Enter opens the format choice, not the dialog. Adding 29 or 500 files leaves the list at the first added file, not the end.
- [ ] Tab from the list goes Convert (when enabled), Add files, Export all, Clear, then Help, Settings, and back to the list. In the empty window it goes Help, Settings, Add files. In Settings the three page tabs are one Tab stop (Left and Right change page; Shift+Tab from the first control lands on the page showing).
- [ ] Page Up and Page Down move a screenful through 500 rows (the pixels change); Shift extends the selection; Ctrl+Home and Ctrl+End go to the first and last row.
- [ ] Delete on three rows says "3 files removed." with an Undo link; Undo (or Ctrl+Z) brings them back in place, selected. Delete on one row says only the count. A removed running row comes back idle.
- [ ] With Narrator, a row reads as "photo.png. 64 by 48 photo. Suggested JPG… File size 9.3 KB. Folder files." and its buttons say their file ("Cancel photo.png"). In Help, Narrator doesn't read lines scrolled out of view.
- [ ] The window never flashes white on start, in dark or in light, at any scale.
- [ ] At the smallest window (480 x 360), in dark and light, Settings > General shows the three key buttons (Convert / SHIFT, Tools / CTRL, Actions / ALT) with nothing overlapping, and every label fits.
- [ ] In a window much wider than 880 px the header, list and footer sit in one centred column, rows have no links (the inspector beside the list carries the actions), and Settings and Help sit at the column's right end.
- [ ] Muted text (second lines, the footer line, Settings labels) is readable on the panel and on a selected row in both themes; button outlines and the lines between rows are visible but quiet.
- [ ] Captions are 13 px. The wheel's group names (Images, Video, ...) stay inside their slots, and Settings and Help fit at the smallest window.
- [ ] Settings reads "Folder shown first when saving" and "Reset suggestions" (Narrator: "Reset learned suggestions"). The empty window has one instruction and the privacy line, no tagline; Help ends with "About Kiln".
- [ ] Right-clicking a row offers Show in folder (the file, or its saved result) and Copy path; with several rows selected it offers Copy path only, and the clipboard holds one path per line.
