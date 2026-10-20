# Contrast, re-measured

Generated from the code by `cargo run -p kiln-app --example contrast_md`. It uses `contrast_report` in `crates/kiln-app/src/theme.rs`, the same function the unit test `every_pair_reaches_its_threshold` checks. Don't edit this file by hand.

Ratios are WCAG 2.x relative-luminance contrast. Washes are composited on what they sit on, as `win/render.rs` draws them. The 3% background grain is ignored. The method is the one in the [Phase 1 table](../phase1/accessibility.md#contrast).

- Text needs 4.5:1. A cue that isn't text needs 3:1.
- Phase 1 is the ratio printed in the Phase 1 table, with its verdict. "new" means the pair wasn't in that table.
- A row with no threshold is exempt (disabled) or advisory. Its name says why, and something else carries the meaning.
- Some pairs Phase 1 marked as fail are now advisory with no threshold: the selected-row wash, the lit-segment wash, the progress track and the control border. Their ratios are unchanged. What carries the meaning now is a pair that passes: the row's accent edge, the segment arc, the progress fill, and the button label.
- Phase 1 measured the old scroll thumb at 50% and the old light accent `#7FB5A0`. The scroll thumb is now 80%, and the light theme uses a darker accent for non-text cues.

## Summary

- Dark: 52 pairs. 47 pass, 0 fail, 5 with no threshold. 8 of the pairs that failed in Phase 1 now pass.
- Light: 52 pairs. 47 pass, 0 fail, 5 with no threshold. 14 of the pairs that failed in Phase 1 now pass.
- 88 rows are matched to Phase 1 and 16 are new. No pair with a threshold fails.

## Dark theme

| Element | Foreground | Background | Phase 1 | Phase 2 | Needs | Result |
|---|---|---|---|---|---|---|
| Body text: file name, row label | `#EDE6DA` | `#2A2825` | 11.85:1 (pass) | 11.85:1 | 4.5:1 | pass |
| Body text on selected row | `#EDE6DA` | `#39413B` | 8.49:1 (pass) | 8.49:1 | 4.5:1 | pass |
| Muted second line (12 px caption) | `#9A8F80` | `#2A2825` | 4.63:1 (pass) | 4.63:1 | 4.5:1 | pass |
| Muted second line on selected row | `#B3AB9F` | `#39413B` | 3.32:1 (fail) | 4.64:1 | 4.5:1 | pass |
| Muted second line on hovered row | `#B3AB9F` | `#32302C` | 4.15:1 (fail) | 5.80:1 | 4.5:1 | pass |
| Empty-state tagline and hint (muted) | `#9A8F80` | `#2A2825` | 4.63:1 (pass) | 4.63:1 | 4.5:1 | pass |
| Footer status line (muted, 12 px) | `#9A8F80` | `#1C1B19` | 5.42:1 (pass) | 5.42:1 | 4.5:1 | pass |
| Header wordmark (text) | `#EDE6DA` | `#1C1B19` | 13.88:1 (pass) | 13.88:1 | 4.5:1 | pass |
| Format chip code (text on line fill) | `#EDE6DA` | `#3B3834` | 9.40:1 (pass) | 9.40:1 | 4.5:1 | pass |
| Wheel labels and icons (text on dish) | `#EDE6DA` | `#2A2825` | 11.85:1 (pass) | 11.85:1 | 4.5:1 | pass |
| Wheel label on lit segment | `#EDE6DA` | `#39413B` | 8.49:1 (pass) | 8.49:1 | 4.5:1 | pass |
| Wheel label at pulse peak | `#EDE6DA` | `#536C60` | 4.60:1 (pass) | 4.60:1 | 4.5:1 | pass |
| Wheel centre source line (muted on ground) | `#9A8F80` | `#1C1B19` | 5.42:1 (pass) | 5.42:1 | 4.5:1 | pass |
| Outer-ring group icon (muted) | `#9A8F80` | `#2A2825` | 4.63:1 (pass) | 4.63:1 | 3:1 | pass |
| Settings row labels (muted) | `#9A8F80` | `#2A2825` | 4.63:1 (pass) | 4.63:1 | 4.5:1 | pass |
| Unselected tab label (muted) | `#9A8F80` | `#2A2825` | 4.63:1 (pass) | 4.63:1 | 4.5:1 | pass |
| Tab label, hovered | `#EDE6DA` | `#3B3834` | new | 9.40:1 | 4.5:1 | pass |
| Selected tab or segment label | `#EDE6DA` | `#39413B` | 8.49:1 (pass) | 8.49:1 | 4.5:1 | pass |
| Button label (Clear, Close, Choose) | `#EDE6DA` | `#2A2825` | 11.85:1 (pass) | 11.85:1 | 4.5:1 | pass |
| Button label, hovered | `#EDE6DA` | `#3B3834` | 9.40:1 (pass) | 9.40:1 | 4.5:1 | pass |
| Disabled "Forget picks" (exempt: disabled) | `#9A8F80` | `#2A2825` | 4.63:1 (n/a) | 4.63:1 | none | n/a |
| Primary Convert label | `#1C1B19` | `#7FB5A0` | 7.39:1 (pass) | 7.39:1 | 4.5:1 | pass |
| Primary Convert label, hovered | `#1C1B19` | `#8CBBA7` | 8.02:1 (pass) | 8.02:1 | 4.5:1 | pass |
| Error line (crack red, 12 px) | `#D99D93` | `#2A2825` | 2.93:1 (fail) | 6.44:1 | 4.5:1 | pass |
| Error line on selected row | `#D99D93` | `#39413B` | 2.10:1 (fail) | 4.61:1 | 4.5:1 | pass |
| Error line on hovered row | `#D99D93` | `#32302C` | new | 5.77:1 | 4.5:1 | pass |
| "Show details" link (muted, 12 px) | `#9A8F80` | `#2A2825` | 4.63:1 (pass) | 4.63:1 | 4.5:1 | pass |
| "Show details" link on selected row | `#B3AB9F` | `#39413B` | 3.32:1 (fail) | 4.64:1 | 4.5:1 | pass |
| Details dialog engine output (muted mono) | `#9A8F80` | `#2A2825` | 4.63:1 (pass) | 4.63:1 | 4.5:1 | pass |
| Gear icon (muted on header ground) | `#9A8F80` | `#1C1B19` | 5.42:1 (pass) | 5.42:1 | 3:1 | pass |
| Cancel × (muted on row) | `#9A8F80` | `#2A2825` | 4.63:1 (pass) | 4.63:1 | 3:1 | pass |
| Cancel × on selected row | `#B3AB9F` | `#39413B` | new | 4.64:1 | 3:1 | pass |
| Focus ring (accent, 1.5 DIP) vs panel | `#7FB5A0` | `#2A2825` | 6.31:1 (pass) | 6.31:1 | 3:1 | pass |
| Focus ring on a selected row | `#7FB5A0` | `#39413B` | new | 4.53:1 | 3:1 | pass |
| Selected-row wash vs unselected row (advisory: the row's edge carries it) | `#39413B` | `#2A2825` | 1.40:1 (fail) | 1.40:1 | none | n/a |
| Selected-row edge (accent, 1 DIP) vs its wash | `#7FB5A0` | `#39413B` | new | 4.53:1 | 3:1 | pass |
| Selected-row edge (accent, 1 DIP) vs surface | `#7FB5A0` | `#2A2825` | new | 6.31:1 | 3:1 | pass |
| Chosen segment edge (accent) vs surface | `#7FB5A0` | `#2A2825` | 6.31:1 (pass) | 6.31:1 | 3:1 | pass |
| Lit wheel segment wash vs dish (advisory: the arc carries it) | `#39413B` | `#2A2825` | 1.40:1 (fail) | 1.40:1 | none | n/a |
| Lit wheel segment arc (accent) vs dish | `#7FB5A0` | `#2A2825` | 6.31:1 (pass) | 6.31:1 | 3:1 | pass |
| Suggestion dot (accent, 4 DIP) vs dish | `#7FB5A0` | `#2A2825` | 6.31:1 (pass) | 6.31:1 | 3:1 | pass |
| Suggestion dot on lit segment | `#7FB5A0` | `#39413B` | 4.53:1 (pass) | 4.53:1 | 3:1 | pass |
| Progress fill (accent) vs track (line) | `#7FB5A0` | `#3B3834` | 5.01:1 (pass) | 5.01:1 | 3:1 | pass |
| Progress fill (accent) vs row surface | `#7FB5A0` | `#2A2825` | new | 6.31:1 | 3:1 | pass |
| Progress fill (accent) on selected row | `#7FB5A0` | `#39413B` | new | 4.53:1 | 3:1 | pass |
| Progress track (line) vs row surface (advisory: the fill carries progress) | `#3B3834` | `#2A2825` | 1.26:1 (fail) | 1.26:1 | none | n/a |
| Progress glow head vs track | `#95BFAC` | `#3B3834` | 6.72:1 (pass) | 5.73:1 | 3:1 | pass |
| Unknown-length progress, dim phase | `#7FB5A0` | `#3B3834` | 1.70:1 (fail) | 5.01:1 | 3:1 | pass |
| Unknown-length progress, bright phase | `#9AC1AE` | `#3B3834` | 2.59:1 (fail) | 5.89:1 | 3:1 | pass |
| Button and control border (line) vs surface (advisory: labels identify buttons) | `#3B3834` | `#2A2825` | 1.26:1 (fail) | 1.26:1 | none | n/a |
| Link underline (muted 80%) | `#847A6E` | `#2A2825` | 3.49:1 (pass) | 3.49:1 | 3:1 | pass |
| Scroll thumb (muted 80%) | `#847A6E` | `#2A2825` | 2.22:1 (fail) | 3.49:1 | 3:1 | pass |

## Light theme

| Element | Foreground | Background | Phase 1 | Phase 2 | Needs | Result |
|---|---|---|---|---|---|---|
| Body text: file name, row label | `#1C1B19` | `#E8E2D7` | 13.35:1 (pass) | 13.35:1 | 4.5:1 | pass |
| Body text on selected row | `#1C1B19` | `#D5DACD` | 12.08:1 (pass) | 12.08:1 | 4.5:1 | pass |
| Muted second line (12 px caption) | `#6B6257` | `#E8E2D7` | 4.64:1 (pass) | 4.64:1 | 4.5:1 | pass |
| Muted second line on selected row | `#645C52` | `#D5DACD` | 4.20:1 (fail) | 4.61:1 | 4.5:1 | pass |
| Muted second line on hovered row | `#645C52` | `#DFD8CC` | 4.23:1 (fail) | 4.64:1 | 4.5:1 | pass |
| Empty-state tagline and hint (muted) | `#6B6257` | `#E8E2D7` | 4.64:1 (pass) | 4.64:1 | 4.5:1 | pass |
| Footer status line (muted, 12 px) | `#6B6257` | `#F3EFE7` | 5.22:1 (pass) | 5.22:1 | 4.5:1 | pass |
| Header wordmark (text) | `#1C1B19` | `#F3EFE7` | 15.01:1 (pass) | 15.01:1 | 4.5:1 | pass |
| Format chip code (text on line fill) | `#1C1B19` | `#D6CEC1` | 11.03:1 (pass) | 11.03:1 | 4.5:1 | pass |
| Wheel labels and icons (text on dish) | `#1C1B19` | `#E8E2D7` | 13.35:1 (pass) | 13.35:1 | 4.5:1 | pass |
| Wheel label on lit segment | `#1C1B19` | `#D5DACD` | 12.08:1 (pass) | 12.08:1 | 4.5:1 | pass |
| Wheel label at pulse peak | `#1C1B19` | `#B6CCBD` | 10.13:1 (pass) | 10.13:1 | 4.5:1 | pass |
| Wheel centre source line (muted on ground) | `#6B6257` | `#F3EFE7` | 5.22:1 (pass) | 5.22:1 | 4.5:1 | pass |
| Outer-ring group icon (muted) | `#6B6257` | `#E8E2D7` | 4.64:1 (pass) | 4.64:1 | 3:1 | pass |
| Settings row labels (muted) | `#6B6257` | `#E8E2D7` | 4.64:1 (pass) | 4.64:1 | 4.5:1 | pass |
| Unselected tab label (muted) | `#6B6257` | `#E8E2D7` | 4.64:1 (pass) | 4.64:1 | 4.5:1 | pass |
| Tab label, hovered | `#1C1B19` | `#D6CEC1` | new | 11.03:1 | 4.5:1 | pass |
| Selected tab or segment label | `#1C1B19` | `#D5DACD` | 12.08:1 (pass) | 12.08:1 | 4.5:1 | pass |
| Button label (Clear, Close, Choose) | `#1C1B19` | `#E8E2D7` | 13.35:1 (pass) | 13.35:1 | 4.5:1 | pass |
| Button label, hovered | `#1C1B19` | `#D6CEC1` | 11.03:1 (pass) | 11.03:1 | 4.5:1 | pass |
| Disabled "Forget picks" (exempt: disabled) | `#6B6257` | `#E8E2D7` | 4.64:1 (n/a) | 4.64:1 | none | n/a |
| Primary Convert label | `#1C1B19` | `#7FB5A0` | 7.39:1 (pass) | 7.39:1 | 4.5:1 | pass |
| Primary Convert label, hovered | `#1C1B19` | `#73A390` | 6.04:1 (pass) | 6.04:1 | 4.5:1 | pass |
| Error line (crack red, 12 px) | `#984335` | `#E8E2D7` | 3.89:1 (fail) | 5.11:1 | 4.5:1 | pass |
| Error line on selected row | `#984335` | `#D5DACD` | 3.52:1 (fail) | 4.62:1 | 4.5:1 | pass |
| Error line on hovered row | `#984335` | `#DFD8CC` | new | 4.65:1 | 4.5:1 | pass |
| "Show details" link (muted, 12 px) | `#6B6257` | `#E8E2D7` | 4.64:1 (pass) | 4.64:1 | 4.5:1 | pass |
| "Show details" link on selected row | `#645C52` | `#D5DACD` | 4.20:1 (fail) | 4.61:1 | 4.5:1 | pass |
| Details dialog engine output (muted mono) | `#6B6257` | `#E8E2D7` | 4.64:1 (pass) | 4.64:1 | 4.5:1 | pass |
| Gear icon (muted on header ground) | `#6B6257` | `#F3EFE7` | 5.22:1 (pass) | 5.22:1 | 3:1 | pass |
| Cancel × (muted on row) | `#6B6257` | `#E8E2D7` | 4.64:1 (pass) | 4.64:1 | 3:1 | pass |
| Cancel × on selected row | `#645C52` | `#D5DACD` | new | 4.61:1 | 3:1 | pass |
| Focus ring (accent, 1.5 DIP) vs panel | `#467966` | `#E8E2D7` | 1.81:1 (fail) | 3.89:1 | 3:1 | pass |
| Focus ring on a selected row | `#467966` | `#D5DACD` | new | 3.52:1 | 3:1 | pass |
| Selected-row wash vs unselected row (advisory: the row's edge carries it) | `#D5DACD` | `#E8E2D7` | 1.11:1 (fail) | 1.11:1 | none | n/a |
| Selected-row edge (accent, 1 DIP) vs its wash | `#467966` | `#D5DACD` | new | 3.52:1 | 3:1 | pass |
| Selected-row edge (accent, 1 DIP) vs surface | `#467966` | `#E8E2D7` | new | 3.89:1 | 3:1 | pass |
| Chosen segment edge (accent) vs surface | `#467966` | `#E8E2D7` | 1.81:1 (fail) | 3.89:1 | 3:1 | pass |
| Lit wheel segment wash vs dish (advisory: the arc carries it) | `#D5DACD` | `#E8E2D7` | 1.11:1 (fail) | 1.11:1 | none | n/a |
| Lit wheel segment arc (accent) vs dish | `#467966` | `#E8E2D7` | 1.81:1 (fail) | 3.89:1 | 3:1 | pass |
| Suggestion dot (accent, 4 DIP) vs dish | `#467966` | `#E8E2D7` | 1.81:1 (fail) | 3.89:1 | 3:1 | pass |
| Suggestion dot on lit segment | `#467966` | `#D5DACD` | 1.63:1 (fail) | 3.52:1 | 3:1 | pass |
| Progress fill (accent) vs track (line) | `#467966` | `#D6CEC1` | 1.49:1 (fail) | 3.21:1 | 3:1 | pass |
| Progress fill (accent) vs row surface | `#467966` | `#E8E2D7` | new | 3.89:1 | 3:1 | pass |
| Progress fill (accent) on selected row | `#467966` | `#D5DACD` | new | 3.52:1 | 3:1 | pass |
| Progress track (line) vs row surface (advisory: the fill carries progress) | `#D6CEC1` | `#E8E2D7` | 1.21:1 (fail) | 1.21:1 | none | n/a |
| Progress glow head vs track | `#3E6657` | `#D6CEC1` | 3.49:1 (pass) | 4.15:1 | 3:1 | pass |
| Unknown-length progress, dim phase | `#467966` | `#D6CEC1` | 1.13:1 (fail) | 3.21:1 | 3:1 | pass |
| Unknown-length progress, bright phase | `#3C6253` | `#D6CEC1` | 1.25:1 (fail) | 4.39:1 | 3:1 | pass |
| Button and control border (line) vs surface (advisory: labels identify buttons) | `#D6CEC1` | `#E8E2D7` | 1.21:1 (fail) | 1.21:1 | none | n/a |
| Link underline (muted 80%) | `#847C71` | `#E8E2D7` | 3.49:1 (pass) | 3.19:1 | 3:1 | pass |
| Scroll thumb (muted 80%) | `#847C71` | `#E8E2D7` | 1.96:1 (fail) | 3.19:1 | 3:1 | pass |

