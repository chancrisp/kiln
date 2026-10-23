//! The platform-independent half of the Kiln app: design tokens, icons,
//! layout and the file list. The Windows half lives in `src/win` and is
//! compiled only for Windows.

pub mod a11y;
pub mod actions;
pub mod anim;
pub mod help;
pub mod icons;
pub mod inspector;
pub mod layout;
pub mod model;
pub mod previews;
pub mod quality;
pub mod settings;
pub mod theme;
pub mod update;
pub mod ways_in;
pub mod wheel;

#[cfg(windows)]
pub mod win;
