use super::*;
use crate::{
    Color, Cx, Direction, El, Env, InlineAlign, InlineBox, Link, PointerButton, Runtime,
    ScrollSpec, TextStyle, TouchPhase, Transition, col, el, text,
};

fn approx_rect(got: [f32; 4], want: [f32; 4], label: &str) {
    for (axis, (g, w)) in got.into_iter().zip(want).enumerate() {
        assert!(
            (g - w).abs() < 0.05,
            "{label}: axis {axis} got {g}, want {w} (got={got:?}, want={want:?})"
        );
    }
}

fn settle<S: 'static>(rt: &mut Runtime<S>) {
    for _ in 0..60 {
        if !rt.frame(1.0 / 60.0) {
            return;
        }
    }
    panic!("never settled");
}

fn box_xywh(r: [f32; 4]) -> [f32; 4] {
    [r[0], r[1], (r[2] - r[0]).max(0.0), (r[3] - r[1]).max(0.0)]
}

#[test]
fn link_hover_enter_dedup_switch_leave_and_matches_tx_link_rect() {
    struct HoverEvent {
        url: Option<String>,
        rect: Option<[f32; 4]>,
        pressing: bool,
        tx_link_rect: Option<[f32; 4]>,
    }

    #[derive(Default)]
    struct State {
        hovers: Vec<HoverEvent>,
        clicked_rect: Option<[f32; 4]>,
    }

    fn view(_: &State, _: &mut Cx<State>) -> El {
        col().size(340.0, 140.0).padding(16.0).gap(12.0)
            .child(
                text("Read alpha then beta here.")
                    .key("p1")
                    .font(TextStyle::new(14.0, 400.0).lh(20.0))
                    .w(280.0)
                    .link(Link::new(5..10, "https://example.com/a"))
                    .link(Link::new(16..20, "https://example.com/b"))
                    .on_link(|s: &mut State, _, tx| s.clicked_rect = tx.link_rect())
                    .on_link_hover(|s: &mut State, url, rect, tx| {
                        s.hovers.push(HoverEvent {
                            url: url.map(str::to_owned),
                            rect,
                            pressing: tx.pressing(),
                            tx_link_rect: tx.link_rect(),
                        });
                    }),
            )
            .child(
                text("Second gamma paragraph.")
                    .key("p2")
                    .font(TextStyle::new(14.0, 400.0).lh(20.0))
                    .w(280.0)
                    .link(Link::new(7..12, "https://example.com/g"))
                    .on_link_hover(|s: &mut State, url, rect, tx| {
                        s.hovers.push(HoverEvent {
                            url: url.map(str::to_owned),
                            rect,
                            pressing: tx.pressing(),
                            tx_link_rect: tx.link_rect(),
                        });
                    }),
            )
    }

    let mut rt = Runtime::new(State::default(), view, (340.0, 140.0), Env::new(true, 1.0));
    rt.frame(0.0);
    settle(&mut rt);

    let p1 = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("p1")).unwrap();
    let p2 = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("p2")).unwrap();
    let ra = box_xywh(rt.core().link_boxes_for(p1, 0, rt.core().drawn_box(p1))[0].1);
    let rb = box_xywh(rt.core().link_boxes_for(p1, 1, rt.core().drawn_box(p1))[0].1);
    let rg = box_xywh(rt.core().link_boxes_for(p2, 0, rt.core().drawn_box(p2))[0].1);
    let (ax, ay) = (ra[0] + ra[2] * 0.5, ra[1] + ra[3] * 0.5);
    let (bx, by) = (rb[0] + rb[2] * 0.5, rb[1] + rb[3] * 0.5);
    let (gx, gy) = (rg[0] + rg[2] * 0.5, rg[1] + rg[3] * 0.5);

    // 1. Enter link A -> fires once with exact rect and pressing=false, Tx::link_rect()=None.
    rt.pointer_move(ax, ay);
    rt.frame(0.0);
    assert_eq!(rt.state.hovers.len(), 1);
    let ev = &rt.state.hovers[0];
    assert_eq!(ev.url.as_deref(), Some("https://example.com/a"));
    approx_rect(ev.rect.expect("rect a"), ra, "enter a");
    assert!(!ev.pressing);
    assert!(ev.tx_link_rect.is_none(), "Tx::link_rect() stays None inside on_link_hover");

    // Verify hovered rect agrees exactly with Tx::link_rect() on click.
    let hovered_rect = ev.rect;
    rt.pointer_down(ax, ay);
    rt.pointer_up(ax, ay);
    rt.frame(0.0);
    assert_eq!(rt.state.clicked_rect, hovered_rect, "hovered rect and Tx::link_rect must match");

    // 2. Moving within the same link does not re-fire.
    rt.state.hovers.clear();
    assert!(!rt.observe_pointer(ax + 1.0, ay), "staying inside same link schedules no frame");
    rt.frame(0.0);
    assert!(rt.state.hovers.is_empty(), "no duplicate callback inside same link");

    // 3. Direct switch to link B in the same text fires one transition directly to B.
    rt.pointer_move(bx, by);
    rt.frame(0.0);
    assert_eq!(rt.state.hovers.len(), 1);
    assert_eq!(rt.state.hovers[0].url.as_deref(), Some("https://example.com/b"));
    approx_rect(rt.state.hovers[0].rect.expect("rect b"), rb, "switch to b");

    // 4. Direct switch to link G in a different text leaves p1 BEFORE entering p2.
    rt.state.hovers.clear();
    rt.pointer_move(gx, gy);
    rt.frame(0.0);
    assert_eq!(rt.state.hovers.len(), 2);
    assert_eq!(rt.state.hovers[0].url, None);
    assert_eq!(rt.state.hovers[0].rect, None);
    assert_eq!(rt.state.hovers[1].url.as_deref(), Some("https://example.com/g"));
    approx_rect(rt.state.hovers[1].rect.expect("rect g"), rg, "enter g");

    // 5. Moving off onto plain text fires leave (None, None).
    rt.state.hovers.clear();
    rt.pointer_move(2.0, 2.0);
    rt.frame(0.0);
    assert_eq!(rt.state.hovers.len(), 1);
    assert_eq!((&rt.state.hovers[0].url, rt.state.hovers[0].rect), (&None, None));

    // 6. Enter again, then pointer_leave fires leave (None, None).
    rt.state.hovers.clear();
    rt.pointer_move(ax, ay);
    rt.frame(0.0);
    assert_eq!(rt.state.hovers.len(), 1);
    rt.pointer_leave();
    rt.frame(0.0);
    assert_eq!(rt.state.hovers.len(), 2);
    assert_eq!((&rt.state.hovers[1].url, rt.state.hovers[1].rect), (&None, None));

    // 7. Enter again, then cancel_pointer fires leave (None, None).
    rt.state.hovers.clear();
    rt.pointer_move(ax, ay);
    rt.frame(0.0);
    rt.cancel_pointer();
    rt.frame(0.0);
    assert_eq!(rt.state.hovers.len(), 2);
    assert_eq!((&rt.state.hovers[1].url, rt.state.hovers[1].rect), (&None, None));
}

#[test]
fn link_hover_wrapped_multiline_reports_hit_line_box_and_does_not_refire_across_lines() {
    #[derive(Default)]
    struct State {
        hovers: Vec<(Option<String>, Option<[f32; 4]>)>,
    }

    const BODY: &str = "Prefix a very long wrapped link label that definitely spans two lines suffix";
    const START: usize = "Prefix ".len();
    const END: usize = BODY.len() - " suffix".len();

    fn view(_: &State, _: &mut Cx<State>) -> El {
        col().size(180.0, 200.0).padding(10.0).child(
            text(BODY)
                .key("wrap")
                .font(TextStyle::new(14.0, 400.0).lh(20.0))
                .wrap()
                .w(150.0)
                .link(Link::new(START..END, "wrapped"))
                .on_link_hover(|s: &mut State, url, rect, _| {
                    s.hovers.push((url.map(str::to_owned), rect));
                }),
        )
    }

    let mut rt = Runtime::new(State::default(), view, (180.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    settle(&mut rt);

    let i = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("wrap")).unwrap();
    let boxes = rt.core().link_boxes_for(i, 0, rt.core().drawn_box(i));
    assert!(boxes.len() >= 2, "must wrap across >= 2 lines, got {}", boxes.len());
    let line0 = box_xywh(boxes[0].1);
    let line1 = box_xywh(boxes[1].1);

    // Entering directly on line 2 reports line 2's box (not line 1 and not the union).
    rt.pointer_move(line1[0] + line1[2] * 0.5, line1[1] + line1[3] * 0.5);
    rt.frame(0.0);
    assert_eq!(rt.state.hovers.len(), 1);
    assert_eq!(rt.state.hovers[0].0.as_deref(), Some("wrapped"));
    approx_rect(rt.state.hovers[0].1.expect("line 2 box"), line1, "enter line 2");

    // Moving onto line 1 of the SAME link does not re-fire.
    rt.pointer_move(line0[0] + line0[2] * 0.5, line0[1] + line0[3] * 0.5);
    rt.frame(0.0);
    assert_eq!(rt.state.hovers.len(), 1, "moving between lines of same link must not re-fire");

    // Leaving then entering on line 1 reports line 1's box.
    rt.pointer_leave();
    rt.frame(0.0);
    assert_eq!(rt.state.hovers.last(), Some(&(None, None)));
    rt.pointer_move(line0[0] + line0[2] * 0.5, line0[1] + line0[3] * 0.5);
    rt.frame(0.0);
    approx_rect(rt.state.hovers.last().unwrap().1.expect("line 1 box"), line0, "enter line 1");
}

#[test]
fn link_hover_ends_on_scroll_under_pointer_unmount_exiting_and_rebuild() {
    struct State {
        show: bool,
        exiting: bool,
        keep_link: bool,
        url: &'static str,
        keep_handler: bool,
        hovers: Vec<(Option<String>, Option<[f32; 4]>)>,
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        let mut root = col().size(280.0, 140.0).key("sc").scroll(ScrollSpec::default());
        if s.show {
            let mut t = text("Target link here")
                .key("doc")
                .font(TextStyle::new(14.0, 400.0).lh(20.0))
                .w(240.0);
            if s.keep_link {
                t = t.link(Link::new(7..11, s.url));
            }
            if s.keep_handler {
                t = t.on_link_hover(|s: &mut State, url, rect, _| {
                    s.hovers.push((url.map(str::to_owned), rect));
                });
            }
            if s.exiting {
                t = t.transition(Transition::opacity());
            }
            root = root.child(t);
        }
        root.child(el().size(240.0, 400.0))
    }

    let mut rt = Runtime::new(
        State { show: true, exiting: false, keep_link: true, url: "u1", keep_handler: true, hovers: Vec::new() },
        view,
        (280.0, 140.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    settle(&mut rt);

    let (lx, ly) = rt.find("u1").unwrap();
    rt.pointer_move(lx, ly);
    rt.frame(0.0);
    assert_eq!(rt.state.hovers.len(), 1);
    assert_eq!(rt.state.hovers[0].0.as_deref(), Some("u1"));

    // 1a. Wheel scroll carries the link out from under the stationary pointer -> leave fires.
    rt.wheel(lx, ly, -120.0);
    settle(&mut rt);
    assert_eq!(rt.state.hovers.last(), Some(&(None, None)), "wheel scroll away must fire leave");

    // 1b. Programmatic scroll back -> resolves once its nudge hold wake fires, re-entering u1.
    rt.update(|_, tx| tx.scroll_to_instant("sc", 0.0, 0.0));
    rt.frame(1.0 / 60.0);
    if let Some(due) = rt.wake_deadline() {
        rt.frame((due - rt.clock).max(0.0) + 1e-4);
    }
    settle(&mut rt);
    assert_eq!(rt.state.hovers.last().unwrap().0.as_deref(), Some("u1"), "scroll back must re-enter");

    // 1c. Programmatic scroll away -> fires leave once its nudge hold wake fires.
    rt.update(|_, tx| tx.scroll_to_instant("sc", 0.0, 120.0));
    rt.frame(1.0 / 60.0);
    if let Some(due) = rt.wake_deadline() {
        rt.frame((due - rt.clock).max(0.0) + 1e-4);
    }
    settle(&mut rt);
    assert_eq!(rt.state.hovers.last(), Some(&(None, None)), "instant scroll away must fire leave after nudge hold");

    // Scroll back to top for the remaining rebuild/unmount checks.
    rt.update(|_, tx| tx.scroll_to_instant("sc", 0.0, 0.0));
    rt.frame(1.0 / 60.0);
    if let Some(due) = rt.wake_deadline() {
        rt.frame((due - rt.clock).max(0.0) + 1e-4);
    }
    settle(&mut rt);
    assert_eq!(rt.state.hovers.last().unwrap().0.as_deref(), Some("u1"));

    // 2. Rebuild changing the link URL at the same range -> reports new URL.
    rt.state.hovers.clear();
    rt.update(|s, _| s.url = "u2");
    settle(&mut rt);
    assert_eq!(rt.state.hovers.len(), 1);
    assert_eq!(rt.state.hovers[0].0.as_deref(), Some("u2"));

    // 3. Rebuild removing the link -> fires leave.
    rt.state.hovers.clear();
    rt.update(|s, _| s.keep_link = false);
    settle(&mut rt);
    assert_eq!(rt.state.hovers, [(None, None)]);

    // Restore link and hover again, then remove the handler while hovered -> saved handler still hears leave.
    rt.update(|s, _| s.keep_link = true);
    settle(&mut rt);
    assert_eq!(rt.state.hovers.last().unwrap().0.as_deref(), Some("u2"));
    rt.state.hovers.clear();
    rt.update(|s, _| s.keep_handler = false);
    settle(&mut rt);
    assert_eq!(rt.state.hovers, [(None, None)], "dropping handler while hovered still delivers leave");

    // 4. Instant unmount while hovered -> fires leave via hover_pruned.
    rt.update(|s, _| s.keep_handler = true);
    settle(&mut rt);
    rt.state.hovers.clear();
    rt.update(|s, _| s.show = false);
    settle(&mut rt);
    assert_eq!(rt.state.hovers, [(None, None)], "instant unmount must fire leave");

    // 5. Exiting transition unmount while hovered -> fires leave immediately and not twice on prune.
    rt.update(|s, _| { s.show = true; s.exiting = true; });
    settle(&mut rt);
    rt.state.hovers.clear();
    rt.update(|s, _| s.show = false);
    settle(&mut rt);
    assert_eq!(rt.state.hovers, [(None, None)], "exiting unmount must fire leave exactly once");
}

#[test]
fn link_hover_respects_occlusion_disabled_inert_and_hover_passes_below() {
    struct State {
        cover: bool,
        passes_below: bool,
        disabled_before: bool,
        disabled_after: bool,
        disabled_parent: bool,
        inert: bool,
        hovers: Vec<(Option<String>, Option<[f32; 4]>)>,
    }

    fn view(s: &State, _: &mut Cx<State>) -> El {
        let mut live = text("Live link")
            .key("live")
            .font(TextStyle::new(14.0, 400.0).lh(20.0))
            .w(200.0);
        if s.disabled_before { live = live.disabled(true); }
        live = live
            .link(Link::new(5..9, "live-url"))
            .on_link_hover(|s: &mut State, url, rect, _| s.hovers.push((url.map(str::to_owned), rect)));
        if s.disabled_after { live = live.disabled(true); }
        if s.inert { live = live.inert(); }

        let mut holder = col().w(220.0).h(40.0).child(live);
        if s.disabled_parent { holder = holder.disabled(true); }

        let mut root = col().size(240.0, 120.0).padding(10.0).child(holder);
        if s.cover {
            let mut ov = el().key("cover").absolute(0.0, 0.0).size(240.0, 120.0).hit();
            if s.passes_below { ov = ov.hover_passes_below(); }
            root = root.child(ov);
        }
        root
    }

    let mut rt = Runtime::new(
        State {
            cover: false,
            passes_below: false,
            disabled_before: false,
            disabled_after: false,
            disabled_parent: false,
            inert: false,
            hovers: Vec::new(),
        },
        view,
        (240.0, 120.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    settle(&mut rt);

    let (lx, ly) = rt.find("live-url").unwrap();
    rt.pointer_move(lx, ly);
    rt.frame(0.0);
    assert_eq!(rt.state.hovers.len(), 1);
    assert_eq!(rt.state.hovers[0].0.as_deref(), Some("live-url"));

    // 1. Mounting an overlying .hit() cover ends link hover (CSS :hover occlusion).
    rt.state.hovers.clear();
    rt.update(|s, _| s.cover = true);
    settle(&mut rt);
    assert_eq!(rt.state.hovers, [(None, None)], "occluding hit cover must fire leave");

    // Moving under the cover never enters.
    rt.state.hovers.clear();
    rt.pointer_move(lx + 1.0, ly);
    rt.frame(0.0);
    assert!(rt.state.hovers.is_empty(), "occluded link must not hover");

    // 2. Marking the cover hover_passes_below lets link hover through.
    rt.update(|s, _| s.passes_below = true);
    settle(&mut rt);
    assert_eq!(rt.state.hovers.len(), 1);
    assert_eq!(rt.state.hovers[0].0.as_deref(), Some("live-url"));

    // Remove cover.
    rt.update(|s, _| { s.cover = false; s.passes_below = false; });
    settle(&mut rt);

    // 3. Disabling a hovered link (in either builder order or via ancestor) or marking it inert
    //    immediately fires leave and blocks re-entry.
    for mode in 0..4 {
        rt.state.hovers.clear();
        rt.update(move |s, _| match mode {
            0 => s.disabled_before = true,
            1 => s.disabled_after = true,
            2 => s.disabled_parent = true,
            _ => s.inert = true,
        });
        settle(&mut rt);
        assert_eq!(rt.state.hovers, [(None, None)], "mode {mode} must fire leave");

        rt.state.hovers.clear();
        rt.pointer_move(lx - 1.0, ly);
        rt.frame(0.0);
        rt.pointer_move(lx, ly);
        rt.frame(0.0);
        assert!(rt.state.hovers.is_empty(), "disabled/inert mode {mode} must not fire enter");

        rt.update(|s, _| {
            s.disabled_before = false;
            s.disabled_after = false;
            s.disabled_parent = false;
            s.inert = false;
        });
        settle(&mut rt);
        assert_eq!(rt.state.hovers.last().unwrap().0.as_deref(), Some("live-url"));
    }
}

#[test]
fn link_hover_ignores_touch_and_reports_tx_pressing_when_mouse_button_held() {
    #[derive(Default)]
    struct State {
        hovers: Vec<(Option<String>, bool)>,
        clicks: Vec<String>,
    }

    fn view(_: &State, _: &mut Cx<State>) -> El {
        col().size(280.0, 100.0).padding(16.0).child(
            text("Drag across link here")
                .key("doc")
                .selectable(true)
                .font(TextStyle::new(14.0, 400.0).lh(20.0))
                .w(240.0)
                .link(Link::new(12..16, "https://example.com/t"))
                .on_link(|s: &mut State, id, _| s.clicks.push(id.to_owned()))
                .on_link_hover(|s: &mut State, url, _, tx| {
                    s.hovers.push((url.map(str::to_owned), tx.pressing()));
                }),
        )
    }

    let mut rt = Runtime::new(State::default(), view, (280.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    settle(&mut rt);

    let (lx, ly) = rt.find("https://example.com/t").unwrap();

    // 1. Touch tap on a link activates on_link, but NEVER fires on_link_hover.
    rt.touch(1, TouchPhase::Started, lx, ly, 0.0);
    rt.frame(1.0 / 60.0);
    rt.touch(1, TouchPhase::Moved, lx + 1.0, ly, 0.02);
    rt.frame(1.0 / 60.0);
    rt.touch(1, TouchPhase::Ended, lx + 1.0, ly, 0.04);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.hovers.is_empty(), "touch must not fire on_link_hover: {:?}", rt.state.hovers);
    assert_eq!(rt.state.clicks, ["https://example.com/t"]);

    // 2. If a mouse is hovering a link and a touch lands, the mouse hover ends cleanly.
    rt.pointer_move(lx, ly);
    rt.frame(0.0);
    assert_eq!(rt.state.hovers, [(Some("https://example.com/t".into()), false)]);
    rt.state.hovers.clear();
    rt.touch(2, TouchPhase::Started, lx, ly, 1.0);
    rt.frame(1.0 / 60.0);
    rt.touch(2, TouchPhase::Cancelled, lx, ly, 1.05);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.hovers, [(None, false)], "touch landing ends open mouse link hover");

    // 3. Dragging across a link with a mouse button held reports tx.pressing() == true.
    rt.state.hovers.clear();
    rt.pointer_move(2.0, ly);
    rt.frame(0.0);
    rt.pointer_down(2.0, ly);
    rt.pointer_move(lx, ly);
    rt.frame(0.0);
    assert_eq!(rt.state.hovers, [(Some("https://example.com/t".into()), true)]);
    rt.pointer_move(2.0, ly);
    rt.pointer_up(2.0, ly);
    rt.frame(0.0);
    assert_eq!(rt.state.hovers.last(), Some(&(None, true)));

    // 4. Secondary button held also reports tx.pressing() == true.
    rt.state.hovers.clear();
    rt.button_down(2.0, ly, PointerButton::Secondary);
    rt.pointer_move(lx, ly);
    rt.frame(0.0);
    assert_eq!(rt.state.hovers, [(Some("https://example.com/t".into()), true)]);
    rt.button_up(lx, ly, PointerButton::Secondary);
}

#[test]
fn link_hover_inline_box_rtl_zoom_scale_hidpi_live_only_and_zero_cost_when_unset() {
    #[derive(Default)]
    struct State {
        hovered: Option<(String, [f32; 4])>,
        clicked: Option<[f32; 4]>,
    }

    // 1. Inline box beside a link + RTL + zoom + scale + HiDPI all match link_first_rect / Tx::link_rect.
    fn rich_view(_: &State, cx: &mut Cx<State>) -> El {
        cx.zoom(1.25);
        let badge = el().key("badge").fill(Color::WHITE);
        col().size(360.0, 180.0).padding(12.0).gap(12.0)
            .child(
                col().scale(1.1).child(
                    text("Open  and read docs")
                        .key("inline-doc")
                        .font(TextStyle::new(14.0, 400.0).lh(20.0))
                        .w(300.0)
                        .link(Link::new(15..19, "docs"))
                        .on_link(|s: &mut State, _, tx| s.clicked = tx.link_rect())
                        .on_link_hover(|s: &mut State, url, rect, _| {
                            s.hovered = url.zip(rect).map(|(u, r)| (u.to_owned(), r));
                        })
                        .inline(5, InlineBox::new(44.0, 18.0).align(InlineAlign::TextCenter), badge),
                ),
            )
            .child(
                text("alpha link omega")
                    .key("rtl-doc")
                    .dir(Direction::Rtl)
                    .font(TextStyle::new(14.0, 400.0).lh(20.0))
                    .w(260.0)
                    .link(Link::new(6..10, "rtl-link"))
                    .on_link(|s: &mut State, _, tx| s.clicked = tx.link_rect())
                    .on_link_hover(|s: &mut State, url, rect, tx| {
                        s.hovered = url.zip(rect).map(|(u, r)| (u.to_owned(), r));
                        tx.live_only();
                    }),
            )
    }

    let mut rt = Runtime::new(State::default(), rich_view, (360.0, 180.0), Env::new(true, 2.0));
    rt.frame(0.0);
    settle(&mut rt);
    assert!(rt.core().tree.has_link_hover);

    // Inline box link hover rect sits past the inline box and matches Tx::link_rect.
    let inline_i = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("inline-doc")).unwrap();
    let want_inline = rt.core().link_first_rect(inline_i, 0).expect("inline link rect");
    let badge_r = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("badge")).unwrap().rect;
    assert!(want_inline[0] > badge_r[0] + badge_r[2], "link past inline box: {want_inline:?} vs {badge_r:?}");
    let (ix, iy) = (want_inline[0] + want_inline[2] * 0.5, want_inline[1] + want_inline[3] * 0.5);
    rt.pointer_move(ix, iy);
    rt.frame(0.0);
    let (id, rect) = rt.state.hovered.as_ref().expect("inline hover");
    assert_eq!(id, "docs");
    let hover_rect = *rect;
    approx_rect(hover_rect, want_inline, "inline+zoom+scale+hidpi hover");
    rt.pointer_down(ix, iy);
    rt.pointer_up(ix, iy);
    rt.frame(0.0);
    assert_eq!(rt.state.clicked, Some(hover_rect));

    // RTL link sits on the right half and tx.live_only() skips a view commit.
    let rtl_i = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("rtl-doc")).unwrap();
    let want_rtl = rt.core().link_first_rect(rtl_i, 0).expect("rtl link rect");
    assert!(want_rtl[0] > 120.0, "RTL link sits on right half: {want_rtl:?}");
    rt.pointer_leave();
    rt.frame(0.0);
    let commits_before = rt.stats.commits;
    let (rx, ry) = (want_rtl[0] + want_rtl[2] * 0.5, want_rtl[1] + want_rtl[3] * 0.5);
    rt.pointer_move(rx, ry);
    rt.frame(0.0);
    assert_eq!(rt.stats.commits, commits_before, "tx.live_only() inside on_link_hover skips view commit");
    let (id, rect) = rt.state.hovered.as_ref().expect("rtl hover");
    assert_eq!(id, "rtl-link");
    approx_rect(*rect, want_rtl, "rtl hover");

    // 2. Zero cost when on_link_hover is unset: has_link_hover is false and observe_pointer adds 0 extra queries.
    fn plain_view(_: &(), _: &mut Cx<()>) -> El {
        col().size(240.0, 80.0).child(
            text("Open docs here")
                .font(TextStyle::new(14.0, 400.0).lh(20.0))
                .w(200.0)
                .link(Link::new(5..9, "docs")),
        )
    }
    let mut prt = Runtime::new((), plain_view, (240.0, 80.0), Env::new(true, 1.0));
    prt.frame(0.0);
    assert!(!prt.core().tree.has_link_hover, "has_link_hover must stay false when unset");
    let (px, py) = prt.find("docs").unwrap();
    let (_, q) = count_span_box_queries(|| prt.observe_pointer(px, py));
    assert!(q <= 2, "unset on_link_hover must add 0 span box queries, got {q}");
}

#[test]
fn link_hover_alone_exposes_link_a11y_role_and_web_mirror_without_click_action() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().size(280.0, 80.0).padding(10.0).child(
            text("Visit Rune docs")
                .key("doc")
                .font(TextStyle::new(14.0, 400.0).lh(20.0))
                .w(240.0)
                .link(Link::new(6..15, "https://example.com/rune"))
                .on_link_hover(|_: &mut (), _, _, _| {}),
        )
    }

    let mut rt = Runtime::new((), view, (280.0, 80.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // AccessKit tree: link has Role::Link, Focus action, and no Click action when on_link is unset.
    rt.a11y_activate("LinkHover");
    let update = rt.core_mut().a11y_update().expect("AccessKit tree update");
    let link_node = update
        .nodes
        .iter()
        .find_map(|(_, n)| (n.role() == accesskit::Role::Link).then_some(n))
        .expect("AccessKit link node");
    assert_eq!(link_node.label(), Some("Rune docs"));
    assert_eq!(link_node.url(), Some("https://example.com/rune"));
    assert!(link_node.supports_action(accesskit::Action::Focus));
    assert!(!link_node.supports_action(accesskit::Action::Click));

    // Web accessibility mirror exposes the inline <a> run.
    let items = rt.core().web_a11y_items();
    let run = items
        .iter()
        .flat_map(|it| it.runs.iter())
        .find(|r| r.href.as_deref() == Some("https://example.com/rune"))
        .expect("web mirror inline link run");
    assert_eq!(run.text, "Rune docs");
}

#[test]
fn a_drag_inside_a_static_texts_link_does_not_open_it() {
    #[derive(Default)]
    struct S { clicks: Vec<String>, selectable: bool }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        col().size(300.0, 80.0).padding(10.0).child(
            text("see open the full report now")
                .key("t")
                .w(260.0)
                .selectable(s.selectable)
                .link(Link::new(4..24, "target"))
                .on_link(|s: &mut S, id, _| s.clicks.push(id.to_owned())),
        )
    }
    for selectable in [true, false] {
        let mut rt = Runtime::new(S { selectable, ..S::default() }, view, (300.0, 80.0), Env::new(true, 1.0));
        rt.frame(0.0);
        settle(&mut rt);
        let t = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("t")).unwrap();
        let r = box_xywh(rt.core().link_boxes_for(t, 0, rt.core().drawn_box(t))[0].1);
        let y = r[1] + r[3] * 0.5;
        let at = |f: f32| r[0] + r[2] * f;

        // A drag that starts and ends inside the link is not a click.
        rt.pointer_down(at(0.2), y);
        for k in 1..=4 {
            rt.pointer_move(at(0.2 + 0.125 * k as f32), y);
            rt.frame(0.0);
        }
        rt.pointer_up(at(0.7), y);
        rt.frame(0.0);
        assert!(rt.state.clicks.is_empty(), "selectable={selectable}: drag opened the link");

        // A plain click, and one that wobbles within the click slop, still open it.
        rt.pointer_down(at(0.5), y);
        rt.pointer_up(at(0.5), y);
        rt.frame(1.0);
        rt.pointer_down(at(0.5), y);
        rt.pointer_move(at(0.5) + 2.0, y);
        rt.pointer_up(at(0.5) + 2.0, y);
        rt.frame(0.0);
        assert_eq!(rt.state.clicks, ["target", "target"], "selectable={selectable}");

        // A finger that moves 6 pt is still a tap (within `TOUCH_SLOP`, past `CLICK_SLOP`);
        // one that drags well past it is not.
        rt.state.clicks.clear();
        rt.touch(1, TouchPhase::Started, at(0.4), y, 10.0);
        rt.frame(0.0);
        rt.touch(1, TouchPhase::Moved, at(0.4) + 6.0, y, 10.02);
        rt.frame(0.0);
        rt.touch(1, TouchPhase::Ended, at(0.4) + 6.0, y, 10.04);
        rt.frame(0.0);
        assert_eq!(rt.state.clicks, ["target"], "selectable={selectable}: touch tap");
        rt.touch(2, TouchPhase::Started, at(0.2), y, 20.0);
        rt.frame(0.0);
        for k in 1..=4 {
            rt.touch(2, TouchPhase::Moved, at(0.2 + 0.125 * k as f32), y, 20.0 + 0.02 * k as f64);
            rt.frame(0.0);
        }
        rt.touch(2, TouchPhase::Ended, at(0.7), y, 20.2);
        rt.frame(0.0);
        assert_eq!(rt.state.clicks, ["target"], "selectable={selectable}: touch drag opened the link");
    }
}
