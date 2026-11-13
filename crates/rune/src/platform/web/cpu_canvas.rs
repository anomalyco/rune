//! A page's frames painted on the CPU (`render::cpu_boot`), from the module's first frame until
//! WebGPU takes over, and for good where it never does. The painter keeps a BGRA buffer of the
//! frame it painted last and says which rectangles changed; those go to a 2D canvas under the
//! GPU's, turned into RGBA on the way. The GPU's canvas lies over it, see-through until the
//! GPU's first frame is done (`web.rs`), and takes the page's input all along.

use std::cell::Cell;

use wasm_bindgen::{JsCast, JsValue};

use crate::Runtime;
use crate::render::cpu_boot::{CpuPainter, HalfRounding, NeedsGpu, Scene, Target};

pub(super) struct CpuCanvas {
    canvas: web_sys::HtmlCanvasElement,
    ctx: web_sys::CanvasRenderingContext2d,
    painter: CpuPainter,
    /// The frame painted last, `size.0 * 4` bytes a row, B, G, R, A.
    buffer: Vec<u8>,
    size: (u32, u32),
    /// One row of a rectangle in RGBA, on its way to `staged`.
    rows: Vec<u8>,
    /// The rectangle's rows in the page's own memory: an image made over the module's memory takes the
    /// browser tens of milliseconds to put (all of it, for a full frame), one over this a few
    /// hundred microseconds.
    staged: js_sys::Uint8ClampedArray,
    /// How the GPU that takes over rounds half floats: once it exists, its own.
    pub(super) half: HalfRounding,
}

/// What a CPU frame cost, for a page that asks (`globalThis.__runeCpuFrames`, an array a
/// measuring harness sets): microseconds painting and putting, and pixels put.
#[derive(Default)]
pub(super) struct Cost {
    /// Filling the frame's new glyph masks.
    pub fill_us: f64,
    /// Binning, hashing tiles and painting the changed ones.
    pub paint_us: f64,
    /// Of that, painting.
    pub raster_us: f64,
    /// Turning the changed rectangles into RGBA and putting them on the canvas.
    pub put_us: f64,
    /// Of that, turning them into RGBA.
    pub swizzle_us: f64,
    pub put_px: u64,
}

thread_local! {
    /// Whether the page asked for each frame's cost, read once.
    static STATS: Cell<Option<bool>> = const { Cell::new(None) };
}

impl CpuCanvas {
    /// A 2D canvas laid out exactly as `over` (the GPU's), drawn under it and taking no input.
    pub(super) fn under(over: &web_sys::HtmlCanvasElement) -> Option<CpuCanvas> {
        let document = over.owner_document()?;
        let canvas: web_sys::HtmlCanvasElement = document.create_element("canvas").ok()?.dyn_into().ok()?;
        let css = over.style().css_text();
        // The GPU's canvas starts see-through: this one shows.
        canvas.style().set_css_text(&format!("{css};pointer-events:none;opacity:1"));
        let _ = canvas.set_attribute("aria-hidden", "true");
        over.parent_node()?.insert_before(&canvas, Some(over)).ok()?;
        let options = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&options, &"alpha".into(), &JsValue::FALSE);
        let ctx = canvas.get_context_with_context_options("2d", &options).ok()??.dyn_into().ok()?;
        Some(CpuCanvas { canvas, ctx, painter: CpuPainter::new(1), buffer: Vec::new(), size: (0, 0), rows: Vec::new(), staged: js_sys::Uint8ClampedArray::new_with_length(0), half: HalfRounding::default() })
    }

    /// Paints the runtime's last frame at `size` physical pixels and puts what changed on the
    /// canvas. A frame only the GPU can draw (`Scene::needs_gpu`) leaves the last one up.
    pub(super) fn paint<S: 'static>(&mut self, rt: &mut Runtime<S>, size: (u32, u32)) -> Result<Cost, NeedsGpu> {
        let (w, h) = (size.0.max(1), size.1.max(1));
        if self.size != (w, h) {
            // A new size clears the canvas, and the buffer starts again: every tile paints.
            self.canvas.set_width(w);
            self.canvas.set_height(h);
            self.buffer.clear();
            self.buffer.resize(w as usize * h as usize * 4, 0);
            self.painter.forget();
            self.size = (w, h);
        }
        let start = crate::time::Instant::now();
        self.painter.fill_masks(&mut rt.text.atlas);
        let filled = crate::time::Instant::now();
        let scene = Scene::new(&rt.instances, &rt.text.atlas, &rt.images, (w, h), rt.clear_color())
            .srgb_blend(rt.srgb_blend())
            .native_surfaces(!rt.surfaces.is_empty())
            .half_rounding(self.half)
            .font_smoothing(rt.font_smoothing());
        let painted = self.painter.paint(&scene, &mut Target::new(&mut self.buffer, w, h))?;
        let put = crate::time::Instant::now();
        let mut px = 0u64;
        let mut swizzle = std::time::Duration::ZERO;
        for &[x0, y0, x1, y1] in &painted.rects {
            let (rw, rh) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
            if rw == 0 || rh == 0 { continue; }
            let began = crate::time::Instant::now();
            let line = rw as usize * 4;
            let len = line * rh as usize;
            if (self.staged.length() as usize) < len {
                self.staged = js_sys::Uint8ClampedArray::new_with_length((w as usize * h as usize * 4).max(len) as u32);
            }
            // A row at a time through one row's buffer: a whole frame's RGBA copy in the module's
            // memory (4 MB at 1280x800) would stay in its high-water mark for good.
            self.rows.resize(line, 0);
            for (i, y) in (y0..y1).enumerate() {
                let at = (y as usize * w as usize + x0 as usize) * 4;
                rgba(&self.buffer[at..at + line], &mut self.rows);
                self.staged.subarray((i * line) as u32, ((i + 1) * line) as u32).copy_from(&self.rows);
            }
            swizzle += began.elapsed();
            let staged = self.staged.subarray(0, len as u32);
            if let Ok(image) = web_sys::ImageData::new_with_js_u8_clamped_array_and_sh(&staged, rw, rh) {
                let _ = self.ctx.put_image_data(&image, f64::from(x0), f64::from(y0));
            }
            px += u64::from(rw) * u64::from(rh);
        }
        let end = crate::time::Instant::now();
        let us = |d: std::time::Duration| d.as_secs_f64() * 1e6;
        let cost = Cost { fill_us: us(filled - start), paint_us: us(put - filled), raster_us: painted.report.raster_ns as f64 / 1e3, put_us: us(end - put), swizzle_us: us(swizzle), put_px: px };
        report(&cost);
        Ok(cost)
    }

    /// The frame painted last: B, G, R, A, `size.0 * 4` bytes a row.
    pub(super) fn pixels(&self) -> &[u8] { &self.buffer }

    /// The canvas goes: the GPU's frames are on screen in its place. Its pixels go now (a 0x0
    /// canvas has none), not when the page next collects garbage: 15.6 MB at 1280x800 until then.
    pub(super) fn remove(self) {
        self.canvas.set_width(0);
        self.canvas.set_height(0);
        self.canvas.remove();
    }
}

/// `bgra`'s pixels as R, G, B, A, into `out` (as long).
fn rgba(bgra: &[u8], out: &mut [u8]) {
    let swap = |v: u32| (v & 0xff00_ff00) | ((v >> 16) & 0xff) | ((v & 0xff) << 16);
    // Whole pixels as words where both sides line up (they do: rows start on whole pixels of
    // buffers the allocator aligns).
    // SAFETY: any bit pattern is a valid u32 and u8; `align_to` keeps the parts it can't align.
    let (dh, dm, dt) = unsafe { out.align_to_mut::<u32>() };
    let (sh, sm, st) = unsafe { bgra.align_to::<u32>() };
    if !(dh.is_empty() && sh.is_empty() && dt.is_empty() && st.is_empty()) {
        for (d, s) in out.chunks_exact_mut(4).zip(bgra.chunks_exact(4)) {
            d.copy_from_slice(&[s[2], s[1], s[0], s[3]]);
        }
        return;
    }
    #[cfg(target_feature = "simd128")]
    {
        use core::arch::wasm32::{i8x16, u8x16_swizzle, v128, v128_load, v128_store};
        // Four pixels at a time: B, G, R, A to R, G, B, A.
        const ORDER: v128 = i8x16(2, 1, 0, 3, 6, 5, 4, 7, 10, 9, 8, 11, 14, 13, 12, 15);
        let whole = dm.len() / 4 * 4;
        let (dq, dr) = dm.split_at_mut(whole);
        let (sq, sr) = sm.split_at(whole);
        for (d, s) in dq.chunks_exact_mut(4).zip(sq.chunks_exact(4)) {
            // SAFETY: each chunk is four u32s, 16 bytes; v128 loads and stores need no alignment.
            unsafe { v128_store(d.as_mut_ptr().cast(), u8x16_swizzle(v128_load(s.as_ptr().cast()), ORDER)) }
        }
        for (d, s) in dr.iter_mut().zip(sr) { *d = swap(*s); }
    }
    #[cfg(not(target_feature = "simd128"))]
    for (d, s) in dm.iter_mut().zip(sm) { *d = swap(*s); }
}
/// Appends a frame's cost to `globalThis.__runeCpuFrames`, when the page made that array.
fn report(cost: &Cost) {
    let wanted = STATS.get().unwrap_or_else(|| {
        let on = js_sys::Reflect::get(&js_sys::global(), &"__runeCpuFrames".into()).is_ok_and(|v| js_sys::Array::is_array(&v));
        STATS.set(Some(on));
        on
    });
    if !wanted { return; }
    let Ok(list) = js_sys::Reflect::get(&js_sys::global(), &"__runeCpuFrames".into()) else { return };
    let list: js_sys::Array = list.unchecked_into();
    let row = js_sys::Array::new();
    for v in [cost.fill_us, cost.paint_us, cost.raster_us, cost.put_us, cost.swizzle_us, cost.put_px as f64] { row.push(&v.into()); }
    list.push(&row);
}
