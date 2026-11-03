//! `El::on_scroll_offset`: telling a scroll viewport's watcher where this frame scrolled it,
//! after the frame's scrolling steps and before it draws, so live values set from it show in
//! the same frame. `El::on_drawn_layout` hears of a move a frame later, and not at all while a
//! glide runs, which is too late for something that rides the scroll.
use super::*;

impl<S: 'static> Runtime<S> {
    /// Calls each viewport's `on_scroll_offset` whose offset or extent moved, that is new, or
    /// whose handler is; true when it called any.
    pub(super) fn tell_scroll_offsets(&mut self) -> bool {
        let mut calls = Vec::new();
        for n in &self.nodes {
            let Some(f) = n.handlers.get().and_then(|h| h.on_scroll_offset.as_ref()) else { continue };
            let Some(st) = self.scrolls.get_mut(&n.id) else { continue };
            let at = crate::ScrollPos { offset: [st.x.offset as f32, st.offset as f32 + st.slack.height()], max: [st.x.max as f32, st.max as f32],
                viewport: [n.local.w, n.local.h] };
            let by = Rc::as_ptr(f) as *const () as usize;
            if st.told.replace(at) != Some(at) || std::mem::replace(&mut st.told_by, by) != by { calls.push((f.clone(), at)); }
        }
        for (f, at) in &calls { f(*at); }
        !calls.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use crate::*;

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
        assert_eq!(TOLD.with(|t| t.get()), ScrollPos { offset: [0.0, 10.0], max: [0.0, 800.0], viewport: [300.0, 200.0] });
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
}
