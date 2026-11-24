use std::sync::{Arc, Mutex};

use rune::Env;
use rune::compose::{Compositor, Frame};
use rune::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
use rune::runtime::Runtime;

use crate::{Scene, State, view};

use super::Gfx;

fn draw_on(gfx: &mut Gfx, rt: &mut Runtime<State>, gpu: &Gpu, ui_ren: &mut Renderer, off: &Offscreen, dt: f64) {
    rt.frame(dt);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    let mut frame = Frame {
        device: &gpu.device,
        queue: &gpu.queue,
        encoder: &mut encoder,
        target: &off.view,
        format: OFFSCREEN_FORMAT,
        size: (off.width, off.height),
        dt,
        update_ms: 0.0,
        rt,
        ui: ui_ren,
    };
    gfx.render(&mut frame);
    ui_ren.submit(&gpu.queue, encoder);
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).ok();
}

#[test]
fn showreel_gfx_rebuilds_fx_renderer_when_device_key_changes() {
    let size = (320, 180);
    let state = State { scene: Scene::Glass, t: 1.0 };

    let mut rt = Runtime::new(state, view, (size.0 as f32, size.1 as f32), Env::default());
    rt.track_marks = true;

    let mut gfx = Gfx::default();

    // First frame on device 1 (glass scene with mesh gradient and backdrop-blurred fluted glass).
    let gpu1 = Gpu::headless();
    let mut ui1 = Renderer::new(&gpu1.device, OFFSCREEN_FORMAT);
    ui1.attach_text(&mut rt.text.atlas);
    let off1 = Offscreen::new(&gpu1.device, size.0, size.1);
    draw_on(&mut gfx, &mut rt, &gpu1, &mut ui1, &off1, 0.0);
    let key1 = ui1.device_key();
    assert_eq!(gfx.fx.as_ref().unwrap().device_key, key1);

    // Replace the device and renderer at the same viewport size and target format,
    // then switch to the fire scene (simulated flames + heat haze backdrop).
    let gpu2 = Gpu::headless();
    let errors: Arc<Mutex<Vec<String>>> = Arc::default();
    {
        let errors = errors.clone();
        gpu2.device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| {
            errors.lock().unwrap().push(e.to_string());
        }));
    }
    let mut ui2 = Renderer::new(&gpu2.device, OFFSCREEN_FORMAT);
    let key2 = ui2.device_key();
    assert_ne!(key1, key2);
    let off2 = Offscreen::new(&gpu2.device, size.0, size.1);
    drop((off1, ui1, gpu1));
    rt.text.atlas.invalidate();
    rt.images.invalidate();
    ui2.attach_text(&mut rt.text.atlas);
    rt.state.scene = Scene::Fire;
    rt.state.t = 1.5;
    rt.invalidate();

    draw_on(&mut gfx, &mut rt, &gpu2, &mut ui2, &off2, 1.0 / 60.0);
    assert_eq!(gfx.fx.as_ref().unwrap().device_key, key2);
    let errors = errors.lock().unwrap();
    assert!(errors.is_empty(), "uncaptured GPU errors: {errors:?}");
    assert!(!gpu2.is_lost());
}

#[test]
fn showreel_gfx_release_gpu_drops_fx_and_rebuilds_cleanly() {
    let size = (320, 180);
    let state = State { scene: Scene::Glass, t: 1.0 };
    let mut rt = Runtime::new(state, view, (size.0 as f32, size.1 as f32), Env::default());
    rt.track_marks = true;

    let mut gfx = Gfx::default();
    let gpu1 = Gpu::headless();
    let mut ui1 = Renderer::new(&gpu1.device, OFFSCREEN_FORMAT);
    ui1.attach_text(&mut rt.text.atlas);
    let off1 = Offscreen::new(&gpu1.device, size.0, size.1);
    draw_on(&mut gfx, &mut rt, &gpu1, &mut ui1, &off1, 0.0);
    assert!(gfx.fx.is_some());

    drop((off1, ui1, gpu1));
    gfx.release_gpu();
    assert!(gfx.fx.is_none(), "release_gpu must drop showreel Gfx::fx");

    let gpu2 = Gpu::headless();
    let mut ui2 = Renderer::new(&gpu2.device, OFFSCREEN_FORMAT);
    rt.text.atlas.invalidate();
    rt.images.invalidate();
    ui2.attach_text(&mut rt.text.atlas);
    let off2 = Offscreen::new(&gpu2.device, size.0, size.1);
    draw_on(&mut gfx, &mut rt, &gpu2, &mut ui2, &off2, 1.0 / 60.0);
    assert_eq!(gfx.fx.as_ref().unwrap().device_key, ui2.device_key());
    assert!(!gpu2.is_lost());
}
