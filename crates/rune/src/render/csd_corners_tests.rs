//! Clearing a window's rounded bottom corners on the GPU: what stays, and what goes.

use super::CsdCorners;
use crate::render::{Gpu, Offscreen, OFFSCREEN_FORMAT};

/// A target of opaque red, its corners faded at `radius` device pixels.
fn cleared(gpu: &Gpu, size: (u32, u32), radius: f32) -> Vec<u8> {
    cleared_with(gpu, size, radius, OFFSCREEN_FORMAT)
}

/// [`cleared`] into a target of `format`.
fn cleared_with(gpu: &Gpu, size: (u32, u32), radius: f32, format: wgpu::TextureFormat) -> Vec<u8> {
    let target = Offscreen::with_format(&gpu.device, size.0, size.1, format);
    let mut pass = CsdCorners::new(&gpu.device, format);
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    {
        let _clear = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::RED),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
    pass.prepare(&gpu.queue, (size.0, size.1), radius);
    pass.draw(&mut enc, &target.view);
    target.copy(&mut enc);
    gpu.queue.submit([enc.finish()]);
    target.read(&gpu.device)
}

/// The target's pixel at `(x, y)`, as RGBA.
fn at(pixels: &[u8], width: u32, (x, y): (u32, u32)) -> [u8; 4] {
    let start = ((y * width + x) * 4) as usize;
    pixels[start..start + 4].try_into().expect("four channels")
}

#[test]
fn the_bottom_corners_clear_and_nothing_else() {
    let gpu = Gpu::new(Gpu::instance(), None);
    let (width, height) = (40, 30);
    let radius = 6.;
    let pixels = cleared(&gpu, (width, height), radius);
    let red = at(&pixels, width, (0, 0));
    assert_eq!(red[3], 255, "the top left corner keeps its pixel: {red:?}");
    assert_eq!(at(&pixels, width, (0, height - 1))[3], 0, "the bottom left corner clears");
    assert_eq!(at(&pixels, width, (width - 1, height - 1))[3], 0, "the bottom right corner clears");
    // Along the foot, the arc is gone only past where it meets the window's edge. The pixels a
    // pixel in from it are inside the fade's inside edge, so they keep all of their coverage.
    assert_eq!(at(&pixels, width, (radius as u32, height - 2))[3], 255, "inside the arc stays");
    assert_eq!(at(&pixels, width, (width - 1 - radius as u32, height - 2))[3], 255, "inside the arc stays");
    // Above the corners' own squares nothing changed, at either side.
    assert_eq!(at(&pixels, width, (0, height - 1 - radius as u32))[3], 255, "above the arc's centre stays");
    assert_eq!(at(&pixels, width, (width - 1, height - 1 - radius as u32))[3], 255, "above the arc's centre stays");
    // The top two corners are the headerbar's business.
    assert_eq!(at(&pixels, width, (width - 1, 0))[3], 255, "the top right corner keeps its pixel");
}

/// The arcs follow the radius: a wider one takes more of the corner.
#[test]
fn a_wider_radius_clears_more_of_the_corner() {
    let gpu = Gpu::new(Gpu::instance(), None);
    let size = (40, 30);
    let narrow = cleared(&gpu, size, 4.);
    let wide = cleared(&gpu, size, 9.);
    // A pixel above the bottom left, on the window's edge: outside a radius-9 arc (9.9 from its
    // centre) but above a radius-4 one's corner square altogether, which only starts four in from
    // the foot.
    let corner = (0, 25);
    assert_eq!(at(&narrow, size.0, corner)[3], 255, "outside the narrow corner's square");
    assert_eq!(at(&wide, size.0, corner)[3], 0, "outside the wide corner's arc");
}

/// A square window (maximized, tiled or fullscreen) draws nothing: nothing is prepared for a
/// radius of zero, and the pass draws nothing when nothing was prepared for it. `Host::draw` skips
/// it outright for that radius (`window::tests`).
#[test]
fn a_square_window_draws_nothing() {
    let gpu = Gpu::new(Gpu::instance(), None);
    let size = (40, 30);
    let target = Offscreen::new(&gpu.device, size.0, size.1);
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    {
        let _clear = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::RED), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
    let mut pass = CsdCorners::new(&gpu.device, OFFSCREEN_FORMAT);
    pass.prepare(&gpu.queue, size, 0.);
    assert!(!pass.is_prepared_for(size, 0.), "a square window prepares no pass");
    // And the pass draws nothing when nothing was prepared for it.
    pass.draw(&mut enc, &target.view);
    target.copy(&mut enc);
    gpu.queue.submit([enc.finish()]);
    let pixels = target.read(&gpu.device);
    for y in 0..size.1 {
        for x in 0..size.0 {
            assert_eq!(at(&pixels, size.0, (x, y))[3], 255, "({x}, {y}) faded with no radius");
        }
    }
}

/// The fade holds in both views the renderer makes: the sRGB one `srgb_blend(false)` draws into and
/// the linear one it makes for `srgb_blend(true)`, which the frame is drawn through in either case.
#[test]
fn both_target_views_fade_the_corners() {
    let gpu = Gpu::new(Gpu::instance(), None);
    let size = (40, 30);
    for format in [OFFSCREEN_FORMAT, OFFSCREEN_FORMAT.remove_srgb_suffix()] {
        let pixels = cleared_with(&gpu, size, 6., format);
        assert_eq!(at(&pixels, size.0, (0, 0))[3], 255, "{format:?}: the top left keeps its pixel");
        assert_eq!(at(&pixels, size.0, (0, size.1 - 1))[3], 0, "{format:?}: the bottom corner fades");
        assert_eq!(at(&pixels, size.0, (6, 27))[3], 255, "{format:?}: inside the arc stays");
        // A pixel the arc crosses keeps part of its colour and of its alpha alike: the fade is a
        // multiply in linear light, so the red the target was cleared to scales with the coverage
        // (encoded, on an sRGB view).
        let across = at(&pixels, size.0, (1, 28));
        assert!(across[3] > 0 && across[3] < 255, "{format:?}: the arc's edge fades ({across:?})");
        let coverage = across[3] as f32 / 255.;
        let want = if format.is_srgb() { (crate::color::srgb_encode(coverage) * 255.).round() as u8 } else { across[3] };
        assert!(
            across[0].abs_diff(want) <= 2,
            "{format:?}: the faded red is {} where the coverage says {want}",
            across[0],
        );
    }
}
