//! The hand-over's drift: how many pixels the GPU's first frame changed from the CPU frame it
//! replaced. A window that took over from `paint` reads its first GPU frame back and calls
//! [`drift`] when [`drift_wanted`] (`RUNE_CPU_BOOT_DRIFT=1`), so the day the two renderers start
//! to disagree shows as a count in the log and in [`last_drift`], not as a flicker nobody traces.

use std::sync::Mutex;

/// What the GPU's frame changed: pixels compared, pixels with any channel different, pixels with
/// a channel more than 2 apart, and the largest channel difference.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Drift {
    pub pixels: u64,
    pub differ: u64,
    pub over_two: u64,
    pub max: u8,
}

/// A frame's pixels: `width × height`, `stride` bytes a row, four bytes a pixel in memory order
/// B, G, R, A (the swap chain's and `paint`'s) or R, G, B, A (an offscreen readback's).
#[derive(Clone, Copy)]
pub struct Pixels<'a> {
    data: &'a [u8],
    width: u32,
    height: u32,
    stride: usize,
    rgba: bool,
}

impl<'a> Pixels<'a> {
    pub fn bgra(data: &'a [u8], width: u32, height: u32, stride: usize) -> Self { Pixels { data, width, height, stride, rgba: false } }

    pub fn rgba(data: &'a [u8], width: u32, height: u32, stride: usize) -> Self { Pixels { data, width, height, stride, rgba: true } }

    /// Pixel `(x, y)` as B, G, R, A.
    fn bgra_at(&self, x: u32, y: u32) -> [u8; 4] {
        let at = y as usize * self.stride + x as usize * 4;
        let p = &self.data[at..at + 4];
        if self.rgba { [p[2], p[1], p[0], p[3]] } else { [p[0], p[1], p[2], p[3]] }
    }
}

static LAST: Mutex<Option<Drift>> = Mutex::new(None);

/// Whether the hand-over should measure its drift (`RUNE_CPU_BOOT_DRIFT` set and not `0`).
pub fn drift_wanted() -> bool {
    std::env::var_os("RUNE_CPU_BOOT_DRIFT").is_some_and(|v| v != "0")
}

/// Compares the CPU frame with the GPU frame that replaced it over their common area, keeps the
/// result for [`last_drift`] and logs it.
pub fn drift(cpu: Pixels<'_>, gpu: Pixels<'_>) -> Drift {
    let (w, h) = (cpu.width.min(gpu.width), cpu.height.min(gpu.height));
    let mut d = Drift { pixels: u64::from(w) * u64::from(h), ..Drift::default() };
    for y in 0..h {
        for x in 0..w {
            let (a, b) = (cpu.bgra_at(x, y), gpu.bgra_at(x, y));
            let m = (0..4).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0);
            if m > 0 { d.differ += 1; }
            if m > 2 { d.over_two += 1; }
            d.max = d.max.max(m);
        }
    }
    *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some(d);
    eprintln!("rune: cpu boot drift: {} of {} px differ from the GPU's first frame, {} by more than 2 (max {})", d.differ, d.pixels, d.over_two, d.max);
    d
}

/// The drift the last hand-over measured, if one did.
pub fn last_drift() -> Option<Drift> {
    *LAST.lock().unwrap_or_else(|e| e.into_inner())
}
