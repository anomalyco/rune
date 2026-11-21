//! A run of already visible faces uses its trailing face as its coordinate. Reordering
//! selects the real rows immediately; only the presented seam is shared. A row passing
//! between two faces opens that seam as its presented edge reaches it.
use super::*;

#[derive(Clone, Copy, Debug)]
struct Face {
    row: NodeId,
    ink: NodeId,
    height: f32,
}

#[derive(Clone, Copy, Debug)]
enum Boundary {
    Joined { raw_from: f32 },
    Opening { between: NodeId },
}

#[derive(Clone, Copy, Debug)]
pub(in crate::runtime) struct Seam {
    top: Face,
    bottom: Face,
    from: f32,
    raw: f32,
    boundary: Boundary,
}

impl RuntimeCore {
    pub(in crate::runtime) fn join_live_rows(
        &self,
        stack: &mut Stack,
        nodes: &[Node],
        rows: &[usize],
        list: &crate::el::virtual_list::VirtualList,
        range: &std::ops::Range<usize>,
    ) {
        let old = std::mem::take(&mut stack.seams);
        if stack.reorder.jumps.is_empty() && old.is_empty() {
            return;
        }
        let mut previous: Option<(usize, Face, f32)> = None;
        let mut saved = paint_tracks::Finder::default();
        for (at, &r) in rows.iter().enumerate() {
            let Some(key) = nodes[r].vis.row_face else { continue };
            if !nodes[r].whole || nodes[r].streaming || nodes[r].instant_changes {
                previous = None;
                continue;
            }
            let Some(i) =
                (r..nodes.len()).take_while(|&j| j == r || nodes[j].parent.is_some_and(|p| p >= r)).find(|&j| nodes[j].named(key))
            else {
                previous = None;
                continue;
            };
            let face = Face { row: nodes[r].id, ink: nodes[i].id, height: nodes[i].local.h };
            let seen = nodes[r]
                .key
                .as_deref()
                .is_some_and(|k| list.index_of(k).is_some_and(|p| range.contains(&p)) && list.drawn.iter().any(|s| s == k));
            let Some(ink) = saved
                .find(&self.tree.nodes, face.ink)
                .and_then(|oi| self.paint.drawn.get(oi))
                .filter(|d| seen && d[3] > 0.0 && (d[3] - nodes[i].local.h).abs() < 0.1)
            else {
                previous = None;
                continue;
            };
            let Some((prev, top, edge)) = previous.replace((at, face, ink[1] + ink[3])) else { continue };
            let joined = prev + 1 == at;
            let was_joined = stack.order.windows(2).any(|w| w == [top.row, face.row]);
            let kept = old.iter().find(|s| s.top.row == top.row && s.bottom.row == face.row);
            if !joined && !was_joined && kept.is_none() {
                continue;
            }
            if kept.is_none() && stack.reorder.jump(top.row) == 0.0 && stack.reorder.jump(face.row) == 0.0 {
                continue;
            }
            let from = ink[1] - edge;
            let raw = kept.map_or(from, |s| s.raw);
            let boundary =
                if joined { Boundary::Joined { raw_from: raw } } else { Boundary::Opening { between: nodes[rows[prev + 1]].id } };
            stack.seams.push(Seam { top, bottom: face, from, raw, boundary });
        }
    }

    pub(in crate::runtime) fn place_joined_rows(&mut self) {
        for stack in self.motion.stacks.values_mut().filter(|s| !s.seams.is_empty()) {
            // Trailing to leading: a third face inherits the same boundary, not a new spring.
            for seam in stack.seams.iter_mut().rev() {
                let indices = |f: Face| Some((self.tree.index_of_id(f.row)?, self.tree.index_of_id(f.ink)?));
                let (Some((a, i)), Some((b, j))) = (indices(seam.top), indices(seam.bottom)) else { continue };
                if self.tree.nodes[i].local.h != seam.top.height || self.tree.nodes[j].local.h != seam.bottom.height {
                    continue;
                }
                let top = |row: usize, ink: usize, dy: &[f32]| {
                    self.scene.presented(self.tree.nodes[ink].id).map(|p| {
                        p.rect.y
                            + std::iter::successors(Some(ink), |&k| self.tree.nodes[k].parent.filter(|&q| q >= row)).map(|k| dy[k]).sum::<f32>()
                    })
                };
                let (Some(ay), Some(by)) = (top(a, i, &self.paint.row_dy), top(b, j, &self.paint.row_dy)) else { continue };
                let end = ay + self.tree.nodes[i].local.h;
                let raw = by - end;
                let goal = self.tree.nodes[j].target[1] - self.tree.nodes[i].target[3];
                let want = match seam.boundary {
                    Boundary::Joined { raw_from } => {
                        let left = if (raw_from - goal).abs() < 0.01 { 0.0 } else { ((raw - goal) / (raw_from - goal)).clamp(0.0, 1.0) };
                        goal + (seam.from - goal) * left
                    }
                    Boundary::Opening { between } => {
                        let Some(k) = self.tree.index_of_id(between) else { continue };
                        let Some(y) = top(k, k, &self.paint.row_dy) else { continue };
                        let depth = self.tree.nodes[k].target[3] - self.tree.nodes[i].target[3];
                        let t = ((y + self.paint.row_h[k] - end) / depth.max(1.0)).clamp(0.0, 1.0);
                        seam.from + (raw - seam.from) * t * t * (3.0 - 2.0 * t)
                    }
                };
                seam.raw = raw;
                self.paint.row_dy[a] += raw - want;
                // These complete faces were both visible before the reorder and have not
                // grown. Only their surrounding room changes. Don't composite their words
                // through a reveal layer; new rows and streaming edges retain their masks.
                for r in [a, b] {
                    if self.motion.grows.get(&self.tree.nodes[r].id).is_some_and(|g| g.whole && g.linear_edge) {
                        self.paint.row_mask[r] = -1.0;
                    }
                }
            }
            if !stack.moving
                && !stack
                    .seams
                    .iter()
                    .any(|s| [s.top.row, s.bottom.row].iter().any(|&id| self.scene.store().is_animating(id, prop::OFFSET)))
            {
                stack.seams.clear();
            }
        }
    }
}
