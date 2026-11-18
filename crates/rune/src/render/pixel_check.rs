//! `RUNE_LOOP_CHECK=2` (Windows): a loop-only frame's picture against the whole frame's. After a
//! loop-only frame redraws its rectangles into the kept texture (`draw_armed`), the same instances
//! are drawn whole into a scratch texture in the same encoder and both are copied out; the next
//! frame of any kind (`damage_drawn`, once the encoding frame was submitted), or the next loop-only
//! frame (`arm_loop`), or the renderer going away, reads them back and compares every pixel, so the
//! last check of a run is compared too. This reaches what the
//! instance comparison (`RUNE_LOOP_CHECK=1`) can't: `arm_loop`'s rectangles, the culled redraw
//! inside them, and so the present rectangles, which are the same rectangles. Stalls on the
//! readback; a diagnostic only. Unset, it costs one `OnceLock` read per loop-only frame.
//!
//! Each check prints a line when any pixel differs (how many, by more than 2 of 255 in a channel,
//! their bounds, and the frame's rectangles), and every 240 checks a running count. A differing check
//! is noted for the whole process (`failed`): the process then ends with exit code 3, whether it
//! exits fast or in order (`quit::end_process`, `event_loop::run_configured`).

use super::*;

/// A check found a differing picture, in any window, at any time in this process.
static FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `RUNE_LOOP_CHECK=2` is set and a check found a differing picture: the process's exit code is 3.
pub(crate) fn failed() -> bool { on() && FAILED.load(std::sync::atomic::Ordering::Relaxed) }

/// frame-profile, `RUNE_LOOP_CHECK_SPOIL=1`: loop-only frames skip their redraw (`draw_armed`).
#[cfg(feature = "frame-profile")]
pub(super) fn spoiled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RUNE_LOOP_CHECK_SPOIL").is_some_and(|v| v == "1"))
}

pub(super) fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RUNE_LOOP_CHECK").is_some_and(|v| v == "2"))
}

/// The scratch target, the two readback buffers, and what the last encode left in them.
pub(super) struct Check {
    device: wgpu::Device,
    size: (u32, u32),
    format: wgpu::TextureFormat,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    kept: wgpu::Buffer,
    whole: wgpu::Buffer,
    row: u32,
    /// The rectangles of the frame whose pictures the buffers hold, and how many frames have ended
    /// since it encoded them (1: its own, not yet submitted; 2: submitted).
    pending: Option<(Vec<[u32; 4]>, u8)>,
    checks: u64,
    differ: u64,
}

impl Check {
    fn new(device: &wgpu::Device, size: (u32, u32), format: wgpu::TextureFormat, view_format: wgpu::TextureFormat) -> Check {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("loop pixel check"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[view_format],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor { format: Some(view_format), ..Default::default() });
        let row = (size.0 * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = |label| device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: u64::from(row) * u64::from(size.1),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        Check { device: device.clone(), size, format, texture, view, kept: buffer("loop pixel check: kept"), whole: buffer("loop pixel check: whole"), row, pending: None, checks: 0, differ: 0 }
    }

    /// Reads back and compares what the last loop-only frame left, if anything.
    fn resolve(&mut self) {
        let Some((rects, _)) = self.pending.take() else { return };
        let device = self.device.clone();
        let (a, b) = (self.kept.slice(..), self.whole.slice(..));
        a.map_async(wgpu::MapMode::Read, |_| {});
        b.map_async(wgpu::MapMode::Read, |_| {});
        if device.poll(wgpu::PollType::wait_indefinitely()).is_err() { return; }
        let (mut any, mut far, mut bounds) = (0u64, 0u64, [u32::MAX, u32::MAX, 0, 0]);
        {
            let (Ok(ka), Ok(wb)) = (a.get_mapped_range(), b.get_mapped_range()) else { self.kept.unmap(); self.whole.unmap(); return };
            for y in 0..self.size.1 {
                let at = (y * self.row) as usize;
                let (kr, wr) = (&ka[at..at + self.size.0 as usize * 4], &wb[at..at + self.size.0 as usize * 4]);
                if kr == wr { continue; }
                for x in 0..self.size.0 as usize {
                    let (p, q) = (&kr[x * 4..x * 4 + 4], &wr[x * 4..x * 4 + 4]);
                    if p == q { continue; }
                    any += 1;
                    if p.iter().zip(q).any(|(u, v)| u.abs_diff(*v) > 2) {
                        far += 1;
                        let x = x as u32;
                        bounds = [bounds[0].min(x), bounds[1].min(y), bounds[2].max(x + 1), bounds[3].max(y + 1)];
                    }
                }
            }
        }
        self.kept.unmap();
        self.whole.unmap();
        self.checks += 1;
        if any > 0 {
            self.differ += 1;
            FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
            eprintln!("rune: loop pixel check: {any} pixels differ from the whole frame, {far} by more than 2/255{}; the frame's rectangles {rects:?}",
                if far > 0 { format!(" within {bounds:?}") } else { String::new() });
        }
        if self.checks % 240 == 0 { eprintln!("rune: loop pixel check: {} loop-only frames compared with whole frames, {} differ", self.checks, self.differ); }
    }
}

impl Check {
    /// A frame ended (`damage_drawn`, before its submit): the pictures the frame before it copied
    /// out are on the GPU's queue by now, and are compared.
    pub(super) fn frame_ended(&mut self) {
        let Some((_, age)) = self.pending.as_mut() else { return };
        *age += 1;
        if *age >= 2 { self.resolve(); }
    }
}

impl Drop for Check {
    fn drop(&mut self) {
        self.resolve();
        if self.checks > 0 { eprintln!("rune: loop pixel check: {} loop-only frames compared with whole frames, {} differ (at the end)", self.checks, self.differ); }
    }
}

impl Renderer {
    /// `arm_loop`'s part: compares the last loop-only frame's pictures, and has a scratch target
    /// for this frame's.
    pub(super) fn pixel_check_arm(&self, device: &wgpu::Device, size: (u32, u32), format: wgpu::TextureFormat) {
        let mut d = self.damage.lock().unwrap();
        if let Some(c) = d.pixel.as_mut() { c.resolve(); }
        if d.pixel.as_ref().is_none_or(|c| c.size != size || c.format != format) { d.pixel = Some(Check::new(device, size, format, self.format)); }
        d.looped = true;
    }

    /// `draw_armed`'s part, after a loop-only frame redrew `kept`: the whole frame into the scratch
    /// target, and both copied out for the next `pixel_check_arm`.
    pub(super) fn pixel_check_encode(&self, encoder: &mut wgpu::CommandEncoder, c: &mut Check, kept: &wgpu::Texture, clear: [f32; 4], rects: &[[u32; 4]]) {
        if (kept.width(), kept.height()) != c.size { return; }
        self.draw_timed(encoder, &c.view, clear, None);
        let extent = wgpu::Extent3d { width: c.size.0, height: c.size.1, depth_or_array_layers: 1 };
        let layout = wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(c.row), rows_per_image: Some(c.size.1) };
        encoder.copy_texture_to_buffer(kept.as_image_copy(), wgpu::TexelCopyBufferInfo { buffer: &c.kept, layout }, extent);
        encoder.copy_texture_to_buffer(c.texture.as_image_copy(), wgpu::TexelCopyBufferInfo { buffer: &c.whole, layout }, extent);
        c.pending = Some((rects.to_vec(), 0));
    }
}
