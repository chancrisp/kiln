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
- **85 formats, 1,245 pairs.** Images, camera RAW, audio, video, subtitles, documents, e-books, PDF, office files, spreadsheets and slides.
- **Suggestions.** Kiln looks inside the files you add and, when one choice clearly helps, points at it with a reason.
- **Quality you can set.** Resolution, frame rate, bitrate and page DPI, offered only where they fit the file, with an estimate of the result.
- **Offline.** Converting never touches the network. Nothing is uploaded, ever.
- **Nothing is saved until you export it**, and Kiln never overwrites a file without asking. Results wait inside Kiln until you export them, one at a time or all at once, to a folder you pick. Exporting into a folder never replaces a file there: a taken name becomes `name (1).jpg`.
- **Small and quick.** A single native executable of about 7.5 MB, drawn with Direct2D. No web view. The window is up in about 215 ms.
- **Other ways in.** **Add files** or <kbd>Ctrl</kbd>+<kbd>O</kbd> opens the Windows Open dialog, <kbd>Ctrl</kbd>+<kbd>V</kbd> adds files copied in File Explorer, and right-clicking a row opens a menu with every wheel.
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

The convert wheel keeps the eight most useful targets close. When a file can
become more than that, a **More** segment pours a second ring around the
first with every other format, grouped by kind.

<div align="center">
<img src="docs/screenshots/m8/wheel-more-dark-100.png" alt="The convert wheel with its More ring open" width="560">
</div>

## Suggestions

When files are added, Kiln looks inside them on this PC and, when one choice
clearly helps, says what it would do and why. A photo saved as PNG suggests
JPG, a MOV suggests MP4, and a large video suggests Compress. When nothing
clearly helps, Kiln says nothing.

- **In the list.** Each row's second line says what Kiln found and what it suggests, as in `4032 × 3024 photo · 3.1 MB · Suggested: JPG, about 2 MB`.
- **In the wheel.** A small celadon dot marks the suggested format or tool. Formats keep their places. Opened from the keyboard, the wheel starts on the suggestion, and the footer gives the reason.
- **Your picks.** When you have picked the same format for a type of file at least three times, and it is the one you pick most, Kiln suggests it from then on.
- **What it looks at.** The size and look of a picture, whether it has transparency or motion, the length and bitrate of audio and video, black bars, and whether a PDF has text. Images are read by Kiln itself, except HEIC, AVIF and JXL, which FFmpeg reads like audio and video.

Nothing is uploaded, and picks are counted only in `settings.toml`. Turn
suggestions off with **Suggestions** in **Settings > General**. **Forget
picks**, beside it, clears the picks Kiln has counted.

<table>
  <tr>
    <td align="center"><img src="docs/screenshots/m9/row-hint-dark-100.png" alt="Two rows, each with what Kiln found and its suggestion" width="270"><br><b>IN THE LIST</b><br><sub>What Kiln found, and what it suggests</sub></td>
    <td align="center"><img src="docs/screenshots/m9/wheel-dot-dark-100.png" alt="The convert wheel with a dot on JPG" width="270"><br><b>THE DOT</b><br><sub>On the suggestion, with the reason below</sub></td>
    <td align="center"><img src="docs/screenshots/m9/wheel-more-dot-light-100.png" alt="The More ring with a dot on QOI and on More" width="270"><br><b>IN MORE</b><br><sub>On More and on its place in the ring</sub></td>
  </tr>
</table>

## A closer look

<table>
  <tr>
    <td align="center"><img src="docs/screenshots/m7/jobs-light-100.png" alt="A running job, a finished one and a failed one" width="400"><br><b>JOBS</b><br><sub>Run in the background, each with its own progress line and cancel</sub></td>
    <td align="center"><img src="docs/screenshots/m7/settings-update-100.png" alt="The settings panel" width="400"><br><b>SETTINGS</b><br><sub>Output folder, theme, wheel keys, quality defaults and engines</sub></td>
  </tr>
  <tr>
    <td align="center"><img src="docs/screenshots/m5/details-100.png" alt="The details dialog with engine output" width="400"><br><b>DETAILS</b><br><sub>A failed job shows one plain line, with the full engine output a click away</sub></td>
    <td align="center"><img src="docs/screenshots/m6/wheel-convert-light-100.png" alt="The convert wheel in light mode" width="400"><br><b>LIGHT MODE</b><br><sub>Same stoneware, lighter glaze</sub></td>
  </tr>
</table>

## Formats

Kiln knows 85 formats and converts between 1,245 pairs. When no single engine
does a pair, Kiln chains two steps through a shared format, such as HEIC to
JPG through PNG, or MOBI to PDF through DOCX. The steps run in one temporary
folder, and only the finished file is kept, held inside Kiln until you
export it.

| Kind | Reads | Converts to | Engine |
|---|---|---|---|
| Image | PNG, JPG, WEBP, BMP, GIF, TIFF, ICO, TGA, QOI, PPM | any other of those, plus AVIF, JXL and PDF | built in |
| | SVG, and camera RAW: DNG, CR2, CR3, NEF, ARW, ORF, RW2, RAF, PEF, SRW | the image formats above, and PDF | built in (resvg draws SVG, Windows decodes RAW) |
| | AVIF, JXL, HEIC | any image format above, and PDF | FFmpeg |
| Animation | GIF, APNG | any video format | FFmpeg |
| Audio | WAV, MP3, FLAC, OGG, OPUS, M4A, AAC, AIFF, WMA, AC3, WV | any other audio format | FFmpeg |
| Video | MP4, M4V, MOV, MKV, WEBM, AVI, WMV, FLV, MPG, TS, 3GP, OGV | any other video format, any audio format, or an animated GIF, WEBP or APNG | FFmpeg |
| Subtitles | SRT, VTT, ASS | any other of those. MKV and MP4 give up their first subtitle track | FFmpeg |
| Document | MD, HTML, DOCX, ODT, EPUB, RST, TEX, ORG, ADOC, TYP, WIKI, TEXTILE, DBK, FB2, IPYNB | any other of those, plus RTF, TXT and PPTX | Pandoc |
| | the same | PDF and DOC | Pandoc, then LibreOffice |
| E-book | EPUB, MOBI, AZW3 | any other of those. MOBI and AZW3 also become documents and PDF | Calibre |
| PDF | PDF | TXT, and PNG, JPG, WEBP, BMP or TIFF with one image per page | built in (Windows draws the pages) |
| | | DOCX, ODT, DOC, RTF, the Pandoc formats, MOBI and AZW3, from its text | built in, then the document engines |
| Office document | DOC, RTF, TXT, and read only PAGES and WPD | DOC, DOCX, ODT, RTF, PDF, and through DOCX the Pandoc formats | LibreOffice |
| | DOCX, ODT | DOC, PDF, and PNG or JPG with one image per page | LibreOffice |
| | read only VSDX and PUB | PDF, and PNG or JPG with one image per page | LibreOffice |
| Spreadsheet | XLSX, XLS, ODS, CSV, and read only NUMBERS | XLSX, XLS, ODS, CSV, PDF and HTML | LibreOffice |
| Presentation | PPTX, PPT, ODP, and read only KEY | PPTX, PPT, ODP, PDF, and PNG or JPG with one image per slide | LibreOffice |

Aliases are accepted on input: JPEG, JFIF, TIF, PNM, PGM, PBM, PAM, NRW, SRF,
SR2, HEIF, HIF, OGA, AIF, MPEG, VOB, MTS, M2TS, SSA, MARKDOWN, HTM, TEXT,
ASCIIDOC, MEDIAWIKI, DOCBOOK, AZW and VSD. The full list lives in
[`crates/kiln-core/formats.toml`](crates/kiln-core/formats.toml), and
`kiln formats FILE` prints what a given file can become.

| Tool | Works on | What it does |
|---|---|---|
| Compress | PNG and JPG images, MP4, MOV, MKV and WebM video, MP3, OGG, Opus, M4A and FLAC audio, PDF | Re-encodes smaller, and keeps the original if the result isn't smaller |
| Crop | images, video | Removes plain borders from images and black bars from video |
| Trim | audio, video | Cuts silence from the start and end of audio, and stretches that are both black and silent from video |
| Split | audio, video, PDF | Cuts audio and video into two halves, and a PDF into one file per page |
| Merge | two or more audio, video, document or PDF files of one type, or images | Joins them in order. Images become one PDF, a page each |

## Quality

Every conversion starts from defaults that suit its kind, and each one can be
changed. Kiln offers only the settings that change the result for that file:

| Converting | Settings offered |
|---|---|
| video to video | quality, resolution, frame rate, audio bitrate |
| an animated GIF or APNG to video | quality, size, frame rate |
| video to an animated GIF or WEBP | quality, size, frame rate |
| video to an animated APNG | size, frame rate |
| video or audio to MP3, OGG, OPUS, M4A, AAC, WMA or AC3 | audio bitrate |
| an image to JPG, AVIF or JXL | quality, size |
| an image to a lossless format | size |
| PDF, document or slide pages to images | page DPI, plus quality for JPG |
| documents, e-books, PDF text, lossless audio | nothing: there's no choice that helps |

**Small** makes noticeably smaller files, fine for sharing. **High**, the
default, is visually lossless for most material. **Best** stays as close to the
source as the format allows. Sizes and frame rates are caps: Kiln never scales
up, so a 720p clip stays 720p when the cap is 1080p.

- **For one conversion.** In the convert wheel, point at a format and press
  <kbd>Tab</kbd> or right-click. The options sheet reads the file first, shows
  what it is (`1280 × 720 · 25 fps · 1:30`), and estimates the result as you
  change things.
- **As defaults.** **Settings > Quality** holds one set for video, images,
  audio and PDF pages. A plain click in the wheel uses them.
- **From the command line.** `--preset`, `--size`, `--fps`, `--audio-kbps`,
  `--dpi` and `--quality` (JPG, 1 to 100).

<table>
  <tr>
    <td align="center"><img src="docs/screenshots/m8/sheet-mov-dark-100.png" alt="The options sheet for an MP4 going to MOV" width="400"><br><b>OPTIONS SHEET</b><br><sub>Only the settings that fit, and an estimate</sub></td>
    <td align="center"><img src="docs/screenshots/m8/settings-quality-light-100.png" alt="The quality defaults in Settings" width="400"><br><b>QUALITY DEFAULTS</b><br><sub>One set for each kind of result</sub></td>
  </tr>
</table>

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
| [Calibre](https://calibre-ebook.com/download) (optional) | MOBI and AZW3 e-books |

PDF text, PDF pages as images, SVG and camera RAW need no engine. Kiln reads
them itself, with Windows drawing PDF pages and decoding RAW.

Kiln looks on PATH, and for LibreOffice and Calibre in their default install
folders. You can
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
output_folder = 'D:\Converted'   # where export dialogs start; leave out
                                 # for the last folder you exported to
theme = "system"                 # system, dark or light
suggestions = true               # false turns suggestions off

[keys]                           # shift, ctrl or alt; one per wheel
convert = "shift"
tools = "ctrl"
actions = "alt"

[quality]                        # the defaults in Settings > Quality
video = "high"                   # small, high or best
resolution = "original"          # original, or a cap in lines: 1080
fps = "original"                 # original, or a cap: 30
images = "high"                  # small, high or best
image_size = "original"          # original, or a cap on the long side: 2048
audio_kbps = 192                 # leave out to follow the preset
page_dpi = 150                   # 36 to 600

[engines]                        # leave out to use the ones on PATH
ffmpeg = 'C:\Tools\ffmpeg\bin\ffmpeg.exe'
pandoc = 'C:\Program Files\Pandoc\pandoc.exe'
libreoffice = 'D:\LibreOffice\program\soffice.exe'
calibre = 'C:\Program Files\Calibre2\ebook-convert.exe'

[habits]                         # picks per source type, counted by Kiln
png = { jpg = 4, webp = 1 }
heic = { png = 3 }
```

**Forget picks** in **Settings > General** clears `[habits]`.

A file Kiln can't read is left alone. The app starts with the defaults and
says why in the footer.

## Command line

```text
kiln convert INPUT FORMAT [-o DIR] [-p small|high|best] [-q JPG_QUALITY] [--size N] [--fps N]
             [--audio-kbps N] [--dpi N] [--ffmpeg PATH] [--pandoc PATH] [--libreoffice PATH] [--calibre PATH]
kiln tool TOOL INPUTS... [-o DIR]
kiln formats FILE
kiln suggest FILE...
```

`kiln convert` and `kiln tool` write next to the input, or into `-o DIR`,
as soon as they finish. Only the app holds results until you export them.

`kiln` exits with 0 on success, 2 when the request can't be done (an unknown
format, or an unsupported pair or tool), and 1 when a conversion fails.

`kiln suggest` prints each file's suggestion and its reason. It adds an
`(all)` line for a tool when one applies, even to a single file, and, for
several files, one for a conversion they share, when there is one. It
ignores your picks, so its output is the same every time. It exits with 2
when a file doesn't exist.

```text
> kiln suggest photo.png sample.mov
photo.png    JPG  A photo. JPG keeps the look at about 4.2 KB.
sample.mov   MP4  Plays on any phone, PC or browser.
```

## Build from source

You need Rust (stable, 1.82 or newer) with the MSVC toolchain on Windows 10 or 11.

```powershell
cargo build --release          # target\release\kiln-app.exe and kiln.exe
cargo test --workspace         # backend tests need ffmpeg, ffprobe, pandoc, LibreOffice and Calibre
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
5. Give it a plain name in `describe_format`, in
   [`crates/kiln-core/src/names.rs`](crates/kiln-core/src/names.rs), such as
   "a small image for the web". A test fails until every extension has one.
6. Add it to the Formats table above. A test checks that this README names
   every format in the registry.

A format that needs a new engine gets a new backend: implement the
`Converter` trait in `kiln-core` and register it in `job.rs`. External
programs are always run with separate arguments, never through a shell.

</details>

<details>
<summary><b>Known limitations</b></summary>

<br>

- A still WEBP is written lossless, so a WEBP made from a JPG can be larger than the JPG. Animated WEBP from video is lossy.
- An animated GIF or APNG converted to another image format keeps only its first frame. Convert it to video to keep the motion.
- HEIC, SVG, camera RAW, Pages, Numbers, Keynote, WordPerfect, Visio and Publisher files are read only.
- PDF to a document keeps the text and its paragraphs, not the layout. A scanned PDF has no text to take out.
- MOBI and AZW3 need Calibre, and only files without DRM convert.
- Camera RAW is decoded by Windows, so a RAW type needs the Raw Image Extension from the Microsoft Store on older Windows builds.
- Compress works on PNG and JPG images, MP4, MOV, MKV and WebM video, MP3, OGG, Opus, M4A and FLAC audio, and PDF. Other types have no tighter encoding to re-save with.
- Trim on video cuts only stretches that are both black and silent, and the kept part is re-encoded.
- Office files and documents other than PDF have no tools other than Merge for documents.
- The update check returns "No releases are published yet" until a release exists on GitHub.

</details>

<details>
<summary><b>Performance</b></summary>

<br>

On the development PC (release build, Windows 11, NVIDIA GPU), the window
appears about 215 ms after launch. Idle, Kiln uses about 13 MB of private
memory, which is the figure Task Manager shows. Its working set is about
53 MB, but most of that is graphics driver code shared with other apps.
`kiln-app.exe` is about 7.5 MB and `kiln.exe` about 2.8 MB. Most of the app's
size is the in-process PDF, SVG and image code that needs no engine.

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
