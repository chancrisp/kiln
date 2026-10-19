# Changelog

All notable changes to Kiln are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [0.1.0] - Unreleased

The first version.

### Added

- `kiln-core`:
  - A format registry in `formats.toml` that rejects duplicate types, aliases and pairs when it loads.
  - Converters for images (built in), audio and video (FFmpeg), documents (Pandoc), and office documents, spreadsheets, slides and PDF output (LibreOffice, optional). That makes 189 format pairs.
  - Images in PNG, JPG, WEBP, BMP, GIF and TIFF, each checked by its signature before decoding.
  - LibreOffice runs with a fresh, locked-down profile per job: no macros, no active content, no update check, and no wait on a sleeping default printer.
  - Cancelling a job stops the engine and every process it started.
  - File tools: compress, crop, trim, split and merge. Trim cuts silence from audio, and stretches that are both black and silent from video.
  - A worker pool with per-job progress and cancel.
  - Per-job temporary folders, output names that never replace anything, and safe handling of reserved Windows names.
- `kiln` command line: `convert`, `tool` and `formats`.
- The Windows app:
  - A drop target and file list.
  - The glaze wheel, with Convert, Tools and Actions.
  - Progress, results and errors shown in each row, plus a details dialog. The progress line glows and breathes while a job runs.
  - Pressed controls show a soft inner shadow, and the empty list's hint follows the wheel keys.
  - One window: files dropped on the icon while Kiln is open go to the open window.
  - Check for updates in Settings, the app's only network call, made only when pressed.
  - A settings panel and `%APPDATA%\Kiln\settings.toml`, covering the output folder, wheel keys, theme and engine paths for FFmpeg, Pandoc and LibreOffice.
  - Light and dark themes that follow Windows.
  - Per-monitor DPI awareness and full keyboard control.
- Packaging: an MSI installer, a portable ZIP and optional code signing.
- CI on windows-latest.
