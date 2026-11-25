use super::*;

#[test]
fn threshold_is_straight_line_travel_beyond_the_platform_distance() {
    let t = DRAG_THRESHOLD;
    assert!(!past_threshold((10.0, 10.0), (10.0, 10.0), t));
    assert!(!past_threshold((10.0, 10.0), (10.0 + t, 10.0), t), "exactly the threshold doesn't start");
    assert!(past_threshold((10.0, 10.0), (10.0 + t + 0.1, 10.0), t));
    let d = t * 0.75;
    assert!(past_threshold((0.0, 0.0), (d, d), t), "diagonal travel counts its length");
}

#[test]
fn a_drag_item_collects_files_image_and_text() {
    let item = DragItem::new().file("/a.png").file("/b.txt").text("hello").png(vec![1, 2, 3]);
    assert_eq!(item.paths(), [PathBuf::from("/a.png"), PathBuf::from("/b.txt")]);
    assert_eq!(item.plain_text(), Some("hello"));
    assert_eq!(item.png_data(), Some(&[1u8, 2, 3][..]));
    assert!(!item.allows_move() && item.drag_image().is_none() && !item.is_empty());
    assert!(DragItem::new().is_empty());
    assert!(DragItem::new().allow_move().allows_move());
}

#[test]
fn a_drag_offers_copy_unless_told_otherwise() {
    let item = DragItem::new();
    assert!(item.offers(DragOperation::Copy) && !item.offers(DragOperation::Move) && !item.offers(DragOperation::Link));
    let item = DragItem::new().allow_move();
    assert!(item.offers(DragOperation::Copy) && item.offers(DragOperation::Move));
    let item = DragItem::new().operation(DragOperation::Move);
    assert!(!item.offers(DragOperation::Copy) && item.offers(DragOperation::Move) && item.allows_move());
    let item = DragItem::new().operation(DragOperation::Link).allow(DragOperation::Copy);
    assert!(item.offers(DragOperation::Copy) && item.offers(DragOperation::Link) && !item.offers(DragOperation::Move));
}

#[test]
fn drag_image_frames_start_over_the_element_or_under_the_pointer() {
    let rect = [20.0, 40.0, 200.0, 32.0];
    let press = (50.0, 60.0);
    let image = DragImage::rgba(1, 1, &[0; 4]).unwrap();
    assert_eq!(image.frame(rect, press, (64.0, 64.0)), [18.0, 28.0, 64.0, 64.0], "a supplied picture is centred under the press");
    assert_eq!(image.clone().size(10.0, 20.0).frame(rect, press, (64.0, 64.0)), [45.0, 50.0, 10.0, 20.0]);
    assert_eq!(image.clone().over_element().frame(rect, press, (64.0, 64.0)), rect, "over_element fills the element's bounds");
    assert_eq!(image.clone().size(10.0, 12.0).over_element().frame(rect, press, (64.0, 64.0)), [20.0, 40.0, 10.0, 12.0], "an explicit size wins");
    assert_eq!(image.over_element().frame([5.0, 5.0, 0.0, 0.0], press, (1.0, 1.0)), [5.0, 5.0, 1.0, 1.0], "never an empty frame");
}

#[test]
fn the_snapshot_frame_is_its_pixel_aligned_crop() {
    let mut image = DragImage::rgba(41, 21, &[0; 41 * 21 * 4]).unwrap().size(20.5, 10.5);
    image.placement = Placement::At(10.0, 5.0);
    assert_eq!(image.frame([10.2, 5.3, 20.0, 10.0], (0.0, 0.0), (41.0, 21.0)), [10.0, 5.0, 20.5, 10.5]);
}

#[test]
fn an_opaque_backing_fills_transparent_pixels_and_keeps_opaque_ones() {
    let image = DragImage::rgba(3, 1, &[255, 0, 0, 255, 0, 0, 0, 0, 255, 255, 255, 128]).unwrap().size(3.0, 1.0).over_element();
    let flat = image.over(Color { r: 0.0, g: 0.0, b: 1.0, a: 1.0 }).unwrap();
    assert_eq!((flat.points(), flat.placement), (Some((3.0, 1.0)), Placement::Element), "size and placement survive");
    let rgba = image::load_from_memory(flat.png_data()).unwrap().to_rgba8();
    assert_eq!(rgba.get_pixel(0, 0).0, [255, 0, 0, 255]);
    assert_eq!(rgba.get_pixel(1, 0).0, [0, 0, 255, 255]);
    let half = rgba.get_pixel(2, 0).0;
    assert_eq!(half[3], 255);
    assert!(half[0] == half[1] && half[0] > 150 && half[2] == 255, "half white over blue, mixed in linear light: {half:?}");
}

#[test]
fn the_picture_is_the_items_image_else_the_snapshot_over_the_backing() {
    let snap = DragImage::rgba(1, 1, &[0, 0, 0, 0]).unwrap();
    let own = DragImage::rgba(1, 1, &[1, 2, 3, 255]).unwrap();
    assert_eq!(picture(&DragItem::new(), Some(snap.clone())), Some(snap.clone()));
    assert_eq!(picture(&DragItem::new().image(own.clone()), Some(snap.clone())), Some(own));
    assert_eq!(picture(&DragItem::new(), None), None);
    let flat = picture(&DragItem::new().opaque(Color::WHITE), Some(snap)).unwrap();
    assert_eq!(image::load_from_memory(flat.png_data()).unwrap().to_rgba8().get_pixel(0, 0).0, [255, 255, 255, 255]);
}

#[test]
fn rgba_encodes_png_and_rejects_a_wrong_length() {
    let item = DragItem::new().rgba(1, 1, &[255, 0, 0, 255]);
    let png = item.png_data().unwrap();
    assert_eq!(&png[1..4], b"PNG");
    assert!(DragItem::new().rgba(2, 2, &[0; 4]).png_data().is_none());
    assert!(DragImage::rgba(0, 0, &[]).is_none());
    assert_eq!(DragImage::rgba(1, 1, &[0; 4]).unwrap().size(8.0, 8.0).points(), Some((8.0, 8.0)));
}

#[test]
fn snapshot_crop_clamps_to_the_frame_in_device_pixels() {
    assert_eq!(crop([10.0, 5.0, 20.0, 10.0], 2.0, (400, 300)), Some([20, 10, 40, 20]));
    assert_eq!(crop([-5.0, -5.0, 10.0, 10.0], 2.0, (400, 300)), Some([0, 0, 10, 10]));
    assert_eq!(crop([190.0, 0.0, 50.0, 10.0], 2.0, (400, 300)), Some([380, 0, 20, 20]));
    assert_eq!(crop([500.0, 0.0, 10.0, 10.0], 2.0, (400, 300)), None);
}

#[test]
fn readback_unpremultiplies_and_swaps_bgra() {
    let mut unorm = [0, 0, 128, 128, 10, 20, 30, 255, 50, 60, 70, 0];
    to_straight_rgba(&mut unorm, true, false);
    assert_eq!(unorm, [255, 0, 0, 128, 30, 20, 10, 255, 0, 0, 0, 0]);

    // Straight linear (0.25, 0.5, 0.125) tested at multiple alphas on both sRGB and non-sRGB targets.
    let lin = [0.25_f32, 0.5, 0.125];
    let straight = lin.map(|v| (srgb_encode(v) * 255.0).round() as u8);
    for a_u8 in [64u8, 128, 192, 255] {
        let a = f32::from(a_u8) / 255.0;
        // *Srgb target bytes: srgb_encode(linear_rgb * a), in BGRA and RGBA order.
        let srgb_rgba = [
            (srgb_encode(lin[0] * a) * 255.0).round() as u8,
            (srgb_encode(lin[1] * a) * 255.0).round() as u8,
            (srgb_encode(lin[2] * a) * 255.0).round() as u8,
            a_u8,
        ];
        let mut from_rgba = srgb_rgba;
        to_straight_rgba(&mut from_rgba, false, true);
        let mut from_bgra = [srgb_rgba[2], srgb_rgba[1], srgb_rgba[0], srgb_rgba[3]];
        to_straight_rgba(&mut from_bgra, true, true);
        assert_eq!(from_rgba, from_bgra);
        for c in 0..3 {
            assert!(from_rgba[c].abs_diff(straight[c]) <= 2, "srgb alpha {a_u8} ch {c}: got {} expected {}", from_rgba[c], straight[c]);
        }
        assert_eq!(from_rgba[3], a_u8);

        // Non-sRGB target bytes (`srgb_blend`): srgb_encode(linear_rgb) * a.
        let unorm_rgba = [
            (srgb_encode(lin[0]) * a * 255.0).round() as u8,
            (srgb_encode(lin[1]) * a * 255.0).round() as u8,
            (srgb_encode(lin[2]) * a * 255.0).round() as u8,
            a_u8,
        ];
        let mut from_unorm = [unorm_rgba[2], unorm_rgba[1], unorm_rgba[0], unorm_rgba[3]];
        to_straight_rgba(&mut from_unorm, true, false);
        for c in 0..3 {
            assert!(from_unorm[c].abs_diff(straight[c]) <= 2, "unorm alpha {a_u8} ch {c}: got {} expected {}", from_unorm[c], straight[c]);
        }
        assert_eq!(from_unorm[3], a_u8);
    }
    let mut zero_srgb = [99, 137, 71, 0];
    to_straight_rgba(&mut zero_srgb, true, true);
    assert_eq!(zero_srgb, [0, 0, 0, 0]);
}

#[test]
fn custom_data_preserves_order_and_replaces_in_place() {
    let item = DragItem::new()
        .data("text/uri-list", "sftp://host/a.txt\r\n")
        .data("x-special/gnome-copied-files", "copy\nsftp://host/a.txt")
        .data("application/x-kde-cutselection", b"0".as_slice())
        .data("application/x-app-drag-id", b"id-1".as_slice())
        .data("x-special/gnome-copied-files", "cut\nsftp://host/a.txt");

    assert!(!item.is_empty());
    let entries: Vec<(&str, &[u8])> = item.custom_data().collect();
    assert_eq!(item.custom_data().len(), 4);
    assert_eq!(entries, [
        ("text/uri-list", b"sftp://host/a.txt\r\n".as_slice()),
        ("x-special/gnome-copied-files", b"cut\nsftp://host/a.txt".as_slice()),
        ("application/x-kde-cutselection", b"0".as_slice()),
        ("application/x-app-drag-id", b"id-1".as_slice()),
    ]);
    assert_eq!(item.data_for("x-special/gnome-copied-files"), Some(b"cut\nsftp://host/a.txt".as_slice()));
    assert_eq!(item.data_for("text/plain"), None);

    // An empty byte payload is still a valid custom entry and makes the item non-empty.
    let marker = DragItem::new().data("application/x-marker", b"".as_slice());
    assert!(!marker.is_empty());
    assert_eq!(marker.data_for("application/x-marker"), Some(b"".as_slice()));
}

#[test]
fn custom_data_validates_mime_strings() {
    let max_ok = "a".repeat(255);
    let too_long = "a".repeat(256);
    let item = DragItem::new()
        .data("", b"x".as_slice())
        .data(" text/plain", b"x".as_slice())
        .data("text/plain ", b"x".as_slice())
        .data("text/plain\0", b"x".as_slice())
        .data("text/plain\n", b"x".as_slice())
        .data("text/plain\r", b"x".as_slice())
        .data("text/plain\t", b"x".as_slice())
        .data("text/plain\x1f", b"x".as_slice())
        .data("text/plain\x7f", b"x".as_slice())
        .data("text/caf\u{00e9}", b"x".as_slice())
        .data(too_long, b"x".as_slice());
    assert!(item.is_empty(), "all invalid MIME entries must be ignored");
    assert_eq!(item.custom_data().len(), 0);

    let ok = DragItem::new()
        .data(&max_ok, b"ok".as_slice())
        .data("text/plain; charset=utf-8", b"hello".as_slice());
    assert_eq!(ok.custom_data().len(), 2);
    assert_eq!(ok.data_for(&max_ok), Some(b"ok".as_slice()));
    assert_eq!(ok.data_for("text/plain; charset=utf-8"), Some(b"hello".as_slice()));
}

#[test]
fn custom_data_enforces_entry_and_byte_bounds() {
    let mut item = DragItem::new();
    for i in 0..MAX_CUSTOM_DATA_ENTRIES {
        item = item.data(format!("application/x-entry-{i}"), [i as u8]);
    }
    assert_eq!(item.custom_data().len(), MAX_CUSTOM_DATA_ENTRIES);
    // The 65th distinct entry is ignored, while replacing an existing entry still works.
    item = item.data("application/x-overflow", b"nope".as_slice());
    assert_eq!(item.custom_data().len(), MAX_CUSTOM_DATA_ENTRIES);
    assert_eq!(item.data_for("application/x-overflow"), None);
    item = item.data("application/x-entry-0", b"updated".as_slice());
    assert_eq!(item.data_for("application/x-entry-0"), Some(b"updated".as_slice()));

    // Total byte bound: 32 MiB fits, +1 byte is rejected, in-place replacement subtracts old size.
    let half = MAX_CUSTOM_DATA_BYTES / 2;
    let sized = DragItem::new()
        .data("application/x-a", vec![1u8; half])
        .data("application/x-b", vec![2u8; MAX_CUSTOM_DATA_BYTES - half]);
    assert_eq!(sized.custom_data().len(), 2);
    let rejected_new = sized.clone().data("application/x-c", b"x".as_slice());
    assert_eq!(rejected_new.data_for("application/x-c"), None);
    // Replacing `a` with an oversized payload leaves `a` untouched.
    let rejected_replace = sized.clone().data("application/x-a", vec![9u8; half + 1]);
    assert_eq!(rejected_replace.data_for("application/x-a").map(<[u8]>::len), Some(half));
    assert_eq!(rejected_replace.data_for("application/x-a").unwrap()[0], 1);
    // Replacing `a` with a payload of the same size fits because the old `a` size is subtracted.
    let replaced = sized.data("application/x-a", vec![7u8; half]);
    assert_eq!(replaced.data_for("application/x-a").unwrap()[0], 7);
    assert_eq!(replaced.custom_data().len(), 2);
}

#[test]
fn drag_image_hotspot_places_frame_and_survives_backing() {
    let rect = [20.0, 40.0, 200.0, 32.0];
    let press = (50.0, 60.0);
    let image = DragImage::rgba(4, 4, &[255, 0, 0, 128].repeat(16)).unwrap();
    assert_eq!(image.hotspot_point(), None);
    assert_eq!(image.clone().over_element().hotspot_point(), None);

    let hot = image.clone().size(48.0, 48.0).hotspot(12.0, 18.0);
    assert_eq!(hot.hotspot_point(), Some((12.0, 18.0)));
    assert_eq!(hot.frame(rect, press, (64.0, 64.0)), [38.0, 42.0, 48.0, 48.0]);

    let natural_hot = image.clone().hotspot(5.0, 10.0);
    assert_eq!(natural_hot.frame(rect, press, (64.0, 32.0)), [45.0, 50.0, 64.0, 32.0]);

    let sanitized = image.clone().hotspot(f32::NAN, f32::INFINITY);
    assert_eq!(sanitized.hotspot_point(), Some((0.0, 0.0)));

    let flat = hot.over(Color::WHITE).unwrap();
    assert_eq!(flat.hotspot_point(), Some((12.0, 18.0)));
    assert_eq!(flat.frame(rect, press, (64.0, 64.0)), [38.0, 42.0, 48.0, 48.0]);
}

#[test]
fn snapshot_unpremultiplies_correctly_on_srgb_and_unorm_targets() {
    use crate::render::{Gpu, Instance, KIND_RECT, NO_CLIP, Renderer};
    let Some(gpu) = Gpu::test_headless() else { return };
    let expected_rgb = [0.25_f32, 0.5, 0.125].map(|v| (srgb_encode(v) * 255.0).round() as u8);
    let expected_a = (0.5_f32 * 255.0).round() as u8;
    for (format, srgb_blend) in [
        (wgpu::TextureFormat::Bgra8UnormSrgb, false),
        (wgpu::TextureFormat::Bgra8Unorm, true),
    ] {
        let mut renderer = Renderer::new(&gpu.device, format);
        renderer.set_srgb_blend(srgb_blend);
        let color = Color { r: 0.25, g: 0.5, b: 0.125, a: 0.5 };
        let fill = if srgb_blend { color.premul_srgb(1.0) } else { color.premul(1.0) };
        let inst = Instance {
            rect: [0.0, 0.0, 8.0, 8.0],
            radii: [0.0; 4],
            fill,
            border: [0.0; 4],
            params: [0.0, KIND_RECT, 0.0, 1.0],
            uv: [0.0; 4],
            clip: NO_CLIP,
            fade: [0; 4],
        };
        let mut atlas = crate::text::Atlas::new();
        renderer.prepare_ui(&gpu.device, &gpu.queue, &mut atlas, &[inst], (8, 8));
        let shot = snapshot(&gpu, &renderer, (8, 8), 1.0, [0.0; 4], [0.0, 0.0, 8.0, 8.0]).expect("snapshot");
        let rgba = crate::image::png_rgba8(shot.png_data()).expect("valid png");
        let px = rgba.get_pixel(4, 4).0;
        for c in 0..3 {
            assert!(
                px[c].abs_diff(expected_rgb[c]) <= 2,
                "{format:?}: channel {c} got {}, expected {} (pixel {px:?})",
                px[c], expected_rgb[c],
            );
        }
        assert!(px[3].abs_diff(expected_a) <= 1, "{format:?}: alpha got {}, expected {expected_a}", px[3]);
    }
}
