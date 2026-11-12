# Vendored C sources

## wavpack

- Library: libwavpack 5.6.0 (`LIBWAVPACK_VERSION_STRING` in `wavpack/src/wavpack_version.h`).
- Source: copied from the WavPack tree inside the `wavpack-sys` 0.4.0 crate
  (cargo registry, wavpack-sys 0.4.0, SHA-256 of the .crate file
  `eeb64ebea7786e7f9ee4c6391d8da68776422ed3aaa49e480ea88ba891e58c77`),
  which carries github.com/dbry/WavPack unmodified. Nothing was downloaded
  outside cargo.
- Licence: BSD-3-Clause, in `wavpack/COPYING`.
- What is kept: the library sources `src/*.c` and the headers they include,
  `include/wavpack.h`, `COPYING` and `README.md`. Left out: the command-line
  programs, the plugins, the assembly optimisations, the legacy (pre-4.0)
  decoder, DSD support, and `open_filename.c` (the one file that opens a file
  by name). Kiln hands libwavpack callbacks over Rust `File`s, so no C file
  I/O is compiled in.
- Built by `build.rs` with the `cc` crate. Kiln declares the few functions it
  uses by hand in `src/wavpack.rs`; there is no bindgen or libclang step.
- To update: copy the same files from a newer wavpack-sys (or the upstream
  tag), keep this list, and update the version and checksum above.

## symphonia-*

- Library: Symphonia 0.5.5 (MPL-2.0, github.com/pdeljanov/Symphonia), the crates
  `symphonia-core`, `-codec-aac`, `-codec-alac`, `-codec-vorbis`, `-format-isomp4`,
  `-format-mkv`, `-format-ogg`, `-format-riff` and `-metadata`.
- Source: copied from the crates cargo had already downloaded into its registry
  cache (`src/` and `Cargo.toml` of each; nothing was downloaded outside cargo).
  Each copy is used through `[patch.crates-io]` in the workspace `Cargo.toml`.
  The other Symphonia crates (`symphonia`, `-bundle-flac`, `-bundle-mp3`,
  `-codec-pcm`, `-utils-xiph`) are the registry's, unpatched.
- Why: the fuzzers (`tests/fuzz_lite.rs`, `fuzz/`) found damaged files that made
  Symphonia panic, or allocate gigabytes from a length field. A reader must give
  an error for those, and wrapping the reader in `catch_unwind` is not a fix.
  Every change is marked with a `Kiln:` comment. Each is small, and a valid
  file decodes as before, with one exception: the two-channel residue fix below
  changes the output for a Vorbis file whose residue submap holds two channels
  that don't start at channel 0, where the old code panicked or wrote the wrong
  channel. Stereo and single-submap files decode identically (checked byte for
  byte against the unpatched decoder).
- The changes:
  - `symphonia-core`: `TimeBase::new` raised a zero numerator or denominator to 1
    instead of panicking (a damaged file can state a zero timescale or sample
    rate). `ReadBytes::read_boxed_slice` and `read_boxed_slice_exact` grow their
    buffer a megabyte at a time as bytes arrive, so a claimed length of 4 GB
    can't size an allocation.
  - `symphonia-metadata`: the Vorbis comment and FLAC picture readers read their
    strings through `read_boxed_slice_exact` instead of `vec![0; claimed_length]`.
  - `symphonia-format-riff`: the same, for RIFF `INFO` values.
  - `symphonia-format-mkv`: the asserts and `expect`s on the element position and
    on an open element are errors; a variable-size integer whose first byte is 0
    is an error; a timestamp scale of 0 or over 32 bits is an error (it was an
    `unwrap`, and later a division by zero); block timestamps saturate; seeking
    without a time base is an error.
  - `symphonia-format-isomp4`: table capacities (`stco`, `co64`, `stsc`, `stsz`,
    `stts`) are capped at 65,536 entries ahead of the data; an `stsc` first chunk
    of 0 is an error, and its sample counts, and `stts` totals, saturate; an
    `esds` with no ES descriptor, a missing chunk-offset table and a missing
    track fragment are errors (they were `unwrap`s).
  - `symphonia-format-ogg`: the Vorbis setup scanner's `lookup1_values` no longer
    asserts.
  - `symphonia-codec-aac`: a section list longer than the band table is an error.
  - `symphonia-codec-alac`: a magic cookie with a frame length of 0 or over 65,536
    is an error (it sized the output buffers: 8 GB from one flipped byte); a
    partial frame longer than the frame length is an error; a frame shorter than
    the predictor order has fewer warm-up samples instead of indexing past its end.
    The 65,536 limit is deliberate, and FFmpeg accepts larger frame lengths, so an
    exotic valid file with bigger frames is refused here.
  - `symphonia-codec-vorbis`: `lookup1_values` finds the exact integer root
    instead of asserting on the float one, and returns 0 for 0 dimensions; a
    vector codebook with 0 dimensions is an error, and so is a setup header whose
    vector codebooks hold more than 16,777,216 values between them (the test
    file's setup holds about 61,000); a residue may not use a codebook with 0
    dimensions as its classbook or a vector book (a scalar book with none is
    left alone);
    a codeword tree with no room left is an error; a residue's classbook is checked
    against the codebook count; the two-channel residue split indexes the right
    channels (the one change that alters output for a file that used to decode); floor lines that run backwards or off the end draw nothing, and floor
    heights are clamped to the table; the floors and residues compare book
    numbers with a codebook count that holds 256, the most a setup may have
    (it was cast to `u8` and wrapped to 0, refusing every book).
- To update: copy the same crates from a newer 0.5.x release, then apply the
  `Kiln:` changes again (search for the marker in the old copies), and run the
  fuzzers. Symphonia 0.6 changes the API `src/sym.rs` is written against; moving
  to it is a task of its own, and may make this directory unnecessary.
