use super::*;
use crate::el::row;

#[test]
fn a_key_path_skips_elements_without_keys() {
    struct State;
    fn view(_: &State, _: &mut Cx<State>) -> El {
        col()
            .child(el().key("other").child(el().key("knob").size(10.0, 10.0)))
            .child(
                el().key("form").child(
                    row().child(el().size(30.0, 10.0)).child(el().key("slider").child(col().child(el().key("knob").size(20.0, 20.0)))),
                ),
            )
    }
    let mut rt = Runtime::new(State, view, (100.0, 100.0), Env { system_dark: true, scale: 2.0 });
    rt.frame(0.0);
    assert_eq!(rt.find("slider/knob"), Some((40.0, 20.0)));
    assert_eq!(rt.find("form/slider/knob"), Some((40.0, 20.0)));
    assert_eq!(rt.find("other/knob"), Some((5.0, 5.0)));
    assert_eq!(rt.find("form/knob"), None, "keyed segments must still be adjacent");
    assert_eq!(rt.find("knob"), Some((5.0, 5.0)), "a bare key finds the first element with it");
}

#[test]
fn scaled_link_and_highlight_elements_keep_unscaled_targets() {
    fn view(scale: &f32, _: &mut Cx<f32>) -> El {
        col().size(400.0, 200.0).child(
            col().size(200.0, 100.0).scale(*scale).origin(0.0, 0.0).child(
                crate::rich("Check link and mark here.", vec![])
                    .key("body").padding(8.0)
                    .link(crate::Link::new(6..10, "doc"))
                    .text_highlight(crate::TextHighlight::new(15..19, Color::WHITE).key("hl")),
            ),
        )
    }
    let make = |scale: f32| {
        let mut rt = Runtime::new(scale, view, (400.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt
    };
    let rt1 = make(1.0);
    let rt2 = make(2.0);
    let find_el = |rt: &Runtime<f32>, key: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap();
    for key in ["doc", "hl"] {
        let e1 = find_el(&rt1, key);
        let e2 = find_el(&rt2, key);
        assert!((e2.rect[2] - e1.rect[2] * 2.0).abs() < 0.5, "{key}: drawn rect scales by 2x");
        assert!(
            e1.target.iter().zip(e2.target).all(|(a, b)| (a - b).abs() < 0.2),
            "{key}: target must stay unscaled layout rect, got {:?} vs {:?}", e1.target, e2.target
        );
    }
}

#[test]
fn keyed_lookups_skip_exiting_nodes_and_popup_anchors_match_keyed_suffix() {
    // 1. PopupMenu anchor_rect matches keyed-suffix paths across unkeyed wrappers (like Runtime::find).
    {
        let mut rt = Runtime::new((), |_: &(), _: &mut Cx<()>| {
            col().size(200.0, 100.0).child(
                col().key("slider").child(row().child(el().key("knob").size(20.0, 20.0))),
            )
        }, (200.0, 100.0), Env::new(true, 1.0));
        rt.frame(0.0);
        assert!(rt.find("slider/knob").is_some());
        rt.update(|_, tx| {
            tx.popup_menu(crate::PopupMenu::new().item(crate::PopupItem::new("a", "A")), "slider/knob", |_: &mut (), _, _| {});
        });
        rt.frame(0.0);
        let opened = rt.take_popup();
        assert!(matches!(opened, Some(Some(_))), "popup anchor_rect must match slider/knob across unkeyed wrapper");
    }

    // 2. reveal_row_x skips exiting (and instantly unmounted) elements instead of scrolling a horizontal viewport to them.
    {
        fn view(show: &bool, _: &mut Cx<bool>) -> El {
            let mut inner = row().w(800.0).h(40.0).child(el().size(180.0, 40.0));
            if *show {
                // Straddles the 200pt viewport edge (180..240) so it draws on screen, plays its exit
                // transition when removed, and would scroll `hx` by 40pt if `reveal_row_x` matched it.
                inner = inner.child(el().key("gone").size(60.0, 40.0).fill(Color::WHITE).transition(crate::Transition::opacity()));
            }
            col().size(200.0, 60.0).child(
                row().key("hx").size(200.0, 40.0).scroll_x().child(inner),
            )
        }
        let mut rt = Runtime::new(true, view, (200.0, 60.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.update(|show, tx| {
            *show = false;
            tx.scroll_into_view("gone");
        });
        rt.frame(1.0 / 60.0);
        let gone_idx = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("gone")).expect("gone plays its exit");
        assert!(rt.core().subtree_exiting(gone_idx), "gone must be actively exiting");
        let hx = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("hx")).unwrap().id;
        assert_eq!(rt.scroll.scrolls[&hx].x.offset, 0.0, "reveal_row_x must ignore exiting target");
    }

    // 3. stuck_cover matches keyed suffixes via Node::named and ignores exiting nodes.
    {
        fn view(show: &bool, _: &mut Cx<bool>) -> El {
            let mut hdr = col().key("hdr").w_full().h(30.0).fill(Color::WHITE).sticky_top(0.0)
                .child(row().child(el().key("pin").size(40.0, 20.0)));
            if *show {
                hdr = hdr.child(el().key("leaving").size(40.0, 20.0).transition(crate::Transition::opacity()));
            }
            col().size(200.0, 120.0).child(
                col().key("sc").size(200.0, 120.0).scroll(crate::ScrollSpec { stick_bottom: false })
                    .child(col().w_full().child(hdr).child(el().w_full().h(400.0))),
            )
        }
        let mut rt = Runtime::new(true, view, (200.0, 120.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.wheel(50.0, 50.0, -80.0);
        rt.update(|show, _| *show = false);
        rt.frame(1.0 / 60.0);
        let sc_path = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("sc")).unwrap().path.clone();
        assert_eq!(rt.core().stuck_cover(&sc_path, None, "hdr/pin"), None, "stuck_cover must match keyed suffix inside stuck header");
        assert!(rt.core().stuck_cover(&sc_path, None, "leaving").is_some_and(|c| c > 0.0), "stuck_cover must ignore exiting node");
    }
}

#[test]
fn presented_resolves_key_paths_while_mounted_and_exiting() {
    fn view(show_card_two_title: &bool, _: &mut Cx<bool>) -> El {
        let mut card_two = row();
        if *show_card_two_title {
            card_two = card_two.child(
                el().key("title").size(60.0, 10.0).fill(Color::WHITE).transition(crate::Transition::opacity()),
            );
        }
        col()
            .child(el().key("title").size(100.0, 20.0))
            .child(col().key("card-1").child(el().key("title").size(80.0, 10.0)))
            .child(col().key("card-2").child(card_two))
    }
    let mut rt = Runtime::new(true, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Plain key keeps returning the first matching node; key paths resolve nested nodes across unkeyed wrappers.
    assert_eq!(rt.presented("title"), Some(([0.0, 0.0, 100.0, 20.0], 1.0)));
    assert_eq!(rt.presented("card-1/title"), Some(([0.0, 20.0, 80.0, 10.0], 1.0)));
    assert_eq!(rt.presented("card-2/title"), Some(([0.0, 30.0, 60.0, 10.0], 1.0)));
    assert_eq!(rt.find("card-2/title"), Some((30.0, 35.0)));
    assert!(rt.presented_pose("card-2/title").is_some());
    assert_eq!(rt.presented("missing/title"), None);

    // While card-2/title is playing its exit transition, find skips it and presented still reads it.
    rt.update(|show, _| *show = false);
    for _ in 0..10 {
        rt.frame(1.0 / 120.0);
    }
    assert_eq!(rt.find("card-2/title"), None);
    let (rect, opacity) = rt.presented("card-2/title").expect("exiting card-2/title is still presented");
    assert_eq!(rect, [0.0, 30.0, 60.0, 10.0]);
    assert!(opacity > 0.0 && opacity < 1.0, "expected fading opacity, got {opacity}");

    // Once the exit transition finishes, presented drops it.
    for _ in 0..240 {
        rt.frame(1.0 / 120.0);
    }
    assert_eq!(rt.presented("card-2/title"), None);
}
