# Kiln usability test: moderator's script

These are moderated, task-based sessions with people who have never used Kiln. The aim is to find out where new users get stuck, measure how bad each problem is, and give a baseline to compare against after the redesign. You run them with this script; afterwards, give me your notes and recordings.

- **Sessions:** 3 to 5 participants, 30 to 40 minutes each, one at a time.
- **Kiln build:** `main` at 2213fa5 (`target\release\kiln-app.exe`). Keep this exact build for every session, so the results stay comparable.
- **What it tests:** the predictions in [walkthrough.md](walkthrough.md) and [heuristics.md](heuristics.md).

---

## 1. Participants

- 3 to 5 people who have never seen or heard about Kiln.
- They use Windows regularly and know how to find a file in File Explorer.
- A mix:
  - at least one mostly on a touchpad;
  - at least one who rarely uses keyboard shortcuts;
  - ideally one heavy keyboard user.
- **Not** developers who have worked on Kiln, and not people who have read the README. Ideally not people who would rather please you than be honest, so friends of friends beat close friends.
- Screen reader users: not in this round. Kiln currently shows nothing to screen readers (see [accessibility.md](accessibility.md)), so a session would only confirm that. Include them in the round after UI Automation support exists.

## 2. Before the first session

### 2.1 Test files

Put the files below in a folder called `Kiln test` on the test PC's desktop. Use files with nothing personal in them, because the screen is recorded.

| File | What it is | How to make it |
|---|---|---|
| `holiday photo.png` | A real photo of 3 to 6 MB, saved as PNG | Open any camera photo in Paint and Save as PNG. |
| `site visit.mov` | A video over 100 MB, so Kiln suggests Compress | Any phone video in MOV. Or run `ffmpeg -f lavfi -i testsrc2=size=1920x1080:rate=30 -f lavfi -i sine -t 60 -c:v libx264 -b:v 20M -c:a aac "site visit.mov"`. |
| `report part 1.pdf`, `report part 2.pdf`, `report part 3.pdf` | Three short text PDFs | Print three short pages to PDF from Word or a browser. |
| `interview.mp4` | A damaged file | Rename any non-video file to `interview.mp4`, such as a `.txt` with a few lines. |

After each session, delete the new files Kiln made (the JPG, the smaller video, the merged PDF, and any copies) from `Kiln test`, so the next participant starts from the same folder. Check the list before deleting.

### 2.2 A fresh Kiln for each participant

Each participant should see Kiln as it is the first time, with no remembered picks or settings. Start it with a new, empty settings folder per person (p1, p2, …) from PowerShell:

```powershell
$env:APPDATA = "C:\kiln\target\ux-test\p1"; & "C:\kiln\target\release\kiln-app.exe"
```

Change `p1` to `p2`, `p3` and so on for later sessions. This only affects that one PowerShell window, so your real Kiln settings stay untouched. Close that PowerShell window after the session.

### 2.3 Screen layout at the start of each task

- Kiln is open, at its normal size, not maximised.
- File Explorer is open on `Kiln test`, partly overlapping Kiln, the way windows usually sit. **Don't** snap them side by side, because that hints at dragging.
- No browser tabs about Kiln, and no README.

### 2.4 Recording

- Ask for consent first (see 3.1). Record the screen and voice with the Snipping Tool's Record option, or Xbox Game Bar (Win+Alt+R).
- Note the clock time when each task starts, so you can find it in the recording.

### 2.5 Pilot

Run the whole script once with someone who doesn't count, or with yourself, before the first real session. Check:
- that the files are right;
- that the timings fit 40 minutes;
- that recording works;
- that you can read the task cards aloud without adding hints.

Pilot results don't go into the data.

---

## 3. The session

### 3.1 Welcome (about 3 min)

Read this aloud:

> Thanks for helping. I'm testing an app called Kiln, not you: if something is hard, that's the app's fault, and it's exactly what I need to find. Please think aloud as you go: say what you're looking at, what you're trying to do and what you expect to happen. I'll mostly stay quiet, and I may not answer questions straight away, because I want to see what you would do on your own. You can stop at any time. Is it OK if I record the screen and our voices? The recording stays with me and is only used to improve the app.

Wait for a clear yes before you start recording.

### 3.2 Warm-up questions (about 3 min)

1. How do you usually change a file from one type to another, for example a photo to a JPG? Note any websites, apps, or "I ask someone".
2. Mouse, touchpad, or both? How often do you use keyboard shortcuts?
3. Have you heard of Kiln before? If yes, note it, and consider whether to keep the session.

### 3.3 Tasks

Read each task card aloud, then put it in front of them in print or on a second screen. Start the timer when you finish reading. Stop it when they say they're done or that they'd give up, or when the time limit passes.

**Don't use Kiln's own words while a task runs**, such as wheel, Convert, Tools, Compress, Merge, Settings or Shift. The task cards are written to avoid them.

**Task 1: a JPG copy of the photo** (limit 5 min)
> A friend's old laptop can't open `holiday photo.png`. They've asked you to send it as a JPG instead. Use Kiln to make a JPG of it.

Success:
- A JPG of the photo exists.
- **Unaided** means no hint above level 1 (see 3.4).

**Task 2: find the new file** (limit 2 min, straight after task 1)
> Show me the JPG you just made.

Success: they open it, or point to it in File Explorer.

Note:
- where they look first: Kiln, File Explorer, Downloads or Desktop;
- whether they try clicking the finished row in Kiln.

**Task 3: a smaller video** (limit 5 min)
> `site visit.mov` is too big to send by email. Use Kiln to make it smaller.

Success: a smaller video exists, made with Compress, or with a conversion whose result is clearly smaller. Record which path they took.

Note:
- whether they read the row's suggestion;
- whether they find Compress at all.

**Task 4: one PDF from three** (limit 5 min)
> The three files called `report part 1`, `2` and `3` are pieces of one report. Use Kiln to turn them into a single PDF.

Success: one merged PDF, with the parts in order 1, 2, 3.

Note:
- whether they add all three first and then look for a way to combine them;
- whether they try Ctrl-click or Shift-click in Kiln's list.

**Task 5: something goes wrong** (limit 3 min)
> Use Kiln to make an MP3 of the sound from `interview.mp4`.

When the error shows, or after 3 minutes, ask:
> What happened? What would you do next?

Success: they can explain in their own words that the file is the problem (damaged, or not really a video), and they name a sensible next step. Note their exact words.

**Task 6: a different place for results** (limit 3 min)
> From now on, you'd like everything Kiln makes to go into a folder called `Converted` on the desktop. Set that up. (It's fine to create the folder.)

Success: the output folder is set to `Desktop\Converted`. To check, convert any file and see where it lands; you can do this after the task.

**Task 7: a smaller photo (optional, if time allows)** (limit 4 min)
> Make another JPG of `holiday photo.png`, but about half as wide and tall, for a website.

Success: they find the options sheet and choose a lower resolution. Note how they found it: Tab, right-click, or the footer hint.

### 3.4 Hints, and when a task stops counting as unaided

Use the lowest level that keeps them moving. Never go straight to level 3.

| Level | What you say | Counts as |
|---|---|---|
| 0 | Nothing | Unaided |
| 1 | "What are you looking for?" or "What do you expect to happen?" | Unaided |
| 2 | "Is there anything else you could try with the mouse?" or "What about the keyboard?" | **Assisted** |
| 3 | You show them the specific action | **Failed**. Note it, help them finish, and move on. |

Write down the level and the moment of every hint you give.

### 3.5 After each task: the Single Ease Question

> Overall, how easy or difficult was that task? From 1, very difficult, to 7, very easy.

Write down the number and any comment.

### 3.6 After all the tasks (about 7 min)

**System Usability Scale.** These are the 10 standard items (Brooke, 1996). Ask the participant to rate each from 1, strongly disagree, to 5, strongly agree, on paper. Don't change the wording.

1. I think that I would like to use this system frequently.
2. I found the system unnecessarily complex.
3. I thought the system was easy to use.
4. I think that I would need the support of a technical person to be able to use this system.
5. I found the various functions in this system were well integrated.
6. I thought there was too much inconsistency in this system.
7. I would imagine that most people would learn to use this system very quickly.
8. I found the system very cumbersome to use.
9. I felt very confident using the system.
10. I needed to learn a lot of things before I could get going with this system.

To score it:
- for odd-numbered items, subtract 1 from the answer;
- for even-numbered items, subtract the answer from 5;
- add the ten results and multiply the sum by 2.5, which gives a score from 0 to 100. The average across many products is about 68.

**Open questions:**
1. What was the most confusing moment?
2. Was there anything on screen whose meaning you weren't sure of? Show them the More ring if they never saw it, and ask what QOI, TGA and WV mean to them.
3. If you could change one thing, what would it be?
4. Would you use this instead of what you use now? Why, or why not?

Thank them. Stop the recording.

---

## 4. Observation sheet (one per participant)

Copy this table for each participant, and fill it in during the session or straight after.

**Participant:** P__ · **Date:** ____ · **Mouse / touchpad / keyboard:** ____ · **Settings folder:** p__

| Task | Outcome (U / A / F / gave up) | Time (m:ss) | Hints (level @ time) | First thing they tried | Wrong paths | SEQ (1–7) | Quotes |
|---|---|---|---|---|---|---|---|
| 1 JPG | | | | | | | |
| 2 Find it | | | | | | | |
| 3 Smaller video | | | | | | | |
| 4 One PDF | | | | | | | |
| 5 Error | | | | | | | |
| 6 Output folder | | | | | | | |
| 7 Smaller photo | | | | | | | |

**SUS score:** ____

**Checklist of the walkthrough's predictions** (tick what you saw):
- [ ] Tried a plain drop first, without a key (walkthrough A2)
- [ ] Got stuck with files listed and no next step (A2′, H-02)
- [ ] Right-clicked a row without a hint (H-09)
- [ ] Looked for File > Open or an Add button (H-01)
- [ ] Clicked a finished row expecting it to open (H-04)
- [ ] Couldn't find Compress (B2)
- [ ] Tried Ctrl-click or Shift-click in the list (H-03)
- [ ] Didn't understand the error line (H-07)
- [ ] Didn't recognise the dial as Settings (H-11)
- [ ] Didn't know a format code (H-08)
- [ ] Clicked Clear while a job ran (H-06)

**Other problems seen** (for each: what happened, the task, and how severe it looked):

---

## 5. Afterwards

Send me, for each participant:
- the filled-in observation sheet;
- the SUS answers;
- the recording, or notes taken from it.

Rough handwriting or a photo of paper sheets is fine. I'll combine them into the Phase 2 findings, ranked by severity × how often they came up × tasks blocked.

**Draft success thresholds,** for comparing against after the redesign. Phase 2 will set the final ones before any design work starts:
- Tasks 1, 3 and 4: at least 4 of 5 participants succeed unaided.
- Task 2: at least 4 of 5 find the result within 1 minute.
- Task 1: median time under 2 minutes.
- Average SEQ of 5.5 or more on every task.
- Mean SUS of 75 or more.
