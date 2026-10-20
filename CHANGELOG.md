# Changelog

All notable changes to Kiln are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [0.1.0] - Unreleased

The first version.

### Added

- `kiln-core`:
  - A format registry in `formats.toml` that rejects duplicate types, aliases and pairs when it loads.
  - Converters for images (built in), audio and video (FFmpeg), documents (Pandoc), office documents, spreadsheets, slides and PDF output (LibreOffice, optional), e-books (Calibre, optional) and PDF (built in). That makes 85 formats and 1,245 pairs.
  - Chains: a pair no single engine can do runs as two steps through a shared format, such as HEIC to JPG through PNG or MOBI to PDF through DOCX, inside one temporary folder.
  - Images in PNG, JPG, WEBP, BMP, GIF, TIFF, ICO, TGA, QOI and PPM, each checked by its signature before decoding. SVG is drawn by resvg, and camera RAW (DNG, CR2, CR3, NEF, ARW, ORF, RW2, RAF, PEF, SRW) is decoded by Windows. AVIF, JPEG XL and HEIC go through FFmpeg.
  - Audio in WAV, MP3, FLAC, OGG, OPUS, M4A, AAC, AIFF, WMA, AC3 and WavPack. Video in MP4, M4V, MOV, MKV, WEBM, AVI, WMV, FLV, MPG, TS, 3GP and OGV, plus animated GIF, WEBP and APNG from video and back.
  - Subtitles in SRT, VTT and ASS, and the first subtitle track out of MKV and MP4.
  - Documents in Org, AsciiDoc, Typst, MediaWiki, Textile, DocBook, FictionBook and Jupyter notebooks, alongside Markdown, HTML, DOCX, ODT, EPUB, RST and LaTeX. Pages, WordPerfect, Visio, Publisher, Numbers and Keynote files are read through LibreOffice.
  - PDF in process: its text out, its pages as images (drawn by Windows), images in as pages, and PDF merge, split and compress, with no engine needed.
  - Quality settings: Small, High and Best presets, caps on resolution, frame rate and image size, audio bitrate and page DPI. `knobs` says which ones change the result of a pair, and `probe` and `estimate` describe the file and the likely result.
  - LibreOffice runs with a fresh, locked-down profile per job: no macros, no active content, no update check, and no wait on a sleeping default printer.
  - Cancelling a job stops the engine and every process it started.
  - File tools: compress, crop, trim, split and merge. Trim cuts silence from audio, and stretches that are both black and silent from video.
  - A worker pool with per-job progress and cancel.
  - Per-job temporary folders, output names that never replace anything, and safe handling of reserved Windows names.
- `kiln` command line: `convert`, `tool` and `formats`, with `--preset`, `--size`, `--fps`, `--audio-kbps` and `--dpi`.
- The Windows app:
  - A drop target and file list.
  - The glaze wheel, with Convert, Tools and Actions. The convert wheel keeps eight common targets close, and a More segment pours a second ring with every other format, grouped by kind and named (Images, Video, Audio, Documents). A lit format says what it is in the footer and on the options sheet, as in "WEBP: a small image for the web".
  - An options sheet: Tab, right-click, or **Options… (Tab)** in the centre on a format in the wheel shows only the quality settings that fit the file, what the file is, and an estimate of the result.
  - Settings in three tabs, General, Quality and Helper apps, with quality defaults for video, images, audio and pages drawn as images. Labels say what they change, as in Sound quality, Sharpness (DPI) and Keys to hold while dropping.
  - Help: a **?** in the header, or F1, lists the ways in, how results wait to be exported, and every keyboard shortcut. Icon-only controls have tooltips with their shortcuts, and a row's tooltip gives its whole line and full folder path.
  - Progress, results and errors shown in each row, plus a details dialog. The progress line glows and breathes while a job runs.
  - Pressed controls show a soft inner shadow, and the empty list's hint follows the wheel keys.
  - One window: files dropped on the icon while Kiln is open go to the open window.
  - Other ways in: an **Add files** button in the empty window and the footer, and Ctrl+O, open the Windows Open dialog; Ctrl+V adds files copied in File Explorer; and right-clicking a row, the Menu key or Shift+F10 opens a native menu with the wheels, Export, Open, Show in folder and Remove. The empty window puts the instruction second and the tagline last, the footer says what to do next once files are listed, and each wheel's title names its key.
  - Check for updates in Settings, the app's only network call, made only when pressed.
  - A settings panel and `%APPDATA%\Kiln\settings.toml`, covering the output folder, wheel keys, theme, quality defaults and engine paths for FFmpeg, Pandoc, LibreOffice and Calibre.
  - Light and dark themes that follow Windows.
  - Per-monitor DPI awareness and full keyboard control.
- Suggestions:
  - `kiln-core` gains `inspect`, which looks inside a file once, stopping each engine run or PDF read after 2 s: the size and look of a picture (photo or graphic), transparency and motion, audio and video details, black bars, and whether a PDF has text. `suggest` turns those facts into at most one conversion and one tool, each with a one-line reason and, where it can, an estimate.
  - Only targets whose engine is installed are suggested. For several files, a conversion is suggested only when every file gets the same answer.
  - Picks are counted per source type in `settings.toml` under `[habits]`. A target picked at least three times, and more than any other, becomes the suggestion.
  - `kiln suggest FILE...` prints each file's suggestion and the set's, ignoring picks.
  - In the app, files are inspected on one low-priority thread as they are added. Each row's second line shows what Kiln found and its suggestion, then the size and folder, fading in over 160 ms.
  - The convert and Tools wheels mark the suggestion with a small celadon dot, on More and in the ring when it sits there. Opened from the keyboard, a wheel starts on the suggestion, and the footer gives the reason.
  - **Settings > General** gains **Suggestions**, On or Off, and **Forget picks**. `settings.toml` gains `suggestions = true`.
- Packaging: an MSI installer, a portable ZIP and optional code signing.
- The installer adds Kiln to Explorer's Open with and Send to.
- CI on windows-latest.
