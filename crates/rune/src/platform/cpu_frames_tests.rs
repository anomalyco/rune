use super::wayland::{Fill, Slots};
use super::{Put, bounds};

const A: [u32; 4] = [0, 0, 64, 32];
const B: [u32; 4] = [64, 32, 128, 64];

#[test]
fn bounds_is_the_rectangle_around_all_cut_to_the_frame() {
    assert_eq!(bounds(&[], (100, 100)), None);
    assert_eq!(bounds(&[A, B], (100, 100)), Some([0, 0, 100, 64]));
    assert_eq!(bounds(&[[120, 0, 160, 10]], (100, 100)), None);
}

#[test]
fn first_frame_paints_every_pixel_of_a_new_slot() {
    let mut slots = Slots::default();
    assert_eq!(slots.pick(), Some((0, Fill::Unknown)));
}

#[test]
fn compositor_that_releases_at_once_keeps_one_buffer_and_copies_nothing() {
    let mut slots = Slots::default();
    let (at, _) = slots.pick().unwrap();
    slots.committed(at, Put::All);
    for _ in 0..4 {
        slots.released(0);
        assert_eq!(slots.pick(), Some((0, Fill::Kept)));
        slots.committed(0, Put::Rects(&[A]));
    }
}

#[test]
fn held_buffer_is_never_painted_and_the_next_takes_what_it_missed() {
    let mut slots = Slots::default();
    let (first, _) = slots.pick().unwrap();
    slots.committed(first, Put::All);
    // The compositor still reads the first: a second buffer takes the whole frame from it.
    assert_eq!(slots.pick(), Some((1, Fill::All { from: 0 })));
    slots.committed(1, Put::Rects(&[A]));
    // It let go of the first, which lacks only what the second frame changed.
    slots.released(0);
    assert_eq!(slots.pick(), Some((0, Fill::Rects { from: 1, rects: vec![A] })));
    slots.committed(0, Put::Rects(&[B]));
    slots.released(1);
    assert_eq!(slots.pick(), Some((1, Fill::Rects { from: 0, rects: vec![B] })));
}

#[test]
fn buffer_that_sat_out_two_frames_takes_both_frames_changes() {
    let mut slots = Slots::default();
    for at in 0..3 {
        assert_eq!(slots.pick().map(|p| p.0), Some(at));
        slots.committed(at, if at == 0 { Put::All } else { Put::Rects(&[A]) });
    }
    // Slot 1 was newest when slot 2 changed `A` again.
    slots.released(1);
    assert_eq!(slots.pick(), Some((1, Fill::Rects { from: 2, rects: vec![A] })));
    slots.committed(1, Put::Rects(&[B]));
    // Slot 0 missed the second and third frames' `A` and the fourth's `B`.
    slots.released(0);
    assert_eq!(slots.pick(), Some((0, Fill::Rects { from: 1, rects: vec![A, A, B] })));
}

#[test]
fn no_buffer_while_the_compositor_holds_all_three() {
    let mut slots = Slots::default();
    for _ in 0..3 {
        let (at, _) = slots.pick().unwrap();
        slots.committed(at, Put::All);
    }
    assert_eq!(slots.pick(), None);
    slots.released(1);
    assert_eq!(slots.pick(), Some((1, Fill::All { from: 2 })));
}

#[test]
fn frame_that_changed_nothing_leaves_its_buffer_free_and_newest() {
    let mut slots = Slots::default();
    let (at, _) = slots.pick().unwrap();
    slots.committed(at, Put::All);
    let (second, fill) = slots.pick().unwrap();
    assert_eq!((second, fill), (1, Fill::All { from: 0 }));
    slots.painted(second);
    assert_eq!(slots.pick(), Some((1, Fill::Kept)));
}

#[test]
fn forgotten_buffers_hold_no_frame() {
    let mut slots = Slots::default();
    let (at, _) = slots.pick().unwrap();
    slots.committed(at, Put::All);
    slots.released(at);
    slots.forget();
    assert_eq!(slots.pick(), Some((0, Fill::Unknown)));
}

#[test]
fn buffer_that_missed_too_many_rectangles_takes_the_whole_frame() {
    let mut slots = Slots::default();
    let (first, _) = slots.pick().unwrap();
    slots.committed(first, Put::All);
    let (second, _) = slots.pick().unwrap();
    slots.committed(second, Put::All);
    slots.released(first);
    let (third, _) = slots.pick().unwrap();
    assert_eq!(third, first);
    slots.committed(third, Put::Rects(&[A]));
    // The second now sits out forty frames of one rectangle each while the first is repainted.
    for _ in 0..40 {
        slots.released(first);
        assert_eq!(slots.pick(), Some((first, Fill::Kept)));
        slots.committed(first, Put::Rects(&[A]));
    }
    slots.released(second);
    // The newest is held, so the second is picked: past the limit it copies everything.
    assert_eq!(slots.pick(), Some((second, Fill::All { from: first })));
}

#[test]
fn wayland_shm_format_selects_argb8888_for_alpha_and_xrgb8888_for_opaque() {
    use wayland_client::protocol::wl_shm::Format;
    assert_eq!(super::wayland::shm_format(true), Format::Argb8888);
    assert_eq!(super::wayland::shm_format(false), Format::Xrgb8888);
}

#[test]
fn x11_visual_matching_supports_32bit_argb_and_24bit_opaque_fallback() {
    use super::x11::{PixmapFormat, VisualFormat, VisualInfo, match_visual};

    let formats = [
        PixmapFormat { depth: 24, bits_per_pixel: 32 },
        PixmapFormat { depth: 32, bits_per_pixel: 32 },
    ];
    let visuals = [
        VisualInfo { id: 0x21, depth: 24, true_color: true, rgb_masks: (0xff0000, 0xff00, 0xff) },
        VisualInfo { id: 0x42, depth: 32, true_color: true, rgb_masks: (0xff0000, 0xff00, 0xff) },
        VisualInfo { id: 0x50, depth: 32, true_color: false, rgb_masks: (0xff0000, 0xff00, 0xff) },
        VisualInfo { id: 0x60, depth: 32, true_color: true, rgb_masks: (0xff, 0xff00, 0xff0000) },
    ];

    // Transparent window on a server with a 32-bit ARGB TrueColor visual.
    assert_eq!(
        match_visual(true, &formats, &visuals, 0x42, true),
        Some(VisualFormat { depth: 32, has_alpha: true })
    );
    // Transparent window on a server with no 32-bit visual (winit fell back to 24-bit TrueColor):
    // falls back to 24-bit opaque presentation so the window paints its theme background cleanly.
    assert_eq!(
        match_visual(true, &formats, &visuals, 0x21, true),
        Some(VisualFormat { depth: 24, has_alpha: false })
    );
    // Opaque window keeps 24-bit matching only.
    assert_eq!(
        match_visual(true, &formats, &visuals, 0x21, false),
        Some(VisualFormat { depth: 24, has_alpha: false })
    );
    assert_eq!(match_visual(true, &formats, &visuals, 0x42, false), None);

    // Non-LSB byte order, non-TrueColor, wrong channel masks, or missing 32-bit pixmap format fail.
    assert_eq!(match_visual(false, &formats, &visuals, 0x42, true), None);
    assert_eq!(match_visual(true, &formats, &visuals, 0x50, true), None);
    assert_eq!(match_visual(true, &formats, &visuals, 0x60, true), None);
    assert_eq!(
        match_visual(true, &[PixmapFormat { depth: 24, bits_per_pixel: 32 }], &visuals, 0x42, true),
        None
    );
}
