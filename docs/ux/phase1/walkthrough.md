# Cognitive walkthrough

This walkthrough follows Wharton, Rieman, Lewis and Polson's method. At each step of each task it asks four questions:

1. **Goal.** Will the person try to achieve the right effect?
2. **Notice.** Will they notice that the right action is available?
3. **Associate.** Will they connect that action with the effect they want?
4. **Feedback.** After acting, will they see that they are making progress?

A "no" marks a likely failure point, and each one cross-references the [heuristics](heuristics.md) finding it confirms.

**User profile.** A first-time user who:
- uses Windows daily with a mouse or touchpad and knows File Explorer;
- has never seen Kiln or its README;
- has file formats such as JPG, PDF and MP4 in their vocabulary, but not codecs or engine names;
- has started Kiln from the Start menu with its window on its own, and has File Explorer open in another window.

**Build.** `main` 2213fa5, with a fresh settings file, FFmpeg installed, and Pandoc and LibreOffice found.

---

## Task A: turn a photo into a JPG

The first launch, with no help.

| Step | Correct action | Goal | Notice | Associate | Feedback | Notes |
|---|---|---|---|---|---|---|
| A1 | Get the photo into Kiln: drag it from File Explorer onto the window | Yes | **Partly** | Yes | Yes | "Drop files here" is clear if File Explorer is beside Kiln. Someone who looks for File > Open or an Add button finds none (H-01). |
| A2 | Hold Shift *while* dropping, to get the wheel | **No** | **Partly** | **No** | — | The instruction is in the third line of small muted text, under the tagline (H-17). Nothing in Windows teaches "hold a key while dropping to get a menu", and Shift already means move (H-12). Most people will drop without a key. |
| A2′ | Having dropped without a key, find a way to convert the listed file | Yes | **No** | **No** | **No** | The footer says "Added 1 file." The row offers nothing to click, and the hint is gone (H-02). The only ways on are right-click, which nothing mentions (H-09), and dragging the file in again with Shift. **This is the most likely place for the session to fail.** |
| A3 | (On right-click) the wheel appears: pick JPG | Yes | Yes | Yes | Yes | The wheel is clear once open: the icon and "JPG", the "PNG → JPG" centre, and the suggestion dot with its reason in the footer. |
| A4 | Know the job worked | — | — | — | **Partly** | The row settles, and the output name appears in place of the second line. Nothing says "Done", and the footer status doesn't name the folder. |
| A5 | Open or find the JPG | Yes | **No** | **No** | **No** | Clicking the finished row does nothing (H-04). The person has to know it was saved next to the original and switch to File Explorer. |

**Predicted outcome:** a first-time user either stalls at A2′ or finds right-click by luck. Those who get through then stall again at A5.

---

## Task B: make a large video smaller

Kiln is already known from Task A.

| Step | Correct action | Goal | Notice | Associate | Feedback | Notes |
|---|---|---|---|---|---|---|
| B1 | Add the video | Yes | Yes | Yes | Yes | Learned in Task A. The suggestion "Large for its length. About 180 MB after." appears on the row's second line once Kiln has read the file. |
| B2 | Find Compress: it's in the Tools wheel, opened with Ctrl held while dropping, or Ctrl+Enter | Yes | **No** | **No** | — | Right-click opens Convert, not Tools (H-09). The row says "Suggested: Compress" but offers no way to act on it. The Ctrl mapping is shown nowhere now that the list isn't empty (H-05). A likely wrong path: convert to MP4 "to make it smaller". |
| B3 | Pick Compress | Yes | Yes | Yes | **Partly** | The Tools wheel is clear and the dot marks Compress. Progress shows as a thin line with no percentage or time left. |
| B4 | Check it's smaller | Yes | **No** | **Partly** | **No** | Neither the finished row nor the footer gives the new size, and the result can't be opened from Kiln (H-04). |

**Predicted outcome:** most first-time users fail at B2 without a hint.

---

## Task C: combine three PDFs into one

| Step | Correct action | Goal | Notice | Associate | Feedback | Notes |
|---|---|---|---|---|---|---|
| C1 | Select all three PDFs in File Explorer and drag them in **with Ctrl held** | Yes | **No** | **No** | — | Merge only appears when several files open one wheel, and only a modified drag does that (H-03). If the PDFs are already listed, there's no multi-select to combine them. |
| C1′ | (Already listed) select all three | Yes | Yes | Yes | **No** | Ctrl-click and Shift-click select one row only, with no message saying why. |
| C2 | Pick Merge | Yes | Yes | Yes | Yes | The dot and the "One PDF, in the order shown." reason are clear. |
| C3 | Check the order | Yes | **No** | — | **No** | "In the order shown" refers to the list order, and there's no way to reorder the list. |

**Predicted outcome:** this task fails for nearly everyone without a hint, and it can't be done by keyboard at all.

---

## Task D: recover from a failed conversion

The file is damaged.

| Step | Correct action | Goal | Notice | Associate | Feedback | Notes |
|---|---|---|---|---|---|---|
| D1 | Read the error | Yes | Yes | **Partly** | — | The brick line is noticed, but it's raw engine text cut off mid-word (H-07). |
| D2 | Decide what to do | Yes | **No** | **No** | **No** | No next step is given. "Show details" opens a full engine log, which helps an expert but not this user. |
| D3 | Remove the failed row | Yes | **Partly** | **Partly** | Yes | Delete works, but no × appears on a finished or failed row; the × shows only on running ones. Clear works, but it also stops any other running jobs (H-06). |

---

## Task E: save results somewhere else

| Step | Correct action | Goal | Notice | Associate | Feedback | Notes |
|---|---|---|---|---|---|---|
| E1 | Open Settings with the dial icon, top right | Yes | **Partly** | **Partly** | Yes | No label or tooltip (H-11). Some will find it by clicking around. |
| E2 | General → Output folder → Choose | Yes | Yes | Yes | Yes | Clear. |
| E3 | Confirm it saved | Yes | Yes | Yes | **Partly** | Changes save at once and there's no Apply button. The "Close" button may make some people wonder whether they need to save. |

---

## Failure points, ranked by how many tasks they block

| Failure point | Tasks blocked | Finding |
|---|---|---|
| No way forward from a listed file | A, B, C | H-02, H-09 |
| Tools and Merge unreachable without a modified drag | B, C | H-03, H-05, H-09 |
| Can't open or find the result | A, B, C | H-04 |
| No Open or Add command | A (and every task for keyboard and touchpad users) | H-01 |
| Raw, cut-off error with no next step | D | H-07 |

These are hypotheses. The usability script in [usability-test.md](usability-test.md) is written to test each one: tasks 1 to 3 map to A, B and C, and task 4 maps to D.
