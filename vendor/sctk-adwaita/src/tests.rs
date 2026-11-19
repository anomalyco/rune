//! The frame's rounded bottom corners: the radius each state gets, the border along the arcs, the
//! shadow the arcs cut out of the part that reaches over the window, and the bottom part's input
//! zone. Everything here draws into a `Pixmap` the way `redraw_inner` does, so it needs no
//! compositor.

use crate::parts::{bottom_input_rect, DecorationParts};
use crate::theme::{ColorTheme, BORDER_SIZE, CORNER_RADIUS, HEADER_SIZE, RESIZE_HANDLE_SIZE, VISIBLE_BORDER_SIZE};
use crate::{
    border_radius, bottom_border_shape, content_radius, draw_bottom, draw_side, CONTENT_CORNER_RADIUS,
};
use smithay_client_toolkit::reexports::csd_frame::WindowState;
use tiny_skia::{FillRule, Pixmap, PixmapPaint, Transform};

/// A pixel's alpha.
fn alpha(pixmap: &Pixmap, x: u32, y: u32) -> u8 {
    pixmap
        .pixel(x, y)
        .unwrap_or_else(|| panic!("({x}, {y}) is outside {}x{}", pixmap.width(), pixmap.height()))
        .alpha()
}

/// Asserts the pixel is the light theme's border colour, within the antialiasing of the arcs, and
/// covered at least `min_alpha` of the way (the band is a point wide, so a pixel on the diagonal
/// is only part of it).
fn assert_border(pixmap: &Pixmap, x: u32, y: u32, min_alpha: u8, what: &str) {
    let pixel = pixmap.pixel(x, y).expect("inside the pixmap").demultiply();
    let got = [pixel.red(), pixel.green(), pixel.blue()];
    let want = [220, 220, 220];
    assert!(
        got.iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 4),
        "{what}: {got:?} at ({x}, {y}) is not the border colour {want:?}",
    );
    assert!(pixel.alpha() >= min_alpha, "{what}: the band covers {}, not {min_alpha}", pixel.alpha());
}

/// The bottom part's pixmap as a window `width` points wide draws it: the shadow, the arcs cleared
/// out of it, and the border along them.
fn bottom_part(width: u32, state: WindowState) -> Pixmap {
    let surface = (width + 2 * BORDER_SIZE, BORDER_SIZE + CORNER_RADIUS);
    let mut pixmap = Pixmap::new(surface.0, surface.1).expect("a pixmap");
    draw_bottom(
        &mut pixmap.as_mut(),
        &mut Default::default(),
        &ColorTheme::light().active,
        &state,
        1,
        surface.0,
    );
    pixmap
}

/// The frame's radius follows the window's state: round while it floats, square when the window
/// fills the screen or is tiled, and square in fullscreen, where the frame hides its borders.
#[test]
fn the_radius_follows_the_window_state() {
    assert_eq!(content_radius(WindowState::empty()), CONTENT_CORNER_RADIUS as f32);
    assert_eq!(border_radius(WindowState::empty()), CORNER_RADIUS as f32);
    // An active window rounds the same as an inactive one.
    assert_eq!(content_radius(WindowState::ACTIVATED), CONTENT_CORNER_RADIUS as f32);
    for square in [
        WindowState::MAXIMIZED,
        WindowState::TILED,
        WindowState::FULLSCREEN,
        WindowState::MAXIMIZED | WindowState::ACTIVATED,
        WindowState::TILED | WindowState::FULLSCREEN,
    ] {
        assert_eq!(content_radius(square), 0., "{square:?} squares the content");
        assert_eq!(border_radius(square), 0., "{square:?} squares the border");
    }
}

/// The border is a one-point band along the arc the content leaves: the content's side of it is
/// clear, the band is the border colour, and the shadow's side of it is the shadow's.
#[test]
fn the_border_follows_the_bottom_arcs() {
    let (left, right, bottom, border, radius) = (43., 243., 10., 1., 10.);
    let shape = bottom_border_shape(left, right, bottom, border, radius).expect("a border shape");
    let mut pixmap = Pixmap::new(288, 54).expect("a pixmap");
    pixmap
        .as_mut()
        .fill_path(&shape, &ColorTheme::light().active.border_paint(), FillRule::EvenOdd, Transform::identity(), None);

    // (46, 7) is 9.2 from the arc's centre (53, 1): inside the one-point band.
    assert_border(&pixmap, 46, 7, 150, "the band on the arc");
    // (50, 5) is 5 from the centre: the content's own corner, which the band leaves clear.
    assert_eq!(alpha(&pixmap, 50, 5), 0, "inside the arc is the content's");
    // The straight band runs the window's whole width, just under its bottom edge.
    assert_border(&pixmap, 100, 10, 250, "the bottom band between the arcs");
    // Nothing is drawn below the band: that is the shadow's and the window's outside.
    assert_eq!(alpha(&pixmap, 100, 12), 0, "below the band the shadow draws");
}

/// The shadow below the window's edge is the square frame's, pixel for pixel: round corners only
/// add the crescents above that edge, and nothing else about the shadow moves.
#[test]
fn a_round_frames_shadow_below_the_window_is_the_square_frames() {
    let round = bottom_part(200, WindowState::empty());
    let square = bottom_part(200, WindowState::MAXIMIZED);
    // The window's bottom edge is a corner radius into the part; the one-point band is just below
    // it, and the shadow below the window starts under that.
    let below = (CORNER_RADIUS + VISIBLE_BORDER_SIZE) as u32;
    for y in below..round.height() {
        for x in 0..round.width() {
            let (rounded, squared) = (alpha(&round, x, y), alpha(&square, x, y));
            assert!(
                rounded.abs_diff(squared) <= 1,
                "({x}, {y}): round corners give {rounded}, the square frame {squared}",
            );
        }
    }
    // And the content's corner is left alone: inside the arc nothing is drawn.
    assert_eq!(alpha(&round, 50, 5), 0, "inside the arc is the content's");
}

/// The last corner column and the first plain one below the window's edge carry the same shadow: the
/// corner's crescent and the straight band meet without a step, at every depth.
#[test]
fn the_shadow_is_smooth_across_the_corner_and_plain_columns() {
    let pixmap = bottom_part(200, WindowState::empty());
    // The corner's columns end where the straight band's begin: `left` plus the corner radius.
    let plain = (BORDER_SIZE - VISIBLE_BORDER_SIZE + CORNER_RADIUS) as u32;
    let corner = plain - 1;
    for y in [11, 15, 20, 30, 40] {
        let (corner_alpha, plain_alpha) = (alpha(&pixmap, corner, y), alpha(&pixmap, plain, y));
        if y <= 30 {
            assert!(plain_alpha > 0, "({plain}, {y}) has the straight band's shadow");
        }
        assert!(
            corner_alpha.abs_diff(plain_alpha) <= 2,
            "row {y}: the corner column has {corner_alpha}, the plain one {plain_alpha}",
        );
    }
    // The crescent above the edge, outside the arc, keeps its shadow too.
    assert!(alpha(&pixmap, 45, 10) > 0, "the crescent outside the arc keeps its shadow");
    // And the mirror corner reads the same.
    let width = pixmap.width() as u32;
    for y in [11, 20, 40] {
        let (corner_alpha, plain_alpha) = (alpha(&pixmap, width - 1 - corner, y), alpha(&pixmap, width - 1 - plain, y));
        assert!(corner_alpha.abs_diff(plain_alpha) <= 2, "row {y} on the right");
    }
}

/// A square window (maximized, tiled or fullscreen) draws a straight band and no shadow over the
/// window at all: nothing is cut out of the content's corners.
#[test]
fn a_square_window_keeps_its_corners() {
    let pixmap = bottom_part(200, WindowState::MAXIMIZED);
    assert_border(&pixmap, 46, 10, 250, "the band straight across the corner");
    assert_eq!(alpha(&pixmap, 45, 9), 0, "no shadow over the window's corner");
    assert_eq!(alpha(&pixmap, 50, 5), 0, "nothing is cut out of the content");

    assert!(alpha(&pixmap, 45, 12) > 0, "the shadow below the window is still drawn");

    let tiled = bottom_part(200, WindowState::TILED);
    assert_border(&tiled, 46, 10, 250, "a tiled window's straight border");
}

/// The bottom part reaches a corner radius above the window's bottom edge, so its input zone — its
/// resize handle — starts at that edge instead, and never takes input from the content.
#[test]
fn the_bottom_input_zone_starts_at_the_windows_edge() {
    let surface = 200 + 2 * BORDER_SIZE;
    let input = bottom_input_rect(surface);
    assert_eq!(input.y, CORNER_RADIUS as i32, "the part's top is a corner radius above the edge");
    assert_eq!(input.height, RESIZE_HANDLE_SIZE);
    assert_eq!(input.width, surface - BORDER_SIZE * 2 + RESIZE_HANDLE_SIZE * 2);
    assert!(input.x >= BORDER_SIZE as i32 - RESIZE_HANDLE_SIZE as i32);
}

/// A window narrower than two corner radii still gets a border: the arcs are rounded as far as
/// they fit and nothing runs outside the part.
#[test]
fn a_narrow_window_still_gets_a_border() {
    let (left, right, bottom, border, radius) = (43., 53., 10., 1., 10.);
    let shape = bottom_border_shape(left, right, bottom, border, radius).expect("a border shape");
    let mut pixmap = Pixmap::new((right + left) as u32, 54).expect("a pixmap");
    pixmap
        .as_mut()
        .fill_path(&shape, &ColorTheme::light().active.border_paint(), FillRule::EvenOdd, Transform::identity(), None);
    assert_border(&pixmap, (left as u32 + right as u32) / 2, 10, 250, "the narrow window's band");
    // Degenerate shapes draw nothing rather than panicking.
    assert!(bottom_border_shape(43., 43., 10., 1., 10.).is_none(), "no width, no shape");
    assert!(bottom_border_shape(43., 53., 10., 0., 10.).is_none(), "no border, no shape");
    assert!(bottom_border_shape(f32::NAN, 53., 10., 1., 10.).is_none(), "not a number, no shape");
}

/// The side and bottom parts composited as the compositor stacks them (left and right below the
/// bottom one), over a window `width` points wide and `height` tall, with `state`. The canvas is in
/// window coordinates: its top left is the window's `(-BORDER_SIZE, -HEADER_SIZE)`, and every part
/// sits at its `surface_rect`.
fn composite(width: u32, height: u32, state: WindowState) -> Pixmap {
    let mut canvas = Pixmap::new(width + 2 * BORDER_SIZE, height + HEADER_SIZE + BORDER_SIZE).expect("a canvas");
    let colors = ColorTheme::light().active;
    let mut shadow = Default::default();
    let parts: [(bool, (u32, u32), (i32, i32), usize); 3] = [
        (false, (BORDER_SIZE, height + HEADER_SIZE), (-(BORDER_SIZE as i32), -(HEADER_SIZE as i32)), DecorationParts::LEFT),
        (true, (BORDER_SIZE, height + HEADER_SIZE), (width as i32, -(HEADER_SIZE as i32)), DecorationParts::RIGHT),
        (false, (width + 2 * BORDER_SIZE, BORDER_SIZE + CORNER_RADIUS), (-(BORDER_SIZE as i32), height as i32 - CORNER_RADIUS as i32), DecorationParts::BOTTOM),
    ];
    for (right, size, at, part) in parts {
        let mut pixmap = Pixmap::new(size.0, size.1).expect("a part");
        match part {
            DecorationParts::BOTTOM => draw_bottom(&mut pixmap.as_mut(), &mut shadow, &colors, &state, 1, size.0),
            _ => draw_side(&mut pixmap.as_mut(), &mut shadow, &colors, &state, 1, right),
        }
        let (x, y) = (at.0 - (-(BORDER_SIZE as i32)), at.1 - (-(HEADER_SIZE as i32)));
        canvas
            .as_mut()
            .draw_pixmap(x, y, pixmap.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    }
    canvas
}

/// A pixel's alpha in window coordinates, from a canvas `composite` drew.
fn window_alpha(canvas: &Pixmap, (x, y): (i32, i32)) -> u8 {
    let cx = (x + BORDER_SIZE as i32) as u32;
    let cy = (y + HEADER_SIZE as i32) as u32;
    alpha(canvas, cx, cy)
}

/// Whether `(x, y)` in window coordinates is inside one of the two corner squares the bottom
/// corners round: `x` in `[-1, r]` or `[width - r, width + 1]`, `y` in `[height - r, height]`.
fn in_corner_square(width: u32, height: u32, (x, y): (i32, i32)) -> bool {
    let r = CONTENT_CORNER_RADIUS as i32;
    let (width, height) = (width as i32, height as i32);
    y >= height - r && y <= height && (x >= -1 && x <= r || x >= width - r && x <= width + 1)
}

/// Beside the bottom corners the composite is a square frame's: the side parts already draw the
/// shadow down to the window's edge, and the bottom part adds nothing over it.
#[test]
fn the_side_and_bottom_parts_composite_like_a_square_frame() {
    let (width, height) = (200, 120);
    let round = composite(width, height, WindowState::empty());
    let square = composite(width, height, WindowState::MAXIMIZED);
    let mut compared = 0;
    let mut worst = (0, (0, 0), 0, 0);
    for cy in 0..round.height() {
        for cx in 0..round.width() {
            let at = (cx as i32 - BORDER_SIZE as i32, cy as i32 - HEADER_SIZE as i32);
            let (x, y) = at;
            // Outside the content rect, and not in a corner square, where the arcs' ring and the
            // shadow's crescents are the intended difference.
            let outside = x < 0 || x >= width as i32 || y < 0 || y >= height as i32;
            if !outside || in_corner_square(width, height, at) {
                continue;
            }
            let (rounded, squared) = (alpha(&round, cx, cy), alpha(&square, cx, cy));
            let off = rounded.abs_diff(squared);
            if off > worst.0 {
                worst = (off, at, rounded, squared);
            }
            compared += 1;
        }
    }
    let (off, at, rounded, squared) = worst;
    assert!(
        off <= 2,
        "the worst pixel is {at:?}: round corners composite {rounded}, the square frame {squared}",
    );
    assert!(compared > 10_000, "the whole frame around the content was compared ({compared})");
}

/// The crescent the bottom part adds inside a corner square meets its neighbours without a step:
/// the shadow on the other side of the window's edge (the side part's own, which a square frame
/// draws) and the straight band just below the edge carry the same shadow at those distances. The
/// bound is the shadow's own gradient over a pixel — a hole (0 against ~29) or a doubled copy
/// (~58) is far past it.
#[test]
fn the_crescent_continues_the_shadow_around_it() {
    let (width, height) = (200, 120);
    let canvas = composite(width, height, WindowState::empty());
    for corner in [(0, height as i32 - 1), (width as i32 - 1, height as i32 - 1)] {
        let crescent = window_alpha(&canvas, corner);
        assert!(crescent > 0, "{corner:?}: the crescent keeps its shadow");
        // Outward, across the window's side edge, and down, across its bottom edge.
        let outward = if corner.0 == 0 { (-1, corner.1) } else { (width as i32, corner.1) };
        let below = (corner.0, height as i32);
        for neighbour in [outward, below] {
            let other = window_alpha(&canvas, neighbour);
            assert!(
                crescent.abs_diff(other) <= 8,
                "{corner:?} has {crescent}, its neighbour {neighbour:?} {other}",
            );
        }
    }
}

/// The straight bands' shadow is the frame's own shadow model, which no patch touched: the
/// composite tests above compare their rounded frame against a square one drawn by the same code,
/// and this pins that reference to the model itself.
#[test]
fn the_straight_bands_shadow_is_the_shadow_model() {
    let (width, height) = (200, 120);
    let square = composite(width, height, WindowState::MAXIMIZED);
    for dy in [1, 3, 8, 20] {
        let y = height as i32 + dy;
        let want = (crate::shadow::shadow(dy as f32 - 0.5, 1, false) * 255.).round() as u8;
        let got = window_alpha(&square, (100, y));
        assert!(got.abs_diff(want) <= 2, "y {y}: the band draws {got}, the model says {want}");
    }
    // The rounded frame's own band, at the same depths, is the same.
    let round = composite(width, height, WindowState::empty());
    for dy in [1, 3, 8, 20] {
        let y = height as i32 + dy;
        assert_eq!(window_alpha(&round, (100, y)), window_alpha(&square, (100, y)), "y {y}");
    }
}
