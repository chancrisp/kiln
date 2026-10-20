# Drag results out: design

Date: 2026-10-20. Status: the design was approved in conversation. This
written spec still needs review.

Builds on [held results](2026-10-19-held-results-design.md). That spec named
this as "a likely next step" and left it out of scope.

## Goal

A held result can be dragged out of Kiln and dropped into Explorer, onto the
desktop or into another app. That's a second way to export it, and it feels
like dragging a file out of an Explorer window.

Success looks like this:

- Dragging a Ready row into an Explorer folder puts the result there and
  takes it out of Kiln's holding folder, as Export does.
- Kiln never loses a result to a drag. A copy, a cancelled drag, or a drop
  onto Kiln itself leaves the result held.
- The close prompt and Export all never count a result that was dragged out.
  A dragged-out result never reads as "no longer there".

## Decisions made

| Question | Decision |
|---|---|
| What happens to Kiln's copy | The drag moves it out of holding, like Export. Move is the preferred effect, and Copy is allowed. |
| The row after a move | `Saved where you dropped it.`, in the result tone. It isn't clickable, because Windows doesn't tell Kiln where the drop went. |
| A drop that only copies (an email, a browser upload, Ctrl held) | The result stays held. The row stays Ready, and close still asks about it. |
| The keyboard | No new key. Export (Ctrl+S) already does the same thing. |

## Starting a drag

- Press on a row that holds a result, then move the pointer past the system
  drag distance (`SM_CXDRAG`, `SM_CYDRAG`) with the button down.
- A row that isn't selected becomes the only selection first, as in
  Explorer. A row inside the selection keeps it.
- The drag carries every held result of the selected rows, once each, in
  list order. It leaves out:
  - rows with no held result (running, failed, cancelled, saved)
  - results an export is moving now
  - results that are gone
- When nothing is left to carry, there is no drag. The press stays an
  ordinary click.
- No drag starts while a panel, dialog or the wheel is open, or when Kiln
  runs without a holding folder (the fallback where results are saved next
  to each file).

## The drag

- The data object comes from the shell: `SHCreateDataObject` over the held
  files. `SHDoDragDrop` runs the drag loop, so the drag image, the cursor
  and every format Explorer expects come from Windows.
- The preferred effect is Move (`CFSTR_PREFERREDDROPEFFECT`). The allowed
  effects are Move and Copy.
- Kiln turns asynchronous mode off on the data object
  (`IDataObjectAsyncCapability::SetAsyncMode(FALSE)`). The drop target then
  finishes the move before the drag returns, so Kiln never removes or
  discards a file Explorer is still copying.
- Kiln's own drop target ignores a drag Kiln started. Dropping back onto
  Kiln does nothing.

## After the drag

- When the drag returns, Kiln checks each file it carried:
  - **The file is gone:** it was moved. The file is marked as saved by a
    drag.
  - **The file is still there:** the drop copied it, or the drag was
    cancelled. Nothing changes.
- Kiln also remembers, for the rest of the run, which files it dragged. If
  one of them disappears later (a drop target that moved it after the drag
  returned), it counts as saved by a drag, not as gone.
- A row whose held results were all moved by a drag says `Saved where you
  dropped it.` in the result tone:
  - The line isn't a link, and a double-click doesn't open anything.
  - The context menu offers what a saved row offers today, minus Show in
    folder.
  - The row's accessible name says the same sentence.
- A row with several results where only some were moved keeps its Ready
  line for the rest: `Ready: part 2.mp3 and 1 more`.
- A drag that moved results says nothing in the footer, because the rows
  say it.

## Counting

- Results saved by a drag aren't held. They leave:
  - the close, Clear and remove prompts
  - Export all and its count
  - the shutdown block reason
  - `held_by` and its relatives
- Results still there after a copy-only drop stay held and are counted as
  usual.

## Help and documents

- Help, Results section: a new line, `Drag out`: `Save a result where you
  drop it.` It's shown only when Kiln has a holding folder.
- README: the held-results paragraph adds that a finished file can be
  dragged out of Kiln to save it where you drop it.
- DESIGN.md, Progress and results: a dragged-out row reads `Saved where you
  dropped it.`

## Errors

- If the shell can't build the data object or the drag fails, nothing
  changes. The footer says `Couldn't start the drag.` The results stay held.
- A drop target that fails partway leaves whatever it didn't move in
  holding. Those results stay held, and their rows stay Ready.

## Testing

- **kiln-app model:**
  - The row line and tone for a dragged-out row.
  - A partly moved row with several results.
  - A dragged-out file that's missing isn't gone.
  - The counts: leave prompts, Export all, `held_by` and the shutdown reason
    leave dragged-out results out and keep copy-only results in.
  - Which rows a drag carries: the selection rule, and running, saving and
    gone rows.
  - The drag-distance rule.
- **Help:** the new line, shown only with a holding folder.
- **GUI pass, by the owner.** Automation here only sends keys to Kiln, so
  it can't drag into Explorer.
  - Drag a Ready row into a folder: the file arrives, and the row says
    `Saved where you dropped it.`
  - Ctrl-drag (copy): the row stays Ready.
  - Drag back onto Kiln: nothing happens.
  - Close: no prompt for the dragged result.

## Out of scope

- Dragging a running row (its result doesn't exist yet).
- Learning where a drop went.
- Virtual files (`CFSTR_FILEDESCRIPTOR`) for apps that don't take real
  files.
