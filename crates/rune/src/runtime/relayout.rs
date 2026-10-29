//! Laying a flattened tree out: Taffy, then each node's place and text.

use super::*;

impl<S: 'static> Measurement<'_, S> {
    /// Lays `nodes` out from `root_id` at the viewport, places them, and lays out their text.
    pub(super) fn solve(&mut self, mut taffy: TaffyTree<usize>, root_id: taffy::NodeId, mut nodes: Vec<Node>) -> (TaffyTree<usize>, Vec<Node>) {
        let scale = self.env.scale;
        let room = Size {
            width: AvailableSpace::Definite(self.viewport.0),
            height: if self.fit_height { AvailableSpace::MaxContent } else { AvailableSpace::Definite(self.viewport.1) },
        };
        {
            let _layout_time = crate::frame_profile::Scope::new(crate::frame_profile::Phase::Layout);
            compute_layout(&mut taffy, root_id, room, &nodes, &mut self.text, scale);
            // `aspect_fit` elements laid out filling their room; fix each at the largest box
            // at its ratio inside it and lay out again, so siblings and alignment follow.
            let mut refit = false;
            for n in &nodes {
                if let Some(fit) = &n.aspect_fit {
                    let style = fitted(fit.0, &fit.1, taffy.layout(n.taffy).unwrap().size);
                    taffy.set_style(n.taffy, style).unwrap();
                    refit = true;
                }
            }
            if refit { compute_layout(&mut taffy, root_id, room, &nodes, &mut self.text, scale); }
        }
        self.stats.layout_nodes = nodes.len();
        #[cfg(debug_assertions)]
        layout_diagnostics::collapsed_scrolls(&taffy, &nodes);

        self.place(&taffy, &mut nodes);
        if self.fit_anchored(&mut taffy, &nodes) {
            compute_layout(&mut taffy, root_id, room, &nodes, &mut self.text, scale);
            self.place(&taffy, &mut nodes);
        }
        for n in &mut nodes {
            let l = taffy.layout(n.taffy).unwrap();
            let ins = n.inset;
            // A cell of a run draws from the run's layout (and keeps it cached) instead of its own.
            let run = n.text.as_ref().and_then(|t| t.advance_in.as_ref().map(|(run, at)| (self.text.run_layout(run, t.style, scale), *at)));
            if let Some((r, _)) = run.filter(|(r, at)| r.x(*at).is_some()) {
                n.text_layout = Some(r.layout.clone());
                n.run_layout = Some(r);
            } else if let Some(spec) = &n.text {
                let w = (l.size.width - ins[0] - ins[2]).max(0.0);
                let wrap = spec.wrap.then_some(w * scale);
                let mut layout = self.text.layout_for_spec(spec, scale, wrap, line_alignment(spec));
                let mut elided = None;
                if spec.truncate && layout.full_width() > w * scale + 0.5 {
                    let (cut, e) = truncated(&mut self.text, spec, scale, w * scale);
                    (layout, elided) = (cut, Some(e));
                } else if let Some((cut, e)) = clamped(&mut self.text, spec, scale, w * scale, &layout) {
                    (layout, elided) = (cut, Some(e));
                }
                n.text_layout = Some(layout);
                n.elided = elided;
            }
        }

        (taffy, nodes)
    }
}
