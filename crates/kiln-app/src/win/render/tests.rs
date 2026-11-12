//! The pieces of partial redraws that need no device: which pixels a
//! frame of changed rows presents.

use super::*;

#[test]
fn dirty_rectangles_are_whole_pixels_cut_to_the_client_area() {
    // At 150%, a DIP is a pixel and a half: edges grow outwards.
    let rects = [Rect::new(10.0, 10.0, 11.0, 5.0)];
    assert_eq!(
        dirty_pixels(&rects, 144.0, (800, 600)),
        vec![RECT {
            left: 15,
            top: 15,
            right: 32,
            bottom: 23
        }]
    );
    // The buffers are bigger than the client: nothing past it is shown.
    let edge = [Rect::new(700.0, 500.0, 300.0, 300.0)];
    assert_eq!(
        dirty_pixels(&edge, 96.0, (800, 600)),
        vec![RECT {
            left: 700,
            top: 500,
            right: 800,
            bottom: 600
        }]
    );
    // And nothing before it.
    let before = [Rect::new(-20.0, -20.0, 30.0, 30.0)];
    assert_eq!(
        dirty_pixels(&before, 96.0, (800, 600)),
        vec![RECT {
            left: 0,
            top: 0,
            right: 10,
            bottom: 10
        }]
    );
}

#[test]
fn a_rectangle_outside_the_client_area_or_empty_is_left_out() {
    let rects = [
        Rect::new(900.0, 10.0, 50.0, 50.0),
        Rect::new(10.0, 700.0, 50.0, 50.0),
        Rect::new(10.0, 10.0, 0.0, 50.0),
        Rect::new(10.0, 10.0, 50.0, 20.0),
    ];
    assert_eq!(
        dirty_pixels(&rects, 96.0, (800, 600)),
        vec![RECT {
            left: 10,
            top: 10,
            right: 60,
            bottom: 30
        }]
    );
    assert!(dirty_pixels(&[], 96.0, (800, 600)).is_empty());
}

#[test]
fn the_drawn_area_is_the_bounds_of_the_rectangles() {
    let a = RECT {
        left: 10,
        top: 100,
        right: 50,
        bottom: 120,
    };
    let b = RECT {
        left: 20,
        top: 10,
        right: 90,
        bottom: 30,
    };
    assert_eq!(
        bounds_of(&[a, b]),
        RECT {
            left: 10,
            top: 10,
            right: 90,
            bottom: 120
        }
    );
    assert_eq!(bounds_of(&[a]), a);
}

#[test]
fn rectangles_overlap_where_both_are() {
    let panel = Rect::new(0.0, 0.0, 100.0, 100.0);
    let row = Rect::new(10.0, 80.0, 200.0, 56.0);
    let at = overlap(&row, &panel).unwrap();
    assert_eq!((at.x, at.y, at.w, at.h), (10.0, 80.0, 90.0, 20.0));
    // A row scrolled out of the panel has nothing to draw.
    assert!(overlap(&Rect::new(0.0, 120.0, 50.0, 56.0), &panel).is_none());
    // One that only touches has none either.
    assert!(overlap(&Rect::new(0.0, 100.0, 50.0, 56.0), &panel).is_none());
}

#[test]
fn the_missing_helper_notice_fits_the_footer_of_the_default_window() {
    // D38: at the default 880 x 600, the footer's notice that a helper is
    // missing was cut short ("...Use Get LibreOffice to...").
    let text = crate::win::text::Text::new().unwrap();
    let l = layout::layout_under(880.0, 600.0, 1, 1.0, 48.0);
    let row = layout::footer_row(&l.footer.unwrap(), false);
    for backend in ["libreoffice", "pandoc", "calibre", "ffmpeg", "audio-mf"] {
        let said = crate::model::helper_notice(backend);
        let w = super::list::line_width(&text, &said, 1.0);
        assert!(w <= row.status.w, "{said}: {w} > {}", row.status.w);
    }
}
