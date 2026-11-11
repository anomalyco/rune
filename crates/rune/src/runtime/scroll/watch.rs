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
/// commit (`instant`), and the scene's animation clock (`now`).
#[doc(hidden)]
pub fn scroll_watch_state() -> (bool, bool, f64) {
    SCROLL_WATCH_STATE.get()
}

impl<S: 'static> Runtime<S> {
    /// Calls each viewport's `on_scroll_offset` whose offset or extent moved, that is new, or
    /// whose handler is; true when it called any.
    pub(super) fn tell_scroll_offsets(&mut self, instant: bool) -> bool {
        let now = self.t;
        let mut calls = Vec::new();
        for n in &self.nodes {
            let Some(h) = n.handlers.get() else { continue };
            let Some(f) = h.on_scroll_offset.as_ref() else { continue };
            let Some(st) = self.scrolls.get_mut(&n.id) else { continue };
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
pub(super) struct Watch {
    /// Where the viewport stood when the handler was last told.
    told: Option<crate::ScrollPos>,
    /// The handler that was told, held weakly: a rebuilt view's new handler hears where the
    /// viewport stands, though it didn't move. A weak reference keeps the old handler's
    /// allocation alive, so a later handler can't be built at its address and pass for it.
    handler: Option<std::rc::Weak<dyn Fn(crate::ScrollPos)>>,
}

pub(super) enum Event<'a> {
    /// The viewport stands at `at`, its handler is `handler` (None when it has none), and a
    /// hand or an app's instant jump moved it since the last tell (`by_hand`).
    Seen { at: crate::ScrollPos, handler: Option<&'a std::rc::Rc<dyn Fn(crate::ScrollPos)>>, by_hand: bool },
}

#[derive(Debug, PartialEq)]
pub(super) enum Effect {
    /// Call the handler with `at`; `scrolled` says a hand moved it since the last tell.
    Tell { at: crate::ScrollPos, scrolled: bool },
}

/// Calls the handler when where the viewport stands changed, or when it is a new one that has
/// not heard it: the effect says what to tell it.
pub(super) fn step(watch: &mut Watch, event: Event<'_>) -> Effects<Effect> {
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
mod tests {
    use crate::*;
    use std::rc::{Rc, Weak};
    use super::{step, Effect, Event, Watch};

    thread_local! { static TOLD: std::cell::Cell<ScrollPos> = std::cell::Cell::new(ScrollPos::default()); }

    struct S {
        follow: Live<[f32; 2]>,
        told: std::rc::Rc<std::cell::Cell<usize>>,
    }

    /// A marker outside the viewport that moves up as far as the content scrolls.
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let (follow, told) = (s.follow.clone(), s.told.clone());
        col().size(300.0, 300.0)
            .child(col().key("scroll").size(300.0, 200.0).scroll(ScrollSpec { stick_bottom: false })
                .on_scroll_offset(move |at: ScrollPos| { TOLD.with(|t| t.set(at)); told.set(told.get() + 1); follow.set([0.0, -at.offset[1]]); })
                .child(el().key("content").w_full().h(1000.0)))
            .child(el().key("marker").size(10.0, 10.0).live_offset(&s.follow))
    }

    fn y(rt: &Runtime<S>, key: &str) -> f32 {
        rt.elements().iter().find(|e| e.key.as_deref() == Some(key)).map(|e| e.rect[1]).unwrap()
    }

    fn rig() -> (Runtime<S>, std::rc::Rc<std::cell::Cell<usize>>) {
        let told = std::rc::Rc::new(std::cell::Cell::new(0));
        let mut rt = Runtime::new(S { follow: Live::new([0.0, 0.0]), told: told.clone() }, view, (300.0, 300.0), Env::default());
        rt.set_live_waker(std::sync::Arc::new(|| {}));
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        (rt, told)
    }

    #[test]
    fn a_live_value_set_from_the_offset_shows_in_the_frame_that_scrolled() {
        let (mut rt, _) = rig();
        let (content0, marker0) = (y(&rt, "content"), y(&rt, "marker"));
        for dy in [-7.0, -13.5, -40.0, 22.0, -3.0] {
            rt.wheel(100.0, 100.0, dy);
            rt.frame(1.0 / 60.0);
            assert_eq!(y(&rt, "marker") - marker0, y(&rt, "content") - content0, "after a wheel of {dy}");
        }
    }

    #[test]
    fn it_follows_every_frame_of_a_glide() {
        let (mut rt, _) = rig();
        let (content0, marker0) = (y(&rt, "content"), y(&rt, "marker"));
        rt.update(|_, tx| tx.scroll_to("scroll", 0.0, 500.0));
        let mut moved = 0;
        for _ in 0..60 {
            let before = y(&rt, "content");
            rt.frame(1.0 / 60.0);
            moved += usize::from(y(&rt, "content") != before);
            assert_eq!(y(&rt, "marker") - marker0, y(&rt, "content") - content0);
        }
        assert!(moved > 5, "the glide ran over several frames ({moved})");
        assert_eq!(y(&rt, "content") - content0, -500.0);
    }

    #[test]
    fn it_is_told_at_first_then_only_when_the_offset_moves() {
        let (mut rt, told) = rig();
        let first = told.get();
        assert!(first >= 1, "told where it starts");
        for _ in 0..10 { rt.frame(1.0 / 60.0); }
        assert_eq!(told.get(), first, "not again while nothing scrolls or rebuilds");
        rt.wheel(100.0, 100.0, -10.0);
        rt.frame(1.0 / 60.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(told.get(), first + 1);
        assert_eq!(TOLD.with(|t| t.get()), ScrollPos { offset: [0.0, 10.0], max: [0.0, 800.0], viewport: [300.0, 200.0], drawn: [300.0, 200.0], direction: Direction::Ltr });
    }

    #[test]
    fn a_rebuilt_handler_hears_where_the_viewport_stands() {
        let (mut rt, told) = rig();
        rt.wheel(100.0, 100.0, -30.0);
        rt.frame(1.0 / 60.0);
        let before = told.get();
        rt.update(|_, _| {});
        rt.frame(1.0 / 60.0);
        assert_eq!(told.get(), before + 1, "the commit told the new handler, though nothing moved");
        assert_eq!(TOLD.with(|t| t.get()).offset, [0.0, 30.0]);
        rt.frame(1.0 / 60.0);
        assert_eq!(told.get(), before + 1, "and the frame after didn't again");
    }

    #[test]
    fn a_live_rect_set_from_the_offset_lands_in_the_frame_that_mounts_it() {
        struct On { on: bool, bar: Live<[f32; 4]> }
        fn view(s: &On, _: &mut Cx<On>) -> El {
            let mut root = col().size(200.0, 100.0);
            if s.on {
                let bar = s.bar.clone();
                root = root
                    .child(col().size(200.0, 100.0).scroll(ScrollSpec { stick_bottom: false })
                        .on_scroll_offset(move |at: ScrollPos| { bar.set([10.0, 5.0, 6.0, at.max[1] / 10.0]); })
                        .child(el().w_full().h(400.0)))
                    .child(el().key("bar").absolute(0.0, 0.0).live_rect(&s.bar).fill(Color::WHITE));
            }
            root
        }
        let mut rt = Runtime::new(On { on: false, bar: Live::new([0.0; 4]) }, view, (200.0, 100.0), Env::default());
        rt.set_live_waker(std::sync::Arc::new(|| {}));
        rt.frame(0.0);
        rt.update(|s, tx| { s.on = true; tx.instant(); });
        rt.frame(1.0 / 120.0);
        let bar = info(&rt, "bar");
        assert_eq!(bar.rect, [10.0, 5.0, 6.0, 30.0]);
        assert!(!bar.moving);
    }

    fn info<T>(rt: &Runtime<T>, key: &str) -> ElementInfo {
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap()
    }

    #[test]
    fn what_a_glide_carries_knows_how_far() {
        let (mut rt, _) = rig();
        let content0 = y(&rt, "content");
        rt.update(|_, tx| tx.scroll_to("scroll", 0.0, 500.0));
        for _ in 0..60 {
            rt.frame(1.0 / 60.0);
            let c = info(&rt, "content");
            assert_eq!(c.rect[1] + c.scrolled[1], content0);
            assert_eq!(info(&rt, "marker").scrolled, [0.0; 2], "outside the viewport");
        }
        assert_eq!(info(&rt, "content").scrolled, [0.0, 500.0]);
    }

    #[test]
    fn what_a_pinned_end_carries_knows_how_far() {
        // Content growing under a stuck bottom carries the rows up without a glide.
        fn view(h: &f32, _: &mut Cx<f32>) -> El {
            col().key("scroll").size(300.0, 200.0).scroll(ScrollSpec { stick_bottom: true })
                .child(col().key("stack").w_full().growth(GrowthSpec::default())
                    .child(el().key("lead").w_full().h(400.0))
                    .child(el().key("grow").w_full().h(*h)))
        }
        let mut rt = Runtime::new(37.0, view, (300.0, 200.0), Env::default());
        rt.frame(0.0);
        rt.update(|h, _| *h = 230.0);
        let mut seen = Vec::new();
        for _ in 0..60 {
            rt.frame(1.0 / 120.0);
            let lead = info(&rt, "lead");
            assert!((lead.rect[1] + lead.scrolled[1]).abs() < 0.01, "{lead:?}");
            assert!(!lead.scrolling);
            if seen.last() != Some(&lead.scrolled[1]) { seen.push(lead.scrolled[1]); }
        }
        assert!(seen.len() > 5, "the end followed over several frames: {seen:?}");
    }

    /// origin/dev's watcher, transcribed from `runtime/scroll/watch.rs`: it kept where it last
    /// told (`told`) and a weak reference to the handler that heard it (`told_by`), and called
    /// when where the viewport stood changed, or the handler was new.
    #[derive(Default)]
    struct Dev {
        told: Option<ScrollPos>,
        told_by: Option<Weak<dyn Fn(ScrollPos)>>,
    }

    impl Dev {
        fn seen(&mut self, y: f32, handler: Option<&Rc<dyn Fn(ScrollPos)>>, by_hand: bool) -> Option<(ScrollPos, bool)> {
            let f = handler?;
            let at = pos(y);
            let prev = self.told.replace(at);
            let changed = prev != Some(at);
            let new_handler = !self.told_by.as_ref().is_some_and(|by| std::ptr::addr_eq(by.as_ptr(), Rc::as_ptr(f)));
            if new_handler {
                self.told_by = Some(Rc::downgrade(f));
            }
            if !changed && !new_handler {
                return None;
            }
            let scrolled = by_hand && prev.is_some_and(|old| {
                (old.offset[0] - at.offset[0]).abs() > 0.01 || (old.offset[1] - at.offset[1]).abs() > 0.01
            });
            Some((at, scrolled))
        }
    }

    fn pos(y: f32) -> ScrollPos {
        ScrollPos { offset: [0.0, y], max: [0.0, 800.0], viewport: [300.0, 200.0], drawn: [300.0, 200.0], direction: Direction::Ltr }
    }

    /// Two handlers built separately, whose allocations stay alive for the walk.
    fn handlers() -> Vec<Rc<dyn Fn(ScrollPos)>> {
        (0..2).map(|_| Rc::new(|_: ScrollPos| {}) as Rc<dyn Fn(ScrollPos)>).collect()
    }

    fn same_handler(a: &Option<Weak<dyn Fn(ScrollPos)>>, b: &Option<Weak<dyn Fn(ScrollPos)>>) -> bool {
        match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => Weak::ptr_eq(a, b),
            _ => false,
        }
    }

    #[derive(Clone, Copy, Debug)]
    struct Act {
        y: f32,
        /// 0: the viewport has no handler; otherwise one of the walk's handlers.
        handler: usize,
        by_hand: bool,
    }

    /// Drives every sequence through the watcher and through dev's, one event at a time, and
    /// compares what each does after every event.
    fn walk(acts: &[Act]) {
        let (mut watch, mut dev) = (Watch::default(), Dev::default());
        let handlers = handlers();
        for (i, act) in acts.iter().enumerate() {
            let handler = (act.handler != 0).then(|| &handlers[act.handler - 1]);
            let effects: Vec<Effect> = step(&mut watch, Event::Seen { at: pos(act.y), handler, by_hand: act.by_hand }).into_iter().collect();
            let expected = dev.seen(act.y, handler, act.by_hand);
            match expected {
                Some((at, scrolled)) => assert_eq!(effects, [Effect::Tell { at, scrolled }], "{acts:?}, event {i}"),
                None => assert!(effects.is_empty(), "{acts:?}, event {i}: told though nothing changed"),
            }
            assert_eq!(watch.told, dev.told, "{acts:?}, event {i}");
            assert!(same_handler(&watch.handler, &dev.told_by), "{acts:?}, event {i}: the handler told");
        }
    }

    /// Two handlers built separately are two handlers, even at the same offset: the weak
    /// reference tells them apart, where an attempt that wrapped would not.
    #[test]
    fn two_handlers_at_the_same_offset_each_hear_the_first_time() {
        let handlers = handlers();
        let (first, second) = (&handlers[0], &handlers[1]);
        let mut watch = Watch::default();
        let at = pos(0.0);
        let told = |watch: &mut Watch, handler: &Rc<dyn Fn(ScrollPos)>| {
            step(watch, Event::Seen { at, handler: Some(handler), by_hand: false }).into_iter().collect::<Vec<_>>()
        };
        assert_eq!(told(&mut watch, first), [Effect::Tell { at, scrolled: false }]);
        assert_eq!(told(&mut watch, first), [], "the same handler hears it once");
        assert_eq!(told(&mut watch, second), [Effect::Tell { at, scrolled: false }], "a second handler is new, even with the same value");
    }

    #[test]
    fn the_watcher_matches_devs_told_offset_and_handler_over_every_order() {
        let acts: Vec<Act> = [0.0, 12.0].into_iter().flat_map(|y| [0, 1, 2].into_iter().flat_map(move |handler| {
            [false, true].into_iter().map(move |by_hand| Act { y, handler, by_hand })
        })).collect();
        for len in 0..=5 {
            for_each_walk(&acts, len, &mut Vec::new());
        }
    }

    fn for_each_walk(acts: &[Act], len: usize, prefix: &mut Vec<Act>) {
        if prefix.len() == len {
            walk(prefix);
            return;
        }
        for &act in acts {
            prefix.push(act);
            for_each_walk(acts, len, prefix);
            prefix.pop();
        }
    }
}
