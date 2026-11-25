use std::f32::consts::FRAC_PI_2;

use super::super::super::{CpuPainter, NeedsGpu, Scene, Target, paint};
use crate::pixels::ImageAtlas;
use crate::render::{Instance, KIND_RECT, NO_CLIP};
use crate::text::Atlas;

const SIZE: (u32, u32) = (96, 64);
const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

fn rect(bounds: [f32; 4], fill: [f32; 4]) -> Instance {
    Instance { rect: bounds, fill, params: [0.0, KIND_RECT, 0.0, 1.0], clip: NO_CLIP, ..bytemuck::Zeroable::zeroed() }
}

fn turned(rect: [f32; 4], angle: f32, pivot: [f32; 2], contents: &[Instance]) -> Vec<Instance> {
    let mut out = vec![Instance::begin_layer(rect, NO_CLIP, Default::default()).turned(angle, pivot)];
    out.extend_from_slice(contents);
    out.push(Instance::end_layer());
    out
}

fn painted(instances: &[Instance]) -> Result<Vec<u8>, NeedsGpu> {
    let (atlas, images) = (Atlas::new(), ImageAtlas::default());
    let mut px = vec![0u8; (SIZE.0 * SIZE.1 * 4) as usize];
    paint(&Scene::new(instances, &atlas, &images, SIZE, CLEAR), &mut Target::new(&mut px, SIZE.0, SIZE.1), 1)?;
    Ok(px)
}

const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];

#[test]
fn quarter_turned_layer_paints_as_its_contents_turned() {
    // A 32 × 16 box, red on the left and green on the right, turned 90° clockwise about its
    // centre: a 16 × 32 box, red on top.
    let r = [16.0, 24.0, 32.0, 16.0];
    let layer = turned(r, FRAC_PI_2, [32.0, 32.0], &[rect([16.0, 24.0, 16.0, 16.0], RED), rect([32.0, 24.0, 16.0, 16.0], GREEN)]);
    let (atlas, images) = (Atlas::new(), ImageAtlas::default());
    assert_eq!(Scene::new(&layer, &atlas, &images, SIZE, CLEAR).needs_gpu(), None);
    let direct = [rect([24.0, 16.0, 16.0, 16.0], RED), rect([24.0, 32.0, 16.0, 16.0], GREEN)];
    assert!(painted(&layer).unwrap() == painted(&direct).unwrap(), "a quarter-turned layer paints its contents turned");

    // A three-quarter turn (as -90°) puts red at the bottom.
    let layer = turned(r, -FRAC_PI_2, [32.0, 32.0], &[rect([16.0, 24.0, 16.0, 16.0], RED), rect([32.0, 24.0, 16.0, 16.0], GREEN)]);
    let direct = [rect([24.0, 32.0, 16.0, 16.0], RED), rect([24.0, 16.0, 16.0, 16.0], GREEN)];
    assert!(painted(&layer).unwrap() == painted(&direct).unwrap(), "a -90° layer paints its contents turned the other way");
}

#[test]
fn quarter_turned_layer_draws_contents_from_outside_the_window() {
    // Upright, the box reaches 8 px left of the window; turned, it's all inside.
    let layer = turned([-8.0, 40.0, 40.0, 8.0], FRAC_PI_2, [12.0, 44.0], &[rect([-8.0, 40.0, 40.0, 8.0], RED)]);
    assert!(painted(&layer).unwrap() == painted(&[rect([8.0, 24.0, 8.0, 40.0], RED)]).unwrap());
}

#[test]
fn other_turns_still_need_the_gpu() {
    let (atlas, images) = (Atlas::new(), ImageAtlas::default());
    let layer = turned([16.0, 24.0, 32.0, 16.0], 0.35, [32.0, 32.0], &[rect([16.0, 24.0, 32.0, 16.0], RED)]);
    assert_eq!(Scene::new(&layer, &atlas, &images, SIZE, CLEAR).needs_gpu(), Some(NeedsGpu::TurnedLayer));
}

#[test]
fn live_painter_repaints_a_turned_layer_whose_contents_change() {
    let (atlas, images) = (Atlas::new(), ImageAtlas::default());
    let frame = |fill| turned([16.0, 24.0, 32.0, 16.0], FRAC_PI_2, [32.0, 32.0], &[rect([16.0, 24.0, 32.0, 16.0], fill)]);
    let mut painter = CpuPainter::new(1);
    let mut px = vec![0u8; (SIZE.0 * SIZE.1 * 4) as usize];
    for fill in [RED, GREEN, RED] {
        let f = frame(fill);
        painter.paint(&Scene::new(&f, &atlas, &images, SIZE, CLEAR), &mut Target::new(&mut px, SIZE.0, SIZE.1)).unwrap();
        assert!(px == painted(&f).unwrap(), "the live painter's frame matches a full paint");
    }
}

#[test]
fn rotated_text_label_paints_on_the_cpu() {
    use crate::{Cx, El, Env, Runtime, col, row, text};
    fn view(rotate: &bool, _: &mut Cx<bool>) -> El {
        let label = text("Time");
        // In a row, the label is only as wide as its text, so it turns about the text's centre.
        col().padding(24.0).child(row().child(if *rotate { label.rotate(FRAC_PI_2) } else { label }))
    }
    // The extent of the pixels that aren't the clear colour: (width, height).
    let extent = |rotate: bool| {
        let (w, h) = (200u32, 120u32);
        let mut rt = Runtime::new(rotate, view, (w as f32, h as f32), Env::new(true, 1.0));
        super::super::super::attach_text(&mut rt.text.atlas);
        rt.frame(0.0);
        super::super::super::fill_masks(&mut rt.text.atlas, 1);
        let scene = Scene::new(&rt.instances, &rt.text.atlas, &rt.images, (w, h), rt.clear_color());
        assert_eq!(scene.needs_gpu(), None, "rotate = {rotate}");
        let mut px = vec![0u8; (w * h * 4) as usize];
        paint(&scene, &mut Target::new(&mut px, w, h), 1).expect("the CPU paints the frame");
        let bg = [px[0], px[1], px[2], px[3]];
        let mut b = [u32::MAX, u32::MAX, 0, 0];
        for y in 0..h {
            for x in 0..w {
                let at = ((y * w + x) * 4) as usize;
                if px[at..at + 4] != bg { b = [b[0].min(x), b[1].min(y), b[2].max(x + 1), b[3].max(y + 1)]; }
            }
        }
        assert!(b[2] > b[0], "rotate = {rotate}: the label drew");
        (b[2] - b[0], b[3] - b[1])
    };
    let (upright, rotated) = (extent(false), extent(true));
    assert!(upright.0 > upright.1, "{upright:?}");
    assert!(rotated.1 > rotated.0, "{rotated:?}");
    assert!(rotated.0.abs_diff(upright.1) <= 1 && rotated.1.abs_diff(upright.0) <= 1, "{upright:?} turned is {rotated:?}");
}

#[test]
fn quarter_turned_layer_keeps_a_wrapped_image_in_place() {
    use crate::render::{KIND_IMAGE, shader_consts as sc};
    // A 4 × 4 tile whose every texel differs, repeated from a placement rect that isn't on the
    // image's corner: a wrong placement shifts the pattern.
    let mut images = ImageAtlas::default();
    let tile = crate::pixels::Pixels::new();
    let rgba: Vec<u8> = (0..4u8).flat_map(|y| (0..4u8).flat_map(move |x| [x * 75 + 20, y * 75 + 20, 40, 255])).collect();
    tile.set(4, 4, rgba);
    let placed = images.place(&tile, [4.0, 4.0]).expect("places the tile");
    let atlas = Atlas::new();
    let image = Instance {
        rect: [17.0, 25.0, 32.0, 16.0],
        fill: [0.0, 0.0, 0.0, 1.0],
        border: [22.0, 28.0, 10.0, 10.0],
        params: [placed.page as f32, KIND_IMAGE, sc::IMAGE_FLAG_NEAREST as f32, sc::IMAGE_WRAP_REPEAT as f32],
        uv: placed.rect.map(|v| v as f32),
        clip: NO_CLIP,
        ..bytemuck::Zeroable::zeroed()
    };
    let draw = |instances: &[Instance]| {
        let mut px = vec![0u8; (SIZE.0 * SIZE.1 * 4) as usize];
        paint(&Scene::new(instances, &atlas, &images, SIZE, CLEAR), &mut Target::new(&mut px, SIZE.0, SIZE.1), 1).expect("paints");
        px
    };
    let at = |px: &[u8], x: u32, y: u32| { let i = ((y * SIZE.0 + x) * 4) as usize; [px[i], px[i + 1], px[i + 2], px[i + 3]] };
    let upright = draw(&[image]);
    assert_ne!(at(&upright, 30, 30), [0, 0, 0, 255], "the image draws");
    // Turned 90° clockwise about (33, 33). The layer's buffer starts at (16, 24) (the image's
    // bounds with its pixel of margin), not the window's corner nor a whole number of 10 px
    // repeats from the placement. Pixel (x, y) shows what upright pixel (y, 65 - x) did.
    let turned_px = draw(&turned(image.rect, FRAC_PI_2, [33.0, 33.0], &[image]));
    for y in 17..49u32 {
        for x in 25..41u32 {
            let (sx, sy) = (y, 65 - x);
            assert_eq!(at(&turned_px, x, y), at(&upright, sx, sy), "turned ({x}, {y}) against upright ({sx}, {sy})");
        }
    }
}

#[test]
fn layer_inside_a_turned_layer_keeps_what_the_turn_brings_into_the_window() {
    // Upright, the faded layer's box reaches 8 px left of the window; turned, it's all inside.
    let faded = |r: [f32; 4]| vec![Instance::begin_layer(r, NO_CLIP, crate::render::LayerSpec { opacity: 0.5, ..Default::default() }), rect(r, RED), Instance::end_layer()];
    let layer = turned([8.0, 40.0, 24.0, 8.0], FRAC_PI_2, [12.0, 44.0], &faded([-8.0, 40.0, 40.0, 8.0]));
    let (got, want) = (painted(&layer).unwrap(), painted(&faded([8.0, 24.0, 8.0, 40.0])).unwrap());
    let at = |px: &[u8], x: u32, y: u32| { let i = ((y * SIZE.0 + x) * 4) as usize; [px[i], px[i + 1], px[i + 2], px[i + 3]] };
    assert_ne!(at(&got, 12, 26), [0, 0, 0, 255], "the part that was left of the window draws");
    // The turned contents are kept in 8 bits, the faded layer's own target in half floats.
    let worst = got.iter().zip(&want).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
    assert!(worst <= 1, "off by {worst}");
}

#[test]
fn turned_layer_inside_a_turned_layer_turns_twice() {
    let halves = [rect([16.0, 24.0, 16.0, 16.0], RED), rect([32.0, 24.0, 16.0, 16.0], GREEN)];
    let inner = turned([16.0, 24.0, 32.0, 16.0], FRAC_PI_2, [32.0, 32.0], &halves);
    let layer = turned([16.0, 24.0, 32.0, 16.0], FRAC_PI_2, [32.0, 32.0], &inner);
    let direct = [rect([32.0, 24.0, 16.0, 16.0], RED), rect([16.0, 24.0, 16.0, 16.0], GREEN)];
    assert!(painted(&layer).unwrap() == painted(&direct).unwrap(), "two quarter turns are a half turn");

    // Two quarter turns about (4, 32) bring a box from 40-50 px left of the window into it. The
    // inner turn alone, cut to the window turned back once, would leave it out.
    let r = [-50.0, 40.0, 10.0, 8.0];
    let inner = turned(r, FRAC_PI_2, [4.0, 32.0], &[rect(r, RED)]);
    let layer = turned(r, FRAC_PI_2, [4.0, 32.0], &inner);
    assert!(painted(&layer).unwrap() == painted(&[rect([48.0, 16.0, 10.0, 8.0], RED)]).unwrap(), "the inner turn's frame is its parent's");
}
