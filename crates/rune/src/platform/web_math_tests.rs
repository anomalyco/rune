use super::*;

#[test]
fn a_phone_canvas_reported_in_css_pixels_gets_its_device_height() {
    // Chromium's emulated device scale reports the CSS box: 390 × 844 at 2× used to take the
    // width's fix and leave the height at 844, stretching the canvas 2× vertically.
    assert_eq!(fix_canvas_size((390.0, 844.0), (390, 844), (1200.0, 900.0), 2.0, false), (780, 1688));
}

#[test]
fn a_canvas_still_at_its_intrinsic_size_is_sized_to_its_box() {
    // The canvas exists at the HTML default 300 × 150 until `resumed` styles it; a frame that
    // races the browser's first ResizeObserver sees that size.
    assert_eq!(fix_canvas_size((393.0, 852.0), (300, 150), (1200.0, 900.0), 3.0, false), (1179, 2556));
}

#[test]
fn a_window_that_fits_its_content_keeps_the_height_it_measured() {
    // `fit_content_height`: the canvas is exactly its content, and the app set that height.
    assert_eq!(fix_canvas_size((600.0, 640.0), (600, 640), (1200.0, 900.0), 2.0, true).1, 640);
    assert_eq!(fix_canvas_size((600.0, 640.0), (300, 150), (1200.0, 900.0), 2.0, true).1, 150);
}

#[test]
fn a_canvas_with_no_box_yet_falls_back_to_the_spec() {
    assert_eq!(fix_canvas_size((0.0, 0.0), (0, 0), (1200.0, 900.0), 2.0, false), (2400, 1800));
    assert_eq!(fix_canvas_size((0.0, 0.0), (0, 0), (1200.0, 900.0), 1.0, true), (1200, 900));
}

#[test]
fn a_size_the_box_agrees_with_is_left_alone() {
    // The box's fractional width at 1.5× rounds to a pixel the browser may report ±1.
    assert_eq!(fix_canvas_size((1001.0, 700.0), (1502, 1050), (1200.0, 900.0), 1.5, false), (1502, 1050));
}
