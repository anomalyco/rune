use x11rb::protocol::xproto::{Depth, Format, ImageOrder, Screen, Setup, VisualClass, Visualtype};

use super::frame_depth;

const RGB: (u32, u32, u32) = (0xff0000, 0xff00, 0xff);

fn visual(id: u32, masks: (u32, u32, u32)) -> Visualtype {
    Visualtype { visual_id: id, class: VisualClass::TRUE_COLOR, bits_per_rgb_value: 8, colormap_entries: 256, red_mask: masks.0, green_mask: masks.1, blue_mask: masks.2 }
}

/// A server with a 24-bit visual 0x21 and a 32-bit (ARGB) visual 0x41, both B, G, R in memory,
/// and a 32 bpp pixmap format for each depth in `formats`.
fn setup(formats: &[u8], masks32: (u32, u32, u32)) -> Setup {
    let depths = vec![
        Depth { depth: 24, visuals: vec![visual(0x21, RGB)] },
        Depth { depth: 32, visuals: vec![visual(0x41, masks32)] },
    ];
    Setup {
        image_byte_order: ImageOrder::LSB_FIRST,
        pixmap_formats: formats.iter().map(|&depth| Format { depth, bits_per_pixel: 32, scanline_pad: 32 }).collect(),
        roots: vec![Screen { allowed_depths: depths, ..Default::default() }],
        ..Default::default()
    }
}

#[test]
fn a_24_bit_visual_takes_frames_with_or_without_alpha() {
    let s = setup(&[24, 32], RGB);
    assert_eq!(frame_depth(&s, 0x21, false), Some(24));
    // Alpha asked of an opaque window: its frames go out, their alpha ignored.
    assert_eq!(frame_depth(&s, 0x21, true), Some(24));
}

#[test]
fn a_32_bit_visual_takes_alpha_frames_only() {
    let s = setup(&[24, 32], RGB);
    assert_eq!(frame_depth(&s, 0x41, true), Some(32));
    assert_eq!(frame_depth(&s, 0x41, false), None);
}

#[test]
fn other_masks_take_no_frames() {
    let s = setup(&[24, 32], (0xff, 0xff00, 0xff0000));
    assert_eq!(frame_depth(&s, 0x41, true), None);
    assert_eq!(frame_depth(&s, 0x99, true), None);
}

#[test]
fn a_depth_without_a_32_bpp_pixmap_format_takes_no_frames() {
    let s = setup(&[24], RGB);
    assert_eq!(frame_depth(&s, 0x41, true), None);
    assert_eq!(frame_depth(&s, 0x21, false), Some(24));
    let msb = Setup { image_byte_order: ImageOrder::MSB_FIRST, ..setup(&[24, 32], RGB) };
    assert_eq!(frame_depth(&msb, 0x21, false), None);
}
