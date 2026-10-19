//! The platform-independent half of the Kiln app: design tokens, icons,
//! layout and the file list. The Windows half lives in `src/win` and is
//! compiled only for Windows.

pub mod icons;
pub mod layout;
pub mod model;
pub mod theme;

#[cfg(windows)]
pub mod win;
