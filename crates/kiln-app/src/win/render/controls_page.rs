//! A gallery of every phase 3 control in each of its states, drawn
//! instead of the window's content when a debug build starts with
//! `KILN_TEST_CONTROLS_PAGE` set to a page from 1 to 6, for checking them
//! by eye in each theme and at each scale. A release build has no such
//! page: this module is compiled into debug builds only. Its words are
//! for the gallery alone.

use super::controls::MenuLine;
use super::*;
use crate::controls::{ButtonKind, ComboState, Lit, Severity, TextBoxState};
use crate::layout::kit::{self, MenuRow};
use crate::theme::fluent::{glyph, radius, FluentPalette};
use crate::win::text::{Ramp, Style};

/// The page asked for, 1 to 6, read once.
pub(super) fn page() -> Option<u8> {
    static PAGE: std::sync::OnceLock<Option<u8>> = std::sync::OnceLock::new();
    *PAGE.get_or_init(|| {
        std::env::var("KILN_TEST_CONTROLS_PAGE")
            .ok()
            .and_then(|v| v.parse::<u8>().ok())
            .filter(|n| (1..=PAGES).contains(n))
    })
}

pub(super) const PAGES: u8 = 6;

const STATES: [(&str, Lit); 4] = [
    ("Rest", Lit::REST),
    ("Hover", Lit::HOVER),
    ("Pressed", Lit::PRESSED),
    ("Disabled", Lit::DISABLED),
];

impl Draw<'_> {
    /// Page `n` of the gallery over a window `w` × `h` layout units.
    pub(super) fn controls_page(&self, w: f32, h: f32, n: u8, p: &FluentPalette) -> Result<()> {
        self.fill(&Rect::new(0.0, 0.0, w, h), 0.0, p.base, 1.0);
        let head = format!(
            "Controls {n}/{PAGES}  {}  zoom {:.2}  dpi {:.0}",
            p.name, self.zoom, self.dpi
        );
        self.styled(
            &head,
            Style::new(Ramp::Caption),
            &Rect::new(16.0, 8.0, w - 32.0, 16.0),
            p.text_secondary,
        )?;
        match n {
            1 => self.page_buttons(p),
            2 => self.page_radios(w, p),
            3 => self.page_info(w, p),
            4 => self.page_combos(w, h, p),
            5 => self.page_expanders(w, h, p),
            _ => {
                self.page_combos(w, h, p)?;
                self.page_dialog(w, h, p)
            }
        }
    }

    fn heading(&self, s: &str, y: f32, p: &FluentPalette) -> Result<()> {
        self.styled(
            s,
            Style::new(Ramp::BodyStrong),
            &Rect::new(16.0, y, 600.0, 20.0),
            p.text,
        )?;
        Ok(())
    }

    /// Buttons of each kind in each state, and ToggleSwitches.
    fn page_buttons(&self, p: &FluentPalette) -> Result<()> {
        self.heading("Buttons: rest, hover, pressed, disabled, focused", 32.0, p)?;
        let kinds = [
            (ButtonKind::Standard, None),
            (ButtonKind::Accent, None),
            (ButtonKind::Subtle, Some(glyph::ADD_FILES)),
            (ButtonKind::Hyperlink, None),
        ];
        let mut y = 60.0;
        for (kind, g) in kinds {
            for (i, (label, lit)) in STATES.iter().enumerate() {
                let r = Rect::new(16.0 + i as f32 * 132.0, y, 120.0, 32.0);
                self.fl_button(kind, &r, label, g, *lit, p)?;
            }
            let r = Rect::new(16.0 + 4.0 * 132.0, y, 120.0, 32.0);
            self.fl_button(kind, &r, "Focused", g, Lit::REST, p)?;
            self.focus_visual(&r, radius::CONTROL, p);
            y += 44.0;
        }
        y += 8.0;
        self.heading("ToggleSwitch", y, p)?;
        y += 28.0;
        let toggles: [(bool, f32, Lit); 10] = [
            (false, 0.0, Lit::REST),
            (false, 0.0, Lit::HOVER),
            (false, 0.0, Lit::PRESSED),
            (false, 0.0, Lit::DISABLED),
            (false, 0.5, Lit::HOVER),
            (true, 1.0, Lit::REST),
            (true, 1.0, Lit::HOVER),
            (true, 1.0, Lit::PRESSED),
            (true, 1.0, Lit::DISABLED),
            (true, 1.0, Lit::REST),
        ];
        for (i, (on, at, lit)) in toggles.iter().enumerate() {
            let (row, col) = (i / 5, i % 5);
            let right = 16.0 + 92.0 + col as f32 * 132.0;
            let cy = y + 16.0 + row as f32 * 44.0;
            self.toggle_switch(right, cy, *on, *at, *lit, p)?;
            if i == 9 {
                let track = Rect::new(right - 40.0, cy - 10.0, 40.0, 20.0);
                self.focus_visual(&track, 10.0, p);
            }
        }
        Ok(())
    }

    /// RadioButtons, ProgressBars and the scroll indicator.
    fn page_radios(&self, w: f32, p: &FluentPalette) -> Result<()> {
        self.heading("RadioButtons", 32.0, p)?;
        let labels = ["10 MB", "25 MB", "50 MB"];
        let columns: [[(bool, Lit); 3]; 4] = [
            [(false, Lit::REST), (true, Lit::REST), (false, Lit::REST)],
            [
                (false, Lit::HOVER),
                (true, Lit::HOVER),
                (false, Lit::PRESSED),
            ],
            [
                (true, Lit::PRESSED),
                (false, Lit::DISABLED),
                (true, Lit::DISABLED),
            ],
            [(false, Lit::REST), (true, Lit::REST), (false, Lit::REST)],
        ];
        for (c, col) in columns.iter().enumerate() {
            let rows = kit::radio_rows(16.0 + c as f32 * 170.0, 56.0, &[60.0; 3]);
            for (k, ((on, lit), parts)) in col.iter().zip(&rows).enumerate() {
                self.radio(parts, labels[k], *on, *lit, p)?;
                if c == 3 && k == 1 {
                    self.focus_visual(&parts.row, radius::CONTROL, p);
                }
            }
        }
        let mut y = 176.0;
        self.heading("ProgressBar: 0, 42, 100, running, still", y, p)?;
        y += 32.0;
        let bars = [
            (0.0, 0.0),
            (0.0, 0.42),
            (0.0, 1.0),
            anim::indeterminate(700.0, true),
            anim::indeterminate(0.0, false),
        ];
        for (i, span) in bars.iter().enumerate() {
            let r = Rect::new(16.0, y + i as f32 * 20.0, 320.0, 3.0);
            self.progress_bar(&r, *span, p);
        }
        // Two lists: the indicator at rest, and under the pointer.
        let top = 176.0;
        for (i, hover) in [0.0, 1.0].iter().enumerate() {
            let list = Rect::new(w - 16.0 - (2 - i) as f32 * 132.0, top, 120.0, 200.0);
            self.tint(&list, radius::LAYER, p.layer.mica);
            self.stroke(&list, radius::LAYER, p.card_stroke);
            self.scroll_indicator(&list, 200.0, 600.0, 150.0, *hover, 1.0, p);
        }
        Ok(())
    }

    /// InfoBars of each severity, and one that wraps with its actions.
    fn page_info(&self, w: f32, p: &FluentPalette) -> Result<()> {
        self.heading("InfoBar", 32.0, p)?;
        let body = Style::new(Ramp::Body);
        let strong = Style::new(Ramp::BodyStrong);
        let wrapped = |m: &str, width: f32| -> f32 {
            self.text
                .styled(m, body.wrapping(), self.zoom, width, 1000.0)
                .ok()
                .map_or(20.0, |laid| {
                    let mut tm = DWRITE_TEXT_METRICS::default();
                    // SAFETY: a plain read of a layout this thread made.
                    let _ = unsafe { laid.layout.GetMetrics(&mut tm) };
                    tm.height
                })
        };
        let bars = [
            (Severity::Info, "Kiln", "These files don't share a format."),
            (
                Severity::Success,
                "Saved",
                "photo.png, next to the original.",
            ),
            (
                Severity::Caution,
                "Needs FFmpeg",
                "Video works once FFmpeg is found.",
            ),
            (
                Severity::Critical,
                "Couldn't read this PDF",
                "It looks damaged.",
            ),
        ];
        let mut y = 60.0;
        let width = (w - 32.0).min(720.0);
        for (sev, title, msg) in bars {
            let (tw, mw) = (self.run(title, strong)?, self.run(msg, body)?);
            let l = kit::info_bar(16.0, y, width, tw, mw, &|x| wrapped(msg, x), &[], true);
            self.info_bar(&l, sev, title, msg, &[], Lit::REST, p)?;
            y = l.rect.bottom() + 8.0;
        }
        let msg = "It looks damaged, or isn't really a PDF. Try opening it in another app.";
        let title = "Couldn't read this PDF";
        let actions = [
            ("Show details", ButtonKind::Standard, Lit::REST),
            ("Remove from list", ButtonKind::Standard, Lit::HOVER),
        ];
        let aw: Vec<f32> = actions
            .iter()
            .map(|(a, _, _)| self.run(a, body).map(|x| kit::button_width(x, false)))
            .collect::<Result<_>>()?;
        let (tw, mw) = (self.run(title, strong)?, self.run(msg, body)?);
        let l = kit::info_bar(
            16.0,
            y + 8.0,
            360.0,
            tw,
            mw,
            &|x| wrapped(msg, x),
            &aw,
            true,
        );
        self.info_bar(&l, Severity::Critical, title, msg, &actions, Lit::HOVER, p)?;
        Ok(())
    }

    /// ComboBoxes closed in each state and one open, and text boxes.
    fn page_combos(&self, w: f32, h: f32, p: &FluentPalette) -> Result<()> {
        self.heading("ComboBox", 32.0, p)?;
        let choices = [
            "Light",
            "Dark",
            "Use Windows' setting",
            "A choice too long to fit in the box",
        ];
        let mut states: Vec<(&str, Lit)> = STATES.to_vec();
        states.push(("Focused", Lit::REST));
        for (i, (_, lit)) in states.iter().enumerate() {
            let (row, col) = (i / 3, i % 3);
            let r = Rect::new(
                16.0 + col as f32 * 176.0,
                60.0 + row as f32 * 44.0,
                160.0,
                32.0,
            );
            self.combo_box(&r, choices[i % choices.len()], *lit, p)?;
            if i == 4 {
                self.focus_visual(&r, radius::CONTROL, p);
            }
        }
        // One open, its value (Use Windows' setting) over the box and the
        // highlight on Dark.
        let anchor = Rect::new(w - 16.0 - 200.0, 104.0, 200.0, 32.0);
        let mut c = ComboState::new(choices.len(), 2);
        c.click();
        c.highlight = 1;
        let bounds = Rect::new(0.0, 0.0, w, h);
        let list = kit::combo_flyout(anchor, choices.len(), c.value, bounds);
        self.combo_box(&anchor, choices[c.value], Lit::PRESSED, p)?;
        self.combo_list(&list, &choices, &[], c.value, c.highlight, p)?;

        let mut y = 172.0;
        self.heading("Text box: rest, hover, focused, selected, disabled", y, p)?;
        y += 28.0;
        let mut sel = TextBoxState::new("2048", 6, true);
        sel.key(crate::controls::Press::shift(crate::controls::Key::Left));
        sel.key(crate::controls::Press::shift(crate::controls::Key::Left));
        let boxes = [
            (TextBoxState::new("15", 6, true), false, Lit::REST),
            (TextBoxState::new("15", 6, true), false, Lit::HOVER),
            (TextBoxState::new("150", 6, true), true, Lit::REST),
            (sel, true, Lit::REST),
            (TextBoxState::new("25", 6, true), false, Lit::DISABLED),
            (TextBoxState::new("", 6, true), false, Lit::REST),
        ];
        for (i, (t, focused, lit)) in boxes.iter().enumerate() {
            let r = Rect::new(16.0 + i as f32 * 100.0, y, 88.0, 32.0);
            self.text_box(&r, t, Some("e.g. 5"), *focused, *lit, p)?;
        }
        Ok(())
    }

    /// Expanders closed, lit, turning and open, and a menu flyout.
    fn page_expanders(&self, w: f32, h: f32, p: &FluentPalette) -> Result<()> {
        self.heading("Expander", 32.0, p)?;
        let ew = 360.0;
        let shut = kit::expander_parts(16.0, 60.0, ew, 0.0, 0.0);
        self.expander(&shut, "More options", 0.0, Lit::REST, p)?;
        let lit = kit::expander_parts(16.0, 116.0, ew, 0.0, 0.0);
        self.expander(&lit, "More options (hover)", 0.0, Lit::HOVER, p)?;
        self.focus_visual(&lit.header, radius::CONTROL, p);
        let turning = kit::expander_parts(16.0, 172.0, ew, 48.0, 0.5);
        self.expander(
            &turning,
            "Halfway",
            anim::chevron(true, 30.0, true),
            Lit::REST,
            p,
        )?;
        let open = kit::expander_parts(16.0, 252.0, ew, 96.0, 1.0);
        self.expander(
            &open,
            "Open",
            anim::chevron(true, 167.0, true),
            Lit::REST,
            p,
        )?;
        if let Some(c) = open.content {
            let label = Rect::new(c.x + 16.0, c.y + 12.0, 200.0, 20.0);
            self.styled(
                "Remove location data",
                Style::new(Ramp::Body),
                &label,
                p.text,
            )?;
            self.toggle_switch(c.right() - 16.0, c.y + 22.0, true, 1.0, Lit::REST, p)?;
            let label = Rect::new(c.x + 16.0, c.y + 56.0, 200.0, 20.0);
            self.styled("Resize photos", Style::new(Ramp::Body), &label, p.text)?;
            self.toggle_switch(c.right() - 16.0, c.y + 66.0, false, 0.0, Lit::REST, p)?;
        }
        self.styled(
            "Menu flyout",
            Style::new(Ramp::BodyStrong),
            &Rect::new(w - 16.0 - 260.0, 32.0, 260.0, 20.0),
            p.text,
        )?;
        let lines = [
            MenuLine::Item {
                label: "Show in folder",
                glyph: Some(glyph::SHOW_IN_FOLDER),
                shortcut: None,
                enabled: true,
            },
            MenuLine::Item {
                label: "Copy path",
                glyph: Some(glyph::COPY_PATH),
                shortcut: Some("Ctrl+Shift+C"),
                enabled: true,
            },
            MenuLine::Separator,
            MenuLine::Item {
                label: "Retry",
                glyph: Some(glyph::RETRY),
                shortcut: None,
                enabled: false,
            },
            MenuLine::Item {
                label: "Remove from list",
                glyph: Some(glyph::REMOVE),
                shortcut: Some("Delete"),
                enabled: true,
            },
        ];
        let caption = Style::new(Ramp::Caption);
        let body = Style::new(Ramp::Body);
        let rows = lines
            .iter()
            .map(|l| match *l {
                MenuLine::Item {
                    label,
                    glyph,
                    shortcut,
                    ..
                } => Ok(MenuRow::Item {
                    label: self.run(label, body)?,
                    accel: shortcut.map_or(Ok(0.0), |s| self.run(s, caption))?,
                    glyph: glyph.is_some(),
                }),
                MenuLine::Separator => Ok(MenuRow::Separator),
            })
            .collect::<Result<Vec<_>>>()?;
        let at = (w - 16.0 - 260.0, 60.0);
        let l = kit::menu_flyout(at, &rows, Rect::new(0.0, 0.0, w, h));
        self.menu_flyout(&l, &lines, Some(1), p)?;
        Ok(())
    }

    /// The ContentDialog's frame over smoke, with its buttons.
    fn page_dialog(&self, w: f32, h: f32, p: &FluentPalette) -> Result<()> {
        self.smoke(w, h, 1.0, p);
        let body = "Kiln has results that aren't saved yet. Save them next to the originals, or let them go.";
        let content_w = kit::DIALOG_WIDTH.min(w - 48.0) - 48.0;
        let content_h = self
            .text
            .styled(
                body,
                Style::new(Ramp::Body).wrapping(),
                self.zoom,
                content_w,
                1000.0,
            )
            .ok()
            .map_or(40.0, |laid| {
                let mut tm = DWRITE_TEXT_METRICS::default();
                // SAFETY: a plain read of a layout this thread made.
                let _ = unsafe { laid.layout.GetMetrics(&mut tm) };
                tm.height
            });
        let l = kit::dialog((w, h), 28.0, content_h, 3);
        self.dialog_frame(&l, "Save before closing?", p)?;
        self.styled(body, Style::new(Ramp::Body).wrapping(), &l.content, p.text)?;
        let buttons = [
            ("Save all", ButtonKind::Accent),
            ("Discard", ButtonKind::Standard),
            ("Cancel", ButtonKind::Standard),
        ];
        for (i, ((label, kind), r)) in buttons.iter().zip(&l.buttons).enumerate() {
            self.fl_button(*kind, r, label, None, Lit::REST, p)?;
            if i == 0 {
                self.focus_visual(r, radius::CONTROL, p);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_debug_build_draws_the_gallery_only_when_asked() {
        let asked = std::env::var("KILN_TEST_CONTROLS_PAGE")
            .ok()
            .and_then(|v| v.parse::<u8>().ok())
            .filter(|n| (1..=super::PAGES).contains(n));
        assert_eq!(super::page(), asked);
    }
}
