# Getting help with Kiln

## Look here first

- **A file says it needs a helper app.** Documents, office files and e-books need a free helper: Pandoc, LibreOffice or Calibre. The row says which one, and **Get it** opens its download page. Once it's installed, Kiln finds it without a restart. The README's [Install](README.md#install) section lists what each helper adds.
- **Windows SmartScreen warns before Kiln starts.** Choose **More info**, then **Run anyway**. Releases are signed once Kiln's code signing is set up; see [the code signing policy](docs/code-signing-policy.md).
- **Windows N or KN editions.** Kiln starts and converts without Windows' Media Feature Pack. A few conversions that use Windows' own media encoders need it, and Kiln says so on the file when one does. Windows offers it under Settings > System > Optional features.
- **Where Kiln keeps things, and how to remove them.** Settings are in `%APPDATA%\Kiln`, and the log, crash reports and results waiting to be saved are in `%LOCALAPPDATA%\Kiln`. Uninstalling leaves them, so a later install keeps your settings; delete those folders after uninstalling to remove them. [PRIVACY.md](PRIVACY.md) lists every folder.
- **A helper app with winget.** Where Windows has winget, **Settings > Helper apps** offers **Install with winget** for a missing Pandoc, LibreOffice or Calibre. It opens a console that runs the install.

## Ask or report

- **A bug:** open an issue with the [bug report](https://github.com/chancrisp/kiln/issues/new?template=bug_report.yml) form. **Settings > About > Diagnostics > Copy diagnostics** puts Kiln's version, Windows' version and the end of its log on the clipboard to paste in. Its log lines can name files and folders, so remove anything you'd rather not share. Include the file types involved. **Open log folder** shows any crash report.
- **A format or tool you'd like:** use the [format or tool request](https://github.com/chancrisp/kiln/issues/new?template=format_request.yml) form.
- **A security problem:** don't open an issue. [SECURITY.md](SECURITY.md) says how to report it privately.

Kiln is made by one person in their own time, so an answer can take a few days.
