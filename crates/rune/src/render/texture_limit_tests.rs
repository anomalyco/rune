//! A device whose largest texture is 2048 texels a side, as WebGL2 and older mobile GPUs report:
//! nothing Rune uploads asks it for a larger one.
use std::sync::Arc;

use crate::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
use crate::text::outline::{OutlineStore, WIDTH};
use crate::{Cx, El, Env, Runtime, el, image::ImageCache, pixels::Pixels};

const LIMIT: u32 = 2048;

fn small_gpu() -> Gpu {
    let base = Gpu::headless();
    let limits = wgpu::Limits { max_texture_dimension_2d: LIMIT, ..base.device.limits() };
    let (device, queue) = pollster::block_on(base.adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("rune 2048"),
        required_features: base.device.features(),
        required_limits: limits,
        experimental_features: Default::default(),
        memory_hints: Default::default(),
        trace: Default::default(),
    })).expect("a device with a 2048 texture limit");
    Gpu::from_device(base.instance.clone(), base.adapter.clone(), device, queue)
}

fn scoped<T>(gpu: &Gpu, f: impl FnOnce() -> T) -> (T, Option<String>) {
    let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let out = f();
    (out, pollster::block_on(scope.pop()).map(|e| e.to_string()))
}

#[test]
fn an_image_larger_than_the_device_s_largest_texture_draws_from_a_mip_that_fits() {
    fn view(s: &Pixels, _: &mut Cx<Pixels>) -> El { el().size(64.0, 64.0).pixels(s) }
    let gpu = small_gpu();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let cache = ImageCache::new(Arc::new(|| {}));
    let image = cache.rgba(4096, 4096, [200u8, 30, 30, 255].repeat(4096 * 4096));
    image.wait().unwrap();
    // Drawn at its full size, the atlas would otherwise place mip 0.
    let mut rt = Runtime::new(image.pixels().clone(), view, (4096.0, 4096.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(rt.images.pages.iter().all(|p| p.width.max(p.height) <= LIMIT), "no page past the device's limit");
    let target = Offscreen::new(&gpu.device, 64, 64);
    let (_, error) = scoped(&gpu, || {
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (64, 64));
        renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut encoder, &target.view, [0.0; 4]);
        target.copy(&mut encoder);
        renderer.submit(&gpu.queue, encoder);
    });
    assert_eq!(error, None);
    let out = target.read(&gpu.device);
    assert_eq!(&out[..4], &[200, 30, 30, 255], "the image drew");
}

#[test]
fn the_outline_store_never_outgrows_the_device_s_largest_texture() {
    let gpu = small_gpu();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let mut atlas = crate::text::TextSystem::default().atlas;
    renderer.attach_text(&mut atlas);
    atlas.outlines = OutlineStore::from_texels(vec![[0; 4]; WIDTH as usize]);
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &[], (64, 64));
    atlas.outlines = OutlineStore::from_texels(vec![[0; 4]; WIDTH as usize * 3000]);
    let (_, error) = scoped(&gpu, || renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &[], (64, 64)));
    assert_eq!(error, None);
    if let Some(outlines) = renderer.uber.outlines() {
        assert!(outlines.height() <= LIMIT, "outline texture is {} rows", outlines.height());
    }
}
