//! `--cpu-boot DIR`: a headless run that also draws every frame on the CPU, as a window does
//! before its GPU: the live painter (`CpuPainter`) repaints the damaged tiles of one retained
//! buffer, after filling the frame's queued masks (which the GPU then uploads and samples, as
//! after a hand-over). Each frame's stage times and damage go to `DIR/frames.jsonl`. At the
//! snapshot frame it writes the draw data (`draw.bin`, `record.rs`), the CPU's frame (`cpu.png`)
//! and the GPU's (`gpu.png`), and prints `rune-cpu-boot {json}` with that frame's numbers and the
//! drift between the two.

use std::io::Write;
use std::path::PathBuf;
use crate::time::Instant;

use serde_json::json;

use super::{CpuPainter, Pixels, Scene, Target};
use crate::runtime::Runtime;

pub(crate) struct Tool {
    dir: PathBuf,
    painter: CpuPainter,
    threads: usize,
    /// The retained buffer, B, G, R, A, rows `stride` bytes apart, as large as the largest frame
    /// so far, as a window keeps its bitmap across a live resize; and the frame's size in it.
    buf: Vec<u8>,
    stride: usize,
    size: (u32, u32),
    vendor: u32,
    log: Option<std::io::BufWriter<std::fs::File>>,
    /// The snapshot frame and what the run said about it, until the GPU's arrives.
    painted: Option<(Vec<u8>, serde_json::Value)>,
}

impl Tool {
    /// A run whose GPU has PCI vendor ID endor, the one whose rounding the CPU follows.
    pub(crate) fn new(dir: PathBuf, threads: usize, vendor: u32) -> Tool {
        let _ = std::fs::create_dir_all(&dir);
        let log = std::fs::File::create(dir.join("frames.jsonl")).ok().map(std::io::BufWriter::new);
        Tool { dir, painter: CpuPainter::new(threads), threads: threads.max(1), buf: Vec::new(), stride: 0, size: (0, 0), vendor, log, painted: None }
    }

    /// Before the renderer prepares frame `k`, which `rt` laid out in `update_ns`: fills its
    /// queued masks and paints it, and at the snapshot frame records it.
    pub(crate) fn frame<S: 'static>(&mut self, rt: &mut Runtime<S>, size: (u32, u32), k: usize, update_ns: u64, snap: bool) {
        let started = Instant::now();
        let fills = self.painter.fill_masks(&mut rt.text.atlas);
        let scene = Scene::new(&rt.instances, &rt.text.atlas, &rt.images, size, rt.clear_color())
            .srgb_blend(rt.srgb_blend())
            .native_surfaces(!rt.surfaces.is_empty())
            .font_smoothing(rt.font_smoothing())
            .half_rounding(super::HalfRounding::of_vendor(self.vendor));
        // Outgrown, the bitmap is made again at the new size: every pixel paints.
        if size.0 as usize * 4 > self.stride || self.buf.len() < self.stride * size.1 as usize {
            let rows = if self.stride == 0 { 0 } else { self.buf.len() / self.stride };
            self.stride = self.stride.max(size.0 as usize * 4);
            self.buf = vec![0u8; self.stride * rows.max(size.1 as usize)];
            self.painter.forget();
        }
        self.size = size;
        let painted = self.painter.paint(&scene, &mut Target::with_stride(&mut self.buf, size.0, size.1, self.stride));
        let total = super::ns(started.elapsed());
        let mut out = json!({ "k": k, "vendor": self.vendor, "update_ns": update_ns, "cpu_ns": total, "instances": rt.instances.len(), "threads": self.threads,
            "fill": { "masks": fills.masks, "texels": fills.texels, "cover_ns": fills.cover_ns, "copy_ns": fills.copy_ns } });
        match &painted {
            Ok(p) => out["paint"] = json!({
                "bin_ns": p.report.bin_ns, "hash_ns": p.hash_ns, "plan_ns": p.plan_ns, "move_ns": p.move_ns, "scrolled": p.scrolled, "slowest_tile_ns": p.report.slowest_tile_ns, "blur_ns": p.report.blur_ns, "raster_ns": p.report.raster_ns, "total_ns": p.report.total_ns, "threads": p.report.threads,
                "dirty_tiles": p.dirty_tiles, "tiles": p.tiles, "rects": p.rects, "span_px": p.report.span_px, "shaded_px": p.report.shaded_px, "glyph_px": p.report.glyph_px,
                "dirty_px": p.rects.iter().map(|r| u64::from(r[2] - r[0]) * u64::from(r[3] - r[1])).sum::<u64>(),
            }),
            Err(why) => out["needs_gpu"] = json!(format!("{why:?}")),
        }
        if let Some(log) = self.log.as_mut() { let _ = writeln!(log, "{out}"); }
        if !snap { return; }
        let mut kinds = [0u32; 16];
        for i in &rt.instances { kinds[(i.params[1].max(0.0) as usize).min(15)] += 1; }
        out["kinds"] = json!(kinds);
        let _ = std::fs::write(self.dir.join("draw.bin"), super::record::encode(&scene));
        // The frame's rows, tightly packed.
        let bgra: Vec<u8> = self.buf.chunks(self.stride).take(size.1 as usize).flat_map(|row| &row[..size.0 as usize * 4]).copied().collect();
        let rgba: Vec<u8> = bgra.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
        let _ = crate::render::write_png(&self.dir.join("cpu.png"), size.0, size.1, &rgba);
        self.painted = Some((bgra, out));
    }

    /// The GPU's snapshot frame, R, G, B, A, tightly packed.
    pub(crate) fn gpu(&mut self, rgba: &[u8], size: (u32, u32)) {
        let Some((cpu, mut out)) = self.painted.take() else { return };
        if let Some(log) = self.log.as_mut() { let _ = log.flush(); }
        let _ = crate::render::write_png(&self.dir.join("gpu.png"), size.0, size.1, rgba);
        if self.size == size {
            let d = super::drift(Pixels::bgra(&cpu, size.0, size.1, size.0 as usize * 4), Pixels::rgba(rgba, size.0, size.1, size.0 as usize * 4));
            out["drift"] = json!({ "pixels": d.pixels, "differ": d.differ, "over_two": d.over_two, "max": d.max });
        }
        println!("rune-cpu-boot {out}");
    }
}
