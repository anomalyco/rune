use super::*;

#[test]
fn inline_delimiters_pick_text_or_display_style() {
    assert_eq!(inline_tex("$e^x$").as_deref(), Some("\\textstyle e^x"));
    assert_eq!(inline_tex("\\( z_i \\)").as_deref(), Some("\\textstyle z_i"));
    assert_eq!(inline_tex("$$\\frac{a}{b}$$").as_deref(), Some("\\frac{a}{b}"));
    assert_eq!(inline_tex("$ $"), None);
    assert_eq!(inline_tex("e^x"), None);
}

#[test]
fn documents_with_any_delimiter_may_have_math() {
    assert!(may_have_math("costs $e^x$"));
    assert!(may_have_math("a \\(x\\) b"));
    assert!(may_have_math("\\[x\\]"));
    assert!(may_have_math("~~~math\nx\n~~~"));
    assert!(!may_have_math("plain text, no math"));
}

#[cfg(not(target_arch = "wasm32"))]
fn raster(tex: &str) -> raster::Rasterized {
    let bytes = eval_wire_for_proof(tex).expect("renders");
    let dl = wire::decode(&bytes).expect("decodes");
    raster::rasterize(&dl, 14.0, Color::WHITE, 2.0).expect("rasterizes")
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn inline_math_stands_on_a_baseline_inside_its_bitmap_and_text_style_is_shorter() {
    for tex in ["\\textstyle e^x", "\\textstyle z_i", "\\textstyle \\frac{e^{z_i}}{\\sum_j e^{z_j}}"] {
        let r = raster(tex);
        assert!(r.baseline_pt > r.margin_pt && r.baseline_pt < r.height_pt - r.margin_pt, "{tex}: {} in {}", r.baseline_pt, r.height_pt);
    }
    // A subscript hangs below the baseline; a superscript doesn't.
    let sub = raster("\\textstyle z_i");
    let sup = raster("\\textstyle e^x");
    assert!(sub.height_pt - sub.baseline_pt > sup.height_pt - sup.baseline_pt);
    let text = raster("\\textstyle \\frac{a}{b}");
    let display = raster("\\frac{a}{b}");
    assert!(text.height_pt < display.height_pt, "{} vs {}", text.height_pt, display.height_pt);
}
