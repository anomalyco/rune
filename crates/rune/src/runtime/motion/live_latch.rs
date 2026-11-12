//! Live values bound to elements: subscribing to their sources, the wake lane, and latching
//! what they publish into the scene each frame.
use super::*;
use std::sync::atomic::Ordering;

/// Whether a window has live work, as one state: hidden or visible, whether a value waits for the
/// next frame, and whether a frame is reading the sources.
///
/// The shared [`crate::live::Wake`] is the other thread's half: publication and visibility
/// changes share an atomic so only one writer wakes the host. [`LiveLane`] owns that wake
/// beside this state, and reads it only when a step asks
/// (at the frame boundary), so the state never has to be written by another thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LiveWake {
    /// Hidden: no frame latches or draws. `dirty` is work kept for the frame after showing.
    Hidden { dirty: bool },
    /// Visible, and nothing waits.
    Idle,
    /// Visible, with a value waiting for the next frame.
    Pending,
    /// A frame has taken the shared wake and is reading what it published. `dirty` is work that
    /// landed after the take, or a value still moving, kept for the next frame.
    Latching { dirty: bool },
}

/// Something that changes whether a window has live work to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LiveEvent {
    /// The window showed or hid (`Runtime::set_live_visible`).
    Show,
    Hide,
    /// A commit re-derived the bound sources; `sampled` says the scene binds a value to read.
    Subscribe { sampled: bool },
    /// A prune re-derived them; `keep` is whether a value was waiting before it.
    Prune { keep: bool },
    /// A live-only event published without a commit.
    Set,
    /// The frame is reading the sources now, after `LiveLane::latch_begin` found work.
    LatchBegin,
    /// The latch read the sources; `active` says one is still moving.
    LatchEnd { active: bool },
}

impl LiveWake {
    /// Work kept for a later frame.
    fn dirty(self) -> bool {
        match self {
            LiveWake::Hidden { dirty } | LiveWake::Latching { dirty } => dirty,
            LiveWake::Pending => true,
            LiveWake::Idle => false,
        }
    }

    /// One transition. Only a frame with work to do reaches this; the empty and hidden fast
    /// paths return first. Returns the next state and the event's answer: whether the latch
    /// should read, or whether a read value is still moving.
    pub(crate) fn step(self, event: LiveEvent) -> (LiveWake, bool) {
        use LiveWake::{Hidden, Idle, Latching, Pending};
        match (self, event) {
            // A hidden window keeps a value for its first visible frame.
            (Hidden { dirty }, LiveEvent::Show) => (if dirty { Pending } else { Idle }, dirty),
            (_, LiveEvent::Show) => (self, self.dirty()),
            (s, LiveEvent::Hide) => (Hidden { dirty: s.dirty() }, false),
            // A commit resets the waiting bit to whether the scene bound a value.
            (Hidden { .. }, LiveEvent::Subscribe { sampled }) => (Hidden { dirty: sampled }, sampled),
            (_, LiveEvent::Subscribe { sampled }) => (if sampled { Pending } else { Idle }, sampled),
            // A prune keeps only work the state already had.
            (s, LiveEvent::Prune { keep }) => {
                let dirty = s.dirty() && keep;
                let next = match s {
                    Hidden { .. } => Hidden { dirty },
                    _ if dirty => Pending,
                    _ => Idle,
                };
                (next, dirty)
            }
            // A live-only event published a value without a commit: one is owed next frame.
            (Hidden { .. }, LiveEvent::Set) => (Hidden { dirty: true }, true),
            (_, LiveEvent::Set) => (Pending, true),
            // The frame reads what it took; a hidden window or one with no sources never gets here.
            (_, LiveEvent::LatchBegin) => (Latching { dirty: false }, true),
            // A value still moving after the read keeps the next frame due.
            (Hidden { dirty }, LiveEvent::LatchEnd { active }) => (Hidden { dirty: dirty || active }, active),
            (_, LiveEvent::LatchEnd { active }) => (if active { Pending } else { Idle }, active),
        }
    }
}

/// A window's live wake: the state above and the shared [`crate::live::Wake`] publishers notify.
pub(crate) struct LiveLane {
    wake: Arc<crate::live::Wake>,
    state: LiveWake,
}

impl LiveLane {
    pub(crate) fn new(waker: Waker) -> Self {
        Self { wake: crate::live::Wake::new(waker), state: LiveWake::Idle }
    }

    /// The shared wake, for a source to subscribe to and a video to notify.
    pub(crate) fn waker(&self) -> &Arc<crate::live::Wake> { &self.wake }

    /// Install a new wake lane, keeping the visibility and the work the last one held.
    pub(crate) fn replace_waker(&mut self, waker: Waker) {
        self.wake = crate::live::Wake::new(waker);
        self.wake.visible(self.visible());
    }

    pub(crate) fn visible(&self) -> bool { !matches!(self.state, LiveWake::Hidden { .. }) }

    pub(crate) fn dirty(&self) -> bool { self.state.dirty() }

    /// Whether a value waits for this window. Hidden windows keep their wake for show, and a
    /// step's own dirty bit is checked before the shared atomic, as published values always were.
    pub(crate) fn pending(&self) -> bool {
        match self.state {
            LiveWake::Hidden { .. } => false,
            LiveWake::Pending => true,
            LiveWake::Idle => self.wake.visible_pending(),
            LiveWake::Latching { dirty } => dirty || self.wake.visible_pending(),
        }
    }

    pub(crate) fn set_visible(&mut self, visible: bool) {
        self.wake.visible(visible);
        self.apply(if visible { LiveEvent::Show } else { LiveEvent::Hide });
    }

    /// A source published on another thread.
    pub(crate) fn publish(&self) { self.wake.notify(); }

    pub(crate) fn subscribed(&mut self, sampled: bool) { self.apply(LiveEvent::Subscribe { sampled }); }

    pub(crate) fn pruned(&mut self, keep: bool) { self.apply(LiveEvent::Prune { keep }); }

    /// A live-only event published a value without a commit.
    pub(crate) fn set(&mut self) { self.apply(LiveEvent::Set); }

    /// Take the shared wake for this frame; a hidden window keeps it for show. The take never
    /// changes what the UI thread holds, so a visible frame with nothing pending does no work
    /// beyond dev's visibility test and swap.
    pub(crate) fn take(&mut self) -> bool {
        if !self.visible() { return false; }
        self.wake.take_visible()
    }

    /// Whether the frame should read its sources now. The hidden, source-less and clean cases
    /// return before any transition, as an unpending frame always did.
    pub(crate) fn latch_begin(&mut self, can: bool, has_subs: bool) -> bool {
        if !self.visible() || !has_subs || (!can && !self.state.dirty()) { return false; }
        self.apply1(LiveEvent::LatchBegin)
    }

    /// Whether a read value is still moving.
    pub(crate) fn latch_end(&mut self, active: bool) -> bool {
        self.apply1(LiveEvent::LatchEnd { active })
    }

    fn apply(&mut self, event: LiveEvent) { let (state, _) = self.state.step(event); self.state = state; }

    fn apply1(&mut self, event: LiveEvent) -> bool {
        let (state, answer) = self.state.step(event);
        self.state = state;
        answer
    }
}

impl<S: 'static> Runtime<S> {
    /// Install before the first frame. Live updates use this separate coalesced wake lane.
    pub fn set_live_waker(&mut self, waker: Waker) {
        for sub in &self.live_subscriptions { sub.active.store(false, Ordering::Release); }
        self.live_subscriptions.clear();
        self.live.replace_waker(waker);
        self.subscribe_live();
    }

    pub fn set_live_visible(&mut self, visible: bool) {
        let changed = visible != self.live.visible();
        self.live.set_visible(visible);
        if !visible { self.loop_wake = None; }
        // The window's own setup shows and hides it before the first frame; the app hears the
        // changes after that (`App::on_visibility`).
        if changed && self.commits > 0 && let Some(f) = self.on_visibility {
            self.update(|s, tx| f(s, visible, tx));
        }
    }
    /// What asks for this runtime's next frame as a live value does, from any thread.
    pub(crate) fn live_notifier(&self) -> impl FnOnce() + Send + 'static {
        let wake = self.live.waker().clone();
        move || wake.notify()
    }
    pub(crate) fn live_visible(&self) -> bool { self.live.visible() }
    pub fn live_pending(&self) -> bool {
        self.live.pending()
    }

    pub(super) fn subscribe_live(&mut self) {
        for node in &mut self.nodes {
            if let Some(cold) = node.cold.get_mut_if_some() {
                for bound in &mut cold.live { bound.attach(&self.live_clock, self.clock); }
            }
        }
        for sub in &self.live_subscriptions { sub.active.store(false, Ordering::Release); }
        // Pixels only wake: each frame's build uploads their current image, so unlike bound
        // values they leave nothing to sample on a later visible frame.
        let mut sampled = false;
        for node in &self.nodes {
            let text = node.text.as_ref().and_then(|t| t.cold.live.as_ref()).map(|t| &t.source);
            let pixels = node.vis.cold.get().and_then(|v| v.pixels.as_ref()).map(|p| &p.source);
            let has_live = node.cold.get().is_some_and(|c| !c.live.is_empty());
            if !has_live && text.is_none() && pixels.is_none() {
                continue;
            }
            // An element removed outright leaves the scene now but `nodes` only at the next
            // prune. Exiting elements and their children are still drawn, so their text and
            // pixels keep waking until the exit ends (commit already dropped their bindings).
            if !self.scene.contains(node.id) { continue; }
            sampled |= text.is_some() || has_live;
            for source in node.live.iter().map(|b| &b.source).chain(text).chain(pixels) {
                if let Some(sub) = self.live_subscriptions.iter().find(|s| s.source_id == source.id()) {
                    sub.active.store(true, Ordering::Release);
                } else {
                    self.live_subscriptions.push(source.subscribe(self.live.waker()));
                }
            }
        }
        self.live_subscriptions.retain(|s| s.active.load(Ordering::Acquire));
        self.live.subscribed(sampled);
    }

    /// Clear before this frame reads anything a source publishes (a picture's size for layout,
    /// a bound value): a concurrent publication either lands in this frame or keeps a wake for
    /// the next. Never clear after reading (lost wake race). Hidden hosts keep the wake for show.
    pub(super) fn take_live_wake(&mut self) -> bool { self.live.take() }

    pub(super) fn latch_live(&mut self, pending: bool) -> bool {
        if !self.live.latch_begin(pending, !self.live_subscriptions.is_empty()) { return false; }
        let mut active = false;
        for node in &mut self.nodes {
            if let Some(spec) = &mut node.text
                && let Some(live) = &spec.cold.live
            {
                let seen = &mut node.cold.live_text_seen;
                if let Some(text) = live.read(seen) && spec.source_text() != &*text {
                    spec.replace_content(text.to_string());
                    spec.apply_case();
                    node.text_layout = Some(TextLayout::Single(self.text.layout_live(&spec.content, spec.style, self.env.scale)));
                }
            }
            let Some(cold) = node.cold.get_mut_if_some() else { continue };
            for bound in &mut cold.live {
                let (op, active_now) = bound.read(&self.live_clock, self.clock);
                active |= active_now;
                let Some(op) = op else { continue };
                let (enter, leave) = (bound.spring, bound.rest_spring);
                use crate::live::{LatchOp, LiveValue, Property};
                fn apply<T: rune_motion::Animatable + Copy>(
                    scene: &mut Scene,
                    id: NodeId,
                    prop: Prop<T>,
                    enter: Option<Spring>,
                    leave: Option<Spring>,
                    decode: impl Fn(u128) -> T,
                    op: LatchOp,
                    choose_leave: impl Fn(Option<T>, T) -> bool,
                ) {
                    match op {
                        LatchOp::Snap(bits) => scene.set(id, prop, decode(bits)),
                        LatchOp::Set(bits) => {
                            let v = decode(bits);
                            let spring = if choose_leave(scene.get(id, prop), v) { leave.or(enter) } else { enter };
                            match spring {
                                Some(s) => scene.animate(id, prop, v, s),
                                None => scene.set(id, prop, v),
                            }
                        }
                        LatchOp::Pulse { active, rest, hold } => {
                            let rest_v = decode(rest);
                            let Some(enter_s) = enter else {
                                scene.set(id, prop, rest_v);
                                return;
                            };
                            let active_v = decode(active);
                            let leave_s = leave.unwrap_or(enter_s);
                            scene.animate(id, prop, active_v, enter_s);
                            scene.animate(id, prop, rest_v, rune_motion::Anim { delay: (hold as f32).max(1e-4), ..leave_s.into() });
                        }
                    }
                }
                match bound.property {
                    Property::Opacity => apply(
                        &mut self.scene, node.id, prop::OPACITY, enter, leave,
                        |b| f32::decode(b).clamp(0.0, 1.0), op,
                        |cur, to| cur.is_some_and(|c| to < c),
                    ),
                    Property::Blur => apply(
                        &mut self.scene, node.id, prop::BLUR, enter, leave,
                        |b| f32::decode(b).max(0.0), op,
                        |cur, to| cur.is_some_and(|c| to < c),
                    ),
                    Property::Scale => apply(
                        &mut self.scene, node.id, prop::SCALE, enter, leave,
                        |b| f32::decode(b).max(0.0), op,
                        |cur, to| cur.is_some_and(|c| to < c),
                    ),
                    Property::Offset => apply(
                        &mut self.scene, node.id, prop::OFFSET, enter, leave,
                        |b| { let [x, y] = <[f32; 2]>::decode(b); rune_motion::Vec2::new(x, y) }, op,
                        |_, _| false,
                    ),
                    Property::Trim => apply(
                        &mut self.scene, node.id, P_TRIM, enter, leave,
                        |b| { let [s, e] = <[f32; 2]>::decode(b); rune_motion::Vec2::new(s, e) }, op,
                        |_, _| false,
                    ),
                    Property::Fill | Property::TextColor | Property::Border | Property::Shadow | Property::GradientTop | Property::GradientBottom => {
                        let bits = match op {
                            LatchOp::Snap(b) | LatchOp::Set(b) | LatchOp::Pulse { rest: b, .. } => b,
                        };
                        // The renderer uses the exact authored colour after its track settles.
                        let c = Color::decode(bits);
                        let p = match bound.property {
                            Property::Fill => { node.vis.fill = c; P_FILL }
                            Property::TextColor => {
                                if let Some(spec) = &mut node.text { if spec.color != c { spec.color = c; } }
                                else if node.vis.icon.is_some_and(|(_, ic)| ic != c) && let Some((_, ic)) = &mut node.vis.cold_mut().icon { *ic = c; }
                                P_TEXT
                            }
                            Property::Border => { node.vis.set_border_color(c); P_BORDER }
                            Property::Shadow => {
                                if node.vis.shadows.first().is_some_and(|s| s.color != c) { node.vis.cold_mut().shadows[0].color = c; }
                                P_SHADOWS[0]
                            }
                            _ => {
                                let k = usize::from(bound.property == Property::GradientBottom);
                                if node.vis.gradient.as_deref().is_some_and(|g| g[k] != c)
                                    && let Some(g) = node.vis.cold_mut().gradient.as_deref_mut() { g[k] = c; }
                                P_GRADIENT[k]
                            }
                        };
                        apply(&mut self.scene, node.id, p, enter, leave, Color::decode, op, |_, _| false);
                    }
                    Property::Rect => {
                        let frame_of = |b| {
                            let [x, y, w, h] = <[f32; 4]>::decode(b);
                            MRect::new(x, y, w.max(0.0), h.max(0.0))
                        };
                        match (op, enter) {
                            (LatchOp::Snap(b) | LatchOp::Pulse { rest: b, .. }, _) | (LatchOp::Set(b), None) => {
                                self.scene.land_frame(node.id, frame_of(b));
                            }
                            (LatchOp::Set(b), Some(spring)) => {
                                self.scene.set_frame(node.id, frame_of(b), LayoutAnim::new(spring));
                            }
                        }
                    }
                }
            }
        }
        self.live.latch_end(active)
    }
}

#[cfg(test)]
#[path = "live_latch_tests.rs"]
mod tests;
