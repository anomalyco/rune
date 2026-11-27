//! Pure upload-slice and `NeedsGpu` policy for the web CPU 2D canvas (`platform/web/cpu_canvas.rs`).
//! Kept under `platform/web_math` so every decision compiles and runs in native unit tests off
//! `wasm32`.

use crate::render::cpu_boot::NeedsGpu;

/// Clamped dirty rectangle and its tight row-aligned byte slice inside the `canvas_w x canvas_h`
/// BGRA framebuffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DirtyRectUpload {
    pub x0: u32,
    pub y0: u32,
    pub rw: u32,
    pub rh: u32,
    /// Start byte offset in the BGRA framebuffer (`y0 * canvas_w * 4`).
    pub byte_start: usize,
    /// Exclusive end byte offset in the BGRA framebuffer (`(y0 + rh) * canvas_w * 4`).
    pub byte_end: usize,
}

/// Clamps `rect` (`[x0, y0, x1, y1)`) to `(canvas_w, canvas_h)` and computes the tight row-aligned
/// byte range passed to `cpu_canvas.js::putCpuRect`. Returns `None` when the clamped rectangle is
/// empty or overflows `usize`.
pub(crate) fn dirty_rect_row_slice(
    canvas_w: u32,
    canvas_h: u32,
    rect: [u32; 4],
) -> Option<DirtyRectUpload> {
    let x0 = rect[0].min(canvas_w);
    let y0 = rect[1].min(canvas_h);
    let x1 = rect[2].min(canvas_w);
    let y1 = rect[3].min(canvas_h);
    let rw = x1.saturating_sub(x0);
    let rh = y1.saturating_sub(y0);
    if rw == 0 || rh == 0 {
        return None;
    }
    let row_bytes = (canvas_w as usize).checked_mul(4)?;
    let byte_start = (y0 as usize).checked_mul(row_bytes)?;
    let byte_end = (y1 as usize).checked_mul(row_bytes)?;
    Some(DirtyRectUpload { x0, y0, rw, rh, byte_start, byte_end })
}

/// How the web CPU 2D canvas responds when an exact `CpuPainter::paint` fails with `NeedsGpu`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WebCpuNeedsGpuAction {
    /// Retry with `Scene::degraded(true)` so the page keeps painting live frames without the
    /// GPU-only modifier during CPU-first startup, hand-over, `CpuFirst::Only`, or fallback.
    PaintDegraded(NeedsGpu),
    /// Non-degradable reason (`SlugText`, `OutlineGlyph`, `UnknownKind`, `Target`): keep the
    /// previously uploaded canvas pixels and request urgent GPU startup if available.
    HoldPreviousFrame(NeedsGpu),
}

/// Classifies an exact-paint `NeedsGpu` error for `CpuCanvas`.
pub(crate) const fn classify_cpu_needs_gpu(reason: NeedsGpu) -> WebCpuNeedsGpuAction {
    if reason.is_degradable() {
        WebCpuNeedsGpuAction::PaintDegraded(reason)
    } else {
        WebCpuNeedsGpuAction::HoldPreviousFrame(reason)
    }
}

/// Bitmask tracking once-per-reason console warnings for `NeedsGpu` on `CpuCanvas`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct NeedsGpuLogMask(u16);

impl NeedsGpuLogMask {
    const fn bit(reason: NeedsGpu) -> u16 {
        match reason {
            NeedsGpu::LinearBlend => 1 << 0,
            NeedsGpu::NativeSurface => 1 << 1,
            NeedsGpu::SlugText => 1 << 2,
            NeedsGpu::LayerBlur => 1 << 3,
            NeedsGpu::TurnedLayer => 1 << 4,
            NeedsGpu::BackdropBlur => 1 << 5,
            NeedsGpu::Effect => 1 << 6,
            NeedsGpu::OutlineGlyph => 1 << 7,
            NeedsGpu::UnknownKind => 1 << 8,
            NeedsGpu::Target => 1 << 9,
        }
    }

    /// Returns `true` the first time `reason` is recorded, and `false` on subsequent frames.
    pub(crate) fn note(&mut self, reason: NeedsGpu) -> bool {
        let mask = Self::bit(reason);
        if (self.0 & mask) != 0 {
            return false;
        }
        self.0 |= mask;
        true
    }
}

/// Whether the web CPU-to-GPU hand-over should run `Readback::count` against `CpuCanvas::pixels`.
///
/// Skipped when the CPU did not paint the hand-over frame (non-degradable `NeedsGpu`) or painted
/// it in degraded fallback mode (different visual output from the GPU by design).
pub(crate) const fn should_count_handover_drift(cpu_drawn: bool, cpu_degraded: bool) -> bool {
    cpu_drawn && !cpu_degraded
}

/// Whether `Browser::gpu_came` may accept a resolved GPU device on a CPU-first or restoring page.
///
/// Normally waits until the first CPU frame has presented (`presented`) so first-paint latency is
/// never delayed by GPU setup. When a non-degradable `NeedsGpu` scene blocked CPU painting before
/// frame 1 (`cpu_blocked_on_gpu`), allows the GPU to take over immediately instead of deadlocking
/// on a blank canvas.
pub(crate) const fn can_take_pending_gpu(
    presented: bool,
    cpu_blocked_on_gpu: bool,
    is_cpu_first: bool,
    cpu_not_failed: bool,
    webgl_restoring: bool,
    is_cpu_or_context_lost: bool,
) -> bool {
    (presented || cpu_blocked_on_gpu)
        && ((is_cpu_first && cpu_not_failed) || (webgl_restoring && is_cpu_or_context_lost))
}

#[cfg(test)]
#[path = "cpu_canvas_tests.rs"]
mod tests;
