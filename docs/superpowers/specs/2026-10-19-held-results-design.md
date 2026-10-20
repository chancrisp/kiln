# Held results: design

Date: 2026-10-19. Status: approved in conversation, awaiting review of this
written spec.

## Goal

Converting a file in the app no longer saves anything where the user would
see it. Results wait inside Kiln until the user exports them, one at a time
or all at once. Nothing the user didn't choose to keep is left on their
disk.

Success looks like this:

- After a conversion, the source's folder and the Output folder are
  unchanged.
- Every result can be exported to a place the user picks, in one step for
  one file and one step for all of them.
- A result is never lost without the user being asked first, except when
  Kiln crashes.
- `kiln convert` and every other command-line command behave exactly as
  today.

## Decisions made

| Question | Decision |
|---|---|
| Where results are held | A private folder Kiln owns, under `%LOCALAPPDATA%\Kiln\held`. Not RAM: a video can be gigabytes, and the engines write files. |
| Closing Kiln with results not exported | Ask: Export all, Discard or Cancel. Nothing is kept after closing. |
| Where Export puts a file | The user picks each time: the Save dialog for one file, a folder picker for several. |
| Clear with results not exported | Ask first, with the same three choices. |
| The command line | Unchanged. It writes next to the input or to `--out-dir`. |
| The Output folder setting | Kept, and now only sets where export dialogs start. |

## Holding

- Each run of the app gets its own folder: `%LOCALAPPDATA%\Kiln\held\<run>`,
  where `<run>` is unique per process (process id and start time).
- The app passes that folder as the job's output folder. Jobs already finish
  in a working folder and move the result with `place`, so engines, tools
  and chained conversions don't change. Results in the run folder keep their
  usual names (`photo.jpg`, `talk part 1.mp3`), and names are unique inside
  the folder as they are in any output folder today.
- Each run folder holds a lock file that its run keeps open. Two Kiln windows
  open at once each have their own folder and never touch the other's.
- On launch, Kiln removes every run folder in `held` whose lock file isn't
  open. That clears what a crashed run left. Kiln removes nothing outside
  `held`.
- When the app closes normally, after the prompt, it removes its own run
  folder.

## Exporting

- **Export** (one row): one result opens the Windows Save dialog, with the
  result's name and type filled in. Several results from one job, such as
  Split's parts or a PDF's pages, open a folder picker, and every part goes
  into that folder.
- **Export all**: a button in the footer next to Clear, shown only while
  something is held, labelled `Export all (3)`. It opens a folder picker once
  and exports every held result into it.
- Keyboard: Ctrl+S exports the selected row and Ctrl+Shift+S exports all.
- Where dialogs start: the folder of the last export in this run, else the
  Output folder from Settings, else the first source's folder.
- Export moves the file out of holding. On the same drive that's a rename.
  Across drives it's a copy, then the held file is removed once the copy is
  complete and closed. A failed copy leaves the held file where it was.
- Names:
  - **Export all and folder exports never overwrite.** A taken name becomes
    `name (1).jpg`, as today.
  - **The Save dialog** asks before replacing a file, as every Windows Save
    dialog does. Replacing happens only when the user confirms a name they
    typed or picked.
- Cancelling a dialog changes nothing.

## The file list

- A finished, held row's second line is `Ready: photo.jpg` (with `and 2 more`
  for several results), in the result tone.
- An exported row's second line is `Saved to Pictures.`, naming the folder.
  Clicking it opens that folder in Explorer with the file selected.
- Double-clicking a held row opens the held result in its usual app, so it
  can be checked before it's kept.
- Each held row shows an **Export** control. It has a keyboard path (Ctrl+S)
  and an accessible name ("Export photo.jpg").
- After a batch, the footer says what happened and what's next:
  `Converted 3 files to JPG. Export to keep them.`
- Failed and cancelled rows are as they are today.

## Leaving with results held

- **Close, Clear, or removing a row** with held results asks first, with a
  Windows task dialog (it's accessible to screen readers):
  - Title: `3 results aren't saved yet.` (`1 result isn't saved yet.`)
  - Buttons: **Export all**, **Discard**, **Cancel**. Export all opens the
    folder picker. Cancelling that picker cancels the close or clear.
- **Running jobs**: Close and Clear also ask when jobs are running:
  `2 files are still converting.` The buttons are **Stop and clear** (or
  **Stop and close**) and **Cancel**. This fixes H-06. When both apply, one
  dialog says both.
- **Windows shutting down or signing out** while results are held: Kiln
  registers a shutdown block reason, `3 results aren't saved yet.`, so
  Windows shows it and the user can cancel. If they go ahead, the results are
  lost, as in a crash, and the next launch clears them.

## Settings and documents

- Settings > General: "Output folder" becomes **Start exports in**, with the
  hint `Unset: the last folder you exported to, or the file's own folder.`
- `settings.toml` keeps the key `output_folder`, so existing files still
  load. Only its comment changes.
- README: "Nothing is overwritten" becomes "Nothing is saved until you export
  it, and Kiln never overwrites a file without asking." The Formats paragraph
  and the settings example change to match. The command-line section says
  `kiln convert` still writes next to the input.
- DESIGN.md, Progress and results: success shows `Ready:` and the output
  name. Export adds `Saved to <folder>.`

## Errors

- **No room to hold a result.** The job fails with
  `There isn't room on C: to hold the result.` and the usual details.
- **Export fails** (no permission, disk full, file in use): the row keeps
  its held result and shows one plain line in brick, for example
  `Couldn't save to D:\Locked. Pick another folder.` Export all keeps going
  with the other files and ends with `Saved 4 of 5. 1 couldn't be saved.`
- **The held file is gone** (removed by hand): the row says
  `The result is no longer there. Convert it again.`

## Testing

- **kiln-core:**
  - The export move, on the same drive and across drives (a temp dir on
    another volume when one exists, else a forced copy path).
  - Name collisions, and that a failed copy leaves the source.
  - The stale-folder sweep: a locked folder is kept, an unlocked one is
    removed, and nothing outside `held` is touched.
- **kiln-app model:** row lines for held, exported and gone results;
  `Export all (n)`; the prompt's wording and counts for close, Clear and row
  removal, including the both-at-once case; where dialogs start.
- **CLI:** the existing tests still pass unchanged.
- **GUI pass:** convert in `target\samples`, then check:
  - the folder is unchanged
  - export one, Export all
  - the close prompt and Clear prompt
  - the shutdown block reason, by checking that it's registered

  Screenshots go in `docs/screenshots/held/`.

## Out of scope

- Dragging a result out of Kiln into Explorer as a way to export. It's a
  likely next step, not part of this.
- Keeping held results between runs.
- Changing anything about the Move file action, which moves the source file
  itself.
