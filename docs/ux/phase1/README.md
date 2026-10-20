# UX Phase 1: baseline

This is the evidence collected before any redesign. It covers Kiln at `main` 2213fa5. Nothing here changes the app.

| File | What it is |
|---|---|
| [heuristics.md](heuristics.md) | Heuristic evaluation: 21 findings ranked by Nielsen severity, each with evidence and a direction |
| [walkthrough.md](walkthrough.md) | Cognitive walkthrough of five first-time tasks, with the predicted failure points |
| [accessibility.md](accessibility.md) | Accessibility audit: what UI Automation sees, keyboard-only tasks, WCAG 2.2 AA checklist, 88 contrast pairs, target sizes |
| [strings.md](strings.md) and [strings.csv](strings.csv) | Every visible string (307), flagged, with a terminology table |
| [usability-test.md](usability-test.md) | The moderator's script for 3 to 5 sessions with new users |
| `uia/` | Raw UI Automation and MSAA dumps |
| `shots/` | Screenshots of the current build, dark theme, at 125% |

## The baseline in one table

| # | Problem | Severity | Sources |
|---|---|---|---|
| 1 | Screen readers get "Kiln, window" and nothing else, in every state | 4 | accessibility |
| 2 | Files can only be added by dragging: there's no Open, Ctrl+O, paste or Explorer entry | 4 | heuristics H-01, accessibility, walkthrough A1 |
| 3 | Once files are listed, nothing says what to do next | 4 | H-02, walkthrough A2′ |
| 4 | Only one file can be selected, so Merge and batches need a key-held drag | 3 | H-03, walkthrough C |
| 5 | A finished file can't be opened or shown in its folder | 3 | H-04, walkthrough A5, B4 |
| 6 | The Shift, Ctrl and Alt meanings must be remembered, and Tools and Actions have no visible path | 3 | H-05, H-09, walkthrough B2 |
| 7 | Errors are raw engine text, cut off, with no next step; "isn't installed" leads to a dead end | 3 | H-07, H-20, strings |
| 8 | Formats are shown as codes only | 3 | H-08, strings |
| 9 | Clear cancels running work with no warning, and has no keyboard path | 3 | H-06, accessibility |
| 10 | Light theme's accent fails non-text contrast (1.1 to 1.8:1), and error red fails text contrast | 3 | accessibility |
| 11 | Windows contrast themes, Text size and Animation effects are ignored | 3 | accessibility |
| 12 | Many names for one idea: tool, action, convert, format, type, engine | 2 | strings terminology |

## Next

1. Run the sessions in [usability-test.md](usability-test.md), then send the observation sheets and recordings.
2. Phase 2 combines the sessions with this baseline into ranked problems, and sets the success thresholds before any design work.
