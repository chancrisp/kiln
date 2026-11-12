//! What the person has chosen and what follows from it: Settings and the
//! file they are kept in, the theme they chose, the contrast theme Windows
//! has on, the palette those make and the backdrop behind the window.
//! `App` holds one and hears when the palette or backdrop changed.

use crate::settings::{self, Settings};
use crate::theme::{self, fluent::FluentPalette};

use super::titlebar;

/// Settings, and the look they and Windows decide on.
pub(super) struct Prefs {
    /// The person's settings as they are now.
    pub(super) settings: Settings,
    /// Where settings.toml is, and whether it could be read.
    pub(super) store: settings::Store,
    /// The chosen theme's palette, or one from a contrast theme.
    pub(super) palette: FluentPalette,
    /// The contrast theme's colours, while one is on.
    pub(super) contrast: Option<theme::SysColors>,
    /// Whether Windows' app mode is light, which the Theme setting's
    /// System follows.
    pub(super) system_light: bool,
    /// What is behind the window: Mica, or Kiln's solid base.
    pub(super) backdrop: titlebar::Backdrop,
}

impl Prefs {
    /// A contrast theme's colours while one is on; otherwise the theme
    /// Settings chose (System follows the app mode), and the backdrop as
    /// transparency effects, a contrast theme and a named theme leave it.
    /// Whether the palette or the backdrop changed.
    pub(super) fn restyle(&mut self) -> bool {
        let p = super::palette_for(self.contrast, self.settings.theme, self.system_light);
        // Transparency effects or a contrast theme may have changed too.
        let backdrop = super::backdrop_now(self.contrast, self.settings.theme);
        if p != self.palette || backdrop != self.backdrop {
            self.palette = p;
            self.backdrop = backdrop;
            true
        } else {
            false
        }
    }
}
