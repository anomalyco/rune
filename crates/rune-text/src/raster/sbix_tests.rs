use swash::scale::image::{Content, Image};

use super::mirror;

#[test]
fn mirror_reverses_each_row_and_reflects_placement_about_the_advance() {
    let mut image = Image::new();
    image.content = Content::Color;
    image.placement.left = 2;
    image.placement.width = 3;
    image.placement.height = 2;
    image.data = (0..24).collect();
    mirror(&mut image, 10.0);
    let px = |i: u8| [i * 4, i * 4 + 1, i * 4 + 2, i * 4 + 3];
    let want: Vec<u8> = [2, 1, 0, 5, 4, 3].into_iter().flat_map(px).collect();
    assert_eq!(image.data, want);
    // Ink from 2..5 of a 10 px advance lands at 5..8.
    assert_eq!(image.placement.left, 5);
}
