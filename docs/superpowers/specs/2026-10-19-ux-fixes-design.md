# UX fixes from the Phase 1 baseline: design

Date: 2026-10-19. Status: the owner asked for the whole overhaul to be built
overnight, with changes and reverts left for the morning. Every decision
below was made without a picker and is marked **Ruling** so it can be
reviewed.

## Where this fits

Phase 1 ([docs/ux/phase1](../../ux/phase1/README.md)) found 21 heuristic
problems and 12 failing WCAG criteria. The plan was to run usability
sessions before designing. The owner chose to build the fixes now from the
baseline alone.

**Ruling:** the sessions in `usability-test.md` still run, against this
build, and they confirm or undo these changes. Each fix is its own commit so
it can be reverted alone.

Held results ([2026-10-19-held-results-design.md](2026-10-19-held-results-design.md))
covers H-04, H-06 and the Delete half of H-15. This spec covers the rest.

## Goal

A first-time user can add files, see what to do next, pick a format by
name, recover from an error, and do all of it by keyboard or with a screen
reader. Every one of the 12 baseline rows in the Phase 1 README is fixed or
explicitly deferred below.

## 1. Ways in (H-01, H-02, H-05, H-09, H-12, H-18, WCAG 2.1.1, 2.5.7)

- **Add files:**
  - An **Add files** button in the empty state, under the instruction, and
    in the footer. Ctrl+O does the same.
  - Each opens the Windows Open dialog with multi-select. Picked files are
    added as a plain drop would add them.
- **Paste:** Ctrl+V adds files copied in File Explorer (`CF_HDROP`).
- **Right-click a row:** opens a native Windows context menu instead of the
  Convert wheel straight away. The items:
  - Convert… (Enter), Tools… (Ctrl+Enter) and File actions… (Alt+Enter).
    Each opens its wheel at the row, for the selection.
  - A separator.
  - For a held result: Export… (Ctrl+S), Open and Show in folder.
  - A separator, then Remove (Delete).

  Items that don't apply are left out, not greyed out.
- **Ruling:** a native menu over a drawn one, because screen readers and the
  keyboard (the Menu key and Shift+F10) get it for free.
- **Footer hint:** shown whenever files are listed and nothing is running.
  It replaces "Added 2 files." after two seconds:
  `Right-click a file, or select it and press Enter, to convert it.`
- **Empty state, in order:**
  1. `Drop files here`
  2. `Or add them with the button below, or Ctrl+O.`
  3. The Add files button.
  4. The modifier hint: `Hold Shift while dropping to convert, Ctrl for tools, or Alt for file actions.`
  5. The tagline, last and muted.

  This fixes H-17.
- **The wheel's title** shows its key: `Convert · Shift`, `Tools · Ctrl`,
  `Actions · Alt`. The keys come from the wheel-keys setting (H-05).
- **Ruling (H-12, H-18):** the modifier drop stays as the fast path,
  unchanged. With the paths above it's no longer the only way in.
- **Deferred:** "Open with" and "Send to" entries in File Explorer. They
  need an installer to register them.

## 2. Selection (H-03)

- Ctrl-click toggles a row, Shift-click selects a range, and Ctrl+A selects
  all. Shift with the arrow keys extends the selection, and Escape clears
  it.
- Every wheel, Export, Remove and the context menu act on the selection.
  Right-clicking a row that isn't selected selects only that row first, as
  File Explorer does.
- Merge appears in Tools when two or more PDFs, or two or more files of
  another mergeable kind, are selected. That's the same rule as a held-Ctrl
  drop of those files.
- The footer shows `3 selected` while more than one row is selected.

## 3. Errors (H-07, H-20, H-21)

- **Known failures become plain sentences.** kiln-core gets a function that
  maps an engine failure to a sentence with a next step. The raw line stays
  in Show details. The first set:

  | Engine output contains | Row line |
  |---|---|
  | `Invalid data found when processing input`, `moov atom not found`, `could not find codec parameters` | `This file looks damaged, or isn't really a {EXT}. Try opening it in another app.` |
  | `Permission denied`, access denied | `Kiln couldn't read or write the file. Check it isn't open in another app.` |
  | `No space left on device` | `There isn't enough free space on the drive.` |
  | `password`, `encrypted` (PDF, office) | `This file is password-protected. Remove the password, then try again.` |
  | `Unknown encoder`, `Encoder not found` | `This FFmpeg can't write {EXT}. A full build of FFmpeg can.` |
  | anything else | today's line, as `{Engine} couldn't convert the file.`, with the engine's first line only in Show details |

- **Never cut a message off.** A row line that doesn't fit ends in `…`, and
  the full sentence is in the row's tooltip and its accessible name.
- **Missing engine (H-20):**
  - The row line becomes `Needs FFmpeg, which isn't installed. Settings > Engines says where to get it.`
  - Each engine row in Settings > Engines gets a line on what it's for:
    - FFmpeg: `Audio, video, GIF, HEIC and AVIF.`
    - Pandoc: `Markdown, HTML and other text documents.`
    - LibreOffice: `Word, Excel and PowerPoint files, and PDF output.`
    - Calibre: `E-books.`
  - A missing engine also gets a **Get it** link that opens its official
    download page in the browser. Kiln still never downloads anything.
  - The links:
    - `https://ffmpeg.org/download.html`
    - `https://pandoc.org/installing.html`
    - `https://www.libreoffice.org/download/`
    - `https://calibre-ebook.com/download`
- **The details dialog (H-21):**
  - Titled `What {Engine} said`.
  - Its output scrolls with the wheel and the keyboard.
  - It has a **Copy** button that copies all of it, and the Escape and Enter
    keys close it.
  - Its close control is drawn with a focus ring.

## 4. Names for formats (H-08, H-16)

- kiln-core gets a plain description for every format it knows, for example:
  - `WEBP: a small image for the web`
  - `QOI: a fast, lossless image`
  - `WV: WavPack, lossless audio`
  - `DBK: a DocBook document`
- While a target is lit in a wheel or the More ring, the wheel's footer shows
  that description in place of the generic hint. The options sheet's title
  carries it as its second line.
- The More ring labels each kind group with text (`Images`, `Audio`,
  `Video`, `Documents`), muted, beside its icon.
- **Deferred:** type-to-filter in the More ring.

## 5. Help and labels (H-10, H-11, H-13, H-14, H-19)

- **Tooltips:** every icon-only control gets a native tooltip with its
  shortcut: `Settings (Ctrl+,)`, `Cancel`, `Export (Ctrl+S)`, `Help (F1)`.
- **Help:** a `?` button in the header and F1 open a help overlay. It shows
  the ways in (drop, Add files, right-click, the held keys) and a table of
  keyboard shortcuts. Escape closes it.
- **Options target (H-10):** while a target is lit, the wheel's centre shows
  `Options… (Tab)` as a clickable target, not only the footer line.
- **Jargon (H-13):**
  - The Settings tab "Engines" becomes **Helper apps**. Each engine is
    described by what it does (above).
  - Sheet labels say what they change: "Audio" becomes "Sound quality" and
    "DPI" becomes "Sharpness (DPI)".
  - "Wheel keys" becomes "Keys to hold while dropping".
  - Kiln uses one word per idea: **convert** (to a format), **tool**
    (Compress, Crop, Trim, Split, Merge), **file action** (Copy path, Move,
    Open terminal) and **helper app** (engine). The full list is in the
    Phase 1 strings report's terminology table.
- **Row second line (H-19):** in this order: what it is, the suggestion,
  the size, the folder.
  `1600 × 1200 photo · Suggested: JPG, about 890 KB · 3.9 MB · samples`.
  The folder is its last part only, and the full path is in the tooltip.

## 6. Accessibility (baseline rows 1, 10, 11; WCAG 1.1.1, 1.3.1, 1.4.1, 1.4.3, 1.4.11, 2.3.3, 2.4.7, 2.5.8, 4.1.2, 4.1.3)

- **UI Automation.** Kiln's window becomes a UIA provider tree, with a role
  and name on every element. The minimum is:
  - The window.
  - The header buttons: Settings, Help.
  - The file list (`List`), each row a `ListItem` with its name and second
    line, and the selection exposed through `SelectionItem`.
  - The row's buttons: Cancel, Export, Show details.
  - The footer buttons: Add files, Export all, Clear.
  - The status text.
  - The wheel as a `Menu` with `MenuItem` segments, named by plain
    description.
  - The Settings panel's tabs (`Tab`/`TabItem`), its steppers and
    segmented groups (`Spinner`, `RadioButton`, named by their labels).
  - The details dialog.

  Status changes ("Added 3 files.", done, failed) raise
  `UiaRaiseNotificationEvent`. Focus changes raise focus events.
- **Contrast:**
  - Light theme: the accent used for focus rings, chosen edges, the
    suggestion dot and progress gets a darker variant with at least 3:1 on
    its surface. The selected-row wash gets a 1 DIP accent edge on its left,
    so position isn't shown by hue alone.
  - The error colour is darkened in light theme and lightened in dark
    theme, to at least 4.5:1 on the surface and on a selected row.
  - Muted text on selected and hovered rows reaches 4.5:1.
  - Every pair in the Phase 1 contrast table is re-measured, and the results
    go in `docs/ux/phase2/contrast.md`.
- **Windows settings:**
  - **High contrast:** when it's on, Kiln draws with the system colours
    (`GetSysColor`: window, window text, highlight, highlight text, gray
    text, hotlight) and drops washes and glows.
  - **Text size** (Settings > Accessibility > Text size): fonts scale by
    `UISettings.TextScaleFactor`, and layout grows with them.
  - **Animation effects off** (`SPI_GETCLIENTAREAANIMATION`): pours, slides
    and fades are skipped. Progress still shows, without the breathing.
- **Focus:** Settings opens with focus on the first tab, and the details
  dialog with focus on Copy. In the list, the focused row keeps a visible
  ring after Escape clears the selection.
- **Target size:** "Show details" gets a 24 DIP tall hit area of its own,
  outside the row's click target.

## Out of scope

- The usability sessions themselves, and Phase 2's success thresholds. Those
  wait for the owner's sessions.
- Explorer "Open with" and "Send to", type-to-filter, Undo for Move, and
  dragging a result out of Kiln.

## Testing

- kiln-core unit tests: the error mapping table, a description for every
  format in the registry, and the terminology for every engine.
- kiln-app model tests:
  - selection: Ctrl, Shift, Ctrl+A, and right-click on an unselected row
  - the hint's timing and wording
  - the empty-state order
  - the row line order
  - the context menu's items for each row state
- A UIA dump (the Phase 1 script, `docs/ux/phase1/uia/`) after the change.
  It must show the elements listed above with names. It's saved to
  `docs/ux/phase2/uia/`.
- A GUI pass with screenshots at 125%, dark and light, in
  `docs/ux/phase2/shots/`.
