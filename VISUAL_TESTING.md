# Visual testing

Run this checklist on real Windows before a release, and whenever a change
touches drawing, layout, the tokens or the glyphs. Automated tests pin the
tokens (`crates/kiln-app/tests/snapshots/fluent_tokens.snap`), the contrast of
every pair, the layout grid and the UI Automation tree, but only eyes can judge
the result. [DESIGN.md](DESIGN.md) is what the window should look like.

Check every item in **dark, light and a contrast theme**, at **100%, 125% and
200%** (Settings > System > Display > Scale). With two monitors at different
scales, drag the window between them as well. Set Text size (Settings >
Accessibility > Text size) to 150% once and look at every surface again.

A fresh settings file saves each result next to its original. A step that needs
a result waiting in Kiln ("Converted, not saved yet", **Save**, **Save all**)
says so: turn **Keep results in Kiln until I save them** on in Settings > Saving
first, and off afterwards.

## Window and title bar

- [ ] The window opens with no black, white or empty frame first. Start it ten times, five dark and five light, with and without files listed.
- [ ] On Windows 11 22H2 and later, Mica shows behind the title bar and around the content layer: over a strongly coloured wallpaper the window takes a faint tint of it. With Transparency effects off, on Windows 10 and under a contrast theme the base is solid (`#202020` dark, `#F3F3F3` light, the theme's window colour).
- [ ] The title bar is 48 DIPs: the K and "Kiln" at the left, Help and Settings, then Minimise, Maximise and Close in Segoe Fluent Icons. No second set of caption buttons shows under Kiln's at any scale.
- [ ] Close lights red under the pointer; Minimise and Maximise light softly. Hovering Maximise shows Snap Layouts.
- [ ] Dragging the empty title bar moves the window; a double click maximises and restores. The frame and the top edge resize it, and over the caption buttons only the top few pixels do.
- [ ] Maximised on a monitor whose taskbar hides itself, the taskbar still slides out at the screen's edge.
- [ ] "Kiln" turns secondary when another window is active.
- [ ] Alt+Space opens the window menu.
- [ ] Growing the window fast by a corner, and maximising and restoring, never shows the desktop or an empty edge.
- [ ] Minimised or hidden in the notification area with a file converting, Kiln uses no CPU in Task Manager; restored, the progress moves again at once.

## The empty window

- [ ] The content layer holds the K, "Drop files to convert", "Photos, videos, sound, documents and e-books." and **Add files** in the accent.
- [ ] With a fresh settings file, a "How it works" card sits under them with its three numbered steps and the right-click line, **Add files**, **Make smaller**, **Show in folder** and **Open with** in semibold. With **Keep results in Kiln until I save them** on, step 3 reads "The new file waits in Kiln until you save it."
- [ ] **Dismiss** takes the card away for good. Help's **Show first-run help** brings it back.
- [ ] Where the window is too short for everything, the K goes first; nothing overlaps.
- [ ] Dragging files over the empty window shows a dashed accent outline inside the layer.
- [ ] **Add files** and Ctrl+O open the Windows Open dialog. Ctrl+V adds files copied in File Explorer.

## The list

- [ ] Each row has a 40-DIP thumbnail (or the shell's icon for the type), the name in Body and what the file is in Caption, such as `3000 × 2000 photo · 3.2 MB`. No rules between rows.
- [ ] Hover fills a row softly; a selected row has the accent's 10% fill and a 3 × 16 jade pill at its left, and its name in Body Strong. Under a contrast theme it is Highlight with HighlightText, and its words stay readable.
- [ ] A converting row reads `Converting to MP4` over a progress bar with its percentage and **Cancel** at the right. A bar with no percentage runs across and back to the start again; with Animation effects off it stands still in the middle.
- [ ] A finished row has a checkmark, `Saved as name.mp4 · 31 MB → 9.6 MB, 69% smaller` and **Show in folder**.
- [ ] A failed row says what happened in one line, has **Show details**, and **Retry** only where trying again can help. Hovering a line cut short shows it whole.
- [ ] The footer reads `Working on 1 file.` or `Working on 3 files.` while files convert, and has **Add files**, **Save all (n)** while results wait, **Clear list** and **Convert** in the accent. With the side pane showing it reads `Convert to JPG` (the pane's outlined format) and converts at once, and `Convert 5 files to JPG` with several selected; with the pane folded it reads `Convert` and opens the list of formats above the footer. Enter on the list does the same. A result or an alert goes after five seconds or at the next selection change.
- [ ] The right-click menu, the Menu key and Shift+F10 open a Fluent menu in the window: **Convert to…**, **Tools…** or **Make smaller**, **Save…**, **Open**, **Cancel**, **Retry**, **Show in folder**, **Copy path**, **Remove from list**. Up, Down, Home, End, Enter and Esc work, and a click off it closes it.
- [ ] Files of kinds that share no format give an InfoBar at the top of the list: `3 files added. They don't share a format, so pick one file or one kind at a time.` It is in the Tab order, its Dismiss works from the keyboard, and Narrator reads it as it shows.
- [ ] A file in a format that is already the target reads `Already JPG`; one that can't become it reads `Can't become WEBP`.

## The side pane

- [ ] In a window 760 DIPs wide or more, the side pane is beside the list, 320 to 400 DIPs wide as the window widens; narrower, it folds away.
- [ ] One file: its picture, name in Subtitle, what it is, its size and folder, then **Convert to** with up to five formats, each with what it is for and its estimate, **Make smaller** first where it applies, then **More formats and tools**.
- [ ] Converting: the percentage in Title, a progress bar, `About 40 sec left` and **Cancel**.
- [ ] Finished: a success InfoBar (`Saved`, or `Converted, not saved yet`) with **Show in folder** or **Save**, and **Open**, then **Convert again**.
- [ ] Failed: the picture, the name, then a critical InfoBar with what happened, **Retry** where it can help and **Show details** (Remove from list is in the row menu and on Delete).
- [ ] A long file name takes a second line of the title. Facts are 20 DIPs apart with their values in one column. Format codes are in one style (14 DIPs, Semibold, tracked) with what each is for starting in one column. Two files' and three files' pictures stand in the same block, 12 DIPs in from each side.

## The wheel

- [ ] A plain drop of files of one kind opens the format wheel where they landed, over a 20% shade of the window. Esc closes it and the files stay listed.
- [ ] One flat ring: 2-DIP gaps, each format's code on its segment, Make smaller and More as a glyph over a caption, the kind as a thin arc inside the outer edge. The lit segment is solid jade with its code readable.
- [ ] The hub shows the file's thumbnail (or its kind's glyph), `JPG → PNG` and the estimate, or `Pick a format`.
- [ ] It opens with a short fade as it grows from 92%; nothing turns, springs or glows. With Animation effects off it is there at once.
- [ ] Picking a format converts at once. Tab on a lit format, or **Options…** in More, opens the options dialog first. A held Enter picks nothing until it is pressed again.
- [ ] More opens a list beside the ring, 360 DIPs wide, with each kind's formats under its heading (**Photos**, **Video**, **Sound**), each with what it is for over at most two lines, then the tools with their glyphs (**Remove location data** has a map pin). Its height is a whole number of rows with a scroll indicator showing at rest where it goes on, and **Options for JPG…** (the code of the lit format, else the suggested one, else the first with settings; the row is left out when no format has any) stays at the bottom under a rule and always opens that format's options. Up, Down, Enter, Esc and Left work in it, and the wheel scrolls it. In a window under 760 DIPs the footer's **Convert**, and in a wider one the pane's **More formats and tools** and the row menu's **Convert to…**, open the same list with no ring, to the left of the pane, never over it. The code column and the words match the side pane's rows.
- [ ] The wheel stays inside the window near every edge and corner.

## The drag-wheel

- [ ] It is off in a fresh settings file. A `settings.toml` that had it on shows, once, `The drag-wheel is now off. You can turn it back on in Settings.` at the top of the window.
- [ ] Turned on in Settings > Drag-wheel, dragging a file anywhere in Windows and tapping Shift opens the same ring under the pointer. A drop on a format converts at once and saves next to the original (or in the folder chosen in Settings); a drop on Make smaller brings Kiln forward on its dialog.
- [ ] While it is on, closing the window leaves Kiln in the notification area, and Task Manager lists Kiln under Startup apps. Turned off, the Startup entry goes and closing the window quits.
- [ ] Signed out and in again with it on, Kiln starts in the notification area with no window shown, and the drag-wheel works.
- [ ] The notification area icon's menu reads **Open Kiln**, **Drag-wheel** (checked) and **Quit Kiln**.

## Dialogs

Every dialog fades in and settles over 250 ms (at once with Animation effects off), over a 30% shade, with its action band at the foot.

- [ ] **Closing with results held** asks `Save before closing?`, `1 converted file isn't saved yet.`, with **Save all**, **Discard** and **Cancel**. The keyboard starts on Save all; Tab and Shift+Tab go round the buttons; Esc is Cancel, which keeps everything.
- [ ] **Closing while files convert** asks `Stop converting and close?` with **Keep converting** and **Close**.
- [ ] **Make smaller**: `clip.mp4 · 31 MB. Keeps MP4.`, Max size as radio buttons with the presets for the kind (video 10, 25 and 50 MB; photos 500 KB, 1 and 2 MB; PDF 1, 5 and 10 MB) and **Custom**; a preset at or above the file's size is disabled and says `already smaller`. The estimate card reads `Working out the size…`, then `About 9.6 MB`, what becomes of the picture and where the file goes. A size it can't reach says `Can't get this under 5 MB.` and Make smaller waits. **More options** opens and closes with its chevron.
- [ ] **Resize**: `The long side of the picture.`, 3840, 2560, 1920 and 1280 px and Custom.
- [ ] **A conversion's options**: `Convert to MP4`, the format and the file, what the conversion leaves out, Quality as **Small**, **High** and **Best** radio buttons (video adds **Original (no re-encode)**), the size it would come to as one line (`About 1.1 MB`), and **More options**, whose **Quality** drop-down lists every step: the only place `q5`, `V2` or `kbps` show. The arrow keys move between the radios and pick as they go; Tab treats them as one stop; Narrator reads each as a selection item.
- [ ] **Details**: the helper's lines in Cascadia Mono in a card that scrolls, **Copy** and **Close**.
- [ ] **Help** (F1): `Convert files`, `Shortcuts`, **Show first-run help** and **Close**; in a short window its content scrolls and Tab brings the link into view.
- [ ] Buttons whose words don't fit side by side stand one under another.
- [ ] Under a contrast theme no dialog has a shadow or shade, and each has a full WindowText edge.

## Settings

- [ ] Settings (Ctrl+, or the title bar) is one page of cards under the title bar, with **Back** in the title bar; Esc or Back closes it.
- [ ] Sections in order: Appearance, Saving, Quality, Drag-wheel, Helper apps, About. The page scrolls; a card too narrow for its words puts its control under them.
- [ ] **Where new files go**: **A folder I choose…** opens a folder picker, and Cancel there keeps **Next to the original**. With a folder chosen, a conversion lands there under a free name. Rename that folder away: the next result stays in Kiln and its row says `Kiln can't save in Converted, so it's kept here until you save it.`, and the card says `Not found at` and the folder.
- [ ] **Helper apps**: FFmpeg reads `Included` with **Use another copy…**; LibreOffice, Pandoc and Calibre read `Found` with where, or `Not found` with **Get it** and **Choose…**; a helper chosen by hand has **Reset**.
- [ ] **About**: the version with **Check for updates**, and **Licences and third-party notices** with **Open**, which opens them in Notepad.
- [ ] Tab goes through every control in order, with Back between the last and the first.

## Notifications

- [ ] Installed Kiln: a result saved while another window is in front gives a Windows notification `Saved photo.jpg` over its sizes and folder, with **Show in folder** and **Dismiss**. Several give `Saved 3 files`. Show in folder shows them in their folder.
- [ ] Portable Kiln: the same news comes as a balloon, and a click on it shows the saved results.

## Themes, contrast and motion

- [ ] Kiln follows the Windows app mode as it changes, and Settings > Appearance's Light and Dark override it.
- [ ] Under High Contrast Black and White (Settings > Accessibility > Contrast themes): no Mica, no alpha and no shadows; every control and card has a 1-DIP edge; selection and the lit wheel segment use the theme's highlight; links use its hyperlink colour. Turning a contrast theme on and off while Kiln runs redraws it at once.
- [ ] With Animation effects off (Settings > Accessibility > Visual effects) nothing moves, and nothing is lost: every state still shows.
- [ ] Text is never pure white on pure black or pure black on pure white outside a contrast theme.

## Keyboard and Narrator

- [ ] Tab reaches every control in the order it is drawn, and the two-tone focus visual shows only for the keyboard.
- [ ] Narrator reads each control's role and name, each Settings card's note on its control, a dialog's text as it opens, and InfoBars and estimates as they change.
- [ ] Accessibility Insights for Windows finds no control without a name and no dialog that isn't a Window.

## Clean PC

On a PC with nothing installed but Kiln (Windows Sandbox or a fresh VM, the portable ZIP):

- [ ] HEIC to JPG, with Microsoft's HEIF Image and HEVC Video Extensions; without them, the row names both.
- [ ] Video Make smaller, photo Make smaller and Resize, Remove location data, PDF Make smaller.
- [ ] WebM to MP4 with the FFmpeg that comes with Kiln.
- [ ] DOCX to PDF says it needs LibreOffice.

## Video extensions

Use a PC without the extension, with FFmpeg hidden from Kiln (Settings > Helper apps > Use another copy… pointed at a missing file). A debug build run with `KILN_HIDE_HEVC` set stands in for a PC without the HEVC extension.

- [ ] An HEVC MP4 reads `Needs Microsoft's HEVC Video Extensions to re-encode HEVC video.` and keeps its Convert; **Get it** in the side pane opens the extension's page in the Microsoft Store.
- [ ] Converting it with nothing changed copies it; a change that re-encodes fails with `Needs Microsoft's HEVC Video Extensions to read HEVC video.` and **Get it**.
- [ ] After the extension is installed, switch back to Kiln: the row says what the file is, without a restart.
- [ ] With FFmpeg available, the same file shows no extension line and converts through FFmpeg.

## Windows' Media Feature Pack

Use an N edition of Windows without the pack, or a debug build run with `KILN_HIDE_MF` set.

- [ ] A WMA or AC3 file reads `Needs Windows' Media Feature Pack, which isn't installed.` with **Get Media Feature Pack**, which opens Settings on Optional features.
- [ ] After the pack is added, switch back to Kiln: the row lists every format again, without a restart.

## App icon

- [ ] Explorer, the taskbar and Alt+Tab show the K seal at 256 and 48 px and the flat K at 32 and 16 px. The title bar's K is the 16-DIP jade tile.

## Results on two drives

Needs a second local drive, here called D:. Start Kiln from a console where `APPDATA` and `LOCALAPPDATA` are set to scratch folders, turn **Keep results in Kiln until I save them** on, and show hidden files in Explorer.

- [ ] **Held on the source's drive.** Convert `clip.mp4` (a few hundred MB) and `photo.png` from D: and `snap.png` from C:. D: now has a hidden `.kiln-held` folder holding the first two; the third is in `%LOCALAPPDATA%\Kiln\held`.
- [ ] **Same drive.** Save `photo.png`'s result to another folder on D: with Ctrl+S. It is there at once.
- [ ] **Across drives, cancelled.** Save `clip.mp4`'s result to a folder on C:. Its row reads `Saving…` with a climbing percentage; cancel it. The row is held again and no partial file is left.
- [ ] **Across drives, finished.** Save it again and let it end: the file is on C:, and the held copy on D: is gone.
- [ ] **Quit.** With results still held on D:, close and choose **Discard**. D: no longer has a `.kiln-held` folder.

## Logs, crashes and recovery

`crates/kiln-app/tests/crash.rs` and `recovery.rs` drive these without looking at them. This is for how they read and look. Use scratch `APPDATA` and `LOCALAPPDATA` as above.

- [ ] **The log.** A failed conversion leaves a line in `%LOCALAPPDATA%\Kiln\logs\kiln.log` with the time, the input, the target and the message.
- [ ] **The recovery prompt.** Leave two results held and end Kiln in Task Manager. Start it: the dialog reads `2 converted files from last time weren't saved.` with **Save next to the originals**, **Save to a folder…**, **Discard** and **Cancel**. Cancel and Esc keep them for the next start.
- [ ] **Save next to the originals** puts each beside its original under a free name, and the footer reads `Saved 2 files from last time.`
- [ ] **After a crash** the logs folder has `crash-<date>-<time>-<process>.txt`, and the next start offers the held results.

## Capture scripts

The maintainer's scripts that take a phase's screenshots and numbers are kept outside the repository. None of them sends mouse or keyboard input: they post messages to Kiln's own windows and capture with PrintWindow.

- `shots.ps1` takes every reachable surface for each theme and scale, with each shot's UI Automation tree.
- `metrics.ps1` measures start-up (the first frame on screen), memory, idle and minimised CPU, and frame pacing.
- `compare.ps1` pairs the shots of two captures by name and highlights what differs.

Test hooks, set in the environment before starting Kiln: `KILN_TEST_CONTRAST=1` (High Contrast Black without changing Windows), `KILN_TEST_TRANSPARENCY=0` (Transparency effects taken as off; debug builds), `KILN_FRAME_LOG=<path>` (a line for each frame), `KILN_TEST_DRIVES` (the drives recovery looks on; debug builds), `KILN_TEST_TYPE_PAGE=1` (every type step and glyph on one page; debug builds) and `KILN_TEST_CONTROLS_PAGE=1` to `6` (the controls kit, one page at a time; debug builds). A scratch `settings.toml` with `theme = 'light'` or `'dark'` sets the theme.
