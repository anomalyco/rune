//! Clearing a CPU frame's rounded corners: the pixels a frame painted, outside the arcs.

use super::{Rounding, clear_corners};

const W: u32 = 64;
const H: u32 = 48;
const STRIDE: usize = W as usize * 4 + 16;

fn frame() -> Vec<u8> { vec![200; STRIDE * H as usize] }

fn pixel(bytes: &[u8], x: u32, y: u32) -> [u8; 4] {
    let at = y as usize * STRIDE + x as usize * 4;
    [bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]
}

const ALL: [[u32; 4]; 1] = [[0, 0, W, H]];

#[test]
fn square_corners_change_nothing() {
    let mut bytes = frame();
    clear_corners(&mut bytes, STRIDE, (W, H), Rounding::bottom(0.), &ALL);
    assert_eq!(bytes, frame());
}

#[test]
fn outside_the_arcs_composites_nothing_and_inside_keeps_its_pixels() {
    let mut bytes = frame();
    clear_corners(&mut bytes, STRIDE, (W, H), Rounding::bottom(12.), &ALL);
    // The two bottom corner pixels are outside their arcs by more than a pixel.
    assert_eq!(pixel(&bytes, 0, H - 1), [0; 4]);
    assert_eq!(pixel(&bytes, W - 1, H - 1), [0; 4]);
    // On the edges where the arcs meet them, and everywhere inside, nothing changes.
    for (x, y) in [(12, H - 1), (0, H - 13), (W - 13, H - 1), (W - 1, H - 13), (6, H - 6), (W / 2, H - 1), (0, 0), (W - 1, 0)] {
        assert_eq!(pixel(&bytes, x, y), [200; 4], "({x}, {y})");
    }
    // The row padding past the frame's width is no pixel.
    assert!(bytes.chunks(STRIDE).all(|row| row[W as usize * 4..].iter().all(|&b| b == 200)));
}

#[test]
fn a_pixel_the_arc_crosses_keeps_part_of_colour_and_alpha_alike() {
    let mut bytes = frame();
    clear_corners(&mut bytes, STRIDE, (W, H), Rounding::bottom(12.), &ALL);
    let crossed: Vec<_> = (0..12).map(|x| pixel(&bytes, x, H - 1)).filter(|p| p[0] != 0 && p[0] != 200).collect();
    assert!(!crossed.is_empty());
    assert!(crossed.iter().all(|p| p.iter().all(|&b| b == p[0])));
}

#[test]
fn both_corners_mirror_each_other() {
    let mut bytes = frame();
    clear_corners(&mut bytes, STRIDE, (W, H), Rounding::bottom(12.), &ALL);
    for y in H - 12..H {
        for x in 0..12 {
            assert_eq!(pixel(&bytes, x, y), pixel(&bytes, W - 1 - x, y), "({x}, {y})");
        }
    }
}

#[test]
fn only_pixels_the_frame_painted_are_cleared_and_once() {
    let mut bytes = frame();
    // The frame repainted the left corner's tile alone.
    let left = [[0, H - 16, 16, H]];
    clear_corners(&mut bytes, STRIDE, (W, H), Rounding::bottom(12.), &left);
    assert_eq!(pixel(&bytes, 0, H - 1), [0; 4]);
    assert_eq!(pixel(&bytes, W - 1, H - 1), [200; 4]);
    // A later frame that repaints nothing leaves the cleared pixels as they are.
    let once = bytes.clone();
    clear_corners(&mut bytes, STRIDE, (W, H), Rounding::bottom(12.), &[]);
    assert_eq!(bytes, once);
}

#[test]
fn a_radius_between_pixels_clears_no_pixel_whose_centre_is_past_the_arcs_centre() {
    let mut bytes = frame();
    clear_corners(&mut bytes, STRIDE, (W, H), Rounding::bottom(7.5), &ALL);
    assert_eq!(pixel(&bytes, 0, H - 1), [0; 4]);
    assert_eq!(pixel(&bytes, 8, H - 1), [200; 4]);
    assert_eq!(pixel(&bytes, 0, H - 9), [200; 4]);
}

/// A frame without a headerbar: the top two corners clear as the bottom two do, each corner the
/// others' mirror, and the pixels between them stay.
#[test]
fn a_frame_without_a_headerbar_clears_the_top_corners_too() {
    let mut bytes = frame();
    clear_corners(&mut bytes, STRIDE, (W, H), Rounding { radius: 12., top: true }, &ALL);
    for (x, y) in [(0, 0), (W - 1, 0), (0, H - 1), (W - 1, H - 1)] {
        assert_eq!(pixel(&bytes, x, y), [0; 4], "({x}, {y})");
    }
    for (x, y) in [(12, 0), (0, 12), (W - 13, 0), (W - 1, 12), (6, 6), (W / 2, 0), (0, H / 2), (W / 2, H / 2)] {
        assert_eq!(pixel(&bytes, x, y), [200; 4], "({x}, {y})");
    }
    for y in 0..12 {
        for x in 0..12 {
            let top_left = pixel(&bytes, x, y);
            assert_eq!(top_left, pixel(&bytes, W - 1 - x, y), "({x}, {y}) and the top right");
            assert_eq!(top_left, pixel(&bytes, x, H - 1 - y), "({x}, {y}) and the bottom left");
            assert_eq!(top_left, pixel(&bytes, W - 1 - x, H - 1 - y), "({x}, {y}) and the bottom right");
        }
    }
    // Under a headerbar the top stays whole.
    let mut bottom = frame();
    clear_corners(&mut bottom, STRIDE, (W, H), Rounding::bottom(12.), &ALL);
    assert_eq!(pixel(&bottom, 0, 0), [200; 4]);
    assert_eq!(pixel(&bottom, W - 1, 0), [200; 4]);
}