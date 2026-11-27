//! A page's frames painted on the CPU (`render::cpu_boot`), from the module's first frame until
//! WebGPU takes over, and for good where it never does. The painter keeps a BGRA buffer of the
//! frame it painted last and says which rectangles changed; those go to a 2D canvas under the
//! GPU's, swizzled to RGBA into a reusable JS-heap `ImageData` and uploaded in one
//! `putImageData(imageData, 0, 0, dx, dy, dw, dh)` call per dirty rectangle (`cpu_canvas.js`).
//! The GPU's canvas lies over it, see-through until the GPU's first frame is done (`web.rs`),
//! and takes the page's input all along.

use std::cell::Cell;

use wasm_bindgen::{JsCast, JsValue, prelude::*};

use super::web_math::cpu_canvas::{
    NeedsGpuLogMask, WebCpuNeedsGpuAction, classify_cpu_needs_gpu, dirty_rect_row_slice,
};
use crate::render::cpu_boot::{CpuPainter, HalfRounding, NeedsGpu, Scene, Target};
use crate::runtime::RuntimeCore;

#[wasm_bindgen(module = "/src/platform/web/cpu_canvas.js")]
extern "C" {
    #[wasm_bindgen(catch, js_name = putCpuRect)]
    fn put_cpu_rect(
        ctx: &web_sys::CanvasRenderingContext2d,
        image: &web_sys::ImageData,
        bgra_rows: &[u8],
        canvas_width: u32,
        x0: u32,
        y0: u32,
        rw: u32,
        rh: u32,
        measure: bool,
    ) -> Result<f64, JsValue>;
}

pub(super) struct CpuCanvas {
    over: web_sys::HtmlCanvasElement,
    canvas: web_sys::HtmlCanvasElement,
    ctx: web_sys::CanvasRenderingContext2d,
    painter: CpuPainter,
    /// The frame painted last, `size.0 * 4` bytes a row, B, G, R, A.
    buffer: Vec<u8>,
    size: (u32, u32),
    /// Reusable full-canvas `ImageData` in the page's own memory: an `ImageData` backed directly
    /// by WASM linear memory takes Chromium tens of milliseconds to put, whereas uploading dirty
    /// rectangles through a JS-heap `ImageData` with `putImageData(image, 0, 0, x0, y0, rw, rh)`
    /// avoids per-scanline WASM<->JS crossings and keeps WASM linear memory free of a duplicate
    /// RGBA framebuffer.
    image: Option<web_sys::ImageData>,
    /// Whether the last painted frame used `Scene::degraded(true)` fallback mode (so hand-over
    /// drift counting skips comparing deliberately degraded CPU pixels against the GPU frame).
    last_degraded: bool,
    /// Once-per-reason console warning mask for `NeedsGpu` degradation or hold events.
    logged_needs_gpu: NeedsGpuLogMask,
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
        canvas.set_attribute("aria-hidden", "true").ok();
        canvas.set_attribute("data-rune-cpu", "").ok();
        over.parent_node()?.insert_before(&canvas, Some(over)).ok()?;
        let options = js_sys::Object::new();
        js_sys::Reflect::set(&options, &"alpha".into(), &JsValue::FALSE).ok();
        let ctx: web_sys::CanvasRenderingContext2d = canvas.get_context_with_context_options("2d", &options).ok()??.dyn_into().ok()?;
        ctx.set_image_smoothing_enabled(false);
        Some(CpuCanvas {
            over: over.clone(),
            canvas,
            ctx,
            painter: CpuPainter::new(1),
            buffer: Vec::new(),
            size: (0, 0),
            image: None,
            last_degraded: false,
            logged_needs_gpu: NeedsGpuLogMask::default(),
            half: HalfRounding::default(),
        })
    }

    /// Keeps the CPU canvas's CSS bounds aligned with `over` if its inline box changed outside
    /// `platform.js::viewport`.
    fn sync_bounds(&self) {
        let src = self.over.style();
        let dst = self.canvas.style();
        for prop in ["width", "height", "left", "top", "transform"] {
            let value = src.get_property_value(prop).unwrap_or_default();
            if dst.get_property_value(prop).unwrap_or_default() != value {
                if value.is_empty() {
                    dst.remove_property(prop).ok();
                } else {
                    dst.set_property(prop, &value).ok();
                }
            }
        }
    }

    /// Paints the runtime's last frame at `size` physical pixels and puts what changed on the
    /// canvas. Degradable GPU-only features (`LayerBlur`, `TurnedLayer`, nested `BackdropBlur`,
    /// `Effect`, `NativeSurface`, `LinearBlend`) paint live via `Scene::degraded(true)` instead
    /// of freezing frames; non-degradable reasons (`SlugText`, `OutlineGlyph`, `UnknownKind`,
    /// `Target`) log once, leave the last frame up, and return `Err(reason)`.
    pub(super) fn paint(&mut self, rt: &mut RuntimeCore, size: (u32, u32)) -> Result<Cost, NeedsGpu> {
        let (w, h) = (size.0.max(1), size.1.max(1));
        if self.size != (w, h) {
            self.sync_bounds();
            // A new size clears the canvas, and the buffer starts again: every tile paints.
            self.canvas.set_width(w);
            self.canvas.set_height(h);
            // Resizing resets the 2D context state back to defaults.
            self.ctx.set_image_smoothing_enabled(false);
            self.buffer.clear();
            self.buffer.resize(w as usize * h as usize * 4, 0);
            self.image = self.ctx.create_image_data_with_sw_and_sh(f64::from(w), f64::from(h)).ok();
            self.painter.forget();
            self.size = (w, h);
        } else if self.image.is_none() {
            self.image = self.ctx.create_image_data_with_sw_and_sh(f64::from(w), f64::from(h)).ok();
        }
        let start = crate::time::Instant::now();
        self.painter.fill_masks(&mut rt.text.atlas);
        let filled = crate::time::Instant::now();
        let scene = Scene::new(&rt.instances, &rt.text.atlas, &rt.images, (w, h), rt.clear_color())
            .srgb_blend(rt.srgb_blend())
            .fade_stops(rt.fade_stops())
            .native_surfaces(!rt.surfaces.is_empty())
            .half_rounding(self.half)
            .font_smoothing(rt.font_smoothing());
        let (painted, degraded) = match self.painter.paint(&scene, &mut Target::new(&mut self.buffer, w, h)) {
            Ok(painted) => (painted, false),
            Err(reason) => match classify_cpu_needs_gpu(reason) {
                WebCpuNeedsGpuAction::PaintDegraded(why) => {
                    if self.logged_needs_gpu.note(why) {
                        web_sys::console::warn_1(
                            &format!(
                                "rune: web CPU canvas degraded GPU-only feature {why:?} so frames stay live until GPU hand-over"
                            )
                            .into(),
                        );
                    }
                    let degraded_scene = scene.degraded(true);
                    match self.painter.paint(&degraded_scene, &mut Target::new(&mut self.buffer, w, h)) {
                        Ok(painted) => {
                            crate::marks::mark("cpu_first.degraded_paint");
                            (painted, true)
                        }
                        Err(non_degradable) => {
                            self.last_degraded = false;
                            if self.logged_needs_gpu.note(non_degradable) {
                                web_sys::console::warn_1(
                                    &format!(
                                        "rune: web CPU canvas cannot paint {non_degradable:?}; holding previous frame for GPU"
                                    )
                                    .into(),
                                );
                            }
                            crate::marks::mark("cpu_first.needs_gpu");
                            return Err(non_degradable);
                        }
                    }
                }
                WebCpuNeedsGpuAction::HoldPreviousFrame(why) => {
                    self.last_degraded = false;
                    if self.logged_needs_gpu.note(why) {
                        web_sys::console::warn_1(
                            &format!(
                                "rune: web CPU canvas cannot paint {why:?}; holding previous frame for GPU"
                            )
                            .into(),
                        );
                    }
                    crate::marks::mark("cpu_first.needs_gpu");
                    return Err(why);
                }
            },
        };
        self.last_degraded = degraded;
        let put = crate::time::Instant::now();
        let plan = self.painter.upload_plan(&painted, (w, h));
        // When the frame scrolled cleanly, shift the moved region inside the opaque 2D canvas
        // directly (exact 1:1 copy without readback or putImageData) and upload only the dirty
        // tiles; fall back to uploading `painted.rects` if the self-blit fails.
        let rects = match plan.blit {
            Some(b)
                if self
                    .ctx
                    .draw_image_with_html_canvas_element_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
                        &self.canvas,
                        f64::from(b.src_x),
                        f64::from(b.src_y),
                        f64::from(b.width),
                        f64::from(b.height),
                        f64::from(b.dst_x),
                        f64::from(b.dst_y),
                        f64::from(b.width),
                        f64::from(b.height),
                    )
                    .is_ok() => plan.uploads,
            _ => &painted.rects,
        };
        let measure = stats_wanted();
        let mut px = 0u64;
        let mut swizzle_ms = 0.0f64;
        if let Some(image) = &self.image {
            for &rect in rects {
                let Some(upload) = dirty_rect_row_slice(w, h, rect) else { continue };
                if let Ok(ms) = put_cpu_rect(
                    &self.ctx,
                    image,
                    &self.buffer[upload.byte_start..upload.byte_end],
                    w,
                    upload.x0,
                    upload.y0,
                    upload.rw,
                    upload.rh,
                    measure,
                ) {
                    swizzle_ms += ms;
                }
                px += u64::from(upload.rw) * u64::from(upload.rh);
            }
        }
        let end = crate::time::Instant::now();
        let us = |d: std::time::Duration| d.as_secs_f64() * 1e6;
        let cost = Cost {
            fill_us: us(filled - start),
            paint_us: us(put - filled),
            raster_us: painted.report.raster_ns as f64 / 1e3,
            put_us: us(end - put),
            swizzle_us: swizzle_ms * 1e3,
            put_px: px,
        };
        if measure {
            report(&cost);
        }
        Ok(cost)
    }

    /// Whether the most recently painted CPU frame used degraded fallback mode.
    pub(super) fn last_degraded(&self) -> bool { self.last_degraded }

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

/// Whether the page attached `globalThis.__runeCpuFrames` to collect per-frame CPU timings.
fn stats_wanted() -> bool {
    STATS.get().unwrap_or_else(|| {
        let on = js_sys::Reflect::get(&js_sys::global(), &"__runeCpuFrames".into())
            .is_ok_and(|v| js_sys::Array::is_array(&v));
        STATS.set(Some(on));
        on
    })
}

/// Appends a frame's cost to `globalThis.__runeCpuFrames`, when the page made that array.
fn report(cost: &Cost) {
    let Ok(list) = js_sys::Reflect::get(&js_sys::global(), &"__runeCpuFrames".into()) else { return };
    let list: js_sys::Array = list.unchecked_into();
    let row = js_sys::Array::new();
    for v in [cost.fill_us, cost.paint_us, cost.raster_us, cost.put_us, cost.swizzle_us, cost.put_px as f64] {
        row.push(&v.into());
    }
    list.push(&row);
}
