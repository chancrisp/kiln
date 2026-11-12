//! The credit line at the main window's bottom left: "Made with ❤️ by
//! Chandler Crisp", the name a link to the author's GitHub page. In Caption
//! and the secondary text colour, under the footer's status while files
//! are listed, and in the empty window's bottom-left corner. Where there
//! isn't room for all of it beside the controls, it isn't shown.

use crate::layout::{EmptyState, FooterRow, Rect, FOOTER_PAD};

/// The words before the heart.
pub const MADE: &str = "Made with ";
/// The heart: the red heart emoji, drawn in colour where Windows can.
pub const HEART: &str = "\u{2764}\u{FE0F}";
/// The heart where there is no colour emoji font: a plain glyph, drawn red.
pub const HEART_PLAIN: &str = "\u{2665}";
/// The words between the heart and the name.
pub const BY: &str = " by ";
/// The name, the link.
pub const NAME: &str = "Chandler Crisp";
/// Where the name goes.
pub const PAGE: &str = "https://github.com/chancrisp";
/// What the link says it does, as its tooltip and its help text.
pub const LINK_HELP: &str = "Opens github.com/chancrisp";
/// What the status says when the page didn't open.
pub const DIDNT_OPEN: &str = "Couldn't open the page.";
/// The words before the link, as a screen reader reads them.
pub const LEAD: &str = "Made with \u{2764}\u{FE0F} by";

/// The credit's line: one Caption line centred in a box this tall, which
/// is also how tall the link is to point at.
pub const LINE: f32 = 24.0;
/// The status's line above the credit in the footer, the same height:
/// the two lines are centred in the footer together.
const FOOTER_TOP: f32 = 8.0;
/// Room kept between the credit and the empty window's group above it.
const EMPTY_GAP: f32 = 8.0;
/// The credit's room from the empty window's bottom edge: its words
/// centred as far from the edge as they are from the footer's foot.
const EMPTY_BOTTOM: f32 = 8.0;

/// How wide each part runs, as measured in Caption.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Widths {
    pub made: f32,
    pub heart: f32,
    pub by: f32,
    pub name: f32,
}

impl Widths {
    /// A guess at Caption's widths, for a tree or a test without a renderer.
    pub const GUESS: Widths = Widths {
        made: 58.0,
        heart: 16.0,
        by: 18.0,
        name: 82.0,
    };

    pub fn total(&self) -> f32 {
        self.made + self.heart + self.by + self.name
    }

    fn sane(&self) -> bool {
        [self.made, self.heart, self.by, self.name]
            .iter()
            .all(|w| w.is_finite() && *w >= 0.0)
    }
}

/// Where the credit's parts are.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Credit {
    /// The whole line.
    pub line: Rect,
    pub made: Rect,
    pub heart: Rect,
    pub by: Rect,
    /// The name: the link, what is pointed at, pressed and focused.
    pub name: Rect,
}

/// The credit on a line starting at `x`, `y`, [`LINE`] tall.
pub fn at(x: f32, y: f32, w: Widths) -> Credit {
    let part = |x: f32, w: f32| Rect::new(x, y, w.ceil(), LINE);
    let made = part(x, w.made);
    let heart = part(made.right(), w.heart);
    let by = part(heart.right(), w.by);
    // A hair wider than the run, so its last letter is never trimmed.
    let name = part(by.right(), w.name + 1.0);
    Credit {
        line: Rect::new(x, y, name.right() - x, LINE),
        made,
        heart,
        by,
        name,
    }
}

/// The footer's row with the credit under the status: the status takes
/// the line above it, the two centred in the footer. Where the status is
/// too narrow for all of the credit, or the footer too short for two
/// lines, the row is as it was, with no credit.
pub fn in_footer(row: FooterRow, w: Widths) -> FooterRow {
    let s = row.status;
    if !w.sane() || w.total() + 1.0 > s.w || s.h < FOOTER_TOP + 2.0 * LINE {
        return FooterRow {
            credit: None,
            ..row
        };
    }
    let status = Rect::new(s.x, s.y + FOOTER_TOP, s.w, LINE);
    FooterRow {
        status,
        credit: Some(at(s.x, status.bottom(), w)),
        ..row
    }
}

/// The credit in the empty window's bottom-left corner, as far in from
/// the side as the footer's status, while it clears the group above it
/// (the status line under Add files, or the first-run card) and the
/// window is wide enough for all of it.
pub fn in_empty(e: &EmptyState, w: Widths) -> Option<Credit> {
    if !w.sane() {
        return None;
    }
    let layer = e.layer;
    let x = layer.x + FOOTER_PAD;
    let y = layer.bottom() - EMPTY_BOTTOM - LINE;
    let above = e.card.map_or(e.status.bottom(), |c| c.card.bottom());
    let fits = x + w.total() + 1.0 <= layer.right() - FOOTER_PAD && above + EMPTY_GAP <= y;
    fits.then(|| at(x, y, w))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{self, CardText};

    fn row(w: f32) -> FooterRow {
        let footer = Rect::new(0.0, 500.0, w, layout::FOOTER_HEIGHT);
        layout::footer_row(&footer, false)
    }

    #[test]
    fn the_credit_says_made_with_a_heart_by_the_author() {
        assert_eq!(
            format!("{MADE}{HEART}{BY}{NAME}"),
            "Made with ❤️ by Chandler Crisp"
        );
        assert_eq!(PAGE, "https://github.com/chancrisp");
        assert_eq!(LEAD, "Made with ❤️ by");
    }

    #[test]
    fn the_parts_run_left_to_right_with_the_name_last() {
        let c = at(16.0, 40.0, Widths::GUESS);
        assert_eq!(c.made.x, 16.0);
        assert_eq!(c.heart.x, c.made.right());
        assert_eq!(c.by.x, c.heart.right());
        assert_eq!(c.name.x, c.by.right());
        assert_eq!(c.line.right(), c.name.right());
        for r in [c.line, c.made, c.heart, c.by, c.name] {
            assert_eq!((r.y, r.h), (40.0, LINE));
        }
    }

    #[test]
    fn in_the_footer_it_sits_under_the_status_both_centred() {
        let before = row(880.0);
        let after = in_footer(before, Widths::GUESS);
        let c = after.credit.expect("room for it");
        let foot = Rect::new(0.0, 500.0, 880.0, layout::FOOTER_HEIGHT);
        // The status above, the credit under it, at the same left edge.
        assert_eq!(after.status.x, before.status.x);
        assert_eq!(c.line.x, before.status.x);
        assert_eq!(c.line.y, after.status.bottom());
        // Together centred in the footer.
        let top = after.status.y - foot.y;
        let bottom = foot.bottom() - c.line.bottom();
        assert_eq!(top, bottom);
        // The buttons haven't moved, and the credit stays clear of them.
        assert_eq!(
            (after.add, after.clear, after.convert),
            (before.add, before.clear, before.convert)
        );
        assert!(c.line.right() < after.add.x);
    }

    #[test]
    fn in_a_narrow_footer_it_gives_way_to_the_controls() {
        // A footer too narrow for the status beside the buttons.
        let narrow = row(layout::MIN_WINDOW.0 - 40.0);
        let got = in_footer(narrow, Widths::GUESS);
        if Widths::GUESS.total() + 1.0 > narrow.status.w {
            assert_eq!(got.credit, None);
            assert_eq!(got.status, narrow.status);
        }
        // However narrow, it never reaches the buttons.
        for w in (300..1600).step_by(20) {
            let r = in_footer(row(w as f32), Widths::GUESS);
            if let Some(c) = r.credit {
                assert!(c.line.right() < r.add.x, "{w}");
            }
        }
    }

    #[test]
    fn widths_that_are_not_numbers_show_no_credit() {
        let bad = Widths {
            name: f32::NAN,
            ..Widths::GUESS
        };
        assert_eq!(in_footer(row(880.0), bad).credit, None);
    }

    #[test]
    fn in_the_empty_window_it_sits_in_the_bottom_left_corner() {
        let layer = layout::content_layer(880.0, 600.0, layout::TITLE_BAR);
        let e = layout::empty_state(&layer, None);
        let c = in_empty(&e, Widths::GUESS).expect("room for it");
        assert_eq!(c.line.x, layer.x + FOOTER_PAD);
        assert_eq!(layer.bottom() - c.line.bottom(), EMPTY_BOTTOM);
        assert!(c.line.y >= e.status.bottom() + EMPTY_GAP);
        // With the first-run card, still under it.
        let e = layout::empty_state(&layer, Some(CardText::ONE_LINE));
        let c = in_empty(&e, Widths::GUESS).expect("room under the card");
        assert!(c.line.y >= e.card.unwrap().card.bottom() + EMPTY_GAP);
    }

    #[test]
    fn in_a_short_empty_window_it_gives_way_to_the_group() {
        // The smallest window, the first-run card running past its foot.
        let (w, h) = layout::MIN_WINDOW;
        let layer = layout::content_layer(w, h, layout::TITLE_BAR);
        let e = layout::empty_state(&layer, Some(CardText::ONE_LINE));
        let above = e.card.unwrap().card.bottom();
        match in_empty(&e, Widths::GUESS) {
            Some(c) => assert!(c.line.y >= above + EMPTY_GAP),
            None => assert!(above + EMPTY_GAP > layer.bottom() - EMPTY_BOTTOM - LINE),
        }
    }
}
