//! A page's frames painted on the CPU (`render::cpu_boot`), from the module's first frame until
//! WebGPU takes over, and for good where it never does. The painter keeps a BGRA buffer of the
//! frame it painted last and says which rectangles changed; those go to a 2D canvas under the
//! GPU's, turned into RGBA on the way. The GPU's canvas lies over it, see-through until the
//! GPU's first frame is done (`web.rs`), and takes the page's input all along.

use std::cell::Cell;

use wasm_bindgen::{Clamped, JsCast, JsValue};

use crate::Runtime;
use crate::render::cpu_boot::{CpuPainter, HalfRounding, NeedsGpu, Scene, Target};

pub(super) struct CpuCanvas {
    canvas: web_sys::HtmlCanvasElement,
    ctx: web_sys::CanvasRenderingContext2d,
    painter: CpuPainter,
    /// The frame painted last, `size.0 * 4` bytes a row, B, G, R, A.
    buffer: Vec<u8>,
    size: (u32, u32),
    /// One rectangle's RGBA rows on their way to the canvas.
    rows: Vec<u8>,
    /// How the GPU that takes over rounds half floats: once it exists, its own.
    pub(super) half: HalfRounding,
}

/// What a CPU frame cost, for a page that asks (`globalThis.__runeCpuFrames`, an array a
/// measuring harness sets): microseconds painting and putting, and pixels put.
#[derive(Default)]
pub(super) struct Cost {
    pub paint_us: f64,
    pub put_us: f64,
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
        canvas.style().set_css_text(&format!("{css};pointer-events:none"));
        let _ = canvas.set_attribute("aria-hidden", "true");
        over.parent_node()?.insert_before(&canvas, Some(over)).ok()?;
        let options = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&options, &"alpha".into(), &JsValue::FALSE);
        let ctx = canvas.get_context_with_context_options("2d", &options).ok()??.dyn_into().ok()?;
        Some(CpuCanvas { canvas, ctx, painter: CpuPainter::new(1), buffer: Vec::new(), size: (0, 0), rows: Vec::new(), half: HalfRounding::default() })
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
        let scene = Scene::new(&rt.instances, &rt.text.atlas, &rt.images, (w, h), rt.clear_color())
            .srgb_blend(rt.srgb_blend())
            .native_surfaces(!rt.surfaces.is_empty())
            .half_rounding(self.half)
            .font_smoothing(rt.font_smoothing());
        let painted = self.painter.paint(&scene, &mut Target::new(&mut self.buffer, w, h))?;
        let put = crate::time::Instant::now();
        let mut px = 0u64;
        for &[x0, y0, x1, y1] in &painted.rects {
            let (rw, rh) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
            if rw == 0 || rh == 0 { continue; }
            self.rows.clear();
            self.rows.reserve(rw as usize * rh as usize * 4);
            for y in y0..y1 {
                let at = (y as usize * w as usize + x0 as usize) * 4;
                rgba(&self.buffer[at..at + rw as usize * 4], &mut self.rows);
            }
            if let Ok(image) = web_sys::ImageData::new_with_u8_clamped_array_and_sh(Clamped(&self.rows), rw, rh) {
                let _ = self.ctx.put_image_data(&image, f64::from(x0), f64::from(y0));
            }
            px += u64::from(rw) * u64::from(rh);
        }
        let end = crate::time::Instant::now();
        let cost = Cost { paint_us: (put - start).as_secs_f64() * 1e6, put_us: (end - put).as_secs_f64() * 1e6, put_px: px };
        report(&cost);
        Ok(cost)
    }

    /// The canvas goes: the GPU's frames are on screen in its place.
    pub(super) fn remove(self) {
        self.canvas.remove();
    }
}

/// `bgra`'s pixels as R, G, B, A, appended to `out`.
fn rgba(bgra: &[u8], out: &mut Vec<u8>) {
    out.extend(bgra.chunks_exact(4).flat_map(|p| {
        let v = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
        ((v & 0xff00_ff00) | ((v >> 16) & 0xff) | ((v & 0xff) << 16)).to_le_bytes()
    }));
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
    list.push(&js_sys::Array::of3(&cost.paint_us.into(), &cost.put_us.into(), &(cost.put_px as f64).into()));
}
