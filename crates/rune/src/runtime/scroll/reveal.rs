//! `Tx::scroll_into_view` and `Tx::scroll_center`: revealing or centring a keyed element,
//! virtual row or `TextHighlight` inside its scroll viewport.
use super::*;
use crate::el::virtual_list::VirtualList;

/// One `Tx::scroll_into_view` or `Tx::scroll_center` request.
#[derive(Clone, Debug)]
pub(crate) struct ScrollReveal {
    pub scope: Option<String>,
    pub key: String,
    /// Centre the target vertically rather than scroll it just into view.
    pub center: bool,
    /// Glide there on the scroll spring rather than jump.
    pub animated: bool,
}

impl ScrollReveal {
    pub fn into_view(scope: Option<String>, key: String) -> Self {
        ScrollReveal { scope, key, center: false, animated: false }
    }

    pub fn center(scope: Option<String>, key: String, animated: bool) -> Self {
        ScrollReveal { scope, key, center: true, animated }
    }
}

/// Whether virtual list `list_path` matches a scope prefix `scope` from a key path: exact
/// path, `/`-aligned encoded path suffix, or keyed-ancestor suffix (skipping unkeyed `#n`
/// segments and decoding escaped keys).
fn list_matches_scope(list_path: &str, scope: &str) -> bool {
    if scope.is_empty() {
        return false;
    }
    list_path == scope
        || list_path.strip_suffix(scope).is_some_and(|head| head.ends_with('/'))
        || element_path::keyed_suffix_matches(list_path, scope)
}

/// Resolves one candidate row key against `list`, accepting either the raw row key or its
/// escaped path segment (`#!...`).
fn row_index(list: &VirtualList, candidate: &str) -> Option<usize> {
    list.index_of(candidate).or_else(|| {
        candidate
            .starts_with("#!")
            .then(|| element_path::decode_key_segment(candidate))
            .flatten()
            .and_then(|decoded| list.index_of(&decoded))
    })
}

/// Resolves a scroll selector `key` (within optional ancestor `scope`) against unmounted
/// virtual `lists` (`(list_path, list)` in tree order), returning `(list_index, row_index)`.
///
/// Direct row matches (plain key or `scope/row`) take precedence over descendant-path matches
/// (`scope/row/mark`, then `row/mark`) so a scoped row in a later list beats an earlier list
/// whose row key happens to equal that scope name.
pub(super) fn resolve_virtual_row_in(
    lists: &[(&str, &VirtualList)],
    scope: Option<&str>,
    key: &str,
) -> Option<(usize, usize)> {
    if key.is_empty() {
        return None;
    }
    let allowed = |&(path, list): &(&str, &VirtualList)| {
        in_scope(path, scope) && in_scope(&list.scroll_path, scope)
    };

    // Pass 1: plain row key (`"row"`, including row keys containing `/`) or scoped row path
    // (`"scope/row"`, including multi-segment scopes, full paths and escaped segments).
    for (list_idx, entry @ &(path, list)) in lists.iter().enumerate() {
        if !allowed(entry) {
            continue;
        }
        if let Some(row_idx) = list.index_of(key) {
            return Some((list_idx, row_idx));
        }
        if let Some(row_idx) = key.rmatch_indices('/').find_map(|(at, _)| {
            let (prefix, row_key) = (&key[..at], &key[at + 1..]);
            list_matches_scope(path, prefix).then(|| row_index(list, row_key))?
        }) {
            return Some((list_idx, row_idx));
        }
    }

    // Pass 2: scoped descendant path (`"scope/row/mark"`, for an element or `TextHighlight`
    // inside `row`). Longest row key wins when row keys contain `/`.
    for (list_idx, entry @ &(path, list)) in lists.iter().enumerate() {
        if !allowed(entry) {
            continue;
        }
        if let Some(row_idx) = key.rmatch_indices('/').find_map(|(at, _)| {
            let (prefix, after) = (&key[..at], &key[at + 1..]);
            if !list_matches_scope(path, prefix) {
                return None;
            }
            after.rmatch_indices('/').find_map(|(end, _)| row_index(list, &after[..end]))
        }) {
            return Some((list_idx, row_idx));
        }
    }

    // Pass 3: unscoped descendant path (`"row/mark"`). Longest row key wins when row keys
    // contain `/`.
    for (list_idx, entry @ &(_, list)) in lists.iter().enumerate() {
        if !allowed(entry) {
            continue;
        }
        if let Some(row_idx) = key.rmatch_indices('/').find_map(|(at, _)| row_index(list, &key[..at])) {
            return Some((list_idx, row_idx));
        }
    }

    None
}

/// The offset that centres `[top, top + height)` in the viewport, or that scrolls it into view
/// from `offset`, before clamping.
pub(in crate::runtime) fn place(center: bool, offset: f64, top: f64, height: f64, viewport: f64, margin: f64) -> f64 {
    if center {
        (top - ((viewport - height) * 0.5).max(0.0)).max(0.0)
    } else {
        scroll_edge::reveal(offset, top, height, viewport, margin)
    }
}

impl RuntimeCore {
    /// Scrolls so row or highlight `r.key` shows, or sits centred when `r.center`. A virtual row
    /// placed by estimates may measure differently once mounted, along with the rows around it,
    /// so this places it again from what the commit measured until it stays put. Those commits
    /// are `instant` when the request's was, or when it glides: the glide then starts back
    /// where the viewport stood and runs to that place, keeping the speed of one already running.
    pub(in crate::runtime) fn reveal_row(&mut self, app: &mut dyn AppDriver, r: &ScrollReveal, instant: bool) {
        let (scope, key) = (r.scope.as_deref(), r.key.as_str());
        let glide = r.animated && !instant && !self.reduced_motion && !self.settled;
        let (mut window, was) = self.reveal_row_once(app, scope, key, r.center, true, instant || glide);
        for _ in 0..8 {
            if !window { break; }
            window = self.reveal_row_once(app, scope, key, r.center, false, instant || glide).0;
        }
        if glide && let Some(Was { id, offset: start, glide: v, viewport }) = was
            && let Some(st) = self.scroll.scrolls.get_mut(&id)
            && (st.offset - start).abs() > 0.5
        {
            // A long trip cuts to a couple of viewports out first, as `Tx::scroll_to` does.
            let end = st.offset;
            let reach = scroll_bottom::GLIDE_REACH * viewport;
            let before = st.offset;
            st.offset = start.clamp(end - reach, end + reach);
            st.mode = st.mode.to_glide(scroll_offset::retarget_v(crate::tok::motion::GLIDE, st.offset, v.unwrap_or(0.0), end), GlideTo::Offset(end));
            st.spring = None;
            st.moved();
            if (st.offset - before).abs() > 1e-6 { st.note_source(crate::ScrollSource::Programmatic); }
            if self.virtual_window_changed() {
                self.commit_virtual_scroll(app, &Tx { commit: Commit::Instant, ..Tx::default() });
            }
        }
        self.reveal_row_x(scope, key);
    }

    /// One placement of [`Self::reveal_row`]: whether it committed, and on the first, where the
    /// viewport stood before. After the first, it moves only when the row would land somewhere
    /// else.
    fn reveal_row_once(&mut self, app: &mut dyn AppDriver, scope: Option<&str>, key: &str, center: bool, first: bool, instant: bool) -> (bool, Option<Was>) {
        let target_node = self.live_named(scope, key)
            .map(|i| (i, self.tree.nodes[i].target[1], self.tree.nodes[i].local.h, true))
            .or_else(|| {
                let (i, [_, y0, _, y1]) = self.highlight_target_in(&self.tree.nodes, scope, key)?;
                (!self.subtree_exiting(i)).then_some((i, y0, (y1 - y0).max(0.0), false))
            });
        let geometry = target_node.and_then(|(i, target_top, target_h, is_whole_node)| {
            let s = self.viewport_of(i).filter(|&s| in_scope(&self.tree.nodes[s].path, scope))?;
            let parent = &self.tree.nodes[s];
            let (laid_top, laid_h) = (target_top - parent.target[1], target_h);
            let (top, height) = std::iter::successors(Some(i), |&p| self.tree.nodes[p].parent)
                .take_while(|&p| p != s)
                .find_map(|r| {
                    let c = self.tree.nodes[r].parent?;
                    self.tree.nodes[c].growth.as_ref()?;
                    let stack = self.motion.stacks.get(&self.tree.nodes[c].id)?;
                    let rid = self.tree.nodes[r].id;
                    let mut count = 0usize;
                    let before_r: f32 = stack.order.iter()
                        .take_while(|&&id| id != rid)
                        .filter_map(|id| self.motion.grows.get(id))
                        .map(|g| { count += 1; g.h })
                        .sum::<f32>() + stack.gap * count as f32;
                    let r_top = (self.tree.nodes[c].target[1] - parent.target[1]) + stack.pad_top + before_r;
                    let r_h = self.motion.grows.get(&rid).map_or(self.tree.nodes[r].local.h, |g| g.h);
                    if i == r && is_whole_node {
                        Some((r_top, r_h))
                    } else {
                        let in_row = target_top - self.tree.nodes[r].target[1];
                        Some((r_top + in_row, target_h))
                    }
                })
                .unwrap_or((laid_top, laid_h));
            let laid_end = self.scroll_children(s).map(|c| self.tree.nodes[c].target[3] - parent.target[1]).reduce(f32::max).unwrap_or(0.0);
            Some((parent.path.clone(), f64::from(top), f64::from(height), f64::from(parent.local.h), f64::from(laid_top), f64::from(laid_h), f64::from(laid_end)))
        }).or_else(|| {
            let (v, index) = self.resolve_virtual_row(scope, key)?;
            let row_key = &v.keys[index];
            let top = f64::from(v.top + v.pad_top + v.prefix(index));
            let height = f64::from(v.height(row_key));
            let end = f64::from(v.top + v.pad_top + v.prefix(v.keys.len()) + v.pad_bottom);
            Some((v.scroll_path.clone(), top, height, f64::from(v.viewport), top, height, end))
        });
        let Some((path, top, height, viewport, laid_top, laid_h, laid_end)) = geometry else { return (false, None) };
        // Stuck headers cover the viewport's top: land the row below them.
        let Some(cover) = self.stuck_cover(&path, scope, key) else { return (false, None) };
        let (top, height) = (top - cover, height + cover);
        let (laid_top, laid_h) = (laid_top - cover, laid_h + cover);
        let margin = self.node(&path).map_or(0.0, |n| f64::from(n.scroll_edges.margin));
        let Some(&id) = self.tree.path_ids.get(&path) else { return (false, None) };
        let Some(&s_idx) = self.tree.node_index.get(&id) else { return (false, None) };
        let stick = self.tree.nodes[s_idx].scroll.is_some_and(|s| s.stick_bottom);
        let max = self.scroll_overflow(s_idx).max(0.0);
        let Some(scroll) = self.scroll.scrolls.get_mut(&id) else { return (false, None) };
        let was = first.then_some(Was { id, offset: scroll.offset, glide: scroll.glide(), viewport });
        scroll.max = max;
        if first { scroll.halt(); }
        let offset = place(center, scroll.offset, top, height, viewport, margin).clamp(0.0, scroll.max);
        let laid_max = (laid_end - viewport).max(0.0);
        let laid_to = place(center, scroll.offset, laid_top, laid_h, viewport, margin).clamp(0.0, laid_max);
        let at_end = stick && (scroll.pinned() || offset >= scroll.max - 0.5) && laid_to >= laid_max - 0.5;
        let effects = self.scroll.requests.step(requests::Event::Place { first, scope, key, viewport: &path,
            center, at_end, from: scroll.offset, to: offset });
        let Some(effect) = effects.into_iter().next() else { return (false, was) };
        match effect {
            requests::Effect::Move(to) => {
                let before = scroll.offset;
                scroll.offset = to;
                scroll.mode = scroll.mode.to_rest();
                scroll.moved();
                if (scroll.offset - before).abs() > 1e-6 { scroll.note_source(crate::ScrollSource::Programmatic); }
            }
            requests::Effect::Rest => {
                scroll.mode = scroll.mode.to_rest();
                return (false, was);
            }
        }
        let window = if first { self.virtual_window_changed() } else { self.virtual_viewport_uncovered() };
        if window {
            self.commit_virtual_scroll(app, &Tx { commit: if instant { Commit::Instant } else { Commit::Animate }, ..Tx::default() });
            // The commit measured rows that were estimates: the next placement clamps to the end
            // they make.
            if let Some(i) = self.tree.nodes.iter().position(|n| n.id == id) {
                let max = self.scroll_overflow(i).max(0.0);
                if let Some(scroll) = self.scroll.scrolls.get_mut(&id) {
                    scroll.max = max;
                    scroll.mode = scroll.mode.to_rest();
                }
            }
        }
        (window, was)
    }
}

impl RuntimeCore {
    /// Resolves a scroll selector `key` (within optional ancestor `scope`) to an unmounted
    /// virtual list and row index in tree order.
    pub(super) fn resolve_virtual_row(&self, scope: Option<&str>, key: &str) -> Option<(&VirtualList, usize)> {
        if key.is_empty() || self.commit.virtuals.is_empty() {
            return None;
        }
        let lists: Vec<(&str, &VirtualList)> = self.tree.nodes.iter()
            .filter_map(|n| Some((n.path.as_str(), self.commit.virtuals.get(&n.path)?)))
            .collect();
        let (list_idx, row_idx) = resolve_virtual_row_in(&lists, scope, key)?;
        Some((lists[list_idx].1, row_idx))
    }
}

/// Where a reveal's viewport stood before it moved, for the glide that follows.
struct Was {
    id: NodeId,
    offset: f64,
    glide: Option<f32>,
    viewport: f64,
}

#[cfg(test)]
#[path = "reveal_tests.rs"]
mod tests;
