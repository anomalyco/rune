//! The stress HUD's numbers, shared so both builds show the same thing: the last [`GRAPH`]
//! frames' build times against the 120 Hz and 60 Hz budgets, a big readout, and frame rate.
//!
//! Live, the HUD reads the frames the meter has seen, and its frame rate and misses are the
//! window's. Replaying (`--hud-from`), it reads a measured run's per-frame CSV by show time, so a
//! capture run, whose own frames the capture slows, shows what the clean run measured at the same
//! moment; its frame rate is then what that CPU cost allows at 120 Hz, nothing presented.

use crate::meter::Meter;

pub const BUDGET_120: f32 = 1000.0 / 120.0;
pub const BUDGET_60: f32 = 1000.0 / 60.0;
/// Frames in the graph.
pub const GRAPH: usize = 120;
/// The graph's top, in ms.
pub const GRAPH_MAX: f32 = 40.0;
/// Frames the big readout takes the median of.
const READOUT: usize = 15;

/// A measured run's frames: show time and build ms, by show time.
pub struct Replay {
    samples: Vec<(f32, f32)>,
}

impl Replay {
    /// Reads [`Meter::frames_csv`] output.
    pub fn load(path: &std::path::Path) -> Result<Replay, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut samples = Vec::new();
        for line in text.lines().skip(1) {
            let mut f = line.split(',');
            let (Some(t), Some(_), Some(ms)) = (f.next(), f.next(), f.next()) else { continue };
            if let (Ok(t), Ok(ms)) = (t.parse::<f32>(), ms.parse::<f32>())
                && t.is_finite()
            {
                samples.push((t, ms));
            }
        }
        samples.sort_by(|a, b| a.0.total_cmp(&b.0));
        Ok(Replay { samples })
    }

    /// The frames at or before show time `t`.
    pub fn until(&self, t: f64) -> &[(f32, f32)] {
        let t = t as f32 + 1e-4;
        &self.samples[..self.samples.partition_point(|s| s.0 <= t)]
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Readout {
    /// Oldest first, at most [`GRAPH`].
    pub graph: Vec<f32>,
    /// The median of the last few frames.
    pub ms: f32,
    pub p95: f32,
    pub fps: f32,
    /// Refreshes missed: live, all run; replayed, in the last second.
    pub missed: u32,
    pub source: &'static str,
}

/// 0 within the 120 Hz budget, 1 within 60 Hz, 2 over.
pub fn grade(ms: f32) -> usize {
    if ms <= BUDGET_120 {
        0
    } else if ms <= BUDGET_60 {
        1
    } else {
        2
    }
}

fn pct(v: &mut [f32], p: f32) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f32::total_cmp);
    v[((v.len() - 1) as f32 * p).round() as usize]
}

impl Readout {
    fn of(samples: impl Iterator<Item = (f32, f32)>) -> Readout {
        let graph: Vec<f32> = samples.map(|s| s.1).collect();
        let graph = graph[graph.len().saturating_sub(GRAPH)..].to_vec();
        let ms = pct(&mut graph[graph.len().saturating_sub(READOUT)..].to_vec(), 0.5);
        let p95 = pct(&mut graph.clone(), 0.95);
        Readout { graph, ms, p95, ..Default::default() }
    }

    /// From a measured run: the frame rate this CPU cost allows at 120 Hz.
    pub fn replay(r: &Replay, t: f64) -> Readout {
        let mut out = Readout::of(r.until(t).iter().copied());
        let refreshes: u32 = out.graph.iter().map(|&ms| (ms / BUDGET_120).ceil().max(1.0) as u32).sum();
        out.missed = refreshes.saturating_sub(out.graph.len() as u32);
        out.fps = if refreshes > 0 { 120.0 * out.graph.len() as f32 / refreshes as f32 } else { 0.0 };
        out.source = "measured headless · live pending";
        out
    }

    pub fn live(m: &Meter) -> Readout {
        let mut out = Readout::of(m.tail(GRAPH));
        let recent = m.recent();
        out.fps = recent.fps as f32;
        out.missed = recent.dropped as u32;
        out.source = "live window";
        out
    }

    pub fn big(&self) -> String {
        format!("{:.1} ms", self.ms)
    }

    pub fn detail(&self) -> String {
        format!("build + layout + paint · p95 {:.1}", self.p95)
    }

    pub fn rate(&self) -> String {
        if self.source == "live window" {
            format!("{:.0} fps · missed {}", self.fps, self.missed)
        } else {
            format!("≈ {:.0} fps at 120 Hz · {} missed / s", self.fps, self.missed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_replay_counts_the_refreshes_slow_frames_miss() {
        let dir = std::env::temp_dir().join(format!("bento-hud-{}.csv", std::process::id()));
        let mut csv = String::from("show_s,load,build_ms,work_ms,start_s\n");
        for i in 0..240 {
            // Half the frames take two refreshes.
            csv.push_str(&format!("{},16,{},0,0\n", i as f32 / 120.0, if i % 2 == 0 { 4.0 } else { 12.0 }));
        }
        std::fs::write(&dir, csv).unwrap();
        let r = Replay::load(&dir).unwrap();
        std::fs::remove_file(dir).unwrap();
        assert_eq!(r.until(0.5).len(), 61);
        let out = Readout::replay(&r, 10.0);
        assert_eq!(out.graph.len(), GRAPH);
        assert_eq!(out.missed, 60);
        assert!((out.fps - 80.0).abs() < 0.01);
        assert_eq!((grade(4.0), grade(12.0), grade(20.0)), (0, 1, 2));
    }
}
