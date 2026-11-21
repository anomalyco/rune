//! `Tx::scroll_to`, `Tx::scroll_to_with`, `Tx::scroll_to_instant`, `Tx::scroll_to_x` and
//! `Tx::scroll_to_y`: moving a viewport to an offset its app picked, gliding there or at once;
//! and `Tx::scroll_key_to`, landing an element at a height in its viewport.
use super::*;
use crate::runtime::scroll_mode::{self as mode, Event as ModeEvent};

/// How a keyed scroll request anchors its target within the viewport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScrollAnchor {
    None,
    /// Place the target's top `y` points below the viewport's top (`Tx::scroll_key_to`).
    Top,
    /// Place the target's vertical midpoint (for a `TextHighlight`, its inline font-metric box
    /// `[baseline - ascent, baseline + descent]`) `y` points below the viewport's top, snapped to
    /// whole device pixels (`Tx::scroll_key_center_at`).
    CenterSnapped,
}

/// A request to move a viewport to an offset on one axis or both.
#[derive(Clone, Debug)]
pub(crate) struct ScrollRequest {
    pub key: String,
    pub x: Option<f32>,
    pub y: Option<f32>,
    /// The spring it glides on, or None to jump.
    pub glide: Option<Spring>,
    pub anchor: ScrollAnchor,
}

impl RuntimeCore {
    /// Each request moves the viewport keyed (or pathed) by its key, or the nearest one around
    /// it that scrolls an axis it asks for, clamped to its content. Axes the viewport doesn't
    /// scroll are left alone.
    pub(in crate::runtime) fn scroll_to_offsets(&mut self, app: &mut dyn AppDriver, requests: Vec<ScrollRequest>) {
        // This frame's commit may have changed the content: measure the extent first.
        self.step_scroll(0.0);
        let animate = !self.reduced_motion && !self.settled;
        // Jumps: (viewport, horizontal, offset), reapplied once virtual rows land.
        let mut jumps = Vec::new();
        // Anchored jumps, placed again from the new layout once virtual rows land.
        let mut anchors = Vec::new();
        for r in requests {
            if r.anchor != ScrollAnchor::None {
                let Some((i, to)) = self.anchor_offset(&r.key, r.y.unwrap_or(0.0), r.anchor) else { continue };
                jumps.push((self.tree.nodes[i].id, false, to));
                anchors.push((jumps.len() - 1, r.key, r.y.unwrap_or(0.0), r.anchor));
                if let Some(st) = self.scroll.scrolls.get_mut(&self.tree.nodes[i].id) { st.halt(); }
                continue;
            }
            let Some(i) = self.scroll_viewport_on(&r.key, r.x.is_some(), r.y.is_some()) else { continue };
            let (id, axes) = (self.tree.nodes[i].id, self.tree.nodes[i].scroll_axes);
            let (view_w, view_h) = (f64::from(self.tree.nodes[i].local.w), f64::from(self.tree.nodes[i].local.h));
            let Some(st) = self.scroll.scrolls.get_mut(&id) else { continue };
            let glide = r.glide.filter(|_| animate);
            if glide.is_some() { st.spring = glide; }
            if let Some(x) = r.x.filter(|_| axes.x()) {
                let to = f64::from(x).max(0.0);
                // A glide that starts during a wheel's ease keeps the ease's speed.
                let v = match st.x.motion { XMotion::Glide { v, .. } => v, XMotion::Wheel(c) => c.velocity() as f32, _ => 0.0 };
                if let Some(spring) = glide && (to.min(st.x.max) - st.x.offset).abs() > 0.5 {
                    // A long trip cuts to a couple of viewports out first, as a glide to the bottom does.
                    let reach = scroll_bottom::GLIDE_REACH * view_w;
                    let end = to.min(st.x.max);
                    st.x.offset = st.x.offset.clamp(end - reach, end + reach);
                    st.x.motion = XMotion::Glide { v: retarget_v(spring, st.x.offset, v, end), aim: to };
                } else {
                    st.x.motion = XMotion::Rest;
                    jumps.push((id, true, to));
                }
            }
            if let Some(y) = r.y.filter(|_| axes.y()) {
                let to = f64::from(y).max(0.0);
                if let Some(spring) = glide && (to.min(st.max) - st.offset).abs() > 0.5 {
                    let reach = scroll_bottom::GLIDE_REACH * view_h;
                    let end = to.min(st.max);
                    st.offset = st.offset.clamp(end - reach, end + reach);
                    let v = retarget_v(spring, st.offset, st.glide().or_else(|| st.mode.wheel().map(|c| c.velocity() as f32)).unwrap_or(0.0), end);
                    st.mode = st.mode.to_glide(v, GlideTo::Offset(to));
                } else {
                    st.halt();
                    jumps.push((id, false, to));
                }
            }
        }
        self.scroll.apply_jumps(&self.tree.nodes, self.clock, &jumps);
        // Landing mounts the virtual rows there, which measure and can move the end it clamped to.
        for _ in 0..3 {
            if !self.virtual_window_changed() { break; }
            // A jump is at once, and so are the rows it mounts: none fades in where it lands.
            self.commit(app, &Tx { commit: Commit::Instant, ..Tx::default() });
            self.step_scroll(0.0);
            for (j, key, at, mode) in &anchors {
                if let Some((_, to)) = self.anchor_offset(key, *at, *mode) { jumps[*j].2 = to; }
            }
            self.scroll.apply_jumps(&self.tree.nodes, self.clock, &jumps);
        }
    }

    /// The vertical viewport around the element or `TextHighlight` `key` and the offset that
    /// anchors it `at` points below the viewport's top (by top edge or device-pixel-snapped
    /// vertical midpoint), before clamping.
    /// A virtual row not mounted yet stands where its list's estimates put it: the jump there
    /// mounts it, and is placed again from its laid-out geometry once it has (`scroll_to_offsets`).
    fn anchor_offset(&self, key: &str, at: f32, mode: ScrollAnchor) -> Option<(usize, f64)> {
        let found = |scope: Option<&str>, key: &str| {
            self.live_named(scope, key)
                .map(|i| {
                    let y0 = self.tree.nodes[i].target[1];
                    let y = match mode {
                        ScrollAnchor::CenterSnapped => y0 + self.tree.nodes[i].local.h * 0.5,
                        _ => y0,
                    };
                    (i, y)
                })
                .or_else(|| {
                    let (i, [_, y0, _, y1]) = match mode {
                        ScrollAnchor::CenterSnapped => self.highlight_inline_target_in(&self.tree.nodes, scope, key)?,
                        _ => self.highlight_target_in(&self.tree.nodes, scope, key)?,
                    };
                    let y = match mode {
                        ScrollAnchor::CenterSnapped => (y0 + y1) * 0.5,
                        _ => y0,
                    };
                    (!self.subtree_exiting(i)).then_some((i, y))
                })
        };
        // `"row/mark"`: element or highlight `mark` anywhere inside the element keyed `row`.
        let target = found(None, key).or_else(|| key.split_once('/').and_then(|(row, mark)| found(Some(row), mark)));
        if let Some((i, y)) = target {
            let v = std::iter::successors(Some(i), |&p| self.tree.nodes[p].parent).find(|&p| self.tree.nodes[p].scroll.is_some() && self.tree.nodes[p].scroll_axes.y())?;
            let raw = f64::from(y - self.tree.nodes[v].target[1] - at).max(0.0);
            let to = match mode {
                ScrollAnchor::CenterSnapped => {
                    let s = f64::from(self.env.scale).max(1.0);
                    (raw * s).round() / s
                }
                _ => raw,
            };
            return Some((v, to));
        }
        self.tree.nodes.iter().find_map(|n| {
            let list = self.commit.virtuals.get(&n.path)?;
            // A row that isn't mounted stands where the estimates put it; the jump mounts it and
            // `scroll_to_offsets` places `mark` once it has.
            let index = list.index_of(key).or_else(|| list.index_of(key.split_once('/')?.0))?;
            let v = *self.tree.node_index.get(self.tree.path_ids.get(&list.scroll_path)?)?;
            Some((v, f64::from(list.top + list.pad_top + list.prefix(index) - at).max(0.0)))
        })
    }

    /// The viewport keyed (or pathed) `key`, or the nearest one around it, that scrolls
    /// horizontally (`x`) or vertically (`y`).
    fn scroll_viewport_on(&self, key: &str, x: bool, y: bool) -> Option<usize> {
        let i = self.tree.nodes.iter().enumerate()
            .position(|(i, n)| n.named(key) && !self.subtree_exiting(i))
            .or_else(|| self.highlight_target_in(&self.tree.nodes, None, key).map(|(i, _)| i))?;
        std::iter::successors(Some(i), |&p| self.tree.nodes[p].parent).find(|&p| {
            let n = &self.tree.nodes[p];
            n.scroll.is_some() && ((x && n.scroll_axes.x()) || (y && n.scroll_axes.y()))
        })
    }

    /// A growth stack shrinking under a reader who scrolled up, until its layout ends above
    /// their offset: viewport `i` glides to that end from rest, as `Tx::scroll_to` would, rather
    /// than riding the presented end down at the collapse's own speed and stopping with it.
    /// `overflow` is this frame's presented overflow. A keep or reveal owns its viewport, and an
    /// end already above the offset (a settled or instant change) clamps at once. The glide
    /// follows the end as it moves again, and stops once the rows regrow past the offset; a
    /// hand scrolling mid-collapse keeps the viewport until the rows settle.
    ///
    /// Returns the layout's end while rows are still moving, which a viewport landed there
    /// waits for before it sticks to the end, so the presented end's last fraction of a point
    /// doesn't lift it back up.
    pub(in crate::runtime) fn ease_to_shrunk_end(&mut self, i: usize, overflow: f64) -> Option<f64> {
        let c = self.scroll_stack(i)?;
        let stack = self.motion.stacks.get(&self.tree.nodes[c].id)?;
        let moving = stack.moving || !stack.closing.is_empty();
        let path = &self.tree.nodes[i].path;
        let owned = self.scroll.requests.owns(path);
        let bottom = self.scroll_children(i).map(|k| self.tree.nodes[k].target[3]).reduce(f32::max).unwrap_or(self.tree.nodes[c].target[3]);
        let end = f64::from(bottom - self.tree.nodes[i].target[3]).max(0.0);
        let held = self.reduced_motion || owned;
        let st = self.scroll.scrolls.get_mut(&self.tree.nodes[i].id)?;
        let _ = mode::step(&mut st.mode, ModeEvent::Growth { moving });
        match st.mode {
            ScrollMode::Glide { to: GlideTo::ShrunkEnd(_), .. } if held || st.shrink_cut() || end >= st.offset => st.mode = st.mode.to_rest(),
            ScrollMode::Glide { v, to: GlideTo::ShrunkEnd(_), .. } => st.mode = st.mode.to_glide(v, GlideTo::ShrunkEnd(end)),
            ScrollMode::Rest(_) | ScrollMode::Momentum { .. } if moving && !(held || st.shrink_cut() || st.offset > overflow || st.offset - end <= 0.5) => {
                st.mode = st.mode.to_glide(0.0, GlideTo::ShrunkEnd(end));
                st.spring = None;
            }
            _ => {}
        }
        moving.then_some(end)
    }
}

/// The speed a running glide at `offset` keeps when it's sent to `to` on `spring`: all of it,
/// unless that would carry it past `to` and back (a critically damped spring heading for its
/// end faster than ω times the distance left overshoots), so a glide sent somewhere nearer
/// stops there on one path instead of bouncing. Retargets that keep their speed, as key
/// repeat sending a glide one row on at a time does, run as one motion rather than steps.
pub(in crate::runtime) fn retarget_v(spring: Spring, offset: f64, v: f32, to: f64) -> f32 {
    let d = (offset - to) as f32;
    let most = spring.omega() * d.abs();
    if v * d < 0.0 && v.abs() > most { -d.signum() * most } else { v }
}

/// How near its end a glide lands, in points: its speed, in points a second, under 40 times that.
const LAND: f32 = 0.1;

/// Steps a glide on one axis toward `to` on `spring`. Returns whether it is still moving.
pub(in crate::runtime) fn glide_axis(spring: Spring, offset: &mut f64, glide: &mut Option<f32>, to: f64, dt: f32) -> bool {
    let Some(v) = *glide else { return false };
    // Sample the displacement, not the absolute offset: f32 loses sub-point precision far down.
    let (d, v) = spring.sample((*offset - to) as f32, v, 0.0, dt);
    // Lands under a tenth of a point out, so the last step is no bigger than the ones before it.
    if d.abs() < LAND && v.abs() < 40.0 * LAND {
        *offset = to;
        *glide = None;
        return false;
    }
    *offset = to + f64::from(d);
    *glide = Some(v);
    true
}

impl ScrollState {
    /// The spring the running glide steps on.
    pub(in crate::runtime) fn glide_spring(&self) -> Spring {
        self.spring.unwrap_or(crate::tok::motion::GLIDE)
    }

    /// A hand, its momentum or a glide is moving the content this frame: what
    /// [`ElementInfo::scrolling`](crate::ElementInfo) reports for everything inside.
    pub(crate) fn moving(&self) -> bool {
        self.scrolling() || self.glide().is_some() || self.mode.wheel().is_some() || self.x.carried()
    }

    /// It follows its content's bottom.
    pub(in crate::runtime) fn pinned(&self) -> bool { self.mode.pinned() }

    /// The running glide's speed (points a second), while one runs.
    pub(in crate::runtime) fn glide(&self) -> Option<f32> { self.mode.glide() }

    /// Where the running glide heads, when that isn't the bottom.
    #[cfg(test)]
    pub(in crate::runtime) fn glide_to(&self) -> Option<f64> {
        match self.mode {
            ScrollMode::Glide { to, .. } => to.offset(),
            _ => None,
        }
    }

    /// Its momentum (points a second), 0 unless coasting.
    pub(in crate::runtime) fn coast(&self) -> f32 { self.mode.coast() }

    /// A hand or its momentum moved it in the last step.
    pub(in crate::runtime) fn scrolling(&self) -> bool { self.mode.hand().scrolling }

    /// A hand or an app moved it since the last step.
    pub(in crate::runtime) fn direct(&self) -> bool { self.mode.hand().direct }

    /// A hand scrolled while the growth stack moved.
    pub(in crate::runtime) fn shrink_cut(&self) -> bool { self.mode.hand().shrink_cut }

    /// The growth stack's layout end while its rows still move.
    pub(in crate::runtime) fn laid(&self) -> Option<f64> { self.mode.hand().laid }

    /// A hand or an app put the offset where it is.
    pub(in crate::runtime) fn moved(&mut self) { let _ = mode::step(&mut self.mode, mode::Event::Moved); }

    /// The viewport was concealed: nothing moved it, whatever a hand did before.
    pub(in crate::runtime) fn still(&mut self) { let _ = mode::step(&mut self.mode, mode::Event::Still); }

    /// Follow the bottom from here (a pinned viewport keeps how its end last stepped).
    pub(in crate::runtime) fn pin(&mut self) { let _ = mode::step(&mut self.mode, mode::Event::Pin); }

    /// Stop following the bottom, where it stands.
    pub(in crate::runtime) fn unpin(&mut self) { let _ = mode::step(&mut self.mode, mode::Event::Unpin); }

    /// Momentum and any glide stop; a pinned viewport stays pinned.
    pub(in crate::runtime) fn halt(&mut self) { let _ = mode::step(&mut self.mode, mode::Event::Halt); }

    /// A hand or an app put the offset where it is: momentum and any glide stop, and a
    /// stick-to-bottom viewport (`stick`) follows its end only if it sits there.
    /// An app's keep moved the offset (rows above measured, an anchor held what's on screen): a
    /// wheel's ease goes on, moved with it on its next step (`WheelCurve::follow`), as Chromium's
    /// `ApplyAdjustment` moves a running smooth scroll; anything else lands as `Self::land` does.
    pub(in crate::runtime) fn keep_land(&mut self, stick: bool) { if self.mode.wheel().is_none() { self.land(stick) } }

    pub(in crate::runtime) fn land(&mut self, stick: bool) {
        if stick && self.offset >= self.max - 0.5 { self.pin() } else { let _ = mode::step(&mut self.mode, mode::Event::Rest); }
    }

    /// An app's instant scroll landed less than `scroll_nudge::NUDGE_HOLD` ago: content moved
    /// under a still pointer, so hover waits as it does for a glide it stepped itself.
    pub(in crate::runtime) fn hold_at(&self, now: f64) -> bool { self.nudge.hold_at(now) }
}

impl requests::ScrollTables {
    pub(super) fn apply_jumps(&mut self, nodes: &[Node], now: f64, jumps: &[(NodeId, bool, f64)]) {
        for &(id, horizontal, to) in jumps {
            let stick = nodes.iter().find(|n| n.id == id).and_then(|n| n.scroll).is_some_and(|s| s.stick_bottom);
            let Some(st) = self.scrolls.get_mut(&id) else { continue };
            st.moved();
            let was = if horizontal { st.x.offset } else { st.offset };
            if horizontal {
                st.x.offset = to.min(st.x.max);
            } else {
                st.offset = to.min(st.max);
                // The jump halted the old motion when queued. Don't cancel a later
                // request in the same transaction to glide from the new offset.
                if st.glide().is_none() { st.land(stick); }
            }
            if (if horizontal { st.x.offset } else { st.offset } - was).abs() > 1e-3 {
                // The arm's due is taken back from the state each frame (`step_nudges`), so a
                // later arm in the same frame wins and nothing here has to keep it.
                let _ = st.nudge.arm(now);
            }
        }
    }

    /// Steps the holds an app's instant scrolls armed, and returns the earliest due one still
    /// pending: the frame that carries it resolves hover at the pointer. A held nudge asks for
    /// no frame itself, so the frame's end wakes for that one, however often the app re-armed it.
    pub(in crate::runtime) fn step_nudges(&mut self, now: f64) -> Option<f64> {
        let mut due: Option<f64> = None;
        for st in self.scrolls.values_mut() {
            st.nudge.tick(now);
            if let Some(at) = st.nudge.due() { due = Some(due.map_or(at, |d: f64| d.min(at))); }
        }
        due
    }
}

impl RuntimeCore {
    #[inline]
    pub(in crate::runtime) fn step_nudges(&mut self) -> Option<f64> {
        self.scroll.step_nudges(self.clock)
    }
}
