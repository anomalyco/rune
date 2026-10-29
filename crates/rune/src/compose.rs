//! Compositor hook: an app takes over drawing each frame (lighting, effect layers) while Rune
//! keeps layout, animation, input, the window and the headless tooling. `rune::run` never
//! builds one, so the default path is unchanged; `rune::run_with` or `App::compositor` attaches
//! one.

use crate::render::{Instance, KIND_GLYPH, Renderer};
use crate::runtime::Runtime;

pub trait Compositor<S: 'static>: 'static {
    /// Device features to request when the adapter has them (e.g. `TIMESTAMP_QUERY`).
    fn features(&self) -> wgpu::Features {
        wgpu::Features::empty()
    }

    /// Draws the frame into `f.target`. Runs after `Runtime::frame` built the instances, in the
    /// window and headless alike. Nothing has been recorded yet; `El::pixels` images are uploaded.
    fn render(&mut self, f: &mut Frame<'_, S>);

    /// True while something the compositor draws moves on its own: the window keeps its display
    /// link running. False lets it go idle (no frames at rest) and rest: a due `wake_after`
    /// runs `poll` alone and draws only when that poll changed something.
    fn animating(&self) -> bool {
        false
    }

    /// Printed after a headless run (e.g. GPU timings).
    fn summary(&mut self, _device: &wgpu::Device) -> Option<String> {
        None
    }
}

/// One frame handed to a compositor.
pub struct Frame<'a, S: 'static> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub encoder: &'a mut wgpu::CommandEncoder,
    /// The window surface or the offscreen target.
    pub target: &'a wgpu::TextureView,
    pub format: wgpu::TextureFormat,
    /// Target size in device pixels.
    pub size: (u32, u32),
    /// Real seconds since the previous frame (0 on the first).
    pub dt: f64,
    /// CPU milliseconds `Runtime::frame` took this frame (handlers, view, layout, animation).
    pub update_ms: f64,
    /// Instances, `marks`, the clock (`t`), the pointer, element rects and app state.
    pub rt: &'a mut Runtime<S>,
    /// Rune's own renderer: `prepare_lists`, `encode_layers`, then `draw_range` draw complete
    /// painter scopes into any target (`add_format` for non-target formats). Layer begin/end
    /// records must remain balanced; do not partition a scope by primitive kind.
    pub ui: &'a mut Renderer,
}

impl<S: 'static> Frame<'_, S> {
    /// True when the frame is sRGB-blended (`App::srgb_blend`): `target` is a non-sRGB view and
    /// premultiplied colours, in `ui` passes and in layers, are sRGB-encoded. False: linear
    /// premultiplied values, encoded by the `*Srgb` target on write. Build effect pipelines and
    /// encode their output to match (`FxRenderer::set_srgb_blend` in rune-fx).
    pub fn srgb_blend(&self) -> bool {
        self.ui.srgb_blend()
    }

    /// `queue.write_buffer` without its per-call staging buffer: `data` goes through Rune's
    /// reused staging memory and lands in `buffer` at `offset` ahead of the commands recorded
    /// into `encoder` after this call. `offset` and `data.len()` are multiples of 4.
    pub fn write_buffer(&mut self, buffer: &wgpu::Buffer, offset: u64, data: &[u8]) {
        self.ui.write_buffer(self.device, self.queue, self.encoder, buffer, offset, data);
    }

    /// The default frame: every instance into the target, cleared to the window colour.
    pub fn draw_ui(&mut self) {
        let clear = self.rt.clear_color();
        self.ui.prepare(self.device, self.queue, &mut self.rt.text.atlas, &self.rt.instances, self.size);
        self.ui.draw(self.encoder, self.target, clear);
    }
}

pub fn is_glyph(i: &Instance) -> bool {
    i.params[1] == KIND_GLYPH
}
