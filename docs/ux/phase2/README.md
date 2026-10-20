# UX Phase 2: fixes and evidence

This is Kiln on branch `overhaul` (from `main` 8c26964), built from the [Phase 1 baseline](../phase1/README.md). The fixes were built overnight, one commit per fix, so each can be reverted alone. The designs are [ux-fixes](../../superpowers/specs/2026-10-19-ux-fixes-design.md) and [held-results](../../superpowers/specs/2026-10-19-held-results-design.md).

Nothing here is a usability result. The sessions in [usability-test.md](../phase1/usability-test.md) still have to be run against this build. They confirm or undo these changes.

| File | What it is |
|---|---|
| [contrast.md](contrast.md) | All 52 contrast pairs, both themes, with the Phase 1 ratio beside the new one. Generated from the code |
| `uia/` | UI Automation dumps of the new build, and `dump.ps1`, the read-only script that made them |
| `shots/` | 21 screenshots of the new build, dark and light, at 100% |

## The baseline rows, one by one

Each row of the Phase 1 table, what was done, and the commits. "Partly fixed" says what is left. The heuristic IDs are from [heuristics.md](../phase1/heuristics.md). The WCAG numbers are from [accessibility.md](../phase1/accessibility.md).

| # | Problem | Status | Commits | Covers |
|---|---|---|---|---|
| 1 | Screen readers get "Kiln, window" and nothing else | **Fixed.** Kiln serves a UI Automation tree: the window, header and footer buttons, the list with a named row each, the row buttons, the status text, the wheel as a menu, Settings, the details dialog and the help overlay. Status changes raise notifications. Dumps are in `uia/` | 8ab4aef, 21d3e22, bae90a7 | WCAG 1.1.1, 1.3.1, 4.1.2, 4.1.3 |
| 2 | Files can only be added by dragging | **Partly fixed.** Add files button (empty state and footer), Ctrl+O, Ctrl+V for files copied in Explorer, and a right-click menu. The modifier drop stays as the fast path. **Deferred:** "Open with" and "Send to" in File Explorer, because they need an installer to register them | e852ee0, 1ddb1a7 | H-01, H-12, H-18, WCAG 2.1.1, 2.5.7 |
| 3 | Once files are listed, nothing says what to do next | **Fixed.** The footer says "Right-click a file, or select it and press Enter, to convert it." The empty state puts the instruction before the tagline. At the minimum width a long hint shortens | e852ee0, 1ddb1a7 | H-02, H-17 |
| 4 | Only one file can be selected | **Fixed.** Ctrl-click, Shift-click, Ctrl+A and Shift with the arrow keys. Wheels, Export, Remove and the menu act on the selection. Merge appears only when the selection allows it. The footer says "3 selected" | 8daf2bf, 328b4e8 | H-03 |
| 5 | A finished file can't be opened or shown in its folder | **Fixed.** Results are held in Kiln. A held row says `Ready: name`, has an Export control, and opens with a double-click. An exported row says `Saved to <folder>.`, and clicking it opens that folder. **Deferred:** dragging a result out of Kiln | 2f87616, bb64b9c, 54cc473, 19d01be, 66a834b, 53e738d | H-04 |
| 6 | The Shift, Ctrl and Alt meanings must be remembered; Tools and Actions have no visible path | **Fixed.** The right-click menu lists Convert, Tools and File actions with their keys. The wheel's title shows its key (`Convert · Shift`). F1 opens a help overlay with every shortcut | e852ee0, 0ab8d68, 77469b9 | H-05, H-09, H-11, H-14 |
| 7 | Errors are raw engine text, cut off, with no next step; "isn't installed" is a dead end | **Fixed.** Known failures become a sentence with a next step. The raw output stays in Show details, which now scrolls, has a Copy button and opens on Copy. A missing helper app says what it is for and has a Get it link. A long row line ends in `…`, with the full sentence in the details | 97307fa, 358bd86, 0ab8d68 | H-07, H-20, H-21 |
| 8 | Formats are shown as codes only | **Fixed.** Every format has a plain description. It shows in the footer while a target is lit, in the options sheet's title, and in the UI Automation name. The More ring labels each group. **Deferred:** type-to-filter in the More ring | 96d959b, b6eba28 | H-08, H-16 |
| 9 | Clear cancels running work with no warning, and has no keyboard path | **Partly fixed.** Clear, Close and removing a row now ask first when jobs are running or results are held, in one prompt. Delete on a running row asks too. **Not fixed:** Clear still has no shortcut and isn't on the Tab path. It has an Invoke pattern for assistive tools. Ctrl+A then Delete empties the list by keyboard, through the same prompt (from the code and tests, not seen live). **Deferred:** Undo for Move | 19d01be, 66a834b, 53e738d, 8ab4aef | H-06, H-15 |
| 10 | Light theme's accent fails non-text contrast, and the error red fails text contrast | **Fixed.** Light theme has a darker accent for rings, edges, dots and progress. The error red is darker in light theme and lighter in dark. Muted text on selected and hovered rows is darker. The selected row has a 1 DIP accent edge. All 47 pairs per theme that have a threshold pass; see [contrast.md](contrast.md) | 75e04bd, 734ecdd | WCAG 1.4.1, 1.4.3, 1.4.11 |
| 11 | Windows contrast themes, Text size and Animation effects are ignored | **Fixed, in code and tests only.** High contrast draws with the system colours. Text size scales the fonts and the layout. Animation effects off skips pours, slides and fades. None of the three was shown live; see below | 75e04bd, 734ecdd | WCAG 1.4.3 under user colours, 1.4.4, 2.3.3, 2.4.7, 2.5.8 |
| 12 | Many names for one idea: tool, action, convert, format, type, engine | **Fixed.** One word per idea: convert, tool, file action, helper app. The Settings tab "Engines" is now "Helper apps". Sheet labels say what they change (Sound quality, Sharpness (DPI), Keys to hold while dropping). Docs match | 0ab8d68, 77469b9, 9888602 | H-13 |

Not in any row, and still deferred: Explorer "Open with" and "Send to" (row 2), type-to-filter (row 8), Undo for Move (row 9), and dragging a result out of Kiln (row 5).

## Seen live, and not seen live

The controller ran the GUI pass on the release build. This is the ledger's own account.

**Seen live, and fine:**
- The empty state, in order.
- The footer hint.
- The wheel's title with its key, the lit target's description, and `Options… (Tab)` in the centre.
- A held row and `Export all (1)`.
- The right-click menu. A held row shows Open and no Show in folder, by design.
- An error row reading "an MP4", and its Show details dialog, with the sentence first and Copy focused.
- Help.
- Settings > Helper apps, with a purpose line for each app.
- The close prompt.
- Export through the Save dialog, then `Saved to export.` on the row.
- The held folder removed when Kiln exits.

**Not seen live:**
- High contrast.
- Text size.
- Animation effects off.
- Tooltips. They appear on hover, and the shots can't catch them.
- UI Automation Invoke. The dumps show that the pattern is offered, not that it works.
- Narrator. The dumps show what a screen reader is given, not what it says.

Rows 10 and 11 rest on unit tests. `every_pair_reaches_its_threshold` in `crates/kiln-app/src/theme.rs` checks every pair in both themes. The contrast-theme palette and the display-setting handling are tested there and in `layout.rs`. Neither is a replacement for looking at it, so row 11 should be checked by a person with those Windows settings on.

One thing the dumps show: the header buttons, Add files and Clear report `keyboardFocusable=False`. Their shortcuts work (Ctrl+O, Ctrl+, and F1), but Clear has none.

## How these were captured

- **Scale.** The shots and dumps are at **100%**. Both monitors now run at 96 DPI. Changing the display scale is a system setting, and it wasn't touched. Phase 1 was captured at 125%, so the two sets don't line up pixel for pixel. The header line in each dump says "(no monitor runs at 100%)". That is stale text in `dump.ps1`. The captures are at 100%.
- **Build and data.** The release `kiln-app`, run with a scratch `APPDATA` and `LOCALAPPDATA`, so the owner's settings and held results were never read or changed. The files are copies of the samples in `C:\kiln\target\p2\samples`.
- **Windows settings.** High contrast, Text size and Animation effects are covered by unit tests only, because showing them would mean changing Windows settings. They weren't changed.
- **The close prompt** is a Windows task dialog, drawn by Windows. It looks the same in both sets because it doesn't take Kiln's theme.
- **The dumps** come from `uia/dump.ps1`. It sends no keys and no mouse input. It walks the tree and writes it as text. The method is Phase 1's.
- **The contrast table** comes from `cargo run -p kiln-app --example contrast_md`, from the repository root.

## UI Automation dumps

| File | State |
|---|---|
| [01-empty.txt](uia/01-empty.txt) | Empty window, nothing listed |
| [02-list.txt](uia/02-list.txt) | Four files listed, suggestions in, hint showing |
| [03-wheel.txt](uia/03-wheel.txt) | Convert wheel open on `harbour.png` with a target lit |
| [07-details.txt](uia/07-details.txt) | Details dialog open on the failed `broken clip.mp4` |
| [08-help.txt](uia/08-help.txt) | Help overlay open |
| [09-settings.txt](uia/09-settings.txt) | Settings open on Helper apps |
| [11-saved.txt](uia/11-saved.txt) | `harbour.pdf` exported to a folder |
| [dump.ps1](uia/dump.ps1) | The script |

The numbers skip 04 to 06 and 10, which are states with no dump of their own. Phase 1's dumps were a different set, so the numbers don't map one to one.

## Screenshots

All at 100%. "Dark" and "light" are Kiln's own themes.

| File | Shows |
|---|---|
| [01-empty-dark.png](shots/01-empty-dark.png), [light](shots/01-empty-light.png) | The empty window: "Drop files here", the Add files line and button, the modifier hint, the privacy line, then the tagline. Help and Settings icons in the header |
| [02-listed-hint-dark.png](shots/02-listed-hint-dark.png), [light](shots/02-listed-hint-light.png) | Four files listed with a type chip each. Each row's second line reads what it is, the suggestion, the size, the folder. The footer hint, and the Add files and Clear buttons. The last row is selected |
| [03-wheel-dark.png](shots/03-wheel-dark.png), [light](shots/03-wheel-light.png) | The Convert wheel open on `harbour.png`, with JPG lit. The centre shows `Options… (Tab)` and `PNG → JPG`. The footer describes JPG and gives the suggestion, cut with `…` |
| [03b-wheel-lit-dark.png](shots/03b-wheel-lit-dark.png) | The same wheel with PDF lit. The centre shows the key, Shift, and the footer says what PDF is |
| [04-held-row-dark.png](shots/04-held-row-dark.png) | A finished result held in Kiln: `Ready: harbour.pdf`, an Export link on the row, `Export all (1)` in the footer, and "Converted 1 file to PDF. Export to keep it." |
| [05-context-menu-dark.png](shots/05-context-menu-dark.png) | The right-click menu on the held row: Convert, Tools, File actions, then Export, Open, then Remove, each with its key |
| [06-error-row-dark.png](shots/06-error-row-dark.png) | The failed row: "This file looks damaged, or isn't really an MP4. Try opening it in another app." with Show details. The held row is still above it |
| [06-error-and-held-light.png](shots/06-error-and-held-light.png) | The same two rows in the light theme, with the error line in the darker red |
| [07-details-dark.png](shots/07-details-dark.png), [light](shots/07-details-light.png) | "What FFmpeg said": the plain sentence first, then FFmpeg's own lines. Copy has focus. Close beside it |
| [08-help-dark.png](shots/08-help-dark.png), [light](shots/08-help-light.png) | The Help overlay, scrolled to the top: Ways in (drop, Ctrl+O, Ctrl+V, right-click, the held keys) and the start of Results. It has a scroll bar |
| [09-settings-dark.png](shots/09-settings-dark.png) | Settings > General: Start exports in, Theme, Keys to hold while dropping, Suggestions. The tabs read General, Quality, Helper apps |
| [09b-settings-helper-apps-dark.png](shots/09b-settings-helper-apps-dark.png), [light](shots/09b-settings-helper-apps-light.png) | Settings > Helper apps: FFmpeg, Pandoc, LibreOffice and Calibre, each with where it was found and a line on what it is for |
| [10-close-prompt-dark.png](shots/10-close-prompt-dark.png), [light](shots/10-close-prompt-light.png) | The close prompt: "1 result isn't saved yet." with Export all, Discard and Cancel. Drawn by Windows, so the two look the same |
| [11-saved-row-dark.png](shots/11-saved-row-dark.png) | After Export: the row says `Saved to export.`, the held result is gone from the footer, and the hint is back |

Missing from the set, and why: a screenshot with the footer hint at a narrow width, and every high contrast, large text and reduced animation view, because those need Windows settings changed.

## Next

1. Run the sessions in [usability-test.md](../phase1/usability-test.md) on this build.
2. Check high contrast, Text size and Animation effects on a PC with those settings on.
3. Set the success thresholds from the sessions, and decide which of these changes stay.
