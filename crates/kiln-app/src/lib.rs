//! The platform-independent half of the Kiln app: design tokens, icons,
//! layout and the file list. The Windows half lives in `src/win` and is
//! compiled only for Windows.

pub mod actions;
pub mod icons;
pub mod layout;
pub mod model;
pub mod settings;
pub mod theme;
pub mod update;
pub mod wheel;

#[cfg(windows)]
pub mod win;
