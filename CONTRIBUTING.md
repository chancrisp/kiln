# Contributing to Kiln

Thanks for helping. Kiln is small on purpose, so the best changes are focused
ones: a new format, a clearer error, a fix with a test.

## Before you start

- Read [DESIGN.md](DESIGN.md) for anything visible. It covers the colours,
  type, icons, the wheel and the voice rules.
- Kiln is a clean-room project. Don't reference, decompile or copy the
  source, binaries, assets, icons or visual design of any other product.
  Work from Kiln's own requirements and DESIGN.md.
- For a large change, such as a new backend or a new wheel, open an issue
  first so we can agree on the shape.
- Kiln is licensed under the GNU General Public License, version 3 or later.
  By sending a change you agree that it is licensed the same way.

## Setting up

You need Rust stable (1.88 or newer). On Windows use the MSVC toolchain.
The backend tests run real engines, so put `ffmpeg`, `ffprobe` and `pandoc`
on PATH. Use the official Pandoc release. Some Linux distribution builds
can't write DOCX, ODT or EPUB in `--sandbox` mode.

```sh
cargo build
cargo test --workspace
```

`kiln-core` and `kiln-cli` build anywhere. `kiln-app` is Windows only, but
its platform-independent parts are tested everywhere: theme, icons, layout,
the file list, the wheel geometry, settings and copy path. To check the
Windows code from Linux:

```sh
rustup target add x86_64-pc-windows-gnu
cargo clippy -p kiln-app --target x86_64-pc-windows-gnu -- -D warnings
```

## Before you open a pull request

CI runs these on windows-latest, and all three must pass:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Also:

- Add or update tests. Converters get fixture-based tests in
  `crates/kiln-core/tests`, and every format pair is converted by
  `crates/kiln-cli/tests/pairs.rs`.
- For a visible change, run the relevant part of
  [VISUAL_TESTING.md](VISUAL_TESTING.md) at 100%, 150% and 200% scaling, in
  both themes. Attach before and after screenshots to the pull request.
- If you changed a theme token on purpose, update the snapshot with
  `KILN_UPDATE_SNAPSHOTS=1 cargo test -p kiln-app` and say why in the
  pull request.
- Add a line to [CHANGELOG.md](CHANGELOG.md) under the newest version.

## Code rules that matter

- Run external tools with `std::process::Command` and pass each argument
  separately. Never build a shell string. FFmpeg inputs and outputs use
  `file:` URLs and `-protocol_whitelist file`, and Pandoc runs with
  `--sandbox`.
- Never overwrite a file. Claim output names with `create_new`, as
  `kiln_core::output` does.
- Converting makes no network calls. The one exception is the update
  check in `kiln-app/src/win/updates.rs`, which runs only when the user
  presses Check for updates.
- User-facing text follows the voice rules: calm, specific, no
  exclamation marks, and "convert" rather than "fire".

## Building Kiln

You need Rust (stable, 1.88 or newer) with the MSVC toolchain on Windows 10 or 11.

```powershell
cargo build --release          # target\release\kiln-app.exe, kiln.exe and lame_enc.dll
cargo test --workspace         # backend tests need ffmpeg, ffprobe, pandoc, LibreOffice and Calibre
.\packaging\package.ps1        # dist\ MSI, portable ZIP and the source and symbols ZIPs (needs WiX 5 and its UI and Util extensions for the MSI: see package.ps1; and Making a release for FFmpeg)
```

Some environment variables are for testing and capturing, and are not settings:

- `KILN_TEST_CONTRAST=1` draws Kiln in High Contrast Black's colours without changing Windows' theme.
- `KILN_FRAME_LOG=<path>` appends one line for each frame drawn, with the time and how long the drawing took, so a run's timing can be read afterwards.
- `KILN_TEST_DRIVES=<drives, separated by ;>` replaces the mounted drives recovery looks on. Empty means none. Debug builds only: the tests set it so they never look on a real drive.
- `KILN_TEST_TRANSPARENCY=0` takes Windows' Transparency effects as off, so the base is solid. Debug builds only.
- `KILN_TEST_TYPE_PAGE=1` and `KILN_TEST_CONTROLS_PAGE=1` to `6` draw every type step and glyph, or the controls, one page at a time, in place of the window's content. Debug builds only.

[VISUAL_TESTING.md](VISUAL_TESTING.md) says how the capture scripts use them.

Kiln writes MP3 with LAME, which is LGPL, so it is not linked into the
executables. It is built as its own DLL, `lame_enc.dll` (`crates/kiln-lame`),
and Kiln loads it from the folder the program is in, and from nowhere else.
`cargo build` for the whole workspace puts it beside `kiln-app.exe` and
`kiln.exe`. `cargo run -p kiln-app` and `cargo run -p kiln-cli` build only the
program they name, so run `cargo build -p kiln-lame` once first (or a plain
`cargo build`), or Kiln says its MP3 encoder is missing. The package script
builds the DLL and puts it in the MSI and the ZIP.

`crates/kiln-media` reads sound with Symphonia, copied into
`crates/kiln-media/vendor` with the small fixes the fuzzers called for, each
marked and listed in `vendor/README.md`. Its quality harness converts a folder
of clips with Kiln and with FFmpeg and compares the two
(`cargo test --release -p kiln-media --features harness --test harness`, set up
as `crates/kiln-media/harness/README.md` says). Every reader is fuzzed by
`crates/kiln-media/fuzz` and, where `cargo fuzz` can't run, by
`cargo test -p kiln-media --release --test fuzz_lite -- --ignored`.

The C runtime is linked into all three (`.cargo/config.toml`), so none of them
needs the Visual C++ Redistributable. The C libraries that CMake builds
(libFLAC and libopus) are told to match through `.cargo/msvc-static-crt.cmake`.
If an old build folder has them built the other way, the link fails or warns
about `__imp_` symbols: run `cargo clean -p libflac-sys -p opusic-sys` once.
Setting a `RUSTFLAGS` environment variable overrides the flag in
`.cargo/config.toml` and breaks the link, so unset it.

`kiln-core` and `kiln-cli` also build and test on Linux and macOS, which is
handy for working on converters. `kiln-app` is Windows only. Its layout, theme,
icon, inspector, motion and settings logic are plain Rust and are tested on
every platform.

`package.ps1` signs nothing. Releases are signed only by the release workflow,
through SignPath or Azure Artifact Signing: see `packaging/SIGNING.md`.

```text
crates/kiln-core   conversion logic, format registry, job runner (no UI)
crates/kiln-cli    the `kiln` command
crates/kiln-media  the audio engine, with its quality harness, fuzzers and vendored Symphonia
crates/kiln-lame   lame_enc.dll, LAME as a DLL of its own (LGPL)
crates/kiln-app    the Windows app: Win32, Direct2D, DirectWrite, DirectComposition
packaging          MSI definition and the packaging script
docs/readme        the README's pictures
```

## Making a release

Pushing a tag such as `v0.1.0` runs `.github/workflows/release.yml`. It runs
CI's checks, checks the tag is `v` and the version in `Cargo.toml`, and builds
the MSI, the portable ZIP, Kiln's source ZIP, FFmpeg's source ZIP, the
symbols ZIP and `SHA256SUMS.txt`. It signs the programs and the MSI, checks
every signature, and attaches the files to a draft release of this repository,
which a person publishes. Running it by hand from Actions makes the same files
on the run without a release. Signing is set up as `packaging/SIGNING.md`
describes. A tag that isn't signed stops, except the one tag the variable
`KILN_UNSIGNED_RELEASE` names (the first release, v0.1.0).

Once the release is published, `packaging\winget.ps1 -Dist <folder>` reads the
MSI's SHA-256 from that folder's `SHA256SUMS.txt` and writes the winget
manifests under `<folder>\winget`. It writes files only; submitting them to
winget-pkgs is a separate step.

FFmpeg is built once. `.github/workflows/ffmpeg-lgpl.yml` builds the LGPL
FFmpeg and keeps it on the release `ffmpeg-lgpl` of this repository, a
prerelease never marked latest: the build zip, its source zip, their SHA-256 in `SHA256SUMS`, and
`builds-commit.txt`. Every release run calls that workflow. When the release
`ffmpeg-lgpl` is there, it builds nothing; the release run downloads its files,
and `packaging\ffmpeg\unpack-stored.ps1` checks them against `SHA256SUMS` and
unpacks them into `packaging\ffmpeg\staged` for `package.ps1`. The first run
builds, which takes hours. It can be started on its own from Actions >
FFmpeg LGPL build. Both workflows use only the run's own `GITHUB_TOKEN` for
this.

The first build takes the latest commit of BtbN's FFmpeg-Builds scripts and
records it in `builds-commit.txt` (and in `ffmpeg\BUILD.txt`). Later runs keep
it. To change it, set `BUILDS_COMMIT` in `ffmpeg-lgpl.yml` to the new commit,
keep a copy of the `ffmpeg-lgpl` release's files, delete that release, and run
the workflow again. With the release there and a different `BUILDS_COMMIT`,
the workflow stops rather than replace the stored build. With the release gone
and `BUILDS_COMMIT` empty, the next build takes the latest commit again.

To package on a PC with the stored FFmpeg, download the four files of the
`ffmpeg-lgpl` release into a folder, run
`.\packaging\ffmpeg\unpack-stored.ps1 -From <that folder>`, then
`.\packaging\package.ps1`. `-WithoutFfmpeg` packages without FFmpeg. The
packages are written to `dist`, unsigned.

## Adding a format

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
6. Add it to the Formats table in [README.md](README.md#formats). A test
   checks that the README names every format in the registry.

A format that needs a new engine gets a new backend: implement the
`Converter` trait in `kiln-core` and register it in `job.rs`. External
programs are always run with separate arguments, never through a shell.

## Conduct

Everyone taking part agrees to the [Code of Conduct](CODE_OF_CONDUCT.md).
