use super::*;
use crate::{
    Align, Cx, Direction, El, Env, Key, KeyEvent, Link, Mark, MarkRange, Mods, PointerButton, Runtime,
    ScrollSpec, Side, TextEdit, TextStyle, TouchPhase, Tx, col, el, text, text_input,
};

fn approx_rect(got: [f32; 4], want: [f32; 4], label: &str) {
    for (axis, (g, w)) in got.into_iter().zip(want).enumerate() {
        assert!(
            (g - w).abs() < 0.05,
            "{label}: axis {axis} got {g}, want {w} (full got={got:?}, want={want:?})"
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

fn first_box_xywh<S: 'static>(rt: &Runtime<S>, node_idx: usize, link_idx: usize) -> [f32; 4] {
    let core = rt.core();
    let (_, r) = core
        .link_boxes_for(node_idx, link_idx, core.drawn_box(node_idx))
        .into_iter()
        .next()
        .expect("expected drawn link box");
    [r[0], r[1], (r[2] - r[0]).max(0.0), (r[3] - r[1]).max(0.0)]
}

#[test]
fn link_rect_matches_first_drawn_box_across_pointer_touch_keyboard_and_a11y() {
    #[derive(Default)]
    struct State {
        activations: Vec<(String, Option<[f32; 4]>, PointerButton)>,
        other_handlers: Vec<(&'static str, Option<[f32; 4]>)>,
        polled: Option<[f32; 4]>,
    }

    fn view(_: &State, _: &mut Cx<State>) -> El {
        col()
            .size(320.0, 220.0)
            .padding(16.0)
            .gap(12.0)
            .on_key(|s: &mut State, _, tx| {
                s.other_handlers.push(("key", tx.link_rect()));
                false
            })
            .child(
                text("Open the reference guide for details.")
                    .key("doc")
                    .font(TextStyle::new(14.0, 400.0).lh(20.0))
                    .wrap()
                    .w(240.0)
                    .link(Link::new(9..24, "guide"))
                    .on_link(|s: &mut State, id, tx| {
                        s.activations.push((id.to_owned(), tx.link_rect(), tx.button()));
                    })
                    .on_link_secondary(|s: &mut State, _, _, tx| {
                        s.other_handlers.push(("secondary", tx.link_rect()));
                    })
                    .on_hover_change(|s: &mut State, _, tx| {
                        s.other_handlers.push(("hover", tx.link_rect()));
                    }),
            )
            .child(
                el().key("btn")
                    .size(100.0, 32.0)
                    .on_press_tx(|s: &mut State, tx| {
                        s.other_handlers.push(("press", tx.link_rect()));
                    }),
            )
    }

    let mut rt = Runtime::new(State::default(), view, (320.0, 220.0), Env::new(true, 1.0));
    rt.frame(0.0);
    settle(&mut rt);

    let doc_i = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("doc")).unwrap();
    let doc_path = rt.tree.nodes[doc_i].path.clone();
    let want = first_box_xywh(&rt, doc_i, 0);
    assert!(want[2] > 20.0 && want[3] > 8.0, "non-empty link box: {want:?}");
    let cx = want[0] + want[2] * 0.5;
    let cy = want[1] + want[3] * 0.5;

    // 1. Primary mouse click.
    rt.pointer_move(cx, cy);
    rt.pointer_down(cx, cy);
    rt.pointer_up(cx, cy);
    rt.frame(0.0);
    let (id, got, btn) = rt.state.activations.pop().expect("primary click");
    assert_eq!((id.as_str(), btn), ("guide", PointerButton::Primary));
    approx_rect(got.expect("primary click link_rect"), want, "primary click");

    // 2. Middle click.
    rt.button_down(cx, cy, PointerButton::Middle);
    rt.button_up(cx, cy, PointerButton::Middle);
    rt.frame(0.0);
    let (id, got, btn) = rt.state.activations.pop().expect("middle click");
    assert_eq!((id.as_str(), btn), ("guide", PointerButton::Middle));
    approx_rect(got.expect("middle click link_rect"), want, "middle click");

    // 3. Touch tap.
    rt.touch(1, TouchPhase::Started, cx, cy, 0.0);
    rt.touch(1, TouchPhase::Ended, cx, cy, 0.05);
    rt.frame(0.0);
    let (id, got, _) = rt.state.activations.pop().expect("touch tap");
    assert_eq!(id, "guide");
    approx_rect(got.expect("touch tap link_rect"), want, "touch tap");

    // 4. Keyboard Enter on focused link.
    rt.update(|_, tx| tx.focus("guide", true));
    settle(&mut rt);
    assert_eq!(rt.focused_key().as_deref(), Some("guide"));
    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    rt.frame(0.0);
    let (id, got, _) = rt.state.activations.pop().expect("Enter on focused link");
    assert_eq!(id, "guide");
    approx_rect(got.expect("Enter link_rect"), want, "Enter");

    // 5. Keyboard Space on focused link.
    rt.key(KeyEvent { key: Key::Text(" ".into()), mods: Mods::default() });
    rt.frame(0.0);
    let (id, got, _) = rt.state.activations.pop().expect("Space on focused link");
    assert_eq!(id, "guide");
    approx_rect(got.expect("Space link_rect"), want, "Space");

    // 6. AccessKit Action::Click.
    rt.core_mut().queue_a11y_link(doc_path.clone(), 0);
    rt.frame(0.0);
    let (id, got, _) = rt.state.activations.pop().expect("a11y click");
    assert_eq!(id, "guide");
    approx_rect(got.expect("a11y click link_rect"), want, "a11y click");

    // 7. Web accessibility mirror a11y_link.
    assert!(rt.web_a11y_action(&serde_json::json!({
        "kind": "a11y_link",
        "path": doc_path,
        "link": 0,
    })));
    rt.frame(0.0);
    let (id, got, _) = rt.state.activations.pop().expect("web a11y_link");
    assert_eq!(id, "guide");
    approx_rect(got.expect("web a11y_link link_rect"), want, "web a11y_link");

    // 8. Secondary press, on_press_tx, on_hover_change, on_key, and update all see None.
    rt.secondary_down(cx, cy);
    rt.secondary_up(cx, cy);
    rt.frame(0.0);
    let (bx, by) = rt.find("btn").expect("btn");
    rt.pointer_down(bx, by);
    rt.pointer_up(bx, by);
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Text("a".into()), mods: Mods::default() });
    rt.frame(0.0);
    rt.update(|s, tx| s.polled = tx.link_rect());
    assert!(rt.state.polled.is_none());
    assert!(
        rt.state.other_handlers.iter().any(|(k, _)| *k == "secondary")
            && rt.state.other_handlers.iter().any(|(k, _)| *k == "press")
            && rt.state.other_handlers.iter().any(|(k, _)| *k == "hover")
            && rt.state.other_handlers.iter().any(|(k, _)| *k == "key"),
        "expected secondary, press, hover, and key handlers to run: {:?}",
        rt.state.other_handlers
    );
    assert!(
        rt.state.other_handlers.iter().all(|(_, r)| r.is_none()),
        "non-on_link handlers must see link_rect() == None: {:?}",
        rt.state.other_handlers
    );
}

#[test]
fn link_rect_returns_only_first_line_box_for_wrapped_link() {
    #[derive(Default)]
    struct State {
        rect: Option<[f32; 4]>,
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
                .on_link(|s: &mut State, _, tx| s.rect = tx.link_rect()),
        )
    }

    let mut rt = Runtime::new(State::default(), view, (180.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    settle(&mut rt);

    let i = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("wrap")).unwrap();
    let all_boxes = rt.core().link_boxes_for(i, 0, rt.core().drawn_box(i));
    assert!(all_boxes.len() >= 2, "link must wrap across at least two lines, got {}", all_boxes.len());
    let first = all_boxes[0].1;
    let second = all_boxes[1].1;
    let want = [first[0], first[1], first[2] - first[0], first[3] - first[1]];

    // Click on the second line of the wrapped link: link_rect still returns the FIRST line box.
    let click_x = (second[0] + second[2]) * 0.5;
    let click_y = (second[1] + second[3]) * 0.5;
    rt.pointer_down(click_x, click_y);
    rt.pointer_up(click_x, click_y);
    rt.frame(0.0);

    let got = rt.state.rect.expect("wrapped link_rect");
    approx_rect(got, want, "wrapped first line box");
    assert!(got[3] < (second[3] - first[1]) * 0.75, "must be first line height, not union height");
}

#[test]
fn link_rect_follows_scroll_zoom_scale_and_hidpi() {
    #[derive(Default)]
    struct State {
        rect: Option<[f32; 4]>,
    }

    fn view(_: &State, cx: &mut Cx<State>) -> El {
        cx.zoom(1.25);
        col()
            .size(280.0, 140.0)
            .key("sc")
            .scroll(ScrollSpec::default())
            .child(el().size(240.0, 100.0))
            .child(
                col().scale(1.1).child(
                    text("Scrolled link target")
                        .key("doc")
                        .font(TextStyle::new(14.0, 400.0).lh(20.0))
                        .w(220.0)
                        .link(Link::new(9..13, "scrolled"))
                        .on_link(|s: &mut State, _, tx| s.rect = tx.link_rect()),
                ),
            )
            .child(el().size(240.0, 200.0))
    }

    let mut rt = Runtime::new(State::default(), view, (280.0, 140.0), Env::new(true, 2.0));
    rt.frame(0.0);
    settle(&mut rt);

    let i = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("doc")).unwrap();
    let before = first_box_xywh(&rt, i, 0);

    rt.update(|_, tx| tx.scroll_to_instant("sc", 0.0, 60.0));
    settle(&mut rt);
    let after = first_box_xywh(&rt, i, 0);
    assert!(before[1] - after[1] > 40.0, "expected scroll shift: before={before:?}, after={after:?}");

    // Keyboard activation after scroll reports scrolled, zoomed, scaled window points.
    rt.update(|_, tx| tx.focus("scrolled", true));
    settle(&mut rt);
    let settled_after = first_box_xywh(&rt, i, 0);
    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    rt.frame(0.0);
    approx_rect(rt.state.rect.expect("scrolled link_rect"), settled_after, "scrolled+zoomed+scaled");
}

#[test]
fn link_rect_handles_rtl_text() {
    #[derive(Default)]
    struct State {
        rect: Option<[f32; 4]>,
    }

    const BODY: &str = "مرحبا بالعالم رابط مهم هنا";
    const START: usize = "مرحبا بالعالم ".len();
    const END: usize = START + "رابط".len();

    fn view(_: &State, _: &mut Cx<State>) -> El {
        col().size(300.0, 100.0).padding(12.0).child(
            text(BODY)
                .key("rtl-doc")
                .dir(Direction::Rtl)
                .font(TextStyle::new(14.0, 400.0).lh(20.0))
                .w(260.0)
                .link(Link::new(START..END, "rtl-link"))
                .on_link(|s: &mut State, _, tx| s.rect = tx.link_rect()),
        )
    }

    let mut rt = Runtime::new(State::default(), view, (300.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    settle(&mut rt);

    let i = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("rtl-doc")).unwrap();
    let want = first_box_xywh(&rt, i, 0);
    assert!(want[0] > 120.0, "RTL link should sit on the right half: {want:?}");

    rt.update(|_, tx| tx.focus("rtl-link", true));
    settle(&mut rt);
    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    rt.frame(0.0);
    approx_rect(rt.state.rect.expect("RTL link_rect"), want, "RTL link");
}

#[test]
fn link_rect_is_none_when_not_drawn_truncated_or_empty() {
    #[derive(Default)]
    struct State {
        calls: Vec<(String, Option<[f32; 4]>)>,
    }

    // 1. Before first paint (never drawn): press_link sees None.
    fn simple_view(_: &State, _: &mut Cx<State>) -> El {
        col().size(200.0, 80.0).child(
            text("Hello world")
                .key("plain")
                .w(180.0)
                .link(Link::new(6..11, "world"))
                .on_link(|s: &mut State, id, tx| s.calls.push((id.to_owned(), tx.link_rect()))),
        )
    }

    let mut rt = Runtime::new(State::default(), simple_view, (200.0, 80.0), Env::new(true, 1.0));
    rt.commit(&Tx::default());
    let plain_path = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("plain")).unwrap().path.clone();
    {
        let (core, mut app) = rt.split_mut();
        let mut tx = Tx::default();
        assert!(core.press_link(&mut app, &plain_path, 0, &mut tx));
        assert!(tx.link_rect().is_none(), "press_link restores tx.link_rect after handler");
    }
    assert_eq!(rt.state.calls, [("world".into(), None)], "before first paint must return None");

    // 2. Truncated away link and empty byte-range link both return None.
    fn edge_view(_: &State, _: &mut Cx<State>) -> El {
        col()
            .size(200.0, 120.0)
            .child(
                text("A very long sentence whose trailing link gets elided away completely")
                    .key("trunc")
                    .font(TextStyle::new(14.0, 400.0).lh(20.0))
                    .truncate()
                    .w(100.0)
                    .link(Link::new(58..68, "elided"))
                    .on_link(|s: &mut State, id, tx| s.calls.push((id.to_owned(), tx.link_rect()))),
            )
            .child(
                text("Empty range link")
                    .key("empty")
                    .w(180.0)
                    .link(Link::new(5..5, "zero"))
                    .on_link(|s: &mut State, id, tx| s.calls.push((id.to_owned(), tx.link_rect()))),
            )
            .child(
                text("Hidden under missing anchor")
                    .key("anchor-hidden")
                    .w(180.0)
                    .anchored("does-not-exist", Side::Bottom, Align::Start, 4.0)
                    .link(Link::new(0..6, "hidden"))
                    .on_link(|s: &mut State, id, tx| s.calls.push((id.to_owned(), tx.link_rect()))),
            )
    }

    let mut rt = Runtime::new(State::default(), edge_view, (200.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);
    settle(&mut rt);

    for (key, expected_id) in [("trunc", "elided"), ("empty", "zero"), ("anchor-hidden", "hidden")] {
        let path = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some(key)).unwrap().path.clone();
        rt.state.calls.clear();
        let (core, mut app) = rt.split_mut();
        let mut tx = Tx::default();
        assert!(core.press_link(&mut app, &path, 0, &mut tx));
        assert_eq!(
            rt.state.calls,
            [(expected_id.into(), None)],
            "{key} must report None when link has no drawn line box"
        );
    }
}

#[test]
fn link_rect_respects_inert_and_disabled_subtrees() {
    #[derive(Default)]
    struct State {
        calls: usize,
    }

    fn view(_: &State, _: &mut Cx<State>) -> El {
        col()
            .size(260.0, 140.0)
            .padding(10.0)
            .gap(10.0)
            .child(
                col().inert().child(
                    text("Inert link")
                        .key("inert-doc")
                        .w(200.0)
                        .link(Link::new(6..10, "inert-link"))
                        .on_link(|s: &mut State, _, _| s.calls += 1),
                ),
            )
            .child(
                text("Disabled link")
                    .key("dis-doc")
                    .w(200.0)
                    .link(Link::new(9..13, "dis-link"))
                    .on_link(|s: &mut State, _, _| s.calls += 1)
                    .disabled(true),
            )
    }

    let mut rt = Runtime::new(State::default(), view, (260.0, 140.0), Env::new(true, 1.0));
    rt.frame(0.0);
    settle(&mut rt);

    // Inert blocks pointer activation and focus.
    if let Some((x, y)) = rt.find("inert-link") {
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
    }
    rt.update(|_, tx| tx.focus("inert-link", true));
    settle(&mut rt);
    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    rt.frame(0.0);

    // Disabled blocks keyboard focus activation (El::disabled stops focus and a11y click).
    rt.update(|_, tx| tx.focus("dis-link", true));
    settle(&mut rt);
    assert!(rt.focused_key().is_none(), "disabled link cannot take focus");
    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    rt.frame(0.0);
    assert_eq!(rt.state.calls, 0, "inert and disabled links must not activate");
}

#[test]
fn link_rect_follows_single_line_text_input_horizontal_scroll() {
    struct Form {
        edit: TextEdit,
        rect: Option<[f32; 4]>,
    }

    fn view(s: &Form, _: &mut Cx<Form>) -> El {
        let style = TextStyle::new(14.0, 400.0).lh(20.0);
        col().size(260.0, 80.0).padding(10.0).child(
            text_input(&s.edit, style)
                .single_line()
                .key("field")
                .label("Field")
                .size(180.0, 24.0)
                .on_edit(|s: &mut Form| &mut s.edit)
                .on_link(|s: &mut Form, _, tx| s.rect = tx.link_rect()),
        )
    }

    let prefix = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ ";
    let body = format!("{prefix}docs");
    let link = MarkRange::new(prefix.len()..body.len(), Mark::Link("https://example.com/docs".into()));
    let mut edit = TextEdit::from_parts(body, vec![link], vec![]);
    edit.focused = true;
    edit.set_caret(prefix.len() + 4);

    let mut rt = Runtime::new(Form { edit, rect: None }, view, (260.0, 80.0), Env::new(true, 1.0));
    rt.frame(0.0);
    settle(&mut rt);

    let i = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("field")).unwrap();
    let path = rt.tree.nodes[i].path.clone();
    let scroll_x = rt.edit.input_scroll.get(&path).copied().unwrap_or(0.0);
    assert!(scroll_x > 20.0, "expected single-line input to scroll horizontally: {scroll_x}");

    let raw = first_box_xywh(&rt, i, 0);
    let want = [raw[0] - scroll_x, raw[1], raw[2], raw[3]];

    let (core, mut app) = rt.split_mut();
    let mut tx = Tx::default();
    assert!(core.press_link(&mut app, &path, 0, &mut tx));
    approx_rect(rt.state.rect.expect("scrolled input link_rect"), want, "single-line input scroll");
}

#[test]
fn press_link_queries_only_the_activated_link_and_zero_without_handler() {
    struct ManyLinks {
        body: String,
        links: Vec<(usize, usize, String)>,
        with_handler: bool,
        last: Option<[f32; 4]>,
    }

    let make = |with_handler: bool| {
        let mut body = String::new();
        let mut links = Vec::with_capacity(40);
        for n in 0..40 {
            if n > 0 {
                body.push(' ');
            }
            let start = body.len();
            body.push_str(&format!("item{n:02}"));
            let end = body.len();
            links.push((start, end, format!("l{n:02}")));
        }
        ManyLinks { body, links, with_handler, last: None }
    };

    fn view(s: &ManyLinks, _: &mut Cx<ManyLinks>) -> El {
        let mut t = text(s.body.clone())
            .key("many")
            .font(TextStyle::new(14.0, 400.0).lh(20.0))
            .wrap()
            .w(240.0);
        for (a, b, id) in &s.links {
            t = t.link(Link::new(*a..*b, id.as_str()));
        }
        if s.with_handler {
            t = t.on_link(|s: &mut ManyLinks, _, tx| s.last = tx.link_rect());
        }
        col().size(260.0, 400.0).child(t)
    }

    let mut rt = Runtime::new(make(true), view, (260.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    settle(&mut rt);
    let i = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("many")).unwrap();
    let path = rt.tree.nodes[i].path.clone();
    let want = first_box_xywh(&rt, i, 25);

    let (handled, queries) = count_span_box_queries(|| {
        let (core, mut app) = rt.split_mut();
        let mut tx = Tx::default();
        core.press_link(&mut app, &path, 25, &mut tx)
    });
    assert!(handled);
    assert_eq!(queries, 1, "press_link must query only the activated link");
    approx_rect(rt.state.last.expect("link 25 rect"), want, "link 25");

    let mut no_handler = Runtime::new(make(false), view, (260.0, 400.0), Env::new(true, 1.0));
    no_handler.frame(0.0);
    settle(&mut no_handler);
    let j = no_handler.tree.nodes.iter().position(|n| n.key.as_deref() == Some("many")).unwrap();
    let path_no = no_handler.tree.nodes[j].path.clone();
    let (handled, queries) = count_span_box_queries(|| {
        let (core, mut app) = no_handler.split_mut();
        let mut tx = Tx::default();
        core.press_link(&mut app, &path_no, 25, &mut tx)
    });
    assert!(!handled);
    assert_eq!(queries, 0, "press_link without on_link must query 0 spans");
}
