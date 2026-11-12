//! Make smaller for video when its first pass lands over the size: the job
//! encodes once more with its size model corrected, and the file it keeps
//! is under the size (spec 3.2, R45).
//!
//! The miss is forced with `KILN_FIT_FIRST_PASS_OVER`, a debug-only hook that
//! multiplies the first pass's bitrate. In a test binary of its own: the
//! variable changes every job in the process.

#![cfg(all(windows, debug_assertions))]

#[path = "../../../tests/support/mod.rs"]
mod support;

use kiln_core::{CancelToken, Kiln, Options, Progress, Tool, ToolOptions};
use support::clips::{can_make_clips, clip, Clip};
use support::files_in;

#[test]
fn a_first_pass_over_the_size_is_encoded_once_more_and_lands_under() {
    let test = "a_first_pass_over_the_size_is_encoded_once_more_and_lands_under";
    let kiln = Kiln::new();
    if !kiln.availability(&Options::default()).has("audio-mf") {
        eprintln!("{test}: skipped, Media Foundation's codecs aren't installed");
        return;
    }
    if !can_make_clips(test) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("noisy.mp4");
    std::fs::copy(clip(Clip::Noisy), &input).unwrap();
    let before = std::fs::read(&input).unwrap();
    let modified = std::fs::metadata(&input).unwrap().modified().unwrap();
    let max = 10_000_000;
    let opts = Options {
        tool: ToolOptions {
            max_bytes: Some(max),
            ..ToolOptions::default()
        },
        ..Options::default()
    };
    std::env::set_var("KILN_FIT_FIRST_PASS_OVER", "2.5");
    let mut seen = Vec::new();
    let out = kiln.run_tool_with(
        Tool::Smaller,
        std::slice::from_ref(&input),
        &opts,
        &mut |p| {
            if let Progress::Fraction(f) = p {
                seen.push(f);
            }
        },
        &CancelToken::new(),
    );
    std::env::remove_var("KILN_FIT_FIRST_PASS_OVER");
    let out = out.unwrap();
    assert!(std::fs::metadata(&out[0]).unwrap().len() <= max);
    // The second pass has the bar from 0.9 to 0.97; a third would go past.
    assert!(
        seen.iter().any(|f| *f > 0.9 && *f < 0.97),
        "no second pass: {seen:?}"
    );
    assert!(
        !seen.iter().any(|f| *f > 0.97 && *f < 0.99),
        "a third pass: {seen:?}"
    );
    assert_eq!(files_in(dir.path()), ["noisy (smaller).mp4", "noisy.mp4"]);
    // The source is untouched.
    assert_eq!(std::fs::read(&input).unwrap(), before);
    assert_eq!(
        std::fs::metadata(&input).unwrap().modified().unwrap(),
        modified
    );
}
