# Privacy

Kiln converts files on your PC. It has no account, no analytics and no ads.

## What Kiln sends

Kiln makes one kind of network request: **Check for updates** in Settings. Only when you choose it does Kiln ask GitHub (`api.github.com`) for the version of the latest release of `chancrisp/kiln`. The request carries what any HTTPS request does, such as your IP address, and the program name and version (`Kiln/0.1.0`). It carries nothing about your files or your PC. GitHub's privacy statement covers that request: https://docs.github.com/site-policy/privacy-policies/github-general-privacy-statement

**Install with winget**, in Settings > Helper apps, asks you first, then opens a console that runs Windows' `winget`. Winget downloads the helper app from its package source, under winget's own terms; Kiln sends nothing itself.

Kiln itself never sends files, logs or crash reports anywhere. Windows' own error reporting may report a crash of any program to Microsoft, as Windows is set to. When you report a problem, you choose what to copy into it. **Copy diagnostics** puts text on the clipboard and sends nothing; its log lines can name files and folders, so read it before you paste it. It writes your user folder as `%USERPROFILE%`.

The helper apps Kiln can use (FFmpeg, Pandoc, LibreOffice and Calibre) run on your PC, with the files you convert.

## What Kiln keeps on your PC

| What | Where | Removed when Kiln is uninstalled? |
|---|---|---|
| Settings | `%APPDATA%\Kiln\settings.toml` | No |
| Converted files on their way to their folder, and any Kiln couldn't save yet (it tries again at the next start) | `%LOCALAPPDATA%\Kiln\held`, or a hidden `.kiln-held` folder at the top of the drive the original is on | No |
| The log, `kiln.log` and `kiln.1.log`, 2 MB at most. A line can name a file or folder Kiln couldn't handle. | `%LOCALAPPDATA%\Kiln\logs` | No |
| A crash report, `crash-<time>.txt`, with the error and Kiln's version; the newest few are kept | `%LOCALAPPDATA%\Kiln\logs` | No |
| Windows' crash dumps of Kiln, five at most (MSI installs only). A dump is a copy of part of Kiln's memory when it crashed, so it can hold names of files that were open. | `%LOCALAPPDATA%\Kiln\CrashDumps` | No |
| Work files during a conversion | `%TEMP%\Kiln` | Removed as each job ends |
| Kiln's start-up entry, its Send to shortcut and its Open with entries | Windows' registry and your Send to folder | Yes, by the MSI's uninstall |

Uninstalling keeps your settings and logs, as most Windows apps do, so a later install starts where you left off. To remove everything Kiln left, uninstall it, then delete the three `Kiln` folders above: in `%APPDATA%`, in `%LOCALAPPDATA%` and in `%TEMP%`, and any `.kiln-held` folder at the top of a drive.
