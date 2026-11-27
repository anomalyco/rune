//! An app embedding its own compute renderer. A compute shader draws a moving pattern into a
//! texture, with its workgroup counts written on the GPU and read by an indirect dispatch, as a
//! compute renderer like vello sizes its work; the compositor paints that texture at an
//! `El::effect` slot inside Rune's own pass (`Renderer::draw_with`). The renderer's pipelines are
//! built on a worker thread through `Frame::gpu_handle`, with a pipeline cache saved for the next
//! launch, while Rune draws the first frames. On macOS it runs on Rune's native Metal backend.

mod gfx;

use rune::tok::{self, type_};
use rune::{App, AppSpec, Cx, El, col, text};

#[derive(Default)]
struct State;

fn view(_: &State, _: &mut Cx<State>) -> El {
    let t = tok::theme(true);
    col().w_full().h_full().center().gap(12.0).fill(t.bg)
        .child(text("A compute shader at an effect slot").font(type_::TITLE).color(t.text))
        .child(rune::effect(gfx::SLOT).key("pattern").size(gfx::WIDTH, gfx::HEIGHT))
}

fn main() {
    App::new(AppSpec::new("GPU compute", (400.0, 300.0), State::default, view)).compositor(gfx::Pattern::new(gfx::cache_file())).run();
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::compose::{Compositor, Frame};
    use rune::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
    use rune::{Env, Runtime};

    /// Draws one frame through the compositor and returns the window's pixels.
    fn frame(pattern: &mut gfx::Pattern, rt: &mut Runtime<State>, gpu: &Gpu, ui: &mut Renderer, off: &Offscreen, dt: f64) -> Vec<u8> {
        rt.frame(dt);
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        let mut f = Frame { device: &gpu.device, queue: &gpu.queue, encoder: &mut encoder, target: &off.view, format: OFFSCREEN_FORMAT,
            size: (off.width, off.height), dt, update_ms: 0.0, rt, ui };
        pattern.render(&mut f);
        off.copy(&mut encoder);
        ui.submit(&gpu.queue, encoder);
        off.read(&gpu.device)
    }

    #[test]
    fn the_compute_renderer_paints_the_slot_and_moves() {
        // The device a window makes for this compositor: with the features it asks for.
        let Ok(gpu) = Gpu::try_with_features(Gpu::instance(), None, gfx::Pattern::default().features()) else { return };
        let (w, h) = (400u32, 300u32);
        let mut rt = Runtime::new(State, view, (w as f32, h as f32), Env { system_dark: true, scale: 1.0 });
        let mut ui = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
        ui.attach_text(&mut rt.text.atlas);
        let off = Offscreen::new(&gpu.device, w, h);
        let dir = std::env::temp_dir().join(format!("gpu-compute-test-{}", std::process::id()));
        let file = dir.join("pipelines.bin");
        let mut pattern = gfx::Pattern::new(Some(file.clone()));
        let blank = frame(&mut pattern, &mut rt, &gpu, &mut ui, &off, 0.0);
        let rect = rt.presented("pattern").expect("the slot is laid out").0;
        let at = |px: &[u8]| {
            let (x, y) = ((rect[0] + rect[2] / 2.0) as u32, (rect[1] + rect[3] / 2.0) as u32);
            let i = ((y * w + x) * 4) as usize;
            px[i..i + 4].to_vec()
        };
        // The renderer is the test's own, so the pattern is built in the first frame.
        let (background, shown) = (blank[..4].to_vec(), blank);
        assert_ne!(at(&shown), background, "the slot shows the compute shader's output");
        let later = frame(&mut pattern, &mut rt, &gpu, &mut ui, &off, 0.5);
        assert_ne!(at(&later), at(&shown), "and it moves");
        let cached = gpu.device.features().contains(wgpu::Features::PIPELINE_CACHE);
        assert_eq!(file.exists(), cached, "the worker saved the pipeline cache where the device has one");
        std::fs::remove_dir_all(dir).ok();
    }
}
