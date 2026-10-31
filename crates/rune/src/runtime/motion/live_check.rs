//! `Tx::live_only` skip accounting and debug stale-view checks.
use super::*;

/// Seconds after a commit before debug builds check a `Tx::live_only` event (`skip_view`).
#[cfg(debug_assertions)]
const LIVE_ONLY_CHECK: f64 = 0.25;

impl<S: 'static> Runtime<S> {
    /// Events whose handler called [`Tx::live_only`] and so didn't re-run the view, since the
    /// runtime started.
    pub fn live_only_events(&self) -> u64 {
        self.live_only_skips
    }

    /// Whether debug builds check `Tx::live_only` handlers (on by default): a live-only event
    /// a quarter second or more after the last commit still commits, and Rune warns when the
    /// view built something else. A test counting commits turns it off. Release builds never
    /// check.
    pub fn check_live_only(&mut self, on: bool) {
        #[cfg(debug_assertions)]
        { self.live_only_check = on; }
        let _ = on;
    }

    /// An event whose handler called `Tx::live_only`: false, so it doesn't commit, while this
    /// frame still latches the live values it set. Debug builds check one such event a quarter
    /// second after the last commit by committing anyway (true) and comparing what it built.
    pub(super) fn skip_view(&mut self, label: impl std::fmt::Display) -> bool {
        #[cfg(debug_assertions)]
        if self.live_only_check && self.clock - self.last_commit_clock >= LIVE_ONLY_CHECK {
            if self.live_check.is_none() { self.live_check = Some((label.to_string(), self.view_fingerprint())); }
            self.live_checks += 1;
            self.live_dirty = true;
            return true;
        }
        let _ = label;
        self.live_only_skips += 1;
        self.live_dirty = true;
        false
    }

    /// What the last commit built: each element's path, layout, colours and text. Live values
    /// are left out, since a commit rebuilds them from the view and the next latch sets them.
    #[cfg(debug_assertions)]
    pub(super) fn view_fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::hash::DefaultHasher::new();
        let floats = |h: &mut std::hash::DefaultHasher, xs: &[f32]| for x in xs { x.to_bits().hash(h) };
        for n in &self.nodes {
            // Retained exits can disappear between commits without a model change.
            if self.is_exiting(n.id) { continue; }
            n.path.hash(&mut h);
            floats(&mut h, &n.target);
            let v = &n.vis;
            if !n.live.iter().any(|b| b.property == crate::live::Property::Fill) {
                floats(&mut h, &[v.fill.r, v.fill.g, v.fill.b, v.fill.a]);
            }
            floats(&mut h, &[v.border.r, v.border.g, v.border.b, v.border.a, v.border_width, v.opacity]);
            floats(&mut h, &v.radius);
            if let Some(t) = &n.text {
                if t.live.is_none() { t.content.hash(&mut h); }
                floats(&mut h, &[t.color.r, t.color.g, t.color.b, t.color.a, t.style.size, t.style.weight]);
                for p in &t.pills {
                    (p.start, p.end, p.icon.map(|i| i.name)).hash(&mut h);
                    for c in [p.bg, p.fg].into_iter().flatten() {
                        floats(&mut h, &[c.r, c.g, c.b, c.a]);
                    }
                }
                for &(a, b, c, w) in &t.rings {
                    (a, b).hash(&mut h);
                    floats(&mut h, &[c.r, c.g, c.b, c.a, w]);
                }
                for &(a, b, c) in &t.strikes {
                    (a, b, c).hash(&mut h);
                }
                for hl in &t.highlights {
                    (hl.start, hl.end, hl.bg, hl.fg, hl.pad.to_bits(), &hl.key).hash(&mut h);
                    if let Some((rc, rw)) = hl.ring {
                        (rc, rw.to_bits()).hash(&mut h);
                    }
                }
            }
        }
        h.finish()
    }

    /// After the commit a live-only check forced: warn when the view built something else, as
    /// it does when the handler changed state the view reads.
    #[cfg(debug_assertions)]
    pub(super) fn compare_live_check(&mut self, check: Option<(String, u64)>) {
        let Some((handler, before)) = check else { return };
        if self.view_fingerprint() == before { return; }
        let warning = format!(
            "rune: {handler} called Tx::live_only, but the view built something else after it. The handler may have changed \
             state the view reads, which stays stale until another event commits. Set only Live values in a \
             live-only handler; for a time-dependent view, disable this heuristic with check_live_only(false)."
        );
        if !self.live_only_warnings.contains(&warning) {
            eprintln!("{warning}");
            self.live_only_warnings.push(warning);
        }
    }
}

/// Folds one handler's `Tx::live_only` into its event's: live-only when every handler that
/// took the event said so.
pub(crate) fn fold_live(live: &mut Option<bool>, took: bool, one: &Tx) {
    if took { *live = Some(live.unwrap_or(true) && one.live_only); }
}
