# Designing for Kiln

Kiln is a Windows 11 app. Its surfaces follow Fluent: Mica behind the window,
Fluent's neutrals and elevation, Segoe UI Variable and Segoe Fluent Icons. The
brand is the name, the K mark, the app icon and jade, the accent of Kiln's own
Light and Dark themes. Read this before changing anything a person can see.

The values live in
[`crates/kiln-app/src/theme/fluent.rs`](crates/kiln-app/src/theme/fluent.rs).
A snapshot test (`crates/kiln-app/tests/snapshots/fluent_tokens.snap`) fails
when any of them changes, so a change is always a deliberate one: change the
code, this file and the snapshot together. The full tables are in
`crates/kiln-app/src/theme/fluent.rs`.

## Principles

1. **Platform first.** Where Windows has a control, a size or a behaviour,
   Kiln uses it as Windows does: the title bar, ContentDialog, InfoBar,
   ToggleSwitch, RadioButtons, ComboBox, Expander, ProgressBar, menus and the
   two-tone focus visual. Someone who knows Notepad or Settings knows Kiln.
2. **Restraint.** Neutral surfaces, one accent per theme, always a green: jade
   in Light and Dark, the theme's own green or aqua in the six named themes.
   The accent marks the primary action
   (one per surface), the lit wheel segment, a toggle that is on, progress,
   selection and links. Kind colours are small cues only: a dot, the wheel's
   thin outer arc, a badge. Never a large fill, never the accent.
3. **Content first.** Files show as Windows shows them: the shell's thumbnail,
   or the shell's own icon for the type. The window adds no texture, glow or
   ornament around them.
4. **Calm motion.** Short, eased, and never needed to understand anything.
   With Windows' Animation effects off, nothing moves and nothing is lost.
5. **Plain words.** See [Voice](#voice).

## Colour

Code asks for a role (`palette.accent`), never a raw value. `theme::fluent`
has a dark, a light, six named palettes and a contrast palette; Kiln follows
the Windows app mode, or the theme chosen in Settings. The tables below are
Dark and Light, and the named themes follow them.

| Role | Dark | Light |
|---|---|---|
| `base` (under Mica, or solid) | `#202020` | `#F3F3F3` |
| `layer` (the content layer) | `#282828` / white 4% | `#F9F9F9` / white 50% |
| `card` (side pane, Settings cards) | `#2B2B2B` / white 5% | `#FBFBFB` / white 70% |
| `overlay` (dialogs, the wheel, flyouts) | `#2C2C2C` | `#F9F9F9` |
| `card_stroke` (outline of cards and of side pane rows, which also take the `control` fill at rest) | white 8% | black 7% |
| `text` / `text_secondary` / `text_disabled` | `#F5F5F5` / `#C5C5C5` / `#7A7A7A` | `#1B1B1B` / `#5D5D5D` / `#9E9E9E` |
| `accent` / `on_accent` | `#6CCBA8` / `#0B1F18` (8.8:1) | `#1F7A5C` / `#FAFAFA` (5.03:1) |
| `accent_text` (links) | `#7ED8B6` | `#17664C` |
| `strong_stroke` (progress track, toggle rim, radio rings) | white 54% | black 45% |
| `field_stroke` (a text box's sides; its bottom edge is secondary text) | white 7% | black 12% |
| `accent_edge` (the accent button's lip along its bottom, so it sits level with the standard buttons beside it) | black 14% | black 24% |
| `critical`, `success`, `caution` | `#FF99A4`, `#6CCB5F`, `#FCE100` | `#C42B1C`, `#0F7B0F`, `#9D5D00` |

- Solid values are the fallback; over Mica the layers are alpha over the
  backdrop. A named theme has no Mica: its layers are solid, and the tests
  check that a layer's Mica tint over its base lands on its solid value (within
  1 in each channel) in every palette.
- No pure white or black in text or fills.
- **Contrast.** Text and its background reach 4.5:1, a cue that isn't text
  3:1. `fluent::contrast_report` measures every pair the surfaces use, and a
  unit test fails on any that falls short. A card's outline needs 1.15:1 against its
  card (`theme::STROKE`), so cards and rows keep a shape in every theme; the
  dark stroke was black at 10%, 1.05:1, and is white at 8% now. The light accent's hover is
  `#247F60`, not `#2A8566`, which reached only 4.33:1.
- **Kind colours.** Image `#4FA3F7`/`#0F6CBD`, video `#A884F3`/`#6B47C9`,
  audio `#E67AA6`/`#B4366E`, documents (sheets, slides, subtitles and e-books
  too) `#C9B458`/`#7A6A1C`, other `#9A9A9A`/`#616161`.
- **Named themes.** Dracula, Gruvbox (the dark one), Monokai, Nord, Solarized
  Dark and Solarized Light are one more palette each: the same roles, the same
  controls, radii, type and motion, only colour. Each has one accent, a green
  or an aqua of its own, and takes its kind colours from its own blues,
  purples, pinks and yellows. Settings > Appearance > Theme lists them after
  System, Light and Dark, and `settings.toml` names them `dracula`, `gruvbox`,
  `monokai`, `nord`, `solarized-dark` and `solarized-light`. Each item in the
  list has three 8-DIP dots at its right (its `base`, `text` and `accent`, each
  with a rim) so a theme can be picked without knowing its name; the dots are
  decorative, and the item's name is what the keyboard and UI Automation read.
  A choice applies at once. While a Windows contrast theme is on, the list stays
  enabled and its note reads "A Windows contrast theme is on, so Kiln is using
  its colours." A name Kiln doesn't know is System. They are tuned until `contrast_report` passes: where a
  published colour fell short (Monokai's and Dracula's comment greys, Solarized's
  body text, Nord's red) a lighter or darker one of the same hue is used.

  | Theme | `base` | `layer` / `card` / `overlay` | `text` / `text_secondary` | `accent` / `on_accent` | `accent_text` |
  |---|---|---|---|---|---|
  | Dracula | `#21222C` | `#282A36` / `#2F3140` / `#343746` | `#F8F8F2` / `#A4ADCA` | `#50FA7B` / `#21222C` | `#69FF94` |
  | Gruvbox | `#1D2021` | `#282828` / `#32302F` / `#3C3836` | `#EBDBB2` / `#BDAE93` | `#8EC07C` / `#1D2021` | `#A0D08E` |
  | Monokai | `#1E1F1C` | `#272822` / `#2D2E27` / `#32332B` | `#F8F8F2` / `#AAA997` | `#A6E22E` / `#1E1F1C` | `#B6EC4A` |
  | Nord | `#2B303B` | `#2E3440` / `#3B4252` / `#3B4252` | `#ECEFF4` / `#C2C9D6` | `#8FBCBB` / `#2E3440` | `#9EC5C4` |
  | Solarized Dark | `#00212B` | `#002B36` / `#073642` / `#073642` | `#EEE8D5` / `#9AA7A7` | `#2AA198` / `#00212B` | `#30B8AD` |
  | Solarized Light | `#EEE8D5` | `#FDF6E3` / `#FFFBF0` / `#FDF6E3` | `#073642` / `#566C73` | `#217D76` / `#FDF6E3` | `#1E736D` |

  The full values, strokes and washes included, are in
  `tests/snapshots/fluent_tokens.snap`. The app icon, the taskbar icon and the
  tray icon stay jade in every theme. Under a Windows contrast theme, none of
  this is used.
- **High contrast.** Every role maps to the eight colours a contrast theme
  sets: Window, WindowText, Highlight, HighlightText, ButtonFace, ButtonText,
  GrayText and Hotlight. No Mica, no alpha and no shadows; every control and
  card has a 1-DIP WindowText edge; selection, the lit wheel segment and a
  selected row's text use Highlight and HighlightText. Kiln follows a contrast
  theme as it is turned on and off.

## Elevation

| Level | What | Treatment |
|---|---|---|
| 0 | Base | Mica, or `base` solid |
| 1 | Content layer | fill only |
| 2 | Cards, side pane, Settings cards | fill and `card_stroke`, no shadow |
| 3 | Dialogs, the wheel, flyouts, menus | `overlay` fill, `overlay_stroke` and a shadow |

Flyouts and the wheel cast `0 8 16`, black 26% dark and 14% light. Dialogs
cast `0 32 64` with `0 2 21`, black 37% dark and 28% light, over 30% smoke.
Nothing else casts a shadow.

## The window

- **Backdrop.** Mica (`DWMSBT_MAINWINDOW`) on Windows 11 22H2 and later, where
  Kiln leaves its base clear. The solid `base` on older Windows, with
  Transparency effects off, under a contrast theme and in the six named themes.
  Windows draws the frame and the tooltips light or dark as the theme's base is. The frame isn't
  extended into the client area: Mica shows behind the swap chain without it,
  and an extended frame has Windows draw its own caption buttons under Kiln's.
- **Title bar, 48 DIPs.** The K at x = 16 and "Kiln" in Caption (secondary
  while the window is inactive), Help and Settings as subtle icon buttons, then
  Minimise, Maximise or Restore and Close, 46 × 48, drawn in Segoe Fluent Icons.
  Close lights `#C42B1C`. Maximise shows Snap Layouts. In Settings, Back comes
  first and the title still reads "Kiln"; the page's own 28-DIP "Settings" is
  its heading.
- **Content layer.** One 8-DIP rounded layer under the title bar, 16 DIPs from
  the window's edge (0 below 600 DIPs wide), holding the list, the side pane
  and the footer, or Settings.

## Type

Segoe UI Variable only, its optical size by size (Small to 12, Text 13 to 19,
Display from 20), and Segoe UI on Windows 10. Cascadia Mono, or Consolas, only
in the Details log.

| Style | Size / line | Weight | Use |
|---|---|---|---|
| Caption | 12 / 16 | Regular | row facts, badges, the title bar's title, flyout headers |
| Body | 14 / 20 | Regular | row names, controls, dialog text |
| Body Strong | 14 / 20 | Semibold | a selected row's name, Settings section headers, the wheel's pair |
| Subtitle | 20 / 28 | Semibold | the side pane's file name, dialog titles, the empty window's headline |
| Title | 28 / 36 | Semibold | the side pane's percentage, in tabular figures |

Format codes are Semibold, uppercase and tracked out by 2%, at Body's 14 DIPs
everywhere: the wheel's segments, More's list and the side pane's rows. In the
pane and in every list of formats a code has a 52-DIP column of its own, so what
each format is for starts in one place; that is Caption in the secondary colour,
wrapping to two lines, from one source (`describe_format`). Windows' Text size scales every style; the title bar stays 48 DIPs and only its text grows.

## Shape and space

- A 4-DIP grid: 4, 8, 12, 16, 24, 32, 48. The base step (4), the list panel's
  padding (16), a hairline (1) and the mark's grid (24) and line (1.5) are in
  `theme::fluent::space` with the rest; there is no other token module.
- Radii: 4 for controls, rows, thumbnails and badges; 8 for the content
  layer, cards, dialogs and flyouts. The wheel is a circle with 2-DIP gaps and
  4-DIP inner corners.
- Rows are 56 DIPs, with a 40-DIP thumbnail and 12 DIPs to the text. No rules
  between rows: hover and selection fills group them.
- A row's name and second line run to its right padding, short of a button by
  12 DIPs, and on a row with a checkmark or error mark short of the mark's 16
  and the 4 before it: a row without a mark keeps no room for one. A link's
  button is as wide as its words, so "Get Media Feature Pack" fits. A running
  row's bar runs to the percentage. A saved result too long for the list says
  itself shortly, `Saved · 29 MB, about the same size`.
- An InfoBar at the top of the list has 8 DIPs above it and 8 between it and
  the first row.
- A row's second line is secondary text whatever it says; only a problem is in
  the critical colour, and the file's size after it is secondary again.
- The side pane widens from 320 to 400 DIPs as the window does, and folds
  away below 760 DIPs. Its parts stack in one order for every state: the
  picture, the name (which wraps to a second line), the status bar with its
  actions, then the facts and the list. Facts are 20 DIPs apart, their labels
  in a 72-DIP column, wide enough for "Dimensions". The Folder value is a link that opens the folder. A stack of pictures keeps a 12-DIP inset at each side
  for two files or three.
- Card padding 16 across and 12 down, dialog padding 24; an estimate card in
  a dialog has the same.
- Every dialog is 480 DIPs wide, or the window less 24 at each side. It stands
  on the content layer, centred below the title bar and 24 DIPs from the
  layer's top and bottom, and its body scrolls where it would be taller. A
  dialog with one button, Help's Close, has a standard (not accent) button as
  wide as a button beside another, at the right of the band. Focus rings are
  drawn after the body's clip, so a full-width control keeps its sides.
- The empty window: 24 DIPs under the K and 24 above Add files. Its first-run
  card pads 16 on every side, with 12 under the heading and between steps.
- Settings: 32 DIPs at the page's sides and under its last card. The page
  scrolls inside the layer's own 1-DIP edge, so a card never covers it. Every
  drop-down on the page is one width, the widest any of them needs. A card's
  glyph is its own: no two rows share one. An open helper shows a path under
  a "Location" Caption label.
- The wheel's hub stacks a 32-DIP picture, 4 DIPs, the 20-DIP pair and the
  16-DIP hint, 72 tall about its centre, so the hint ("Formats, tools") stays
  inside the 104-DIP disc. A wheel poured over the list stays on the content
  layer with its shadow's 24 DIPs inside it, on every side.
- A note beside a control in More options wraps to as many lines as it needs
  and its row grows with it, 16 DIPs a line; an empty size box shows its hint
  ("e.g. 5").

## Icons

Segoe Fluent Icons, the system font, at 16 in rows and buttons and 20 in the
title bar; Segoe MDL2 Assets on Windows 10, which has every glyph Kiln uses at
the same code point. Kiln bundles no icon art but the K mark
(`icons::MARK_PATH`), which the app icon generator
(`crates/kiln-app/examples/gen_icon.rs`) also draws. The glyph table is in
`theme::fluent::glyph`.

## Motion

| What | Time | Easing |
|---|---|---|
| Hover, press | 83 ms | linear |
| A small change, a flyout or the wheel coming in, a toggle | 167 ms | `cubic-bezier(0,0,0,1)` |
| A dialog, a pane, a row added (a 10-DIP rise) | 250 ms | `cubic-bezier(0,0,0,1)` |
| Anything leaving | 167 ms | `cubic-bezier(0.7,0,1,0.5)` |
| An Expander's chevron | 167 ms, turning 180° | as a small change |
| A short fade with no curve of its own (a row, a drop outline) | 120 ms | ease-out cubic |

The wheel opens with a fade as it grows from 92% about its centre; no turn and
no spring. An indeterminate ProgressBar runs across while it shows; nothing
else loops. With Animation effects off every value is at its end at once and an
indeterminate bar stands still in the middle. Frames are paced by the display
while something moves, and by a slower clock while only a running job's
progress moves; an idle or hidden Kiln draws nothing.

## Controls

Each control is a drawing function in `win/render/controls.rs`, a layout
helper in `layout.rs` and a UI Automation role with its patterns.

| Control | Shape | Keyboard | UI Automation |
|---|---|---|---|
| Button | 32 tall, 4 radius; standard, accent (one per surface) or subtle | Tab, Enter, Space | Button, Invoke |
| Hyperlink | `accent_text`, underlined on hover | Tab, Enter | Hyperlink, Invoke |
| ComboBox | 32 tall, value left, chevron right, a flyout list | Alt+Down or Enter opens, Up and Down step, Esc closes | ComboBox, ExpandCollapse, Selection, Value |
| ToggleSwitch | 40 × 20 pill, On or Off to its left | Space | Button with Toggle |
| RadioButtons | rings in `strong_stroke`, the chosen one filled | the arrows move and pick; Tab leaves | RadioButton, SelectionItem |
| Expander | a full-width header, its chevron at the right | Enter, Space | Group with ExpandCollapse |
| TextBox | digits only, for a custom size | the usual caret keys, Ctrl+A, C and V | Edit, Value |
| ProgressBar | a 1-DIP `strong_stroke` track under a 3-DIP accent bar; indeterminate too | none | ProgressBar, RangeValue |
| InfoBar | 8 radius, the kind's fill, its icon, a title and one line, actions, Dismiss | its buttons in Tab order | a group named by its title, announced as it shows |
| ContentDialog | 8 radius, `overlay` body, the action band in `base`, over smoke | focus starts on the accent; Tab stays inside; Esc is Cancel | a modal Window, IsDialog |
| Details log | the engine's words in the Details dialog, text in the body font | none: Copy takes it | Text, with the Text pattern: a document read by character, word and line (a wrapped line counts as one) |
| Menu | a Fluent flyout with a glyph column (Convert to, Make smaller, Show in folder, Copy path, Retry, Add files, Remove) and shortcuts at the right | Up, Down, Home, End, Enter, Esc | Menu, MenuItem |
| Tray menu | the same flyout in a small window of its own, with room for its shadow, opening at the click inside the work area; a check mark in front of Drag-wheel while it is on | Up, Down, Home, End, Enter, Space, Esc and the access keys O, D and Q; it closes on losing the focus | Menu, MenuItem (Drag-wheel toggles) |
| Focus visual | 2-DIP outer ring (`#F5F5F5` dark, `#1B1B1B` light), 1-DIP inner ring in the inverse | keyboard focus only | — |

## The surfaces

In short:

1. **The empty window.** The K at 64 DIPs, "Drop files to convert", one line,
   **Add files** in the accent, and until it is dismissed a "How it works" card
   with three steps.
2. **The list.** Rows with a thumbnail, the name and what the file is; a
   converting row's progress and Cancel; a finished row's checkmark, result and
   Show in folder; a failed row's plain reason, Show details, and Retry where
   trying again can help. The side pane beside it, and the footer with
   **Convert** in the accent: with the pane showing it says what it converts to
   ("Convert to JPG", the pane's outlined format) and converts at once; with the
   pane folded it opens the list of formats above the footer. The wheel is for a
   drop and the drag-wheel only.
3. **The wheel.** One flat ring of format codes, Make smaller and More as a
   glyph over a caption, the kind as a thin arc inside the edge (only when the
   formats are of more than one kind, and never on the lit segment), the lit
   segment solid jade, and the hub, in the base out to the ring so no gap shows
   what is behind the wheel, with the file's thumbnail and "JPG → PNG". More opens
   a grouped list beside the ring (the same list the pane and the footer open
   with no ring: 360 DIPs, rows snapped, scroll indicator at rest, group
   headings, a footer row "Options for JPG…", always usable). The drag-wheel
   draws the same ring and its list at 300 DIPs.
4. **Dialogs.** Make smaller, Resize, a conversion's options, Help, Details and
   Kiln's questions are ContentDialogs in the window.
5. **Settings.** One page of cards: Appearance, Saving, Quality, Drag-wheel,
   Helper apps and About.

## Keyboard and assistive tools

- Every control is reachable by Tab, in the order it is drawn, and shows the
  focus visual only for the keyboard.
- Narrator reads each control by its role and name, a card's note as the
  control's help text, a dialog's text as it opens, and InfoBars and estimates
  as they change.
- Per-monitor DPI: every size is in DIPs, and glyphs and 1-DIP lines land on
  whole pixels at 100, 125, 150 and 200%.
- Windows' Text size grows every text style; controls grow to fit their words,
  and a Settings card too narrow puts its control under its words.

## Voice

Kiln talks like a capable person helping a friend: short sentences, plain
words, the thing itself named.

- One word per idea: **Save** (never Export), **Make smaller**, **Max size**,
  **More options**, **Remove** (a row), **Clear list**, **Convert to**.
- Files are called by one word for their kind: **photo**, **video**, **sound**,
  **document**. A format's description starts with a capital and has no article
  ("Photo that opens anywhere"). A link that gets something names it ("Get
  FFmpeg"), never "Get it". The longest side of a picture is "Longest side".
- Tools say what they do: Remove borders, Remove black bars, Remove silence at
  the ends, Remove black at the ends, Cut in half, Save each page as a PDF,
  Merge.
- An error says what happened and what to do next: `This video looks damaged.
  Try playing it in another app.`
- No taglines, slogans, reassurance lines, or size or speed boasts. No
  exclamation marks. Sentence case everywhere.

### Glossary

| Word | Means | Not |
|---|---|---|
| Convert | make a file of another format | transcode, export |
| Make smaller | save a copy in the same format at or under a size | compress |
| Save | put a result in its folder: next to the original, in the folder chosen in Settings, or in Documents\Kiln results | export |
| Remove | take a row out of the list (the file is untouched) | delete |
| Clear list | remove every row | reset |
| Retry | run the row's last job again | redo |
| Cancel | stop a running job (the button on its row; the footer's accent button never means stop), or close a dialog without acting | abort |
| Helper app | a program Kiln uses for some formats, such as Pandoc | engine, dependency |
| Drag-wheel | the wheel that opens while a file is dragged anywhere in Windows, and its setting | overlay, floating menu |

## References

Kiln is designed from its own requirements. Matching Windows and Fluent is
platform conformance, and is expected. Windows' own apps (Notepad, Photos,
Settings, PowerToys Settings) are for calibration: sizes, spacing, behaviour.
Don't copy another product's code, binaries, assets or icon art.
