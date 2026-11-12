//! The platform-independent half of the Kiln app: design tokens, icons,
//! layout and the file list. The Windows half lives in `src/win` and is
//! compiled only for Windows.

pub mod a11y;
pub mod actions;
pub mod anim;
pub mod autostart;
pub mod controls;
pub mod credit;
pub mod diag;
pub mod drag_watch;
pub mod help;
pub mod icons;
pub mod inspector;
pub mod layout;
pub mod model;
pub mod previews;
pub mod quality;
pub mod settings;
pub mod settings_page;
pub mod smaller;
pub mod textrange;
pub mod theme;
pub mod tune_sheet;
pub mod update;
pub mod ways_in;
pub mod wheel;

#[cfg(windows)]
pub mod win;

/// The mark a Kiln build with the test hooks carries (a debug build, or one
/// made with the `test-hooks` feature), so a test given a Kiln to start by
/// path can tell it honours the test switches. A shipped build has no such
/// module, and no mark: the tests' own code looks for it.
#[cfg(any(debug_assertions, feature = "test-hooks"))]
pub mod test_build {
    /// The mark, kept in the program by a use on the start's path
    /// (`held::volumes`).
    pub static MARK: [u8; 32] = *b"kiln:test-hooks:build:6b1f0c93e2";
}
