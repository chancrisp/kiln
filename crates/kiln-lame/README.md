# kiln-lame

Builds `lame_enc.dll`: the LAME MP3 encoder, version 3.100, as a DLL of its own.

LAME is licensed under the LGPL (see `COPYING`). Kiln keeps it out of its own
executables: `kiln-media` loads `lame_enc.dll` at run time from the folder the
program is in, so the LGPL code stays a separate, replaceable library.

## Where the source comes from

The LAME 3.100 source is the copy bundled in the `mp3lame-sys` 0.1.11 crate,
taken through cargo. It is not vendored here. `mp3lame-sys` builds it with the
`cc` crate on Windows. Only this crate depends on `mp3lame-sys`; `kiln-media`
has no build-time dependency on LAME, and lists this crate as a
dev-dependency only so that the tests find the DLL next to the test binaries.

## What the DLL exports

`build.rs` tells the linker to export, unchanged, the LAME C functions that
Kiln's MP3 sink calls (the list is `EXPORTS` in `build.rs`). There is no Rust
API.

## Where the DLL goes

`packaging\package.ps1` builds it with `cargo build --release -p kiln-lame` and
puts it in the MSI and the portable ZIP, in the same folder as `kiln-app.exe`
and `kiln.exe`, with this crate's `COPYING` as `LAME-COPYING.txt`. Anyone can
swap in their own build of LAME there: the programs ask the DLL for the
functions in `EXPORTS` and nothing else. In a source checkout, `cargo build`
(the whole workspace) or `cargo build -p kiln-lame` puts it in
`target\<profile>\`, beside the programs built there.
