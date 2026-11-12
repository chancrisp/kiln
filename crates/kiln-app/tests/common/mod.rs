//! What every test that starts a Kiln checks first: that the Kiln it
//! would start honours the test switches (`KILN_TEST_DRIVES`,
//! `KILN_TEST_DOCUMENTS` and the rest). A debug build does, and so does
//! one made with the `test-hooks` feature. A build without them looks in
//! the real drives' holding folders at start and delivers into the real
//! Documents, so a test must never start one.
//!
//! Test code only: nothing here is in a shipped Kiln.

#![allow(dead_code)]

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

/// Whether the Kiln these tests were built with (`CARGO_BIN_EXE_kiln-app`)
/// honours the test switches: the tests and the program share one build's
/// profile and features.
pub const HOOKS: bool = cfg!(any(debug_assertions, feature = "test-hooks"));

/// Why a test refuses to start a Kiln without the hooks.
pub const REFUSED: &str = "a test never starts a Kiln built without the test hooks: it would look in the real drives' holding folders and save into the real Documents. Build it in debug or with --features test-hooks.";

/// What a mark's byte is masked with, so the code that looks for the mark
/// never carries it whole.
const MASK: u8 = 0x5a;

const fn masked(text: [u8; 32]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        out[i] = text[i] ^ MASK;
        i += 1;
    }
    out
}

/// The mark a build with the hooks carries (`kiln_app::test_build::MARK`),
/// masked.
const MASKED: [u8; 32] = masked(*b"kiln:test-hooks:build:6b1f0c93e2");

/// The mark, unmasked as it is needed.
pub fn mark() -> Vec<u8> {
    MASKED.iter().map(|b| b ^ MASK).collect()
}

/// Stop a test that would start the Kiln built with it when that build
/// doesn't honour the test switches ([`HOOKS`]).
pub fn refuse_without_hooks() {
    if !HOOKS {
        panic!("{REFUSED}");
    }
}

/// Stop a test that would start the Kiln at `exe`, a build given by path,
/// unless it carries the hooks' mark ([`has_hooks`]).
pub fn refuse_unless_hooked(exe: &Path) {
    match has_hooks(exe) {
        Ok(true) => {}
        Ok(false) => panic!("{}: {REFUSED}", exe.display()),
        Err(e) => panic!("{} can't be read: {e}", exe.display()),
    }
}

/// Whether the program `exe` is a Kiln build with the test hooks: it
/// carries their mark. A test given a Kiln to start by path checks this
/// first, and refuses one without them.
pub fn has_hooks(exe: &Path) -> io::Result<bool> {
    let needle = mark();
    let mut f = File::open(exe)?;
    let mut buf = vec![0u8; (1 << 20) + needle.len()];
    // The tail of the last read, so a mark across two reads is found.
    let mut kept = 0;
    loop {
        let n = f.read(&mut buf[kept..])?;
        if n == 0 {
            return Ok(false);
        }
        let have = kept + n;
        if buf[..have].windows(needle.len()).any(|w| w == needle) {
            return Ok(true);
        }
        kept = (needle.len() - 1).min(have);
        buf.copy_within(have - kept..have, 0);
    }
}
