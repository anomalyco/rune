//! Where a scroll viewport stands against its bottom: `El::on_at_bottom` reports changes, and
//! `Tx::scroll_to_bottom` glides there (a chat's "jump to latest").
use super::*;

/// The glide's spring (`motion::GLIDE`) retargets each frame, so content growing under it
/// extends the trip.
/// A glide starts at most this many viewports above the bottom (a longer trip cuts first).
pub(in crate::runtime) const GLIDE_REACH: f64 = 2.0;
/// Within this many points of the bottom a viewport counts as showing it.
const AT_BOTTOM: f64 = 1.0;
/// Slack starts closing once its stack has stopped growing for this long (seconds).
const SLACK_WAIT: f64 = 0.12;

/// Room a pinned viewport gains at its bottom (a composer shrinking under it), held under its
/// growth stack's last row instead of carrying the rows down. Rows growing in fill it first;
/// once the stack has stopped growing for `SLACK_WAIT`, the rest closes on the growth spring.
/// Without it, a send dips the rows: the composer gives its room back faster than the new
/// row's capped growth takes it.
#[derive(Clone, Copy, Debug, Default)]
pub(in crate::runtime) struct Slack {
    /// `h` is the slack in points, stepped toward 0 once closing.
    track: GrowTrack,
    phase: SlackPhase,
    /// The last step moved `h`, so the frame after it still counts as moving.
    stepped: bool,
    /// The viewport's presented top and bottom last frame.
    edges: Option<[f32; 2]>,
    /// The window size at the last commit: a resize lands the slack.
    pub(in crate::runtime) window: (f32, f32),
    /// `Tx::follow_edge`: until the viewport's edges reach their layout, the rows ride them
    /// and the room they free adds no slack.
    pub(in crate::runtime) follow: bool,
}

/// Whether the slack waits under the rows or closes on the growth spring.
#[derive(Clone, Copy, Debug, PartialEq)]
enum SlackPhase {
    /// Held while the stack grows, and for `SLACK_WAIT` after a row last grew (`grew_at`).
    Held { grew_at: f64 },
    /// Closing, stepped by each frame's time.
    Closing,
}

impl Default for SlackPhase {
    fn default() -> Self { SlackPhase::Held { grew_at: 0.0 } }
}

/// Done closing without the stack growing again: the wait is already over, so slack that opens
/// next closes at once.
const WAITED: SlackPhase = SlackPhase::Held { grew_at: f64::NEG_INFINITY };

impl Slack {
    pub(in crate::runtime) fn height(&self) -> f32 { self.track.h }

    pub(in crate::runtime) fn moving(&self) -> bool { self.track.h > 0.0 || self.stepped }

    /// No slack, in a window of this size.
    pub(in crate::runtime) fn landed(window: (f32, f32)) -> Slack { Slack { window, ..Slack::default() } }

    /// Moves the phase on at `now` with `dt` of frame time, once `h` has this frame's growth
    /// and edges: a growing stack holds the slack, one quiet for `SLACK_WAIT` starts it closing,
    /// and closing steps `h` toward 0 on the stack's spring.
    fn settle(&mut self, growing: bool, now: f64, dt: f32, spec: &crate::GrowthSpec) {
        if growing {
            self.phase = SlackPhase::Held { grew_at: now };
            self.track.v = 0.0;
        }
        if self.track.h <= 1.0 / 128.0 {
            self.track = GrowTrack::default();
            if matches!(self.phase, SlackPhase::Closing) { self.phase = WAITED; }
        } else if let SlackPhase::Held { grew_at } = self.phase && now - grew_at >= SLACK_WAIT {
            self.phase = SlackPhase::Closing;
            self.track.age = 0.0;
            self.track.v = 0.0;
        }
        if let SlackPhase::Closing = self.phase {
            for step in crate::growth::substeps(dt) {
                self.track.step_by(step, spec.spring, &spec.edge, false);
            }
            self.phase = if self.track.moving() { SlackPhase::Closing } else {
                self.track = GrowTrack::default();
                WAITED
            };
        }
    }
}

/// Steps a glide toward the current bottom, or the offset `Tx::scroll_to` asked for while the
/// content reaches it. Returns whether it is still moving.
pub(in crate::runtime) fn glide(st: &mut ScrollState, dt: f32) -> bool {
    let ScrollMode::Glide { v, to: aim, .. } = st.mode else { return false };
    let to = aim.offset().map_or(st.max, |to| to.min(st.max));
    // Content shrinking past the glide's end lands it there, rather than its speed carrying
    // it past and springing back.
    if st.offset >= st.max && to >= st.max {
        st.offset = st.max;
        st.mode = st.mode.to_rest();
        return false;
    }
    let mut glide = Some(v);
    let moving = scroll_offset::glide_axis(st.glide_spring(), &mut st.offset, &mut glide, to, dt);
    st.mode = match glide {
        Some(v) => st.mode.to_glide(v, aim),
        None => st.mode.to_rest(),
    };
    moving
}

impl requests::ScrollTables {
    #[inline]
    pub(in crate::runtime) fn at_bottom(&self, id: NodeId) -> Option<bool> {
        self.scrolls.get(&id).map(|st| st.offset >= st.max - AT_BOTTOM)
    }
}

impl RuntimeCore {
    /// The scroll viewport `key` names (`Node::named`), or the nearest one containing it.
    pub(in crate::runtime) fn scroll_viewport(&self, key: &str) -> Option<usize> {
        let mut i = self.tree.nodes.iter().enumerate()
            .position(|(i, n)| n.named(key) && !self.subtree_exiting(i))?;
        loop {
            if self.tree.nodes[i].scroll.is_some() && self.tree.nodes[i].scroll_axes.y() { return Some(i); }
            i = self.tree.nodes[i].parent?;
        }
    }

    pub(in crate::runtime) fn scroll_to_bottom(&mut self, app: &mut dyn AppDriver, requests: Vec<(String, bool)>) {
        // This frame's commit may have changed the content: measure the extent first.
        self.step_scroll(0.0);
        let mut targets = Vec::new();
        for (key, animated) in requests {
            let Some(i) = self.scroll_viewport(&key) else { continue };
            let (id, sticky, view_h) = (self.tree.nodes[i].id, self.tree.nodes[i].scroll.is_some_and(|s| s.stick_bottom), f64::from(self.tree.nodes[i].local.h));
            let Some(st) = self.scroll.scrolls.get_mut(&id) else { continue };
            targets.push((id, animated, sticky, view_h, st.offset, st.glide()));
            // Stand at the bottom first so a virtual list mounts and measures its tail rows,
            // which can differ from the estimate: the trip below then ends at the true bottom.
            st.offset = st.max;
            st.pin();
        }
        for _ in 0..3 {
            if !self.virtual_window_changed() { break; }
            self.commit_virtual_scroll(app, &Tx::default());
            self.step_scroll(0.0);
            for (id, ..) in &targets {
                if let Some(st) = self.scroll.scrolls.get_mut(id) { st.offset = st.max; }
            }
        }
        for (id, animated, sticky, view_h, start, glide) in targets {
            let Some(st) = self.scroll.scrolls.get_mut(&id) else { continue };
            // A settled frame (a window showing or uncovering) lands at the end.
            if animated && !self.reduced_motion && !self.settled && st.max - start > 0.5 {
                st.offset = start.max(st.max - GLIDE_REACH * view_h);
                st.mode = st.mode.to_glide(glide.unwrap_or(0.0), GlideTo::Bottom);
                st.spring = None;
            } else {
                st.offset = st.max;
                if sticky { st.pin() } else { st.mode = st.mode.to_rest() }
            }
        }
        if self.virtual_window_changed() {
            self.commit_virtual_scroll(app, &Tx::default());
            self.step_scroll(0.0);
        }
    }

    /// `Tx::scroll_page`: each viewport moves by its requested pages.
    pub(in crate::runtime) fn scroll_pages(&mut self, app: &mut dyn AppDriver, requests: Vec<(String, i32)>) {
        // This frame's commit may have changed the content: measure the extent first.
        self.step_scroll(0.0);
        for (key, pages) in requests {
            let Some(i) = self.scroll_viewport(&key) else { continue };
            self.page_to(i, |offset, _, page| offset + page * f64::from(pages));
        }
        if self.virtual_window_changed() {
            self.commit_virtual_scroll(app, &Tx::default());
            self.step_scroll(0.0);
        }
    }

    /// The offset to draw stuck-to-bottom viewport `i` at, whose presented bottom edge is
    /// `bottom`: layout's offset keeps the content's target bottom on the viewport's target
    /// bottom, but while either is still animating (the viewport resizing under a banner)
    /// that would jump the rows and slide them back. This keeps the presented edges together.
    ///
    /// Rows that sat on the bottom hold it as the content crosses between overflowing and
    /// fitting: while they still end on the bottom (`justify_end`), or until the content's top
    /// would carry them above it, after which they ride the content's top. The offset goes
    /// negative while fitting rows sit low in a box that has not shrunk yet.
    pub(in crate::runtime) fn stuck_offset(&self, i: usize, bottom: f32) -> Option<f64> {
        let n = &self.tree.nodes[i];
        let st = self.scroll.scrolls.get(&n.id).filter(|st| st.pinned())?;
        if !n.scroll.is_some_and(|s| s.stick_bottom) { return None; }
        let c = self.scroll_content(i)?;
        let lag = self.stuck_lag(i, c, bottom)?;
        // How far the rows end above the bottom in the layout (negative past it), beside the
        // stack's own bottom padding and border.
        let content_bottom = self.scroll_children(i)
            .map(|k| {
                let pad = self.motion.stacks.get(&self.tree.nodes[k].id).map_or(0.0, |s| (s.pad_bottom - self.tree.nodes[k].inset[3]).max(0.0));
                self.tree.nodes[k].target[3] - pad
            })
            .reduce(f32::max)?;
        let short = f64::from(n.target[3] - content_bottom);
        // Stuck on a shrinking stack's end: the rows' last fraction collapses under it (see `step_scroll`).
        let end = st.laid().filter(|&end| st.offset <= end + 1e-6).unwrap_or(f64::INFINITY);
        if st.held && (short <= 0.5 || lag - short >= -0.5) {
            Some(self.scroll_overflow(i).min(end) + lag)
        } else if st.max > 0.0 {
            Some((st.max.min(end) + lag).max(0.0))
        } else {
            // Content that fits the viewport's layout can still reach past its presented bottom
            // while the viewport grows toward it (a composer shrinking after a send): follow it
            // from there, not from when the layout overflows, which landed the gap in one frame.
            let at = self.scroll_overflow(i).min(end) + lag;
            (at > 0.0).then_some(at)
        }
    }

    /// The offset that draws viewport `i`'s content bottom on its presented bottom `bottom`.
    pub(in crate::runtime) fn bottom_offset(&self, i: usize, bottom: f32) -> Option<f64> {
        let c = self.scroll_content(i)?;
        Some(self.scroll_overflow(i) + self.stuck_lag(i, c, bottom)?)
    }

    /// Scroll viewport `i`'s present direct children.
    pub(in crate::runtime) fn scroll_children(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        (i + 1..self.tree.nodes.len())
            .take_while(move |&j| self.tree.nodes[j].parent >= Some(i))
            .filter(move |&j| self.tree.nodes[j].parent == Some(i) && !self.is_exiting(self.tree.nodes[j].id) && self.scene.contains(self.tree.nodes[j].id))
    }

    /// Scroll viewport `i`'s growth-stack child, or the child that supplies its content end.
    pub(in crate::runtime) fn scroll_content(&self, i: usize) -> Option<usize> {
        let view = self.tree.nodes[i].target;
        self.scroll_stack(i).or_else(|| {
            self.scroll_children(i).max_by(|&a, &b| {
                let end = |c: usize| {
                    let n = &self.tree.nodes[c];
                    f64::from(n.target[1] - view[1]) + self.stack_extent(c).unwrap_or(f64::from(n.target[3] - n.target[1]))
                };
                end(a).total_cmp(&end(b))
            })
        })
    }

    /// Scroll viewport `i`'s growth-stack child, if any.
    pub(in crate::runtime) fn scroll_stack(&self, i: usize) -> Option<usize> {
        self.scroll_children(i).find(|&c| self.tree.nodes[c].growth.is_some())
    }

    /// How much further content `c` of viewport `i` has to travel to its layout than the
    /// viewport's presented bottom `bottom` has.
    fn stuck_lag(&self, i: usize, c: usize, bottom: f32) -> Option<f64> {
        let top = self.scene.presented(self.tree.nodes[c].id)?.rect.edges()[1];
        Some(f64::from((top - self.tree.nodes[c].target[1]) - (bottom - self.tree.nodes[i].target[3])))
    }

    /// Steps stick-to-bottom viewport `i`'s `Slack` after its offset. Returns whether any is left.
    pub(in crate::runtime) fn step_slack(&mut self, i: usize, dt: f32) -> bool {
        let id = self.tree.nodes[i].id;
        if self.scroll.scrolls.get(&id).is_some_and(|st| !st.pinned() && !st.slack.moving()) {
            if let Some(st) = self.scroll.scrolls.get_mut(&id) { st.slack.edges = None; }
            return false;
        }
        let Some(c) = self.scroll_content(i) else { return false };
        let stack = self.tree.nodes[c].growth.as_deref().copied().and_then(|spec| Some((spec, self.motion.stacks.get_mut(&self.tree.nodes[c].id)?)));
        let view = self.scene.presented(id).map(|p| p.rect.edges());
        let (Some((spec, stack)), Some(view), false) = (stack, view, self.reduced_motion) else {
            if let Some(st) = self.scroll.scrolls.get_mut(&id) { st.slack = Slack::landed(st.slack.window); }
            return false;
        };
        let grown = std::mem::take(&mut stack.grown);
        let growing = stack.growing;
        let pad = (stack.pad_bottom - self.tree.nodes[c].inset[3]).max(0.0);
        let short = self.tree.nodes[i].target[3] - (self.tree.nodes[c].target[3] - pad);
        let top_c = self.scene.presented(self.tree.nodes[c].id).map_or(view[1], |p| p.rect.edges()[1]);
        let limit = top_c + (self.tree.nodes[c].target[3] - self.tree.nodes[c].target[1] - pad);
        let now = self.t;
        let target = self.tree.nodes[i].target;
        let Some(st) = self.scroll.scrolls.get_mut(&id) else { return false };
        let pinned = st.pinned();
        let s = &mut st.slack;
        let h0 = s.track.h;
        let [top, bottom] = [view[1], view[3]];
        if pinned {
            // Growth fills the slack first. Room the bottom edge gains while the rows sit on it
            // joins it, unless the whole viewport moved down with it.
            let mut h = s.track.h - grown;
            if let Some([top0, bottom0]) = s.edges && st.held && !s.follow {
                let (b0, b1) = if short > 0.5 { (bottom0.min(limit), bottom.min(limit)) } else { (bottom0, bottom) };
                h += (b1 - b0) - (top - top0).max(0.0);
            }
            s.track.h = h.max(0.0);
        }
        s.edges = Some([top, bottom]);
        if s.follow && (top - target[1]).abs() < 0.01 && (bottom - target[3]).abs() < 0.01 { s.follow = false; }
        s.settle(growing, now, dt, &spec);
        s.stepped = (s.track.h - h0).abs() > 1e-5;
        s.moving()
    }

    /// Queues `on_at_bottom` and `on_far_from_bottom` for viewports whose state changed this
    /// frame.
    pub(in crate::runtime) fn report_at_bottom(&mut self) {
        for i in 0..self.tree.nodes.len() {
            let n = &self.tree.nodes[i];
            let Some(h) = n.handlers.get().filter(|h| h.on_at_bottom.is_some() || h.on_far_from_bottom.is_some()) else { continue };
            if self.subtree_exiting(i) { continue; }
            let Some(st) = self.scroll.scrolls.get_mut(&n.id) else { continue };
            if h.on_at_bottom.is_some() {
                let at = st.offset >= st.max - AT_BOTTOM;
                if st.at_bottom != Some(at) {
                    st.at_bottom = Some(at);
                    self.frame.queue.push(Action::AtBottom(n.path.clone(), at));
                }
            }
            if let Some((min, under, _)) = &h.on_far_from_bottom {
                // Under a frame, its height counts: the nearest ancestor with that key.
                let tall = under.as_deref().and_then(|k| {
                    self.tree.nodes.iter().filter(|m| m.key.as_deref() == Some(k) && n.path.starts_with(&m.path) && n.path[m.path.len()..].starts_with('/'))
                        .max_by_key(|m| m.path.len()).map(|m| m.local.h)
                }).unwrap_or(n.local.h);
                // Rows collapsing measure from the end they head for, glide or not.
                let end = st.laid().map_or(st.max, |end| end.min(st.max));
                let far = end - st.offset > f64::from(min.max(tall));
                if st.far_from_bottom != Some(far) {
                    st.far_from_bottom = Some(far);
                    self.frame.queue.push(Action::FarFromBottom(n.path.clone(), far));
                }
            }
        }
    }

    /// Whether the scroll viewport keyed `key` (or the one containing it) shows its bottom,
    /// as of the last frame.
    pub fn scroll_at_bottom(&self, key: &str) -> Option<bool> {
        let i = self.scroll_viewport(key)?;
        self.scroll.at_bottom(self.tree.nodes[i].id)
    }
}

#[cfg(test)]
#[path = "bottom_tests.rs"]
mod tests;
