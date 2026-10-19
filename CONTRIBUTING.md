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

## Setting up

You need Rust stable (1.82 or newer). On Windows use the MSVC toolchain.
The backend tests run real engines, so put `ffmpeg`, `ffprobe` and `pandoc`
on PATH. Use the official Pandoc release. Some Linux distribution builds
can't write DOCX, ODT or EPUB in `--sandbox` mode.

```sh
cargo build
cargo test --workspace
```

`kiln-core` and `kiln-cli` build anywhere. `kiln-app` is Windows only, but
its platform-independent parts are tested everywhere: theme, icons, layout,
the file list, the wheel geometry, settings and file actions. To check the
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

## Adding a format

See [Add a format](README.md#add-a-format) in the README.

## Conduct

Everyone taking part agrees to the [Code of Conduct](CODE_OF_CONDUCT.md).
