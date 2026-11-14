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
    if let Some(dir) = std::env::var_os("IB3_DRIFT_DUMP") { ib3_dump(std::path::Path::new(&dir), cpu, gpu, w, h); }
    d
}

/// IB-3 measuring: both frames, a map of where they differ (red, over the CPU frame dimmed), and
/// each 16 px cell with a difference: its pixels and largest channel difference.
fn ib3_dump(dir: &std::path::Path, cpu: Pixels<'_>, gpu: Pixels<'_>, w: u32, h: u32) {
    let _ = std::fs::create_dir_all(dir);
    let rgba = |p: &Pixels<'_>| (0..h).flat_map(|y| (0..w).flat_map(move |x| { let [b, g, r, a] = p.bgra_at(x, y); [r, g, b, a] })).collect::<Vec<u8>>();
    let (c, g) = (rgba(&cpu), rgba(&gpu));
    let mut map = Vec::with_capacity(c.len());
    let mut cells: std::collections::BTreeMap<(u32, u32), (u32, u8, [u8; 4], [u8; 4])> = Default::default();
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let m = (0..4).map(|k| c[i + k].abs_diff(g[i + k])).max().unwrap_or(0);
            if m > 0 {
                map.extend_from_slice(&[255, 0, 0, 255]);
                let e = cells.entry((x / 16, y / 16)).or_insert((0, 0, [0; 4], [0; 4]));
                e.0 += 1;
                if m > e.1 { *e = (e.0, m, [c[i], c[i + 1], c[i + 2], c[i + 3]], [g[i], g[i + 1], g[i + 2], g[i + 3]]); }
            } else {
                let l = ((u32::from(c[i]) + u32::from(c[i + 1]) + u32::from(c[i + 2])) / 6) as u8;
                map.extend_from_slice(&[l, l, l, 255]);
            }
        }
    }
    let _ = crate::render::write_png(&dir.join("cpu.png"), w, h, &c);
    let _ = crate::render::write_png(&dir.join("gpu.png"), w, h, &g);
    let _ = crate::render::write_png(&dir.join("diff.png"), w, h, &map);
    let lines: Vec<String> = cells.iter().map(|((cx, cy), (n, m, a, b))| format!("cell x {} y {}: {n} px, max {m}, cpu {a:?} gpu {b:?}", cx * 16, cy * 16)).collect();
    let _ = std::fs::write(dir.join("cells.txt"), lines.join("\n"));
}

/// The drift the last hand-over measured, if one did.
pub fn last_drift() -> Option<Drift> {
    *LAST.lock().unwrap_or_else(|e| e.into_inner())
}
