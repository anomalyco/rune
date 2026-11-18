//! `RUNE_LOOP_CHECK=2` (Windows): a loop-only frame's picture against the whole frame's. After a
//! loop-only frame redraws its rectangles into the kept texture (`draw_armed`), the same instances
//! are drawn whole into a scratch texture in the same encoder and both are copied out; the next
//! loop-only frame (`arm_loop`) reads them back and compares every pixel. This reaches what the
//! instance comparison (`RUNE_LOOP_CHECK=1`) can't: `arm_loop`'s rectangles, the culled redraw
//! inside them, and so the present rectangles, which are the same rectangles. Stalls on the
//! readback; a diagnostic only. Unset, it costs one `OnceLock` read per loop-only frame.
//!
//! Each check prints a line when any pixel differs (how many, by more than 2 of 255 in a channel,
//! their bounds, and the frame's rectangles), and every 240 checks a running count.

use super::*;

pub(super) fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RUNE_LOOP_CHECK").is_some_and(|v| v == "2"))
}

/// The scratch target, the two readback buffers, and what the last encode left in them.
pub(super) struct Check {
    size: (u32, u32),
    format: wgpu::TextureFormat,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    kept: wgpu::Buffer,
    whole: wgpu::Buffer,
    row: u32,
    pending: Option<Vec<[u32; 4]>>,
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
        Check { size, format, texture, view, kept: buffer("loop pixel check: kept"), whole: buffer("loop pixel check: whole"), row, pending: None, checks: 0, differ: 0 }
    }

    /// Reads back and compares what the last loop-only frame left, if anything.
    fn resolve(&mut self, device: &wgpu::Device) {
        let Some(rects) = self.pending.take() else { return };
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
            eprintln!("rune: loop pixel check: {any} pixels differ from the whole frame, {far} by more than 2/255{}; the frame's rectangles {rects:?}",
                if far > 0 { format!(" within {bounds:?}") } else { String::new() });
        }
        if self.checks % 240 == 0 { eprintln!("rune: loop pixel check: {} loop-only frames compared with whole frames, {} differ", self.checks, self.differ); }
    }
}

impl Renderer {
    /// `arm_loop`'s part: compares the last loop-only frame's pictures, and has a scratch target
    /// for this frame's.
    pub(super) fn pixel_check_arm(&self, device: &wgpu::Device, size: (u32, u32), format: wgpu::TextureFormat) {
        let mut d = self.damage.lock().unwrap();
        if let Some(c) = d.pixel.as_mut() { c.resolve(device); }
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
        c.pending = Some(rects.to_vec());
    }
}
