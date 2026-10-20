# Kiln accessibility audit (UX Phase 1)

Build: `target/release/kiln-app.exe` from `main` (clean tree), audited 2026-10-19 on Windows 11 Pro 10.0.26300.
Method: UI Automation and MSAA tree dumps of the running app, keyboard-only runs through a focus-guarded driver, WCAG 2.x contrast maths on the tokens in `crates/kiln-app/src/theme.rs` as `win/render.rs` composites them, target sizes from `layout.rs` and `wheel.rs`, and a code read of `win/mod.rs`, `win/system.rs` and `win/render.rs`.

Two limits shape this report (details in [What could not be tested](#what-could-not-be-tested)):

- **Every monitor on this PC runs at 125%**, and display settings were off limits. Kiln ran and was captured at 125%. Sizes below are in DIPs, which equal CSS px and physical px at 100%.
- **The GUI run stopped part-way.** Before the options-sheet step, File Explorer took the foreground. The focus guard killed Kiln and sent nothing, and the GUI part ended there as the rules require. Error-row, Tools, Actions, options sheet, converting, done and light-list states were never captured. Their findings come from code and are marked "code".

## The 5 most serious barriers

| # | Barrier | Who it blocks | Severity (Nielsen 0-4) | WCAG |
|---|---|---|---|---|
| 1 | **Kiln exposes nothing to assistive technology.** UIA sees one `Window` named "Kiln" with no children in every state, and MSAA shows one empty `client`. There is no `WM_GETOBJECT` handler, no `UiaReturnRawElementProvider`, no focus or `NotifyWinEvent` events, and the `windows` crate is not even built with `Win32_UI_Accessibility`. Narrator, NVDA, JAWS, Voice Access grids and Magnifier focus tracking all get "Kiln, window" and silence. | Screen reader users, voice control users, Magnifier users | **4, catastrophe** | 1.1.1, 1.3.1, 4.1.2, 4.1.3 |
| 2 | **Files can only be added by dragging.** There is no Open dialog, no Ctrl+O, no paste and no Explorer "Open with" or "Send to" entry. Ctrl+O, Ctrl+V, Tab and Shift+F10 on the empty window did nothing (tested live). The only other route is command-line arguments. A keyboard-only user can't start the core task. | Keyboard-only, switch, voice and tremor users | **4, catastrophe** | 2.1.1, 2.5.7 |
| 3 | **The main window has no Tab stops, and several controls are mouse-only.** Tab does nothing (tested live). The gear opens Settings only through an unadvertised Ctrl+comma. **Clear has no keyboard path at all.** A running job's × is mouse-only (Delete cancels only by removing the row). "Show details" works only through an unadvertised Space. | Keyboard-only users; everyone who doesn't know the hidden keys | **3, major** | 2.1.1 |
| 4 | **Focus and state are hard to see.** Settings opens with no focus shown until the first Tab (live screenshot `settings-general.png`), and the Details dialog never shows focus. In the list, the only cue for the keyboard position is the selected-row wash at 1.40:1 (dark) and 1.11:1 (light), and there is no cue once Escape clears the selection. In light theme the focus ring, chosen segment, lit wheel segment, suggestion dot and progress line all fall to 1.1-1.8:1. Error text in crack red is 2.93:1 (dark) and 3.89:1 (light) at 12 px. | Low-vision and keyboard users | **3, major** | 2.4.7, 1.4.11, 1.4.3, 1.4.1 |
| 5 | **Kiln ignores Windows contrast themes, Text size and Animation effects.** It never reads `SPI_GETHIGHCONTRAST`, `GetSysColor`, `UISettings.TextScaleFactor` or `SPI_GETCLIENTAREAANIMATION`. `WM_SETTINGCHANGE` reacts only to `ImmersiveColorSet`. So it keeps its own palette under a contrast theme, keeps 12 DIP captions when Text size is 150%, and keeps animating when Animation effects are off. | Low-vision users who rely on contrast themes or large text; people with vestibular disorders | **3, major** | 1.4.4 (risk), 2.3.3, 1.4.3 under user colours |

## WCAG 2.2 AA checklist

Results: **8 pass, 12 fail, 3 not tested live** (23 criteria; 2.3.3 is AAA and is included because it was asked for).

| Criterion | Result | Evidence |
|---|---|---|
| 1.1.1 Non-text Content | **Fail** | The gear, cancel ×, wheel icons, format chips and suggestion dot have no text alternative, because no element exists to carry one (`uia/*.txt`: one `Window` node, zero children). |
| 1.3.1 Info and Relationships | **Fail** | The list, rows, tabs, segmented groups, steppers and dialogs are drawn with Direct2D and exposed as nothing. Settings labels are not tied to their controls in any API. |
| 1.4.1 Use of Color | **Fail** | In light theme, the chosen segment (wash 1.11:1, edge 1.81:1), selected row (1.11:1) and lit wheel segment (1.11:1, arc 1.81:1) differ from their neighbours almost only by hue. Dark theme passes through lightness: the edges and arcs are 6.31:1. The error state also has words, so text carries it. |
| 1.4.3 Contrast (Minimum) | **Fail** | Error line #B5503F: 2.93:1 on surface (dark), 3.89:1 (light), 2.10:1 on a selected row (dark). Muted second line on a selected row: 3.32:1 (dark), 4.20:1 (light); on a hovered row: 4.15:1 and 4.23:1. "Show details" on a selected row: 3.32:1 and 4.20:1. Plain muted text on surface passes at 4.63:1 by a hair. See the [contrast table](#contrast). |
| 1.4.4 Resize Text | **Not tested live** | Code: text scales with the display DPI (`WM_DPICHANGED`, per-monitor v2) but ignores Windows Text size (`TextScaleFactor` is never read). Type is fixed at 12/14/16/22/32 DIPs in clipped fixed-height boxes (`D2D1_DRAW_TEXT_OPTIONS_CLIP`, row lines 20 DIPs). Pass through display scaling is likely; the Text size setting fails. |
| 1.4.10 Reflow | **Not tested live** | Code: minimum window 480 × 360 DIPs, fixed 56-DIP rows, single-line clipped labels, and a settings panel capped at 560 DIPs. A 320-DIP-wide equivalent can't be reached: the window won't shrink below 480. Needs a live check at 200-400% display scale. |
| 1.4.11 Non-text Contrast | **Fail** | Selected-row wash, the only keyboard-position cue in the list: 1.40:1 (dark), 1.11:1 (light). Light: focus ring 1.81:1, chosen-segment edge 1.81:1, suggestion dot 1.81:1, progress fill on its track 1.49:1. Unknown-length progress: 1.70-2.59:1 (dark) and 1.13-1.25:1 (light). Dark passes for the focus ring, edges, dot and progress fill (5.0-6.3:1). |
| 1.4.12 Text Spacing | **Not tested live** | A native, custom-drawn app gives users no way to override spacing. Code: labels sit in fixed-height clipped rects, so any added spacing would clip. Not applicable as written; a risk if the app gains a text-size option. |
| 2.1.1 Keyboard | **Fail** | No keyboard way to add files (live: Ctrl+O, Ctrl+V and Tab did nothing), press Clear, cancel a job without removing its row, or reach the gear except through the hidden Ctrl+comma. See the [keyboard table](#keyboard-only-tasks). |
| 2.1.2 No Keyboard Trap | Pass | Escape closes the wheel (outer ring: first back to More), the options sheet, Settings and Details (`mod.rs` 1817-1820, 945, 1530, 1779). Tab wraps inside the modal Settings panel by design (live). |
| 2.2.2 Pause, Stop, Hide | Pass (caveat) | The only motion longer than 5 s is the running job's progress line, whose "heat" glow breathes on a 2.4 s cycle (`theme.rs` `HEAT_PERIOD_MS`) for the whole job. It reports progress, so it is treated as part of the indicator. The glow itself is decorative and can't be turned off. |
| 2.3.3 Animation from Interactions (AAA) | **Fail** | The wheel pour (scale 0.55 to 1 over 160 ms), More ring (0.66 to 1), sheet rise (8 DIPs), tab-marker slide (160 ms) and row fades run on every interaction, with no check of `SPI_GETCLIENTAREAANIMATION`. |
| 2.4.3 Focus Order | Pass | Settings: tabs, then rows top to bottom and left to right, then Check for updates and Close; disabled "Forget picks" is skipped, and Tab wraps (live, 15 stops on General). Sheet: knob rows top to bottom, then Cancel and Convert (`layout.rs` 491-553). The main window has no focus order at all, which is logged under 2.1.1. |
| 2.4.7 Focus Visible | **Fail** | Settings opens with `focus: None`, so no ring shows until Tab (`mod.rs` 1357; live `settings-general.png`). The Details dialog draws no focus ring. In the list, nothing marks keyboard position once Escape clears the selection. Where a ring exists it is clear in dark theme (`settings-quality.png`). |
| 2.4.11 Focus Not Obscured (Minimum) | Pass | `scroll_to_selection` keeps the selected row in view (`mod.rs` 626). The wheel and the panels take focus while they cover the list. |
| 2.5.1 Pointer Gestures | Pass | No path-based or multipoint gestures. Drag-and-drop is a dragging movement (see 2.5.7). |
| 2.5.7 Dragging Movements | **Fail** | Adding files needs a drag onto the window or the app icon; Kiln has no single-pointer alternative such as an Add files button. The modifier-held drop wheels do have single-pointer alternatives (right-click a row opens Convert). |
| 2.5.8 Target Size (Minimum) | **Fail** | The "Show details" link's hit area is 88 × 20 DIPs (`layout.rs` 150-157) and sits inside the row's own click target, so the spacing exception doesn't apply. Every other target is at least 24 × 24. The cancel × is exactly 24 × 24. |
| 3.2.1 On Focus | Pass | Moving focus never changes context. In Settings, arrows on a segment choose it (radio-group behaviour), and the theme switches at once (live). |
| 3.3.1 Error Identification | Pass (code) | A failed row's second line becomes the error's first line in words, with the engine output behind "Show details" (`mod.rs` 1138-1141; `model.rs` 188). Visual only: AT can't read it (4.1.2, 4.1.3). Not seen live. |
| 3.3.2 Labels or Instructions | Pass | Settings rows, sheet knobs and wheel items carry visible labels (screenshots). The stepper's split behaviour (left end steps back) has no visible instruction. |
| 4.1.2 Name, Role, Value | **Fail** | No control has a name, role, state or value in UIA or MSAA. ElementFromPoint over the gear, a row, the list and Clear all return the top-level `Window` (`uia/*.txt`). |
| 4.1.3 Status Messages | **Fail** | "Added 3 files.", progress, "Suggested: …", done and failed results are drawn only. No UIA notification or live-region event is raised. |

## What a screen reader user hears

From the UIA raw, control and content views plus the MSAA tree, saved in `uia/` (Narrator itself was not started):

| State | Dump | What Narrator gets |
|---|---|---|
| Empty window | `uia/01-empty.txt` | "Kiln, window." Item navigation finds only the Windows-drawn title bar (System menu, Minimize, Maximize, Close) and an unnamed "Kiln" client. "Drop files here", the privacy note, the drag hint and the gear are silent. |
| List of three files | `uia/02-list.txt` | Exactly the same tree. No file names, sizes, suggestions or status line. Up and Down change the selection without any event, so nothing is said. |
| Convert wheel open | `uia/03-wheel.txt` | Exactly the same tree. Opening the wheel, moving between JPG, PDF, More and the rest, and starting a conversion are all silent. |
| Settings panel | `uia/04-settings.txt`, `uia/04b-settings-tab-focus.txt` | Exactly the same tree (these two were dumped before the MSAA section was added to the script; the UIA part is complete). Tabs, theme segments, steppers and Close are silent; Tab moves an invisible-to-AT focus. |
| Error row | not captured | The GUI run stopped first (see below). From code nothing differs: the error text, the "Show details" link and the Details dialog are drawn only. |

Every `IsKeyboardFocusable` is `False`, and the focused element is always the `Window` itself.

## Keyboard-only tasks

Live = run through the guarded driver at 125%. Code = read from `win/mod.rs` because the run stopped first.

| Task | Possible? | Exact keys | Evidence |
|---|---|---|---|
| Add files | **No** | None. There is no Open dialog; Ctrl+O, Ctrl+V, Tab and Shift+F10 do nothing. Only drag-and-drop or launching `kiln-app.exe "file"` from a command line. | Live, plus code (`key()` 1827-1852 has no add path) |
| Pick a row | Yes | Up, Down, Home, End; Escape deselects | Live (Home, Down) |
| Convert it | Yes | Enter opens the Convert wheel on the selected row (Shift+Enter, Shift+F10 and the Apps key do too). Arrows move round the ring, starting on the suggestion. Enter or Space converts. On More, Enter pours the outer ring. | Live: wheel opened, Right/Left moved the lit segment (`wheel-convert.png`). Enter-to-convert: code (1821, 694-717) |
| Open Tools and Actions | Yes | Ctrl+Enter (Tools), Alt+Enter (Actions), following Settings > Wheel keys | Code (1836-1843, 2485-2488); the live step was about to run when the guard stopped |
| Open the options sheet | Yes | In the wheel, Tab or Shift+F10 on a format that has options. Inside: Tab, Shift+Tab, Up and Down move; Left and Right change a value; Space presses; Enter converts (unless on Cancel); Escape cancels. Focus starts on Convert. | Code (1791, 2451-2462, 935-993) |
| Open Settings | Yes, hidden | Ctrl+comma. The gear is not a Tab stop, and nothing on screen mentions the shortcut. | Live |
| Change a setting: General | Yes | Ctrl+comma, Tab ×6 to "Dark", Right selects Light (theme changes at once), Left goes back | Live |
| Change a setting: Quality | Yes | From the General tab, Right to Quality, Tab ×2 to the Video "Small" segment, then Left or Right | Live: page switch. Value change: code (1531-1546) |
| Change a setting: Engines | Partly | Right to Engines, Tab to FFmpeg "Choose", Enter opens the standard "Choose ffmpeg.exe" dialog. Finishing needs typing in that Windows dialog. | Live: the dialog opened from the keyboard; it was closed with WM_CLOSE so no keys went outside Kiln |
| Read an error's details | Yes, hidden | Select the failed row, then Space. Escape, Enter or Space closes. Nothing on screen says Space works; the link looks mouse-only. | Code (1845-1850, 1778-1783) |
| Remove a row | Yes | Delete or Backspace on the selected row (it also cancels the row's job) | Code (1832, 487-499) |
| Clear | **No** | None. The Clear button is not a Tab stop and has no shortcut. Workaround: Delete on every row. | Code (`clear()` is called only from `mouse_up`, 1684) |
| Cancel a running job and keep the row | **No** | The × is mouse-only. Delete cancels but removes the row. | Code (1757-1758) |
| Close the wheel | Yes | Escape (on the outer ring the first Escape steps back to More) | Code (1817-1820) |

**Focus visibility:**
- The list shows only the selected-row wash (1.40:1 dark, 1.11:1 light), and nothing once Escape deselects.
- The wheel lights its segment with a wash and a 1.5 DIP celadon arc: 6.31:1 dark, 1.81:1 light.
- Settings shows a 1.5 DIP celadon ring 3 DIPs outside the control, but only after the first Tab.
- The options sheet opens with the ring on Convert (code).
- The Details dialog never draws one.

**Tab order:**
- In the main window, Tab does nothing (live).
- In Settings and the sheet, the order is logical and wraps (live in Settings).
- In the wheel, Tab is overloaded: it opens the options sheet when the lit format has options, and moves to the next segment otherwise.

## Contrast

All ratios are WCAG 2.x relative-luminance contrast. Alpha fills are composited on the background they really sit on, per `render.rs`:
- selected row, chosen segment and lit wheel segment: accent at 18% on surface;
- hovered row: line at 50%;
- pulse peak: accent at 48%;
- link underline: muted at 80%;
- scroll thumb: muted at 50%.

The 3% background grain is ignored.

Cross-check from real pixels in the screenshots:
- selected row `#39423B` against a computed `#39413B`;
- light selected segment `#D5DACD`, the same as computed;
- light focus ring `#7FB5A0`;
- dark surface `#2A2825` and ground `#1C1B19`, exact.

Text is 12 DIPs (caption) or 14 DIPs (body), so 4.5:1 applies. Only the 22 DIP semibold serif titles would count as large.

| Theme | Element | Foreground | Background (composited) | Ratio | Needs | Result |
|---|---|---|---|---|---|---|
| dark | Body text: file name, row label | `#EDE6DA` | `#2A2825` (surface) | 11.85:1 | 4.5:1 | pass |
| dark | Body text on selected row | `#EDE6DA` | `#39413B` (accent 18% on surface) | 8.49:1 | 4.5:1 | pass |
| dark | Muted second line (12 px caption) | `#9A8F80` | `#2A2825` (surface) | 4.63:1 | 4.5:1 | pass |
| dark | Muted second line on selected row | `#9A8F80` | `#39413B` (accent 18% on surface) | 3.32:1 | 4.5:1 | **fail** |
| dark | Muted second line on hovered row | `#9A8F80` | `#32302C` (line 50% on surface) | 4.15:1 | 4.5:1 | **fail** |
| dark | Empty-state tagline and hint (muted) | `#9A8F80` | `#2A2825` (surface) | 4.63:1 | 4.5:1 | pass |
| dark | Footer status line (muted, 12 px) | `#9A8F80` | `#1C1B19` (ground) | 5.42:1 | 4.5:1 | pass |
| dark | Header wordmark (text) | `#EDE6DA` | `#1C1B19` (ground) | 13.88:1 | 4.5:1 | pass |
| dark | Format chip code (text on line fill) | `#EDE6DA` | `#3B3834` (line) | 9.40:1 | 4.5:1 | pass |
| dark | Wheel labels and icons (text on dish) | `#EDE6DA` | `#2A2825` (surface) | 11.85:1 | 4.5:1 | pass |
| dark | Wheel label on lit segment | `#EDE6DA` | `#39413B` (accent 18% on surface) | 8.49:1 | 4.5:1 | pass |
| dark | Wheel label at pulse peak | `#EDE6DA` | `#536C60` (accent 48% on surface) | 4.60:1 | 4.5:1 | pass |
| dark | Wheel centre source line (muted on ground) | `#9A8F80` | `#1C1B19` (ground) | 5.42:1 | 4.5:1 | pass |
| dark | Outer-ring group icon (muted) | `#9A8F80` | `#2A2825` (surface) | 4.63:1 | 3:1 | pass |
| dark | Settings row labels (muted) | `#9A8F80` | `#2A2825` (surface) | 4.63:1 | 4.5:1 | pass |
| dark | Unselected tab label (muted) | `#9A8F80` | `#2A2825` (surface) | 4.63:1 | 4.5:1 | pass |
| dark | Selected tab or segment label | `#EDE6DA` | `#39413B` (accent 18% on surface) | 8.49:1 | 4.5:1 | pass |
| dark | Button label (Clear, Close, Choose) | `#EDE6DA` | `#2A2825` (surface) | 11.85:1 | 4.5:1 | pass |
| dark | Button label, hovered | `#EDE6DA` | `#3B3834` (line) | 9.40:1 | 4.5:1 | pass |
| dark | Disabled "Forget picks" | `#9A8F80` | `#2A2825` (surface) | 4.63:1 | exempt | n/a |
| dark | Primary Convert label | `#1C1B19` | `#7FB5A0` (accent) | 7.39:1 | 4.5:1 | pass |
| dark | Primary Convert label, hovered | `#1C1B19` | `#8CBBA7` (accent + 12% text) | 8.02:1 | 4.5:1 | pass |
| dark | Error line (crack red, 12 px) | `#B5503F` | `#2A2825` (surface) | 2.93:1 | 4.5:1 | **fail** |
| dark | Error line on selected row | `#B5503F` | `#39413B` (accent 18% on surface) | 2.10:1 | 4.5:1 | **fail** |
| dark | "Show details" link (muted, 12 px) | `#9A8F80` | `#2A2825` (surface) | 4.63:1 | 4.5:1 | pass |
| dark | "Show details" link on selected row | `#9A8F80` | `#39413B` (accent 18% on surface) | 3.32:1 | 4.5:1 | **fail** |
| dark | Details dialog engine output (muted mono) | `#9A8F80` | `#2A2825` (surface) | 4.63:1 | 4.5:1 | pass |
| dark | Gear icon (muted on header ground) | `#9A8F80` | `#1C1B19` (ground) | 5.42:1 | 3:1 | pass |
| dark | Cancel × (muted on row) | `#9A8F80` | `#2A2825` (surface) | 4.63:1 | 3:1 | pass |
| dark | Focus ring (accent, 1.5 DIP) vs panel | `#7FB5A0` | `#2A2825` (surface) | 6.31:1 | 3:1 | pass |
| dark | Selected-row wash vs unselected row (only list focus cue) | `#39413B` | `#2A2825` (surface) | 1.40:1 | 3:1 | **fail** |
| dark | Chosen segment edge (accent) vs surface | `#7FB5A0` | `#2A2825` (surface) | 6.31:1 | 3:1 | pass |
| dark | Lit wheel segment wash vs dish | `#39413B` | `#2A2825` (surface) | 1.40:1 | 3:1 | fail (the arc carries it) |
| dark | Lit wheel segment arc (accent) vs dish | `#7FB5A0` | `#2A2825` (surface) | 6.31:1 | 3:1 | pass |
| dark | Suggestion dot (accent, 4 DIP) vs dish | `#7FB5A0` | `#2A2825` (surface) | 6.31:1 | 3:1 | pass |
| dark | Suggestion dot on lit segment | `#7FB5A0` | `#39413B` (accent 18% on surface) | 4.53:1 | 3:1 | pass |
| dark | Progress fill (accent) vs track (line) | `#7FB5A0` | `#3B3834` (line) | 5.01:1 | 3:1 | pass |
| dark | Progress track (line) vs row surface | `#3B3834` | `#2A2825` (surface) | 1.26:1 | 3:1 | **fail** |
| dark | Progress glow head vs track | `#B0CBBA` | `#3B3834` (line) | 6.72:1 | 3:1 | pass |
| dark | Unknown-length progress, dim phase | `#4F5E54` | `#3B3834` (line) | 1.70:1 | 3:1 | **fail** |
| dark | Unknown-length progress, bright phase | `#607D6F` | `#3B3834` (line) | 2.59:1 | 3:1 | **fail** |
| dark | Button and control border (line) vs surface | `#3B3834` | `#2A2825` (surface) | 1.26:1 | 3:1 | fail (advisory: labels identify buttons) |
| dark | Link underline (muted 80%) | `#847A6E` | `#2A2825` (surface) | 3.49:1 | 3:1 | pass |
| dark | Scroll thumb (muted 50%) | `#625C52` | `#2A2825` (surface) | 2.22:1 | 3:1 | **fail** |
| light | Body text: file name, row label | `#1C1B19` | `#E8E2D7` (surface) | 13.35:1 | 4.5:1 | pass |
| light | Body text on selected row | `#1C1B19` | `#D5DACD` (accent 18% on surface) | 12.08:1 | 4.5:1 | pass |
| light | Muted second line (12 px caption) | `#6B6257` | `#E8E2D7` (surface) | 4.64:1 | 4.5:1 | pass |
| light | Muted second line on selected row | `#6B6257` | `#D5DACD` (accent 18% on surface) | 4.20:1 | 4.5:1 | **fail** |
| light | Muted second line on hovered row | `#6B6257` | `#DFD8CC` (line 50% on surface) | 4.23:1 | 4.5:1 | **fail** |
| light | Empty-state tagline and hint (muted) | `#6B6257` | `#E8E2D7` (surface) | 4.64:1 | 4.5:1 | pass |
| light | Footer status line (muted, 12 px) | `#6B6257` | `#F3EFE7` (ground) | 5.22:1 | 4.5:1 | pass |
| light | Header wordmark (text) | `#1C1B19` | `#F3EFE7` (ground) | 15.01:1 | 4.5:1 | pass |
| light | Format chip code (text on line fill) | `#1C1B19` | `#D6CEC1` (line) | 11.03:1 | 4.5:1 | pass |
| light | Wheel labels and icons (text on dish) | `#1C1B19` | `#E8E2D7` (surface) | 13.35:1 | 4.5:1 | pass |
| light | Wheel label on lit segment | `#1C1B19` | `#D5DACD` (accent 18% on surface) | 12.08:1 | 4.5:1 | pass |
| light | Wheel label at pulse peak | `#1C1B19` | `#B6CCBD` (accent 48% on surface) | 10.13:1 | 4.5:1 | pass |
| light | Wheel centre source line (muted on ground) | `#6B6257` | `#F3EFE7` (ground) | 5.22:1 | 4.5:1 | pass |
| light | Outer-ring group icon (muted) | `#6B6257` | `#E8E2D7` (surface) | 4.64:1 | 3:1 | pass |
| light | Settings row labels (muted) | `#6B6257` | `#E8E2D7` (surface) | 4.64:1 | 4.5:1 | pass |
| light | Unselected tab label (muted) | `#6B6257` | `#E8E2D7` (surface) | 4.64:1 | 4.5:1 | pass |
| light | Selected tab or segment label | `#1C1B19` | `#D5DACD` (accent 18% on surface) | 12.08:1 | 4.5:1 | pass |
| light | Button label (Clear, Close, Choose) | `#1C1B19` | `#E8E2D7` (surface) | 13.35:1 | 4.5:1 | pass |
| light | Button label, hovered | `#1C1B19` | `#D6CEC1` (line) | 11.03:1 | 4.5:1 | pass |
| light | Disabled "Forget picks" | `#6B6257` | `#E8E2D7` (surface) | 4.64:1 | exempt | n/a |
| light | Primary Convert label | `#1C1B19` | `#7FB5A0` (accent) | 7.39:1 | 4.5:1 | pass |
| light | Primary Convert label, hovered | `#1C1B19` | `#73A390` (accent + 12% text) | 6.04:1 | 4.5:1 | pass |
| light | Error line (crack red, 12 px) | `#B5503F` | `#E8E2D7` (surface) | 3.89:1 | 4.5:1 | **fail** |
| light | Error line on selected row | `#B5503F` | `#D5DACD` (accent 18% on surface) | 3.52:1 | 4.5:1 | **fail** |
| light | "Show details" link (muted, 12 px) | `#6B6257` | `#E8E2D7` (surface) | 4.64:1 | 4.5:1 | pass |
| light | "Show details" link on selected row | `#6B6257` | `#D5DACD` (accent 18% on surface) | 4.20:1 | 4.5:1 | **fail** |
| light | Details dialog engine output (muted mono) | `#6B6257` | `#E8E2D7` (surface) | 4.64:1 | 4.5:1 | pass |
| light | Gear icon (muted on header ground) | `#6B6257` | `#F3EFE7` (ground) | 5.22:1 | 3:1 | pass |
| light | Cancel × (muted on row) | `#6B6257` | `#E8E2D7` (surface) | 4.64:1 | 3:1 | pass |
| light | Focus ring (accent, 1.5 DIP) vs panel | `#7FB5A0` | `#E8E2D7` (surface) | 1.81:1 | 3:1 | **fail** |
| light | Selected-row wash vs unselected row (only list focus cue) | `#D5DACD` | `#E8E2D7` (surface) | 1.11:1 | 3:1 | **fail** |
| light | Chosen segment edge (accent) vs surface | `#7FB5A0` | `#E8E2D7` (surface) | 1.81:1 | 3:1 | **fail** |
| light | Lit wheel segment wash vs dish | `#D5DACD` | `#E8E2D7` (surface) | 1.11:1 | 3:1 | **fail** |
| light | Lit wheel segment arc (accent) vs dish | `#7FB5A0` | `#E8E2D7` (surface) | 1.81:1 | 3:1 | **fail** |
| light | Suggestion dot (accent, 4 DIP) vs dish | `#7FB5A0` | `#E8E2D7` (surface) | 1.81:1 | 3:1 | **fail** |
| light | Suggestion dot on lit segment | `#7FB5A0` | `#D5DACD` (accent 18% on surface) | 1.63:1 | 3:1 | **fail** |
| light | Progress fill (accent) vs track (line) | `#7FB5A0` | `#D6CEC1` (line) | 1.49:1 | 3:1 | **fail** |
| light | Progress track (line) vs row surface | `#D6CEC1` | `#E8E2D7` (surface) | 1.21:1 | 3:1 | **fail** |
| light | Progress glow head vs track | `#527063` | `#D6CEC1` (line) | 3.49:1 | 3:1 | pass |
| light | Unknown-length progress, dim phase | `#BCC6B7` | `#D6CEC1` (line) | 1.13:1 | 3:1 | **fail** |
| light | Unknown-length progress, bright phase | `#A6C0AF` | `#D6CEC1` (line) | 1.25:1 | 3:1 | **fail** |
| light | Button and control border (line) vs surface | `#D6CEC1` | `#E8E2D7` (surface) | 1.21:1 | 3:1 | fail (advisory: labels identify buttons) |
| light | Link underline (muted 80%) | `#847C71` | `#E8E2D7` (surface) | 3.49:1 | 3:1 | pass |
| light | Scroll thumb (muted 50%) | `#AAA297` | `#E8E2D7` (surface) | 1.96:1 | 3:1 | **fail** |

Notes:
- `theme.rs`'s own test, `text_is_readable_on_every_surface`, checks text and muted text against ground and surface only. It never sees the composited row states, the crack red, or the accent as a non-text cue, which is where every failure above sits.
- The celadon accent `#7FB5A0` is shared by both palettes and works only on dark grounds. Light theme needs a darker accent for rings, edges, dots and progress, around 3:1 on `#E8E2D7` (for example `#4F8A74` or darker; verify).
- The progress fill passes in dark theme, but the running row's second line gives no percentage, so on light the bar is the only progress cue and it fails.

## Target size

All values are in DIPs, which equal CSS px at 100%, from `layout.rs` and `wheel.rs`. WCAG 2.2 2.5.8 needs 24 × 24, or the spacing exception. The 44 DIP figure is the comfort guide (2.5.5 AAA and platform guidance).

| Target | Hit area | 2.5.8 (24) | 44 guide | Notes |
|---|---|---|---|---|
| Gear (Settings) button | 32 × 32 (24 icon) | pass | below | `layout.rs` 83 |
| "Show details" link | 88 × 20 (text about 70 × 16) | **fail** | below | `layout.rs` 150-157. 20 tall and inside the row's own target, so no spacing exception |
| Remove × | none | n/a | n/a | No per-row remove control exists; rows are removed with Delete. The only × is a running job's cancel. |
| Cancel × on a running row | 24 × 24 | pass (exactly) | below | `layout.rs` 145-147. Drawn only while the row is hovered or selected |
| Clear | 72 × 32 | pass | below (height) | `layout.rs` 122-127 |
| File row | full width × 56 | pass | pass | Click selects; right-click opens Convert |
| Wheel inner segment (9 with More) | annular sector r 56-136, hit out to r 184; 40° each | pass | pass | Largest inscribed circle 69; chord at the inner edge 38. In a minimum 480 × 360 window with More: scale 0.83, so circle 57 |
| Wheel outer ring (More) | band r 144-208, hit r 140-256 | pass for real files | pass for real files | PNG has 4 extra formats plus 1 group icon = 5 slots (circle 64). MOV has 17 plus 3 = 20 slots (circle 56; 47 at minimum window). A file type with about 48 or more slots would fall under 24 (computed) |
| Settings tabs | 80 × 32 | pass | below | |
| Settings segments (Theme, Suggestions, presets) | 64 × 32 | pass | below | |
| Settings steppers | 96 × 32 (back zone 28 × 32, forward 68 × 32) | pass | below | Two targets in one box with no visible divider |
| Choose / Reset / Close | 72 × 32 | pass | below | |
| Wheel-key buttons | up to 120 × 32 | pass | below | |
| Forget picks / Check for updates | 104 × 32 / 136 × 32 | pass | below | |
| Options sheet: segments, steppers, Cancel, Convert | 64 × 32, 136 × 32 (back zone 28), 72 × 32, 88 × 32 | pass | below | |
| Details dialog Close | 72 × 32 | pass | below | |
| Title-bar Minimize / Maximize / Close | about 47 × 30 (MSAA, 96-DPI virtualised) | pass | about at guide | Standard caption buttons drawn by Windows, not custom; Kiln only recolours them through DWM |

## Motion and timing (code)

- **Animation effects is not honoured.** There is no `SystemParametersInfo` call anywhere in the repo, so `SPI_GETCLIENTAREAANIMATION` is never read. These all run regardless:
  - wheel pour: scale 0.55 to 1 plus a fade, 160 ms;
  - More ring: scale 0.66 to 1, 160 ms;
  - choose pulse: 180 ms;
  - sheet: fade with an 8 DIP rise, 160 ms;
  - Settings tab marker slide (160 ms) and page crossfade (120 ms);
  - rows: appear (180 ms), settle (200 ms) and facts crossfade (160 ms);
  - drop-zone wash: 120 ms;
  - the running progress line's breathing glow: continuous, 2.4 s period (`theme.rs` 148-163, `mod.rs` 81-84 and 407-432).
- Nothing flashes: the glow is a smooth 0.42 Hz luminance change, so 2.3.1 passes. The motion counts against 2.3.3 (AAA). 2.2.2 is passed with the caveat above.
- **Held keys.** The Shift, Ctrl and Alt drop gestures need a modifier held during a drag (`drag_over`, `mod.rs` 1861-1888). Each has a no-hold alternative after the drop: Enter, Shift+Enter, Ctrl+Enter or Alt+Enter on the selected row, Shift+F10, Apps or right-click. So 2.1.1 doesn't fail on the gesture itself, and 2.5.1 doesn't apply (not path-based).
  - The alternatives are not mentioned in the UI: the empty-state hint describes only the held-key drop.
  - Not tested: whether Sticky Keys' latched modifiers reach OLE drag key state (`MK_SHIFT`, `MK_CONTROL`, `MK_ALT`).
- No time limits anywhere (2.2.1 not applicable).

## High contrast and text size (code only; not tested live)

- **Contrast themes.** Kiln never calls `SystemParametersInfo(SPI_GETHIGHCONTRAST)` or `GetSysColor`, and doesn't handle `WM_SYSCOLORCHANGE`. `WM_SETTINGCHANGE` reacts only to `ImmersiveColorSet`, and only to re-read `AppsUseLightTheme` (`mod.rs` 2497-2506, `system.rs` 48-65). Under a Windows contrast theme, Kiln:
  - keeps drawing its own soot/bone or bone/soot palette, celadon accent and brick error colour;
  - sets its own DWM caption colours (`style_title_bar`);
  - ignores the user's chosen text, highlight, hyperlink and button colours.

  Text on surfaces stays readable (11.9-13.4:1). But the focus ring, selection, wheel highlight, progress and error colours ignore the user's palette, and in light theme they stay at 1.1-1.9:1.
- **Text size.** `UISettings.TextScaleFactor` is never read, and the `windows` features list has no `UI_ViewManagement`. DirectWrite formats are built once at fixed DIP sizes (`text.rs`: 12, 14, 16, 22 and 32). Kiln therefore follows display scaling (100-500%) but not Settings > Accessibility > Text size. A user who raises Text size to 150-225% still gets 12 DIP (9 pt) captions for the second line, status line, error line and "Show details".

  Honouring it later needs layout work: row lines, labels and buttons are fixed-height clipped boxes (`ROW_HEIGHT` 56, 20 DIP line rects, 32 DIP buttons).

## What could not be tested

- **100% scale.** All four monitors run at 125%, and changing display settings was forbidden. Screenshots and UIA rectangles are at 125% (900 × 600 px client). Target sizes come from the DIP layout code, which is scale-independent.
- **Most of the GUI run.** At the options-sheet step, the focus guard found File Explorer ("This PC") in the foreground. It killed Kiln, sent nothing, and threw. Per the rules, the GUI part stopped there. Never captured:
  - screenshots `wheel-tools`, `wheel-actions`, `options-sheet`, `error-row`, `error-details`, `converting`, `done-row` and `light-list`;
  - the error-row UIA dump;
  - live runs of Enter-to-convert, Ctrl+Enter, Alt+Enter, Delete, Space-for-details, Escape-to-close-wheel and Clear.

  Those rows are marked "code" above. `broken.mp4` was never created, so the samples folder is unchanged.
- **Captured screenshots** in `shots/`: `empty`, `list` (suggestions arrived), `wheel-convert`, `settings-general`, `settings-quality` and `settings-engines`. All are dark theme at 125%. Each was opened and checked: it matches its name, and no personal path appears (only `C:\kiln\target\samples` and `C:\Program Files\...`).
- **Narrator, contrast themes, Text size, Animation effects.** Not started or changed, as the rules require. What Narrator would say is inferred from the UIA and MSAA trees, and the rest from code.
- **Engines setting change.** The Choose dialog was opened from the keyboard but not completed, since that would mean typing into a Windows dialog rather than Kiln.
- **Reflow and text spacing at 200-400%.** Evaluated from code only.

## Artefacts

- `docs/ux/phase1/uia/01-empty.txt`, `02-list.txt`, `03-wheel.txt`, `04-settings.txt`, `04b-settings-tab-focus.txt`: raw UIA dumps (raw, control and content views, focused element, ElementFromPoint probes, plus MSAA from 01 to 03).
- `docs/ux/phase1/shots/*.png`: the six screenshots listed above.
