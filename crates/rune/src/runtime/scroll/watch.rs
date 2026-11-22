//! `El::on_scroll_offset`: telling a scroll viewport's watcher where this frame scrolled it,
//! after the frame's scrolling steps and before it draws, so live values set from it show in
//! the same frame. `El::on_drawn_layout` hears of a move a frame later, and not at all while a
//! glide runs, which is too late for something that rides the scroll.
use super::*;
use crate::machine::Effects;
use crate::runtime::scroll_mode::{self, Event as ModeEvent};

thread_local! {
    static SCROLL_WATCH_STATE: std::cell::Cell<(bool, bool, f64)> = const { std::cell::Cell::new((false, false, 0.0)) };
}

/// Inside an [`El::on_scroll_offset`](crate::El::on_scroll_offset) callback: whether a hand
/// moved this viewport's scroll offset (`scrolled`: the wheel, keys, a drag, their momentum or
/// an app's instant jump, but not following its bottom as content grows, a glide such as
/// `Tx::scroll_to_bottom`, or an anchor holding what's on screen), whether this frame ran an instant or settled
/// commit (`instant`), and the runtime's unscaled clock (`now`, the same clock [`Tx::clock`](crate::Tx::clock)
/// publishes).
#[doc(hidden)]
pub fn scroll_watch_state() -> (bool, bool, f64) {
    SCROLL_WATCH_STATE.get()
}

impl RuntimeCore {
    /// Calls each viewport's `on_scroll_offset` whose offset or extent moved, that is new, or
    /// whose handler is; true when it called any.
    pub(in crate::runtime) fn tell_scroll_offsets(&mut self, instant: bool) -> bool {
        let now = self.clock;
        let mut calls = Vec::new();
        for n in &self.tree.nodes {
            let Some(h) = n.handlers.get() else { continue };
            let Some(f) = h.on_scroll_offset.as_ref() else { continue };
            let Some(st) = self.scroll.scrolls.get_mut(&n.id) else { continue };
            let viewport = [n.local.w, n.local.h];
            // The presented rect is snapped to the pixel grid; only a spring moves it further.
            let drawn = self.scene.presented(n.id).map_or(viewport, |p| {
                let near = |drawn: f32, target: f32| if (drawn - target).abs() > 0.05 { drawn } else { target };
                [near(p.rect.w, viewport[0]), near(p.rect.h, viewport[1])]
            });
            let direction = match n.flow { crate::el::Flow::Ltr => crate::Direction::Ltr, crate::el::Flow::Rtl => crate::Direction::Rtl };
            let at = crate::ScrollPos { offset: [st.x.offset as f32, st.offset as f32 + st.slack.height()], max: [st.x.max as f32, st.max as f32],
                viewport, drawn, direction };
            let by_hand = !scroll_mode::step(&mut st.mode, ModeEvent::Take).is_empty();
            for effect in step(&mut st.watch, Event::Seen { at, handler: Some(f), by_hand }) {
                let Effect::Tell { at, scrolled } = effect;
                calls.push((f.clone(), at, scrolled));
            }
        }
        for (f, at, scrolled) in &calls {
            let prev_ctx = SCROLL_WATCH_STATE.replace((*scrolled, instant, now));
            f(*at);
            SCROLL_WATCH_STATE.set(prev_ctx);
        }
        !calls.is_empty()
    }
}

/// What a viewport's `El::on_scroll_offset` has been told, and which of its handlers heard it.
#[derive(Clone, Debug, Default)]
pub(in crate::runtime) struct Watch {
    /// Where the viewport stood when the handler was last told.
    told: Option<crate::ScrollPos>,
    /// The handler that was told, held weakly: a rebuilt view's new handler hears where the
    /// viewport stands, though it didn't move. A weak reference keeps the old handler's
    /// allocation alive, so a later handler can't be built at its address and pass for it.
    handler: Option<std::rc::Weak<dyn Fn(crate::ScrollPos)>>,
}

pub(in crate::runtime) enum Event<'a> {
    /// The viewport stands at `at`, its handler is `handler` (None when it has none), and a
    /// hand or an app's instant jump moved it since the last tell (`by_hand`).
    Seen { at: crate::ScrollPos, handler: Option<&'a std::rc::Rc<dyn Fn(crate::ScrollPos)>>, by_hand: bool },
}

#[derive(Debug, PartialEq)]
pub(in crate::runtime) enum Effect {
    /// Call the handler with `at`; `scrolled` says a hand moved it since the last tell.
    Tell { at: crate::ScrollPos, scrolled: bool },
}

/// Calls the handler when where the viewport stands changed, or when it is a new one that has
/// not heard it: the effect says what to tell it.
pub(in crate::runtime) fn step(watch: &mut Watch, event: Event<'_>) -> Effects<Effect> {
    let Event::Seen { at, handler, by_hand } = event;
    let Some(f) = handler else { return Effects::none() };
    let prev = watch.told.replace(at);
    let changed = prev != Some(at);
    let new_handler = !watch.handler.as_ref().is_some_and(|by| std::ptr::addr_eq(by.as_ptr(), std::rc::Rc::as_ptr(f)));
    if new_handler {
        watch.handler = Some(std::rc::Rc::downgrade(f));
    }
    if !changed && !new_handler {
        return Effects::none();
    }
    let scrolled = by_hand && prev.is_some_and(|old| {
        (old.offset[0] - at.offset[0]).abs() > 0.01 || (old.offset[1] - at.offset[1]).abs() > 0.01
    });
    Effects::one(Effect::Tell { at, scrolled })
}

#[cfg(test)]
#[path = "watch_tests.rs"]
mod tests;
