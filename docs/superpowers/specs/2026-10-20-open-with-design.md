# Open with and Send to: design

Date: 2026-10-20. Status: the design was approved in conversation. This
written spec still needs review.

Builds on the [UX fixes](2026-10-19-ux-fixes-design.md), which left
"Explorer 'Open with' and 'Send to'" out of scope.

## Goal

Files can be handed to Kiln from Explorer's right-click menu, without
opening Kiln first and dragging them in.

Success looks like this:

- Right-click a file Kiln reads, choose Open with > Kiln, and it arrives
  as a row in Kiln's window, as if it had been dropped there.
- Select several files, choose Send to > Kiln, and they all arrive in one
  Kiln window.
- Double-clicking a file still opens whatever app it opened before.
- Uninstalling Kiln removes both entries.

## Decisions made

| Question | Decision |
|---|---|
| Entry points | Open with and Send to. No classic "Convert with Kiln" verb, and no Windows 11 top-level menu item (that needs a packaged app). |
| Portable ZIP | Gets neither. Only the MSI adds Explorer entries, so the portable build stays free of registry writes. |
| Default app | Kiln never claims a file type. It's offered in Open with only. Windows' own "Always use this app" button can't be hidden, so that choice stays with the person. |
| Who gets Send to | The account that installs Kiln. Other accounts on the PC still get Open with. |
| The list of file types | Checked into `packaging/`, with a test that fails when it drifts from `formats.toml`. |

## Open with

- The MSI registers Kiln for Open with, per machine, under
  `HKLM\Software\Classes`:
  - `Applications\kiln-app.exe`:
    - `FriendlyAppName` = `Kiln`
    - `SupportedTypes` lists every extension Kiln converts from
    - `shell\open\command` = `"[INSTALLFOLDER]kiln-app.exe" "%1"`
  - A ProgID, `Kiln.Convert`, with the same open command and Kiln's icon.
    Each supported extension lists it under `.<ext>\OpenWithProgids`, so
    Kiln shows in the right-click Open with submenu, not only behind
    "Choose another app".
- Nothing is written to an extension's default value or to `UserChoice`.
  Kiln is never the default.
- The extensions are the ones Kiln converts from: each input format and
  its aliases, lowercase, with a dot.
- `.ts` is left out, because TypeScript files share the extension.
- With several files selected, Explorer starts one Kiln per file, and
  the existing hand-off gathers them into the first window.
  - `MultiSelectModel` is left unset. `Player` could hand a plain command
    line only the first file, and the others would be lost.
  - Explorer's own limit on how many selected files one command handles
    (15 by default, `MultipleInvokePromptMinimum`) stays as Windows has it.
- Without a version resource, Open with shows Kiln as `kiln-app.exe`. So
  `kiln-app.exe` gets one, with the file description and product name
  `Kiln`.

## Send to

- The MSI puts a shortcut named `Kiln` into the installing account's Send
  to folder (`SendToFolder`). It points to `kiln-app.exe` with Kiln's
  icon.
- Send to passes every selected item as an argument.
  - A type Kiln doesn't read gets the same row a drop would give it.
  - A folder is treated as dropping that folder would be.

## In Kiln

- No change to how Kiln behaves is expected, apart from the version
  resource. Open with and Send to both start
  `kiln-app.exe` with paths as arguments, which Kiln already handles:
  - The first Kiln opens its window with those files.
  - A later launch hands them to the open window and brings it forward.
- Help doesn't mention the two entries, because the portable build has
  neither. The README does.

## Uninstall and upgrade

- Every key and the shortcut belong to MSI components, so uninstalling
  removes them.
- A major upgrade replaces them, with no leftovers when an extension is
  dropped from the list.
- No custom actions.

## Documents

- README, Install: the installer also adds Kiln to Open with for the
  files it converts, and to Send to.
- CHANGELOG: one line under the next version.

## Testing

- **A drift test (Rust):** it reads the checked-in list and compares it
  with the input extensions from the format registry, failing with the
  extensions missing from one side or the other.
- **The MSI builds** with `packaging/package.ps1`, provided WiX 5 is
  present on the build machine.
- **The owner's install check.** Installing an MSI changes the machine,
  so automation here doesn't do it.
  - Install the MSI.
  - Right-click a .png: Open with > Kiln opens it in a row.
  - Select three files of different types: Send to > Kiln puts all three
    into one window.
  - With Kiln already open, Open with a .mp4: it joins the open window.
  - Double-click a .png: it opens in its usual app.
  - Double-click a .txt and a .mp4 not opened since the install: each opens
    in its usual app, with no "How do you want to open this file?" prompt.
  - Uninstall: Kiln has gone from Open with and Send to.
  - Uninstall between check runs: a rebuilt MSI with the same version
    installs as a second copy.
  - Open with on ten or more files on the first launch after install: one
    window, every file a row.

## Out of scope

- A "Convert with Kiln" verb, cascading menus, and the Windows 11
  top-level context menu.
- Send to for every account on the PC.
- Explorer entries for the portable build, and a Settings switch for
  them.
- Starting a conversion straight from the menu (for example "Convert to
  JPG"). The files arrive as rows, and the person picks from there.
