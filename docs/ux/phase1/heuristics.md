# Heuristic evaluation

Kiln at `main` 2213fa5, release build, Windows 11, 100% scale. The evaluation was done from the source code and the screenshots in `docs/screenshots/` and `docs/ux/phase1/shots/`. Every finding cites the code or the screenshot it comes from.

The method is Nielsen's 10 usability heuristics, with his 0 to 4 severity scale:

| Severity | Meaning |
|---|---|
| 0 | Not a problem |
| 1 | Cosmetic |
| 2 | Minor |
| 3 | Major: important to fix |
| 4 | Catastrophe: fix before release |

Severity weighs how often a problem comes up, how much it hurts when it does, and whether it keeps hurting after people learn the app. Accessibility barriers have their own report, [accessibility.md](accessibility.md); the ones that also block sighted mouse users appear here too. The wording problems are listed string by string in [strings.md](strings.md).

One evaluator did this review. Nielsen's data says one evaluator finds about a third of the problems. Treat this list as a floor, and let the usability sessions confirm or reorder it.

## Ranked findings

| ID | Sev | Heuristic | Finding |
|---|---|---|---|
| H-01 | 4 | Consistency and standards | There is no way to add files except dragging them in |
| H-02 | 4 | Visibility of system status | Once files are listed, nothing says what to do next |
| H-03 | 3 | Consistency and standards | Only one file can be selected, so Merge and batch work need a modified drag |
| H-04 | 3 | Visibility, recognition | A finished file can't be opened or shown in its folder |
| H-05 | 3 | Recognition rather than recall | The Shift, Ctrl and Alt meanings must be remembered, and nothing on screen shows them |
| H-06 | 3 | User control and freedom | Clear cancels every running conversion, with no warning and no undo |
| H-07 | 3 | Error recovery | Error rows show the engine's raw first line, cut off, with no next step |
| H-08 | 3 | Match with the real world | Formats are shown only as codes (QOI, PPM, WV, ADOC, DBK) |
| H-20 | 3 | Error recovery | "{engine} isn't installed. Get it from Settings > Engines." leads to a page that can only point at a program already installed |
| H-21 | 2 | Error recovery | The details dialog clips raw engine output, with no scrolling |
| H-09 | 2 | Recognition rather than recall | Right-click opens the convert wheel, but nothing says so, and Tools and Actions have no mouse path |
| H-10 | 2 | Recognition rather than recall | The options sheet (Tab or right-click on a target) is found only from a footer line |
| H-11 | 2 | Recognition rather than recall | The Settings button is an unlabelled dial icon, and Kiln has no tooltips anywhere |
| H-12 | 2 | Match with the real world | Holding Shift, Ctrl or Alt during a drag already means move, copy and shortcut in Windows |
| H-13 | 2 | Match with the real world | Jargon: Engines, FFmpeg, Pandoc, bitrate, DPI, "Audio: Auto", "Wheel keys" |
| H-14 | 2 | Help and documentation | There is no help in the app: no first-run guide, no shortcut list, no "?" |
| H-15 | 2 | User control and freedom | Delete or Backspace on a running row quietly cancels its job, and Move has no undo |
| H-16 | 2 | Aesthetic and minimalist design | The More ring packs up to about 30 small monospace codes with no names |
| H-17 | 2 | Match with the real world | The empty state spends its second line on the tagline, not the instruction |
| H-18 | 1 | Error prevention | The wheel only opens while a key is held mid-drag, so early or late presses miss it |
| H-19 | 1 | Aesthetic and minimalist design | The row's second line leads with the folder, the least useful part, and truncates first |

## Findings in detail

### H-01 · Severity 4 · There is no way to add files except dragging them in

- **Evidence:** the app's only file picker is `system::pick` (`crates/kiln-app/src/win/system.rs:77`). It picks folders and engine programs, not inputs. There is no Open button, no Ctrl+O and no menu. Files come in only by drag and drop (`win/drop.rs`), as launch arguments, or by being handed to a running window (`win/handoff.rs`).
- **Why it matters:**
  - Every Windows app that opens files has an Open command, so this is the first thing many people look for.
  - People on a touchpad, people who find dragging hard, and keyboard-only users have no way in at all. For a screen reader user this blocks every task.
  - The empty state says "Drop files here", which assumes File Explorer is open beside Kiln.
- **Direction:** an "Add files" button in the empty state and the footer, Ctrl+O, and Kiln as a target in File Explorer's "Open with" and "Send to" menus.

### H-02 · Severity 4 · Once files are listed, nothing says what to do next

- **Evidence:** the instruction "Hold Shift while dropping to convert, Ctrl for tools, or Alt for file actions." appears only on the empty screen (`settings.rs:563`, drawn in `render.rs:577`). After a plain drop, the footer reads "Added 2 files." (`model.rs:458`), and the rows offer nothing to click (screenshot `m9/row-hint-dark-100.png`).
- **Why it matters:**
  - The likeliest first move, a plain drag and drop, leaves the person in a dead end.
  - The list's two working mouse paths have no visible cue: right-click and dropping again with a key held. A first-time user who missed or misread the empty-state hint has no way forward.
- **Direction:** a visible affordance on each row (a "Convert" button, or a chevron that opens the wheel), plus a footer hint whenever files are listed and nothing is running, such as "Right-click a file, or select it and press Enter, to convert it."

### H-03 · Severity 3 · Only one file can be selected, so Merge and batch work need a modified drag

- **Evidence:**
  - `FileList::select` takes a single index. `open_wheel_for_selection` (`win/mod.rs:670`) and the right-click path (`win/mod.rs:2471-2479`) both build `vec![one path]`.
  - Merge needs two or more files (DESIGN.md, The glaze wheel → Contents), so it can only be reached by dragging several files in while holding Ctrl.
  - There is no Ctrl-click, Shift-click or Ctrl+A.
- **Why it matters:**
  - "Combine these PDFs" and "turn all these photos into JPG" are core jobs.
  - Once files are in the list, the only way to do them is to go back to File Explorer and drag them all in again with the right key held.
  - Keyboard-only users can't merge at all.
- **Direction:** standard multi-select (Ctrl-click, Shift-click, Ctrl+A, Shift+arrows). The wheel then acts on the selection, or on every file when nothing is selected.

### H-04 · Severity 3 · A finished file can't be opened or shown in its folder

- **Evidence:** `hit_row` (`win/mod.rs:1590`) knows two targets in a row: cancel, on a running job, and "Show details", on a failed one. A finished row shows the output name (screenshot `m5/jobs-100.png`, "Glaze test.jpg") and nothing to click. The app has no `SHOpenFolderAndSelectItems` call and no ShellExecute open.
- **Why it matters:** the job isn't done until the person has the file. They have to know where Kiln puts it ("next to each file", or a folder set in Settings) and find it in File Explorer.
- **Direction:** clicking a finished row, or pressing Enter on it, opens the result. A "Show in folder" link sits where "Show details" sits on errors. The footer after a batch says "Converted 3 files to JPG. Show in folder".

### H-05 · Severity 3 · The Shift, Ctrl and Alt meanings must be remembered, and nothing on screen shows them

- **Evidence:**
  - The mapping appears only in the empty-state hint.
  - The wheels are titled "Convert", "Tools" and "Actions" with the source type below; their key isn't shown.
  - Settings shows "Wheel keys: Convert SHIFT, Tools CTRL, Actions ALT" (`m8/settings-general-dark-100.png`).
- **Why it matters:** recall is harder than recognition. Even after one success, a person who comes back a week later has to remember which key does which, and a wrong guess tells them nothing.
- **Direction:** show the key on every path into a wheel, for example a "Convert ⇧" button label and a footer hint, and make every wheel reachable without a held key (see H-01, H-02 and H-09).

### H-06 · Severity 3 · Clear cancels every running conversion, with no warning and no undo

- **Evidence:** `clear()` (`win/mod.rs:501`) calls `runner.cancel_all()`, `inspector.cancel_all()` and empties the list. The button is always shown in the footer, beside the status (`render.rs:855`).
- **Why it matters:**
  - "Clear" reads as "tidy the list", not "stop my work".
  - One click during a 20-minute video batch throws all of it away, and there's no way to get the list back.
- **Direction:** while jobs are running, label it "Cancel all" or ask first ("Stop 3 conversions and clear the list?"). Otherwise let Clear remove only finished and idle rows, and offer an Undo in the footer for a few seconds.

### H-07 · Severity 3 · Error rows show the engine's raw first line, cut off, with no next step

- **Evidence:** screenshot `m5/jobs-100.png` shows "FFmpeg couldn't convert the file: Error opening input: Invalid data found when proce…" in brick, with "Show details" opening the full engine output.
- **Why it matters:**
  - DESIGN.md's own voice rule says error messages "name the problem and the next step". This one names an engine, quotes its internals and gets cut off mid-word.
  - The most common real causes (a damaged file, a file that isn't what its extension says, a missing engine) each have a plain next step.
- **Direction:** map known engine failures to plain sentences with a next step ("This file looks damaged or isn't really an MP4. Try opening it in another app first."), keep the raw line under Show details, and never cut a message off without a way to read all of it.

### H-08 · Severity 3 · Formats are shown only as codes

- **Evidence:**
  - Wheel segments show an icon and a code. The More ring shows codes only, with no icons (DESIGN.md, The glaze wheel → More; `m8/wheel-more-light-100.png`).
  - The codes include QOI, PPM, TGA, JXL, WV, AC3, ADOC, DBK, TEXTILE and FB2.
- **Why it matters:**
  - The request was "clearly labelled". Most people know JPG, PNG, MP4, MP3 and PDF, and almost nobody knows QOI, WV or DBK.
  - With codes alone a person can't tell what they'd get, or which choice suits their purpose, such as "for a website" or "for editing".
- **Direction:**
  - Keep the code, since it's the signature, and add a plain name where the person is choosing. Put it in the centre or the footer while a target is lit: "WEBP: a small image for the web".
  - The options sheet title can carry it too.
  - Consider grouping the More ring under kind names (Images, Audio) as text, not just as icons.

### H-09 · Severity 2 · Right-click opens the convert wheel, but nothing says so, and Tools and Actions have no mouse path

- **Evidence:** `WM_CONTEXTMENU` (`win/mod.rs:2451-2481`) opens the Convert wheel on the row under the pointer. Tools and Actions open only with a held Ctrl or Alt during a drag, or with Ctrl+Enter or Alt+Enter on a selected row.
- **Why it matters:**
  - Right-click is the one discoverable mouse path, but people expect a context menu there.
  - A mouse user who finds Convert by right-click still can't find Compress or Merge.
- **Direction:** right-click opens a wheel, or a small menu, with Convert, Tools and Actions reachable from it. Alternatively, the row affordance from H-02 opens a chooser.

### H-10 · Severity 2 · The options sheet is found only from a footer line

- **Evidence:** "Tab or right-click for MP3 options." appears in the footer only while a target is lit (DESIGN.md, The glaze wheel → Options).
- **Why it matters:** people setting quality or resolution won't look at the footer while their eyes are on the wheel.
- **Direction:** a small, visible "Options…" target near the lit segment or in the wheel centre. Also mention in the sheet's context that it can be reopened.

### H-11 · Severity 2 · The Settings button is an unlabelled dial icon, and Kiln has no tooltips anywhere

- **Evidence:**
  - Header, top right (`m7/empty-100.png`): the icon is "dial" from `icons.rs`.
  - A search of `crates/kiln-app/src` for tooltip finds no matches.
  - The row's cancel × is unlabelled too.
- **Why it matters:** the dial looks like a speed or performance gauge, not Settings, and with no tooltip there's no way to check without clicking.
- **Direction:** standard tooltips on every icon-only control, and consider the conventional gear icon or a text label. Ctrl+, already opens Settings, so put that in the tooltip.

### H-12 · Severity 2 · Holding Shift, Ctrl or Alt during a drag already means something in Windows

- **Evidence:**
  - Kiln's drop target always reports `DROPEFFECT_COPY` (`win/drop.rs:103`).
  - In File Explorer, a held Shift means move, Ctrl means copy and Alt means create a shortcut.
  - Kiln gives the same keys its own meanings: Convert, Tools and Actions.
- **Why it matters:** people who know Windows will guess the gesture moves or copies their file. Not tested live: what the cursor badge says over Kiln.
- **Direction:** keep the gesture as the fast path for people who learn it. Make the clicked paths (H-01, H-02 and H-09) the main way.

### H-13 · Severity 2 · Jargon

- **Evidence:** Settings has a tab named "Engines" listing FFmpeg, Pandoc, LibreOffice and Calibre. The sheet labels include "Audio: Auto" (meaning kbps) and DPI. The General tab has "Wheel keys". See the `JARGON` flags in [strings.md](strings.md) for the full list.
- **Why it matters:** "Engines" is the place a person has to go when a conversion fails for a missing program, and they won't know that's what the word means.
- **Direction:** describe each engine by what it does ("FFmpeg: audio and video"), label settings by purpose ("Sound quality" rather than "Audio"), and keep the technical terms as secondary text.

### H-14 · Severity 2 · There is no help in the app

- **Evidence:** there is no help, about or shortcut screen. The README on GitHub is the only documentation.
- **Direction:** a "?" or F1 that opens a short overlay with the three ways in and the keys. Optionally, a dismissible first-run tip that shows over the list after the first drop.

### H-15 · Severity 2 · Silent cancel on Delete, and Move can't be undone

- **Evidence:**
  - `remove_selected` (`win/mod.rs:487`) cancels the row's jobs before removing it.
  - The Actions wheel's Move has no undo (`m6/moved-100.png`).
- **Direction:** while a job runs, Delete asks first or does nothing. Add Undo for Move in the footer.

### H-16 · Severity 2 · The More ring is dense

- **Evidence:** `m8/wheel-more-light-100.png` shows about 30 codes in 12 px monospace around a 208 DIP ring. Each kind group is marked only by a muted icon.
- **Why it matters:** Hick's law means more choices take longer, and codes alone (H-08) make each one costly to read. The ring's slots are also small targets (see the target-size table in accessibility.md).
- **Direction:** a type-to-filter (start typing "web" and the ring narrows), group names as text, and plain names on hover or focus.

### H-17 · Severity 2 · The tagline takes the instruction's place

- **Evidence:** the empty state (`m7/empty-100.png`) reads "Drop files here", then "Fire raw files into finished ones.", then the real instruction in smaller, muted text.
- **Why it matters:** the second line is where people read on. A metaphor sits there, and the working instruction is pushed below it in a weaker style.
- **Direction:** put the instruction second and the tagline last, or drop the tagline from the working UI. DESIGN.md allows "Fire" only in onboarding, and this is the onboarding.

### H-18 · Severity 1 · The wheel only opens while a key is held mid-drag

- **Evidence:** DESIGN.md says the wheel "opens at the pointer while a modifier is held during a drag". Releasing the key fades it out.
- **Direction:** fine as an expert path. Once H-02 and H-09 are fixed, it's no longer the only one.

### H-19 · Severity 1 · The row's second line leads with the folder

- **Evidence:** "C:\kiln\target\samples · 1600 × 1200 photo · 3.9 MB · Suggested: JPG, about 890 KB" (`m9/row-hint-dark-100.png`). When space runs short, the end is what gets cut, and the end is the suggestion.
- **Direction:** order it by usefulness (what it is, then the suggestion, then the size, then the folder), or move the folder to a tooltip.

### H-20 · Severity 3 · The "isn't installed" error leads to a dead end

- **Evidence:** this comes from the string inventory, [strings.md](strings.md), item 4 of the 15 worst strings.
  - The error reads "{engine} isn't installed. Get it from Settings > Engines."
  - Settings > Engines shows "Not installed (Settings > Engines)" and offers only Choose (`system::pick_program`, `win/system.rs:72`) and Reset. There is no link, no download page and no word on what each engine is for.
- **Why it matters:** a new PC has none of FFmpeg, Pandoc, LibreOffice or Calibre, so this is many people's first error. The message promises a next step that the page can't deliver. Kiln never downloads engines, by design, but it can still say where to get them.
- **Direction:** each missing engine gets one plain line on what it's for ("FFmpeg: audio and video"), with an official-site link opened in the browser. The error itself names the kind of file it's needed for.

### H-21 · Severity 2 · The details dialog clips its text

- **Evidence:** [strings.md](strings.md), item 15 of the 15 worst strings. The raw engine output is wrapped and clipped with no scrolling, and the title is just "Details".
- **Direction:** make the output scrollable and selectable, add Copy, and title the dialog "What the engine said" or similar.

## What works and should be kept

- The wheel shows only what applies, so people can't pick a conversion that won't work. This is strong error prevention.
- The options sheet shows only the settings that change the result, and gives an estimate of the outcome.
- Suggestions give a reason, and stay quiet when there's nothing useful to say.
- Results never overwrite anything, and the offline promise is stated plainly.
- The visual identity is calm and consistent, and the voice is short and specific almost everywhere outside the engine errors.
- Keyboard paths exist for the single-file case: arrows, Enter, Tab for options and Escape.

## Open questions for the sessions

1. Do first-time users try a plain drop first, a key-held drop, or neither? This decides how urgent H-02 is compared with H-05.
2. Does anyone right-click without being prompted? This tests H-09.
3. After converting, where do people look for the result? This tests H-04.
4. Which codes in the More ring can participants explain? This tests H-08.
