use rune::{Color, Cx, El, Env, Runtime, col, el, text};
use rune_kit::{Kit, Markdown, Placement, popover};

#[derive(Default)]
struct State {
    hovered: Option<(String, [f32; 4])>,
    events: Vec<(Option<String>, Option<[f32; 4]>)>,
    clicks: Vec<String>,
    tag: usize,
}

fn settle(rt: &mut Runtime<State>) {
    for _ in 0..30 {
        if !rt.frame(1.0 / 60.0) {
            return;
        }
    }
}

#[test]
fn markdown_on_link_hover_works_without_on_link_across_blocks_and_wrapped_links() {
    const MD: &str = "First paragraph with [alpha](https://example.com/alpha) and [a very long wrapped link destination spanning two lines in a narrow column](https://example.com/wrapped).\n\nSecond paragraph with [beta](https://example.com/beta) and `src/lib.rs`.";

    fn view(s: &State, _: &mut Cx<State>) -> El {
        let k = Kit::neutral(true);
        let mut layer = Vec::new();
        if let Some((url, rect)) = &s.hovered {
            layer.push(
                el().key("hover-anchor")
                    .absolute(rect[0], rect[1])
                    .size(rect[2], rect[3])
                    .a11y_hidden(),
            );
            let card = popover::raised(
                &k,
                col().w(200.0).padding(8.0).child(text(url.clone()).key("hover-card-url")),
                k.radius.md,
            )
            .key("hover-card")
            .hover_passes_below();
            layer.push(popover::anchored(&k, card, "hover-anchor", Placement::BottomStart, 6.0));
        }
        let body = col().size(320.0, 280.0).padding(16.0).child(
            Markdown::new(MD)
                .code_link(Color::WHITE, |code| code.ends_with(".rs").then(|| format!("file://{code}")))
                .on_link_hover(|s: &mut State, url, rect, _| {
                    s.events.push((url.map(str::to_owned), rect));
                    s.hovered = url.zip(rect).map(|(u, r)| (u.to_owned(), r));
                })
                .build(&k),
        );
        rune_kit::root(body, layer)
    }

    let mut rt = Runtime::new(State::default(), view, (320.0, 280.0), Env::new(true, 1.0));
    rt.frame(0.0);
    settle(&mut rt);

    // 1. Hovering alpha (with NO on_link set!) fires on_link_hover and anchors preview card.
    let alpha = rt
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some("https://example.com/alpha"))
        .expect("alpha link element active with on_link_hover alone");
    rt.pointer_move(alpha.rect[0] + alpha.rect[2] * 0.5, alpha.rect[1] + alpha.rect[3] * 0.5);
    settle(&mut rt);

    let (url, rect) = rt.state.hovered.as_ref().expect("hovered alpha");
    assert_eq!(url, "https://example.com/alpha");
    for axis in 0..4 {
        assert!((rect[axis] - alpha.rect[axis]).abs() < 0.5, "alpha axis {axis}: {rect:?} vs {:?}", alpha.rect);
    }
    let card = rt
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some("hover-card"))
        .expect("hover preview card mounted");
    assert!((card.rect[1] - (rect[1] + rect[3] + 6.0)).abs() < 1.5);

    // 2. Moving directly from paragraph 1's link to paragraph 2's link leaves p1 before entering p2
    //    so s.hovered settles on beta without getting cleared.
    let beta = rt
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some("https://example.com/beta"))
        .expect("beta link element");
    rt.state.events.clear();
    rt.pointer_move(beta.rect[0] + beta.rect[2] * 0.5, beta.rect[1] + beta.rect[3] * 0.5);
    settle(&mut rt);

    assert_eq!(rt.state.events.len(), 2, "cross-paragraph switch fires leave then enter");
    assert_eq!(rt.state.events[0], (None, None));
    assert_eq!(rt.state.events[1].0.as_deref(), Some("https://example.com/beta"));
    assert_eq!(
        rt.state.hovered.as_ref().map(|(u, _)| u.as_str()),
        Some("https://example.com/beta")
    );

    // 3. Code links also report hover.
    let code_link = rt
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some("file://src/lib.rs"))
        .expect("code_link element");
    rt.pointer_move(code_link.rect[0] + code_link.rect[2] * 0.5, code_link.rect[1] + code_link.rect[3] * 0.5);
    settle(&mut rt);
    assert_eq!(
        rt.state.hovered.as_ref().map(|(u, _)| u.as_str()),
        Some("file://src/lib.rs")
    );

    // 4. Wrapped link in Markdown reports a single line box (height < union height).
    let wrapped = rt
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some("https://example.com/wrapped"))
        .expect("wrapped link element");
    assert!(wrapped.rect[3] > 30.0, "wrapped link union spans >= 2 lines: {:?}", wrapped.rect);
    rt.pointer_move(wrapped.rect[0] + 10.0, wrapped.rect[1] + wrapped.rect[3] - 6.0);
    settle(&mut rt);
    let (wurl, wrect) = rt.state.hovered.as_ref().expect("hovered wrapped");
    assert_eq!(wurl, "https://example.com/wrapped");
    assert!(wrect[3] < wrapped.rect[3] * 0.7, "wrapped hover rect must be single hit line box: {wrect:?} vs union {:?}", wrapped.rect);

    // 5. Pointer leave clears hovered state and unmounts card.
    rt.pointer_leave();
    settle(&mut rt);
    assert!(rt.state.hovered.is_none());
    assert!(rt.elements().into_iter().all(|e| e.key.as_deref() != Some("hover-card")));
}

#[test]
fn markdown_on_link_hover_coexists_with_on_link_and_invalidates_capturing_memo() {
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let k = Kit::neutral(true);
        let tag = s.tag;
        col().size(320.0, 120.0).padding(16.0).child(
            Markdown::new("Click or hover [docs](https://example.com/docs).")
                .on_link(|s: &mut State, url| s.clicks.push(url.to_owned()))
                .on_link_hover(move |s: &mut State, url, rect, _| {
                    s.events.push((url.map(|u| format!("{tag}:{u}")), rect));
                })
                .build(&k),
        )
    }

    let mut rt = Runtime::new(State { tag: 1, ..State::default() }, view, (320.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);
    settle(&mut rt);

    let (x, y) = rt.find("https://example.com/docs").unwrap();
    rt.pointer_move(x, y);
    rt.frame(0.0);
    assert_eq!(rt.state.events.last().unwrap().0.as_deref(), Some("1:https://example.com/docs"));

    // Clicking still runs on_link.
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    assert_eq!(rt.state.clicks, ["https://example.com/docs"]);

    // Updating captured `tag` must not reuse a stale memoised closure.
    rt.pointer_leave();
    rt.frame(0.0);
    rt.state.events.clear();
    rt.update(|s, _| s.tag = 2);
    settle(&mut rt);
    rt.pointer_move(x, y);
    rt.frame(0.0);
    assert_eq!(rt.state.events.last().unwrap().0.as_deref(), Some("2:https://example.com/docs"));
}
