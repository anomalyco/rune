//! Content-aware scroll edges: `El::scroll_edge_fade` fades an edge only while content is hidden
//! under it, and `El::scroll_margin` stops `Tx::scroll_into_view` clear of that fade.
use super::*;

/// Hidden extent (points) at which an edge's fade reaches full strength.
const FADE_IN: f64 = 15.0;

/// A fade at `strength` (0..=1) as a packed ramp `(width, offset)`: the shader's smoothstep ramp
/// starts `offset` of the way in, so the edge shows `1 - strength` and reaches full alpha
/// `width` in. Zero strength is a hard edge.
fn partial(width: f32, strength: f32) -> (f32, f32) {
    if strength <= 0.0 || width <= 0.0 { return (0.0, 0.0); }
    if strength >= 1.0 { return (width, 0.0); }
    // Inverse smoothstep: where the ramp already stands at alpha `1 - strength`.
    let offset = 0.5 - ((2.0 * strength - 1.0).clamp(-1.0, 1.0).asin() / 3.0).sin();
    let offset = offset.clamp(0.0, 0.995);
    (width / (1.0 - offset), offset)
}

/// The scroll offset that shows `[top, top + height)` with `margin` to spare, moving as little
/// as possible from `offset` (unclamped). A row taller than the viewport that already fills it
/// stays put, so placing it again doesn't flip it from its end to its start.
pub(super) fn reveal(offset: f64, top: f64, height: f64, viewport: f64, margin: f64) -> f64 {
    let margin = margin.min(((viewport - height) / 2.0).max(0.0));
    let (start, end) = (top - margin, top + height + margin);
    if start <= offset && end >= offset + viewport { offset }
    else if start < offset { start }
    else if end > offset + viewport { end - viewport }
    else { offset }
}

impl<S: 'static> Runtime<S> {
    /// Node `i`'s own soft-clip widths `f` (points) as a device-pixel fade and the rect its ramps
    /// start from. With `scroll_edge_fade`, top and bottom follow the content hidden under them:
    /// the ramp starts outside `r` so the edge itself only partly fades. `bottom` is the
    /// presented bottom edge (see `stuck_offset`).
    pub(super) fn scroll_edge_fade(&self, i: usize, f: [f32; 4], r: [f32; 4], k: f32, bottom: f32) -> ([f32; 4], [f32; 4]) {
        let n = &self.nodes[i];
        let mut own = f.map(|v| v * k);
        let width = n.scroll_edges.fade;
        let Some(st) = self.scrolls.get(&n.id).filter(|_| width > 0.0 && n.scroll.is_some()) else { return (own, r) };
        let stuck = self.stuck_offset(i, bottom);
        let offset = stuck.unwrap_or(st.offset) + f64::from(st.slack.height());
        let below = if stuck.is_some() { 0.0 } else { st.max - offset };
        let mut origin = r;
        let (y, x) = (n.scroll_axes.y(), n.scroll_axes.x());
        let edges = [(y, 1, offset, -1.0), (y, 3, below, 1.0), (x, 0, st.x.offset, -1.0), (x, 2, st.x.max - st.x.offset, 1.0)];
        for (_, edge, hidden, out) in edges.into_iter().filter(|e| e.0) {
            let (w, o) = partial(width * k, ((hidden / FADE_IN).clamp(0.0, 1.0)) as f32);
            own[edge] = w;
            origin[edge] = r[edge] + out * o * w;
        }
        (own, origin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_ramp_starts_at_one_minus_strength_and_ends_at_the_full_width() {
        let smooth = |x: f32| { let x = x.clamp(0.0, 1.0); x * x * (3.0 - 2.0 * x) };
        assert_eq!(partial(24.0, 0.0), (0.0, 0.0), "nothing hidden: a hard edge");
        assert_eq!(partial(24.0, 1.0), (24.0, 0.0), "fully hidden: the plain ramp");
        for s in [0.1, 0.25, 0.5, 0.75, 0.9] {
            let (w, o) = partial(24.0, s);
            assert!((smooth(o) - (1.0 - s)).abs() < 1e-4, "edge alpha {} at strength {s}", smooth(o));
            assert!(((1.0 - o) * w - 24.0).abs() < 1e-3, "full alpha 24 in at strength {s}");
        }
    }

    #[test]
    fn reveal_leaves_the_margin_unless_the_row_cannot_fit_it() {
        assert_eq!(reveal(100.0, 300.0, 20.0, 200.0, 0.0), 120.0, "flush below");
        assert_eq!(reveal(100.0, 300.0, 20.0, 200.0, 24.0), 144.0, "clear of a 24pt fade below");
        assert_eq!(reveal(100.0, 90.0, 20.0, 200.0, 24.0), 66.0, "and above");
        assert_eq!(reveal(100.0, 150.0, 20.0, 200.0, 24.0), 100.0, "already clear: no move");
        assert_eq!(reveal(100.0, 300.0, 180.0, 200.0, 24.0), 290.0, "a tall row splits what space is left");
        assert_eq!(reveal(100.0, 300.0, 500.0, 200.0, 24.0), 600.0, "a row taller than the viewport, below: its end");
        assert_eq!(reveal(600.0, 300.0, 500.0, 200.0, 24.0), 600.0, "and placing it again leaves it there");
        assert_eq!(reveal(400.0, 300.0, 500.0, 200.0, 24.0), 400.0, "a row filling the viewport: no move");
        assert_eq!(reveal(700.0, 300.0, 500.0, 200.0, 24.0), 300.0, "its end in view: its start");
    }
}
