//! A text handoff in screen coordinates. The destination is measured after growth and scroll
//! each frame. One analytic growth-spring progress interpolates source to live destination,
//! so the copy joins the bubble without a second spring lagging behind scroll follow.

use rune_motion::{Animated, PropClass, Rect};
use crate::growth::GROWTH;
use crate::render::Instance;

pub(crate) struct TextFlight {
    pub source_key: String,
    pub target_key: String,
    pub source: Rect,
    pub source_clip: [f32; 4],
    pub glyphs: Vec<Instance>,
    pub scale: f32,
    pub destination: Rect,
    progress: Animated<f32>,
    born: f64,
    pub presented: Rect,
    pub clip: [f32; 4],
    pub source_alpha: f32,
    pub target_alpha: f32,
}

impl TextFlight {
    pub fn new(source_key: String, target_key: String, source: Rect, source_clip: [f32; 4], glyphs: Vec<Instance>, scale: f32, now: f64) -> Self {
        let mut progress = Animated::new(0.0).with_class(PropClass::Opacity);
        progress.animate_to(1.0, GROWTH, now);
        Self { source_key, target_key, source, source_clip, glyphs, scale,
            destination: source, progress, born: now, presented: source,
            clip: source_clip, source_alpha: 1.0, target_alpha: 0.0 }
    }

    pub fn progress(&self, now: f64, reduced: bool) -> f32 {
        if reduced { ((now - self.born) / 0.15).clamp(0.0, 1.0) as f32 }
        else { self.progress.sample(now).clamp(0.0, 1.0) }
    }

    pub fn measure(&mut self, target: Rect, now: f64, reduced: bool) -> bool {
        self.destination = target;
        let p = if reduced { 1.0 } else { self.progress(now, false) };
        let from = self.source;
        self.presented = Rect::new(from.x + (target.x - from.x) * p, from.y + (target.y - from.y) * p,
            from.w + (target.w - from.w) * p, from.h + (target.h - from.h) * p);
        if reduced { self.progress(now, true) >= 1.0 } else { self.progress.is_settled(now) }
    }

    pub fn alphas(&self, target: Rect, now: f64, reduced: bool) -> (f32, f32) {
        let p = self.progress(now, reduced);
        if reduced { return (1.0 - p, p); }
        let rewrap = (target.h - self.source.h).abs() > 2.0;
        let (out, end) = if rewrap { (0.25, 0.4) } else { (0.4, 0.5) };
        ((1.0 - p / out).clamp(0.0, 1.0),
            if rewrap { ((p - 0.1) / (end - 0.1)).clamp(0.0, 1.0) } else { (p / end).clamp(0.0, 1.0) })
    }
}

/// Transform glyph geometry without changing its atlas footprint. Clip edges share the same
/// transform; a soft inset handles lines that were only partly visible in the source editor.
pub(crate) fn transformed(mut q: Instance, from: Rect, to: Rect, scale: f32, alpha: f32, clip: [f32; 4], fade: [f32; 4]) -> Instance {
    let zoom = to.w / from.w.max(0.01);
    q.uv[2] = q.rect[2];
    q.uv[3] = q.rect[3];
    q.rect = [
        (to.x + (q.rect[0] / scale - from.x) * zoom) * scale,
        (to.y + (q.rect[1] / scale - from.y) * zoom) * scale,
        q.rect[2] * zoom,
        q.rect[3] * zoom,
    ];
    q.fill = q.fill.map(|v| v * alpha);
    q.clip = clip.map(|v| v * scale);
    q.fade = crate::render::pack_fade(fade.map(|v| v * scale), q.clip, q.clip);
    q
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flight() -> TextFlight {
        TextFlight::new("source".into(), "target".into(), Rect::new(40.0, 400.0, 500.0, 40.0),
            [40.0, 400.0, 540.0, 440.0], Vec::new(), 2.0, 0.0)
    }

    #[test]
    fn moving_destination_uses_shared_progress_without_restarting_it() {
        let mut f = flight();
        f.measure(Rect::new(120.0, 260.0, 400.0, 22.0), 0.0, false);
        let p = f.progress.sample(0.08);
        let v = f.progress.velocity(0.08);
        f.measure(Rect::new(120.0, 220.0, 400.0, 22.0), 0.08, false);
        assert_eq!(f.progress.sample(0.08), p);
        assert_eq!(f.progress.velocity(0.08), v);
        assert_eq!(f.presented.y, 400.0 + (220.0 - 400.0) * p);
        assert!(f.measure(Rect::new(120.0, 220.0, 400.0, 22.0), 2.0, false));
    }

    #[test]
    fn flight_is_time_based_and_reduced_motion_only_fades() {
        let target = Rect::new(120.0, 260.0, 400.0, 22.0);
        let mut slow = flight();
        let mut fast = flight();
        slow.measure(target, 0.0, false);
        fast.measure(target, 0.0, false);
        for i in 1..=30 {
            slow.measure(target, i as f64 / 30.0, false);
            for j in (i * 4 - 3)..=i * 4 { fast.measure(target, j as f64 / 120.0, false); }
            assert_eq!(slow.presented, fast.presented);
        }
        let mut f = flight();
        assert!(!f.measure(target, 0.075, true));
        assert_eq!(f.presented, target);
        assert_eq!(f.alphas(target, 0.075, true), (0.5, 0.5));
        assert!(f.measure(target, 0.15, true));
    }
}
