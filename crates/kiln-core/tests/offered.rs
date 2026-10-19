//! What Kiln offers for a file, given the engines that are installed.

#[path = "../../../tests/support/mod.rs"]
mod support;

use std::path::PathBuf;

use kiln_core::{Kiln, Options, Tool};
use support::fixture;

fn no_ffmpeg() -> Options {
    let mut o = Options::default();
    o.engines.ffmpeg = Some(PathBuf::from(r"C:\nowhere\ffmpeg.exe"));
    o
}

#[test]
fn image_targets_need_no_engine() {
    let k = Kiln::new();
    let png = fixture("rgb_gradient.png");
    let o = k.offered(&[png.as_path()], &no_ffmpeg());
    assert!(o.targets.contains(&"jpg".to_owned()));
}

#[test]
fn a_missing_engine_offers_nothing_it_runs() {
    let k = Kiln::new();
    let mov = fixture("sample.mov");
    let o = k.offered(&[mov.as_path()], &no_ffmpeg());
    assert!(o.targets.is_empty());
    assert!(o.tools.is_empty());
}

#[test]
fn several_files_share_their_common_targets() {
    let k = Kiln::new();
    let (a, b) = (fixture("rgb_gradient.png"), fixture("sample.mp3"));
    assert!(k
        .offered(&[a.as_path(), b.as_path()], &Options::default())
        .targets
        .is_empty());
}

#[test]
fn merge_is_offered_for_two_pdfs() {
    let k = Kiln::new();
    let p = fixture("sample.pdf");
    let o = k.offered(&[p.as_path(), p.as_path()], &no_ffmpeg());
    assert!(o.tools.contains(&Tool::Merge));
}
