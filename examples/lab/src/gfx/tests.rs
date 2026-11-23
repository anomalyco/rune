use rune::Env;
use rune::compose::{Compositor, Frame};
use rune::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
use rune::runtime::Runtime;

use crate::knobs::{Scene, specs};
use crate::{Lab, ui};

use super::LabGfx;

fn draw_on(gfx: &mut LabGfx, rt: &mut Runtime<Lab>, gpu: &Gpu, ui_ren: &mut Renderer, off: &Offscreen, dt: f64) {
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
fn lab_gfx_rebuilds_resources_and_hdr_format_when_device_key_changes() {
    let size = (320, 200);
    let mut lab = Lab::new();
    lab.scene = Scene::Lens;
    let frost = specs(Scene::Lens).iter().position(|k| k.id == "frost").unwrap();
    lab.vals[Scene::Lens.idx()][frost] = 1.0;

    let mut rt = Runtime::new(lab, ui::view, (size.0 as f32, size.1 as f32), Env::default());
    rt.track_marks = true;

    let mut gfx = LabGfx::default();

    // First frame on device 1 (lens-frost scene).
    let gpu1 = Gpu::headless();
    let mut ui1 = Renderer::new(&gpu1.device, OFFSCREEN_FORMAT);
    ui1.attach_text(&mut rt.text.atlas);
    let off1 = Offscreen::new(&gpu1.device, size.0, size.1);
    draw_on(&mut gfx, &mut rt, &gpu1, &mut ui1, &off1, 0.0);
    let key1 = ui1.device_key();
    assert_eq!(gfx.res.as_ref().unwrap().device_key, key1);

    // Replace the device and renderer at the same viewport size and target format,
    // then switch to the lit Lamp scene.
    let gpu2 = Gpu::headless();
    let mut ui2 = Renderer::new(&gpu2.device, OFFSCREEN_FORMAT);
    let key2 = ui2.device_key();
    assert_ne!(key1, key2);
    let off2 = Offscreen::new(&gpu2.device, size.0, size.1);
    drop((off1, ui1, gpu1));
    rt.text.atlas.invalidate();
    rt.images.invalidate();
    ui2.attach_text(&mut rt.text.atlas);
    rt.state.scene = Scene::Lamp;
    rt.invalidate();

    draw_on(&mut gfx, &mut rt, &gpu2, &mut ui2, &off2, 1.0 / 60.0);
    assert_eq!(gfx.res.as_ref().unwrap().device_key, key2);
    assert!(!gpu2.is_lost());
}
