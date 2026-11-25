use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, col, el, text};
use rune_kit::{Kit, Markdown, Placement, popover};

#[derive(Default)]
struct State {
    opened: Option<(String, [f32; 4])>,
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let k = Kit::neutral(true);
    let mut layer = Vec::new();
    if let Some((url, rect)) = &s.opened {
        layer.push(
            el().key("md-link-anchor")
                .absolute(rect[0], rect[1])
                .size(rect[2], rect[3])
                .a11y_hidden(),
        );
        let card = popover::raised(
            &k,
            col()
                .w(180.0)
                .padding(8.0)
                .child(text(url.clone()).key("md-link-popover-url")),
            k.radius.md,
        )
        .key("md-link-popover");
        layer.push(popover::anchored(&k, card, "md-link-anchor", Placement::BottomStart, 6.0));
    }
    let body = col().size(480.0, 260.0).padding(16.0).child(
        Markdown::new("Ping [@alex](mention:alex) or read [#2061](ref:2061) for details.")
            .on_link_tx(|s: &mut State, url, tx| {
                if let Some(rect) = tx.link_rect() {
                    s.opened = Some((url.to_owned(), rect));
                }
            })
            .build(&k),
    );
    rune_kit::root(body, layer)
}

fn settle(rt: &mut Runtime<State>) {
    for _ in 0..30 {
        if !rt.frame(1.0 / 60.0) {
            return;
        }
    }
}

#[test]
fn markdown_on_link_tx_anchors_popover_beside_link_on_click_and_keyboard() {
    let mut rt = Runtime::new(State::default(), view, (480.0, 260.0), Env::new(true, 1.0));
    rt.frame(0.0);
    settle(&mut rt);

    // 1. Click on @alex opens popover anchored directly below @alex's link box.
    let alex = rt
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some("mention:alex"))
        .expect("@alex link element");
    let (ax, ay) = (alex.rect[0] + alex.rect[2] * 0.5, alex.rect[1] + alex.rect[3] * 0.5);
    rt.pointer_down(ax, ay);
    rt.pointer_up(ax, ay);
    settle(&mut rt);

    let (url, rect) = rt.state.opened.as_ref().expect("popover opened by click");
    assert_eq!(url, "mention:alex");
    for axis in 0..4 {
        assert!((rect[axis] - alex.rect[axis]).abs() < 0.5, "click axis {axis}: got {rect:?}, want {:?}", alex.rect);
    }
    let pop = rt
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some("md-link-popover"))
        .expect("popover mounted");
    assert!((pop.rect[0] - rect[0]).abs() < 1.0, "popover left aligns with link: {:?} vs {:?}", pop.rect, rect);
    assert!((pop.rect[1] - (rect[1] + rect[3] + 6.0)).abs() < 1.0, "popover sits 6pt below link: {:?} vs {:?}", pop.rect, rect);

    // 2. Focus #2061 and press Enter -> moves popover beside #2061.
    let ref_link = rt
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some("ref:2061"))
        .expect("#2061 link element");
    rt.update(|_, tx| tx.focus("ref:2061", true));
    settle(&mut rt);
    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    settle(&mut rt);

    let (url, rect) = rt.state.opened.as_ref().expect("popover opened by Enter");
    assert_eq!(url, "ref:2061");
    for axis in 0..4 {
        assert!((rect[axis] - ref_link.rect[axis]).abs() < 0.5, "Enter axis {axis}: got {rect:?}, want {:?}", ref_link.rect);
    }

    // 3. Space on focused #2061 also reports the same link_rect.
    rt.state.opened = None;
    rt.invalidate();
    settle(&mut rt);
    rt.key(KeyEvent { key: Key::Text(" ".into()), mods: Mods::default() });
    settle(&mut rt);
    let (url, rect_space) = rt.state.opened.as_ref().expect("popover opened by Space");
    assert_eq!(url, "ref:2061");
    for axis in 0..4 {
        assert!((rect_space[axis] - ref_link.rect[axis]).abs() < 0.5, "Space axis {axis}: got {rect_space:?}, want {:?}", ref_link.rect);
    }
}
