//! The app icon's SVG master parses, whole and with its detail cut out,
//! and the cut really removes the grain and the hand-cut displacement.

use kiln_app::icons::without_blocks;

const MASTER: &str = include_str!("../assets/kiln-seal.svg");

fn parse(svg: &str) -> resvg::usvg::Tree {
    resvg::usvg::Tree::from_str(svg, &resvg::usvg::Options::default()).expect("the master parses")
}

#[test]
fn the_master_is_256_px_over_a_120_unit_viewbox_with_the_mark_in_it() {
    let tree = parse(MASTER);
    let size = tree.size();
    assert_eq!((size.width(), size.height()), (256.0, 256.0));
    assert!(MASTER.contains("viewBox=\"0 0 120 120\""));
    assert!(MASTER.contains("id=\"kflame\""));
    for name in ["<text", "<image", "href=\"http"] {
        assert!(!MASTER.contains(name), "the master must not need {name}");
    }
}

#[test]
fn markers_come_in_pairs() {
    for name in ["detail", "rough"] {
        let open = MASTER.matches(&format!("<!-- {name} -->")).count();
        let close = MASTER.matches(&format!("<!-- /{name} -->")).count();
        assert_eq!(open, close, "{name}");
        assert!(open >= 1, "{name}");
    }
}

#[test]
fn the_small_variant_keeps_the_relief_and_loses_the_grain() {
    let small = without_blocks(&without_blocks(MASTER, "detail"), "rough");
    parse(&small);
    for kept in [
        "url(#deboss)",
        "url(#edge)",
        "url(#glaze)",
        "url(#pool)",
        "url(#gloss)",
    ] {
        assert!(small.contains(kept), "{kept} stays at 48 px");
    }
    for cut in [
        "url(#grain)",
        "url(#mottle)",
        "url(#speckle)",
        "url(#roughK)",
    ] {
        assert!(!small.contains(cut), "{cut} goes at 48 px");
    }
    // The rough block removes only the group's open and close tags, so
    // the glazed K inside it is still there.
    assert_eq!(small.matches("xlink:href=\"#kflame\"").count(), 3);
}
