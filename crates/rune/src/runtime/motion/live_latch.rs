//! Live values bound to elements: subscribing to their sources, the wake lane, and latching
//! what they publish into the scene each frame.
use super::*;

impl<S: 'static> Runtime<S> {
    /// Install before the first frame. Live updates use this separate coalesced wake lane.
    pub fn set_live_waker(&mut self, waker: Waker) {
        for sub in &self.live_subscriptions { sub.active.store(false, std::sync::atomic::Ordering::Release); }
        self.live_subscriptions.clear();
        self.live_wake = crate::live::Wake::new(waker);
        self.live_wake.visible(self.surface_visible);
        self.subscribe_live();
    }

    pub fn set_live_visible(&mut self, visible: bool) {
        self.live_wake.visible(visible);
        self.surface_visible = visible;
        if !visible { self.loop_wake = None; }
    }
    pub(crate) fn live_visible(&self) -> bool { self.surface_visible }
    pub fn live_pending(&self) -> bool {
        self.surface_visible && (self.live_dirty || self.live_wake.pending.load(std::sync::atomic::Ordering::Acquire))
    }

    pub(super) fn subscribe_live(&mut self) {
        use std::sync::atomic::Ordering;
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
            let text = node.text.as_ref().and_then(|t| t.live.as_ref()).map(|t| &t.source);
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
                    self.live_subscriptions.push(source.subscribe(&self.live_wake));
                }
            }
        }
        self.live_subscriptions.retain(|s| s.active.load(Ordering::Acquire));
        self.live_dirty = sampled;
    }

    /// Clear before this frame reads anything a source publishes (a picture's size for layout,
    /// a bound value): a concurrent publication either lands in this frame or keeps a wake for
    /// the next. Never clear after reading (lost wake race). Hidden hosts keep the wake for show.
    pub(super) fn take_live_wake(&self) -> bool {
        self.surface_visible && self.live_wake.pending.swap(false, std::sync::atomic::Ordering::AcqRel)
    }

    pub(super) fn latch_live(&mut self, pending: bool) -> bool {
        if !self.surface_visible || self.live_subscriptions.is_empty() { return false; }
        if !pending && !self.live_dirty { return false; }
        self.live_dirty = false;
        for node in &mut self.nodes {
            if let Some(spec) = &mut node.text
                && let Some(live) = &spec.live
            {
                let seen = &mut node.cold.live_text_seen;
                if let Some(text) = live.read(seen) && spec.source_text() != &*text {
                    spec.replace_content(text.to_string());
                    spec.apply_case();
                    node.text_layout = Some(self.text.layout_live(&spec.content, spec.style, self.env.scale));
                }
            }
            let Some(cold) = node.cold.get_mut_if_some() else { continue };
            for bound in &mut cold.live {
                let initial = bound.seen.is_none();
                let (bits, active) = bound.read(&self.live_clock, self.clock);
                self.live_dirty |= active;
                let Some(bits) = bits else { continue };
                let spring = if initial { None } else { bound.spring };
                use crate::live::{LiveValue, Property};
                fn apply<T: rune_motion::Animatable>(scene: &mut Scene, id: NodeId, prop: Prop<T>, value: T, spring: Option<Spring>) {
                    if let Some(spring) = spring { scene.animate(id, prop, value, spring); }
                    else { scene.set(id, prop, value); }
                }
                match bound.property {
                    Property::Opacity => apply(&mut self.scene, node.id, prop::OPACITY, f32::decode(bits).clamp(0.0, 1.0), spring),
                    Property::Blur => apply(&mut self.scene, node.id, prop::BLUR, f32::decode(bits).max(0.0), spring),
                    Property::Scale => apply(&mut self.scene, node.id, prop::SCALE, f32::decode(bits).max(0.0), spring),
                    Property::Offset => {
                        let [x, y] = <[f32; 2]>::decode(bits);
                        apply(&mut self.scene, node.id, prop::OFFSET, rune_motion::Vec2::new(x, y), spring);
                    }
                    Property::Fill => {
                        // The renderer uses the exact authored colour after its track settles.
                        node.vis.fill = Color::decode(bits);
                        apply(&mut self.scene, node.id, P_FILL, node.vis.fill, spring);
                    }
                    Property::Rect => {
                        let [x, y, w, h] = <[f32; 4]>::decode(bits);
                        self.scene.set_frame(node.id, MRect::new(x, y, w.max(0.0), h.max(0.0)),
                            spring.map_or(LayoutAnim::INSTANT, LayoutAnim::new));
                    }
                }
            }
        }
        self.live_dirty
    }
}
