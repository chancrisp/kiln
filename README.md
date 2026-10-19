<div align="center">

<img src="docs/readme/banner.png" alt="Kiln, a native Windows file converter" width="820">

### Fire raw files into finished ones

A native Windows file converter. Drop a file, hold a key, pick a format. Nothing is uploaded.

<a href="https://github.com/chancrisp/kiln/releases"><img src="https://img.shields.io/badge/DOWNLOAD-Windows_10_%26_11-7FB5A0?style=for-the-badge&logo=windows&logoColor=white&labelColor=1C1B19" alt="Download for Windows 10 and 11"></a>
<a href="LICENSE"><img src="https://img.shields.io/badge/LICENSE-Apache_2.0-9A8F80?style=for-the-badge&labelColor=1C1B19" alt="Apache 2.0 license"></a>
<a href="https://www.rust-lang.org"><img src="https://img.shields.io/badge/BUILT_WITH-Rust-B5503F?style=for-the-badge&logo=rust&logoColor=white&labelColor=1C1B19" alt="Built with Rust"></a>

</div>

## What is Kiln?

Kiln turns one kind of file into another. Drag files onto its window, hold a
key, and a radial menu called the **glaze wheel** opens under the pointer with
only the formats those files can become. Pick one and Kiln does the rest in the
background, right on your PC.

- **One gesture.** Hold Shift to convert, Ctrl for file tools and Alt for file actions. Every key can be reassigned.
- **189 format pairs.** Images, audio, video, documents, office files, spreadsheets and slides.
- **Offline.** Converting never touches the network. Nothing is uploaded, ever.
- **Nothing is overwritten.** Results land next to the input, or in a folder you choose. A taken name becomes `name (1).jpg`.
- **Small and quick.** A single native executable under 3 MB, drawn with Direct2D. No web view. The window is up in about 215 ms.
- **Keyboard first.** Arrow keys pick a row, Enter opens the wheel, Escape closes it.
- **Light and dark.** Follows the Windows app mode and every monitor's DPI.
- **A command line too.** `kiln convert photo.png jpg` does the same work from a terminal.

<div align="center">
<img src="docs/screenshots/m7/empty-100.png" alt="Kiln's empty window, ready for files" width="736">
</div>

## The glaze wheel

Three wheels, one per modifier key. Each shows only what fits the files you dropped.

<table>
  <tr>
    <td align="center"><img src="docs/screenshots/m5/wheel-convert-100.png" alt="The convert wheel" width="270"><br><b>CONVERT</b><br><sub>Hold <kbd>Shift</kbd>. Every format the files can become</sub></td>
    <td align="center"><img src="docs/screenshots/m5/wheel-tools-100.png" alt="The tools wheel" width="270"><br><b>TOOLS</b><br><sub>Hold <kbd>Ctrl</kbd>. Compress, crop, trim, split, merge</sub></td>
    <td align="center"><img src="docs/screenshots/m6/wheel-actions-100.png" alt="The actions wheel" width="270"><br><b>ACTIONS</b><br><sub>Hold <kbd>Alt</kbd>. Copy path, move, open in Terminal</sub></td>
  </tr>
</table>

From the keyboard, select a row and press <kbd>Enter</kbd> for Convert,
<kbd>Ctrl</kbd>+<kbd>Enter</kbd> for Tools or <kbd>Alt</kbd>+<kbd>Enter</kbd>
for Actions. Inside a wheel, the arrows move, Enter applies and Escape closes.

## A closer look

<table>
  <tr>
    <td align="center"><img src="docs/screenshots/m7/jobs-light-100.png" alt="A running job, a finished one and a failed one" width="400"><br><b>JOBS</b><br><sub>Run in the background, each with its own progress line and cancel</sub></td>
    <td align="center"><img src="docs/screenshots/m7/settings-update-100.png" alt="The settings panel" width="400"><br><b>SETTINGS</b><br><sub>Output folder, theme, wheel keys and engines</sub></td>
  </tr>
  <tr>
    <td align="center"><img src="docs/screenshots/m5/details-100.png" alt="The details dialog with engine output" width="400"><br><b>DETAILS</b><br><sub>A failed job shows one plain line, with the full engine output a click away</sub></td>
    <td align="center"><img src="docs/screenshots/m6/wheel-convert-light-100.png" alt="The convert wheel in light mode" width="400"><br><b>LIGHT MODE</b><br><sub>Same stoneware, lighter glaze</sub></td>
  </tr>
</table>

## Formats

| Kind | Reads | Converts to | Engine |
|---|---|---|---|
| Image | PNG, JPG, WEBP, BMP, GIF, TIFF | any other of those | built in |
| Audio | WAV, MP3, FLAC, OGG, OPUS, M4A | any other audio format | FFmpeg |
| Video | MP4, MOV, MKV, WEBM | any other video format, or audio (WAV, MP3, FLAC, OGG, OPUS, M4A) | FFmpeg |
| Document | MD, HTML, DOCX, ODT, EPUB, RST, TEX | any other of those, plus RTF and TXT | Pandoc |
| Office document | DOC, RTF | DOC, DOCX, ODT, RTF, PDF | LibreOffice |
| | DOCX, ODT | DOC, PDF | LibreOffice |
| Spreadsheet | XLSX, XLS, ODS, CSV | any other of those, plus PDF | LibreOffice |
| Presentation | PPTX, PPT, ODP | any other of those, plus PDF | LibreOffice |

Aliases are accepted on input: JPEG, TIF, OGA, M4V, MARKDOWN and HTM. The full
list lives in [`crates/kiln-core/formats.toml`](crates/kiln-core/formats.toml),
and `kiln formats FILE` prints what a given file can become.

| Tool | Works on | What it does |
|---|---|---|
| Compress | PNG and JPG images, video, audio (not WAV) | Re-encodes smaller, and keeps the original if the result isn't smaller |
| Crop | images, video | Removes plain borders from images and black bars from video |
| Trim | audio, video | Cuts silence from the start and end of audio, and stretches that are both black and silent from video |
| Split | audio, video | Cuts the file into two halves |
| Merge | two or more audio, video or document files of one type | Joins them in order |

## Install

- **Installer.** `Kiln-<version>-x64.msi` installs into Program Files, adds a Start menu entry and puts `kiln` on PATH.
- **Portable.** Unzip `Kiln-<version>-x64-portable.zip` anywhere and run `kiln-app.exe`.

Images convert out of the box. The other kinds use free engines that Kiln
finds on your PC. It never bundles or downloads them.

| Engine | Needed for |
|---|---|
| [FFmpeg](https://ffmpeg.org/download.html) | audio and video |
| [Pandoc](https://pandoc.org/installing.html) | documents |
| [LibreOffice](https://www.libreoffice.org/download/) (optional) | office files, spreadsheets, slides and PDF output |

Kiln looks on PATH, and for LibreOffice in its default install folder. You can
also choose the executables in **Settings > Engines**. Each LibreOffice job
runs with a fresh profile inside its temporary folder, with macros and active
content (OLE objects, DDE and other self-updating links) turned off and no
update check.

The only network call Kiln ever makes is **Check for updates** in Settings,
which asks GitHub for the latest release when you press it, and never on its own.

## Settings

Open Settings with the dial in the header or <kbd>Ctrl</kbd>+<kbd>,</kbd>.
Every change is saved straight away to `%APPDATA%\Kiln\settings.toml`, which
you can also edit by hand:

```toml
output_folder = 'D:\Converted'   # leave out to write next to each file
theme = "system"                 # system, dark or light

[keys]                           # shift, ctrl or alt; one per wheel
convert = "shift"
tools = "ctrl"
actions = "alt"

[engines]                        # leave out to use the ones on PATH
ffmpeg = 'C:\Tools\ffmpeg\bin\ffmpeg.exe'
pandoc = 'C:\Program Files\Pandoc\pandoc.exe'
libreoffice = 'D:\LibreOffice\program\soffice.exe'
```

A file Kiln can't read is left alone. The app starts with the defaults and
says why in the footer.

## Command line

```text
kiln convert INPUT FORMAT [-o DIR] [-q QUALITY] [--ffmpeg PATH] [--pandoc PATH] [--libreoffice PATH]
kiln tool TOOL INPUTS... [-o DIR]
kiln formats FILE
```

`kiln` exits with 0 on success, 2 when the request can't be done (an unknown
format, or an unsupported pair or tool), and 1 when a conversion fails.

## Build from source

You need Rust (stable, 1.82 or newer) with the MSVC toolchain on Windows 10 or 11.

```powershell
cargo build --release          # target\release\kiln-app.exe and kiln.exe
cargo test --workspace         # backend tests need ffmpeg, ffprobe, pandoc and LibreOffice
.\packaging\package.ps1        # dist\ MSI and portable ZIP (needs WiX 5 for the MSI)
```

`kiln-core` and `kiln-cli` also build and test on Linux and macOS, which is
handy for working on converters. `kiln-app` is Windows only. Its layout, theme,
icon and settings logic are plain Rust and are tested on every platform.

Signing is optional. Set `KILN_SIGN_CERT` (a .pfx path) and
`KILN_SIGN_PASSWORD` before running `package.ps1`, and the executables and
MSI are signed with signtool. Without them, packages are built unsigned.

```text
crates/kiln-core   conversion logic, format registry, job runner (no UI)
crates/kiln-cli    the `kiln` command
crates/kiln-app    the Windows app: Win32, Direct2D, DirectWrite
packaging          MSI definition and the packaging script
docs/screenshots   previews from each milestone
```

<details>
<summary><b>Add a format</b></summary>

<br>

1. Describe the type in [`crates/kiln-core/formats.toml`](crates/kiln-core/formats.toml):

   ```toml
   [formats.aiff]
   kind = "audio"
   aliases = ["aif"]
   ffmpeg = ["-c:a", "pcm_s16be"]
   ```

2. Add it to a route's `from` or `to` list, or to a new `[[route]]`. A pair
   declared twice is an error when the registry loads, so the tests catch it.
3. Add a small sample to `tests/fixtures`. The generator is
   `cargo run -p kiln-core --example gen_fixtures`.
4. Run `cargo test --workspace`. `crates/kiln-cli/tests/pairs.rs` converts
   every supported pair, so the new type is tested both ways.
5. Add it to the Formats table above. A test checks that this README names
   every format in the registry.

A format that needs a new engine gets a new backend: implement the
`Converter` trait in `kiln-core` and register it in `job.rs`. External
programs are always run with separate arguments, never through a shell.

</details>

<details>
<summary><b>Known limitations</b></summary>

<br>

- WEBP is written lossless only, so a WEBP made from a JPG can be larger than the JPG.
- An animated GIF keeps only its first frame.
- Compress works on PNG and JPG images, video, and audio other than WAV. Other image types have no tighter encoding to re-save with.
- Trim on video cuts only stretches that are both black and silent, and the kept part is re-encoded.
- Office files, PDF and documents have no tools other than Merge for documents.
- The update check returns "No releases are published yet" until a release exists on GitHub.

</details>

<details>
<summary><b>Performance</b></summary>

<br>

On the development PC (release build, Windows 11, NVIDIA GPU), the window
appears about 215 ms after launch. Idle, Kiln uses about 13 MB of private
memory, which is the figure Task Manager shows. Its working set is about
53 MB, but most of that is graphics driver code shared with other apps.
The MSI is 1.9 MB and the portable ZIP 2.4 MB.

</details>

## Contributing

Bug reports and format requests are welcome in
[Issues](https://github.com/chancrisp/kiln/issues). Before changing code, read
[CONTRIBUTING.md](CONTRIBUTING.md). [DESIGN.md](DESIGN.md) explains how Kiln
looks and sounds, and why. [VISUAL_TESTING.md](VISUAL_TESTING.md) is the
checklist to run before a release.

<div align="center">

<br>

**[Download Kiln](https://github.com/chancrisp/kiln/releases)**

<sub>Built with Rust, Win32, Direct2D and DirectWrite. Made by Chandler Crisp.<br>
Apache License 2.0. See <a href="LICENSE">LICENSE</a> and <a href="THIRD_PARTY_LICENSES">THIRD_PARTY_LICENSES</a>.</sub>

</div>
