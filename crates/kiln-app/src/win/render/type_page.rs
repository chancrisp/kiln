//! A page of every type-ramp style and every glyph, drawn instead of the
//! window's content when a debug build starts with `KILN_TEST_TYPE_PAGE=1`,
//! for checking them by eye at each scale. A release build has no such
//! page: this module is compiled into debug builds only.

use super::*;
use crate::theme::fluent::{self, glyph};
use crate::win::text::{Ramp, Style};

/// Whether this debug build started with `KILN_TEST_TYPE_PAGE=1`. Read
/// once.
pub(super) fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("KILN_TEST_TYPE_PAGE").is_some_and(|v| v == "1"))
}

impl Draw<'_> {
    /// The page, over a window `w` × `h` layout units.
    pub(super) fn type_page(&self, w: f32, h: f32, p: &FluentPalette) -> Result<()> {
        let secondary = if p.high_contrast {
            p.text
        } else {
            p.text_secondary
        };
        let (ground, text) = (p.base, p.text);
        self.fill(&Rect::new(0.0, 0.0, w, h), 0.0, ground, 1.0);
        let caption = Style::new(Ramp::Caption);
        let ui = self.text.ui_family();
        let header = format!(
            "zoom {:.2}  dpi {:.0}  ui {:?}  icons {:?}",
            self.zoom,
            self.dpi,
            ui,
            self.text.icon_family()
        );
        self.styled(
            &header,
            caption,
            &Rect::new(16.0, 8.0, w - 32.0, 16.0),
            secondary,
        )?;
        let mut y = 32.0;
        for ramp in Ramp::ALL {
            let s = ramp.style();
            let m = crate::win::text::metrics(Style::new(ramp), ui, self.zoom);
            let words = if ramp == Ramp::Title {
                "42% 18% 100%".to_owned()
            } else {
                format!("{} {}/{} {}", s.name, s.size, s.line, m.family)
            };
            // A faint band shows the line box the style claims.
            self.fill(&Rect::new(16.0, y, w - 32.0, s.line), 0.0, secondary, 0.12);
            self.styled(
                &words,
                Style::new(ramp),
                &Rect::new(16.0, y, w - 32.0, s.line),
                text,
            )?;
            y += s.line + 4.0;
        }
        let para = "Body, wrapping: a line that runs past the edge of its box goes on to the next line, each line as tall as the style says.";
        let block = Rect::new(16.0, y, (w - 32.0).min(360.0), 60.0);
        self.styled(para, Style::new(Ramp::Body).wrapping(), &block, text)?;
        y += 64.0;
        let mid = Rect::new(16.0, y, w - 32.0, 20.0);
        self.fill(&mid, 0.0, secondary, 0.12);
        self.styled(
            "Body Strong, centred",
            Style::new(Ramp::BodyStrong).centered(),
            &mid,
            text,
        )?;
        y += 28.0;
        let code = Style::new(Ramp::Caption).code();
        let row = Rect::new(16.0, y, w - 32.0, 16.0);
        self.styled("png  webp  mp4  pdf", code, &row, text)?;
        self.styled("12.4 MB", caption.trailing(), &row, secondary)?;
        y += 24.0;
        for size in [
            fluent::type_ramp::ICON_SIZE,
            fluent::type_ramp::ICON_SIZE_TITLE_BAR,
        ] {
            let cell = size + 16.0;
            let mut x = 16.0;
            for (_, code) in glyph::ALL {
                if x + cell > w - 16.0 {
                    x = 16.0;
                    y += cell + 4.0;
                }
                let r = Rect::new(x, y, cell, cell);
                self.fill(&r, 4.0, secondary, 0.08);
                self.glyph(code, &r, size, text);
                x += cell + 4.0;
            }
            y += cell + 12.0;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_debug_build_draws_the_page_only_when_asked() {
        let asked = std::env::var_os("KILN_TEST_TYPE_PAGE").is_some_and(|v| v == "1");
        assert_eq!(super::super::type_page_on(), asked);
    }
}
