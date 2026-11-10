//! Contents that slide with their growth row. When the top of a parent's contents moves in a
//! growth row (a padding or gap above them changed) and the row's height changes the other way
//! to fit, they slide as its growth opens: the slide left shrinks with the growth left. A list
//! pinned to its end moves the row by what it grows, so contents whose place doesn't change hold
//! still on every frame, instead of netting a spring against the growth. A growth retargeted so
//! it no longer opens toward them lets the rest spring out on the row's growth spring.
use super::*;

/// The contents of a growth stack's rows sliding with them, drawn outside the scene.
#[derive(Clone, Debug, Default)]
pub(super) struct ContentSlides(Vec<Slide>);

#[derive(Clone, Copy, Debug)]
struct Slide {
    node: NodeId,
    row: NodeId,
    phase: SlidePhase,
}

/// Where a slide is: how far down from its layout it shows, and how fast that changes
/// (points, per second).
#[derive(Clone, Copy, Debug, PartialEq)]
enum SlidePhase {
    /// Shrinking with the growth left to open; `at` is the row's presented height when `dy`
    /// was last measured, and `v` how fast it last moved, which a spring takes over.
    Following { dy: f32, v: f32, at: f32 },
    /// Springing to its layout on the row's growth spring, once the growth stopped opening
    /// toward it. It never follows again.
    Settling { dy: f32, v: f32 },
}

impl SlidePhase {
    fn dy(self) -> f32 {
        match self { SlidePhase::Following { dy, .. } | SlidePhase::Settling { dy, .. } => dy }
    }

    /// The phase after row growth `g` stepped `steps` substeps.
    fn step(self, g: &GrowTrack, spring: Spring, steps: usize) -> SlidePhase {
        let dt = steps as f32 * SUBSTEP;
        let (mut dy, mut v) = match self {
            SlidePhase::Following { dy, v, at } => {
                let (was, left) = (g.target - at, g.target - g.h);
                if was * dy < 0.0 && was.abs() > 1.0 / 64.0 {
                    let next = dy * (left / was).clamp(0.0, 1.0);
                    let v = if dt > 0.0 { (next - dy) / dt } else { v };
                    return SlidePhase::Following { dy: next, v, at: g.h };
                }
                (dy, v)
            }
            SlidePhase::Settling { dy, v } => (dy, v),
        };
        for _ in 0..steps {
            (dy, v) = spring.sample(dy, v, 0.0, SUBSTEP);
        }
        SlidePhase::Settling { dy, v }
    }

    /// Whether it still shows off its layout, or still moves toward it.
    fn live(self) -> bool {
        match self {
            SlidePhase::Following { dy, .. } => dy.abs() > 1.0 / 128.0,
            SlidePhase::Settling { dy, v } => dy.abs() > 1.0 / 128.0 || v.abs() > 0.5,
        }
    }
}

impl ContentSlides {
    /// Each sliding node and how far down from its layout it shows.
    pub(super) fn shifts(&self) -> impl Iterator<Item = (NodeId, f32)> + '_ {
        self.0.iter().map(|s| (s.node, s.phase.dy()))
    }

    /// Slides `node`, drawn `dy` points down from its layout plus any slide it still had, with
    /// row `row`'s growth `g`, if that growth opens toward its layout. Otherwise it stops
    /// following and the whole amount is returned, for the caller to slide some other way.
    fn follow(&mut self, node: NodeId, row: NodeId, g: &GrowTrack, dy: f32) -> f32 {
        let dy = dy + self.0.iter().position(|s| s.node == node).map_or(0.0, |k| self.0.swap_remove(k).phase.dy());
        let left = g.target - g.h;
        if left * dy < 0.0 && left.abs() > 1.0 / 64.0 {
            self.0.push(Slide { node, row, phase: SlidePhase::Following { dy, v: 0.0, at: g.h } });
            return 0.0;
        }
        dy
    }

    /// Lands row `row`'s slides at once (its growth landed at once).
    pub(super) fn land(&mut self, row: NodeId) {
        self.0.retain(|s| s.row != row);
    }

    /// Steps row `row`'s slides after its growth `g` stepped `steps` substeps. Whether any still
    /// moves.
    pub(super) fn step(&mut self, row: NodeId, g: &GrowTrack, spring: Spring, steps: usize) -> bool {
        for s in self.0.iter_mut().filter(|s| s.row == row) {
            s.phase = s.phase.step(g, spring, steps);
        }
        self.0.retain(|s| s.phase.live());
        self.0.iter().any(|s| s.row == row)
    }

    /// Drops the slides of rows that left the stack.
    pub(super) fn keep(&mut self, rows: &[NodeId]) {
        self.0.retain(|s| rows.contains(&s.row));
    }
}

impl<S: 'static> Runtime<S> {
    /// Before node `id`, a child of `parent` whose contents' top moved `dy` points down in its
    /// growth row (drawn less new), takes that slide: with the row's growth where it opens
    /// toward them. How far it still has to slide from where it was drawn.
    pub(super) fn slide_with_row(&mut self, nodes: &[Node], parent: usize, id: NodeId, dy: f32) -> f32 {
        let Some(row) = std::iter::successors(Some(parent), |&p| nodes[p].parent)
            .find(|&r| nodes[r].parent.is_some_and(|c| nodes[c].growth.is_some()))
        else { return dy };
        let Some(c) = nodes[row].parent else { return dy };
        let (Some(stack), Some(g)) = (self.stacks.get_mut(&nodes[c].id), self.grows.get(&nodes[row].id)) else { return dy };
        stack.slides.follow(id, nodes[row].id, g, dy)
    }
}

#[cfg(test)]
#[path = "content_slide_tests.rs"]
mod tests;
