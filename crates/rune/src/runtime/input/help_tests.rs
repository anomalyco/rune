use crate::{Cx, El, Env, el, row};
use super::{Runtime, subtract};

#[derive(Default)]
struct S { modal: bool, cover: bool }

fn view(s: &S, _: &mut Cx<S>) -> El {
    let mut root = el().w_full().h_full()
        .child(el().key("copy").size(40.0, 20.0).pin(Some(20.0), Some(20.0), None, None).help("Copy"))
        .child(el().key("low").size(40.0, 20.0).pin(Some(20.0), Some(200.0), None, None).help("Low"));
    if s.cover {
        // A popover drawn over the right half of "copy".
        root = root.child(el().key("pop").size(100.0, 60.0).pin(Some(40.0), Some(10.0), None, None).on_press(|_: &mut S| {}));
    }
    if s.modal {
        root = root.child(row().key("sheet").size(80.0, 40.0).pin(Some(200.0), Some(200.0), None, None)
            .modal(false, |_: &mut S| {})
            .child(el().key("inside").size(30.0, 20.0).help("Inside")));
    }
    root
}

fn runtime(s: S, native: bool) -> Runtime<S> {
    let mut rt = Runtime::new(s, view, (300.0, 300.0), Env::new(true, 2.0));
    rt.pointer.help.native = native;
    rt.frame(0.0);
    rt
}

fn texts(rt: &Runtime<S>) -> Vec<&str> { rt.help_regions().iter().map(|(_, t)| t.as_str()).collect() }

#[test]
fn native_regions_skip_what_a_modal_or_popover_covers() {
    let rt = runtime(S::default(), true);
    assert_eq!(texts(&rt), ["Copy", "Low"]);
    let rt = runtime(S { modal: true, ..S::default() }, true);
    assert_eq!(texts(&rt), ["Inside"], "behind a modal, nothing registers");
    let rt = runtime(S { cover: true, ..S::default() }, true);
    let copy: Vec<[f32; 4]> = rt.help_regions().iter().filter(|(_, t)| t == "Copy").map(|(r, _)| *r).collect();
    assert_eq!(copy, [[20.0, 20.0, 40.0, 40.0]], "only the uncovered half of copy");
}

#[test]
fn subtract_leaves_what_is_outside() {
    assert_eq!(subtract([0.0, 0.0, 10.0, 10.0], [20.0, 0.0, 30.0, 10.0]), [[0.0, 0.0, 10.0, 10.0]]);
    assert_eq!(subtract([0.0, 0.0, 10.0, 10.0], [-1.0, -1.0, 11.0, 11.0]), Vec::<[f32; 4]>::new());
    assert_eq!(subtract([0.0, 0.0, 10.0, 10.0], [4.0, 4.0, 6.0, 6.0]).len(), 4);
}

#[test]
fn reaching_help_wakes_a_host_that_skips_unchanged_frames() {
    // An overlay draws only when observe_pointer says something changed; help that has no
    // hover paint of its own must still start its wait.
    let mut rt = runtime(S::default(), false);
    assert!(!rt.observe_pointer(150.0, 150.0), "nothing there");
    assert!(rt.observe_pointer(30.0, 30.0), "onto help");
    rt.frame(1.0 / 60.0);
    assert!(!rt.observe_pointer(31.0, 30.0), "still on it");
    rt.frame(rt.help_delay);
    assert!(rt.help_tooltip().is_some());
    assert!(rt.observe_pointer(150.0, 150.0), "off it hides the tooltip");
}

fn linked(k: &f32, _: &mut Cx<f32>) -> El {
    let label = crate::text("MMMM MMMM")
        .font(crate::TextStyle::new(16.0, 400.0))
        .link(crate::Link::new(0..4, "a"))
        .link(crate::Link::new(5..9, "b").help("Goes to b"))
        .on_link(|_: &mut f32, _, _| {});
    el().w_full().h_full().child(el().pin(Some(10.0), Some(10.0), None, None).scale(*k).child(label))
}

/// Rests on (x, y) past the delay and returns the tooltip shown.
fn rest(rt: &mut Runtime<f32>, x: f32, y: f32) -> Option<(String, [f32; 4])> {
    rt.pointer_move(x, y);
    rt.frame(1.0 / 60.0);
    rt.frame(rt.help_delay + 0.1);
    rt.help_tooltip().map(|(t, r)| (t.to_owned(), r))
}

#[test]
fn a_link_shows_its_own_help_under_its_glyphs() {
    for k in [1.0, 1.6] {
        let mut rt = Runtime::new(k, linked, (400.0, 300.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let i = rt.tree.nodes.iter().position(|n| n.text.is_some()).unwrap();
        let boxes = rt.link_boxes(i, rt.drawn_box(i));
        let b = boxes.iter().find(|(j, _)| *j == 1).unwrap().1;
        let a = boxes.iter().find(|(j, _)| *j == 0).unwrap().1;
        assert!(rest(&mut rt, (a[0] + a[2]) * 0.5, (a[1] + a[3]) * 0.5).is_none(), "zoom {k}: a link without help shows none");
        let (text, r) = rest(&mut rt, (b[0] + b[2]) * 0.5, (b[1] + b[3]) * 0.5).expect("tip on the link");
        assert_eq!(text, "Goes to b");
        assert!(r[1] >= b[3] && r[0] < b[2] && r[0] + r[2] > b[0], "zoom {k}: tip {r:?} sits under the link {b:?}");
        assert!(rest(&mut rt, 300.0, 250.0).is_none(), "leaving hides it");
    }
}

/// "Under" sits clear below the window's foot; "foot" is too near it, so its tooltip opens over it.
fn lined(line: &bool, cx: &mut Cx<bool>) -> El {
    let look = crate::HelpLook::system(true).padding(6.0, 5.0).font(crate::TextStyle::new(11.0, 530.0).lh(12.0)).gap(4.0);
    cx.help_look(if *line { look.sit_on_line(crate::TextStyle::new(13.0, 400.0).lh(24.0)) } else { look });
    el().w_full().h_full()
        .child(el().key("under").size(40.0, 28.0).pin(Some(20.0), Some(20.0), None, None).help("Under"))
        .child(el().key("foot").size(40.0, 28.0).pin(Some(20.0), Some(262.0), None, None).help("Foot"))
}

#[test]
fn a_tooltip_on_a_line_drops_under_its_element_and_keeps_its_bottom_over_it() {
    let tip = |line: bool, x: f32, y: f32| {
        let mut rt = Runtime::new(line, lined, (300.0, 300.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.pointer_move(x, y);
        rt.frame(1.0 / 60.0);
        rt.frame(rt.help_delay + 0.1);
        rt.help_tooltip().map(|(_, r)| r).expect("tooltip shown")
    };
    let (off, on) = (tip(false, 40.0, 34.0), tip(true, 40.0, 34.0));
    assert_eq!(off[1], 52.0, "4 pt under the element's 48 pt foot");
    let drop = on[1] - off[1];
    assert!(drop > 0.0 && drop <= 3.0, "a 13/24 line drops an 11/12 tooltip a little: {off:?} -> {on:?}");
    assert_eq!([on[0], on[2], on[3]], [off[0], off[2], off[3]], "only its top moves");
    let (off, on) = (tip(false, 40.0, 276.0), tip(true, 40.0, 276.0));
    assert!(off[1] + off[3] <= 262.0, "foot's tooltip opens over it: {off:?}");
    assert_eq!(on, off, "over its element a tooltip on a line keeps its bottom");
}

#[test]
fn native_regions_carry_a_links_help() {
    let mut rt = Runtime::new(1.0, linked, (400.0, 300.0), Env::new(true, 2.0));
    rt.pointer.help.native = true;
    rt.frame(0.0);
    let texts: Vec<&str> = rt.help_regions().iter().map(|(_, t)| t.as_str()).collect();
    assert_eq!(texts, ["Goes to b"]);
}

#[test]
fn link_tooltip_lays_out_only_hovered_link() {
    struct Doc {
        body: String,
        links: Vec<(usize, usize, String, Option<String>)>,
    }
    let mut body = String::new();
    let mut links = Vec::with_capacity(40);
    for idx in 0..40 {
        if idx > 0 { body.push(' '); }
        let start = body.len();
        let (label, id, help) = if idx == 15 {
            ("long_wrapped_link_phrase across_two_lines".to_owned(), "wrapped".to_owned(), Some("Help for wrapped".to_owned()))
        } else if idx == 10 {
            (format!("link{idx:02}"), format!("l{idx}"), Some("Help for l10".to_owned()))
        } else {
            (format!("link{idx:02}"), format!("l{idx}"), None)
        };
        body.push_str(&label);
        links.push((start, body.len(), id, help));
    }
    fn view(s: &Doc, _: &mut Cx<Doc>) -> El {
        let mut t = crate::text(s.body.clone())
            .key("doc")
            .font(crate::TextStyle::new(14.0, 400.0).lh(20.0))
            .wrap()
            .w_full();
        for (start, end, id, help) in &s.links {
            let mut l = crate::Link::new(*start..*end, id.as_str());
            if let Some(h) = help { l = l.help(h.as_str()); }
            t = t.link(l);
        }
        el().w_full().h_full()
            .child(el().pin(Some(10.0), Some(10.0), None, None).w(180.0).child(t))
            .child(el().key("plain").size(40.0, 20.0).pin(Some(220.0), Some(20.0), None, None).help("Plain"))
    }
    let mut rt = Runtime::new(Doc { body, links }, view, (320.0, 500.0), Env::new(true, 1.0));
    rt.frame(0.0);

    let i = rt.tree.nodes.iter().position(|n| n.text.is_some()).expect("text node");
    let drawn_boxes = rt.link_boxes(i, rt.drawn_box(i));
    let link_index = |rt: &Runtime<Doc>, id: &str| {
        rt.tree.nodes[i].text.as_ref().unwrap().cold.links.iter().position(|l| l.id == id).unwrap()
    };
    let wrapped_k = link_index(&rt, "wrapped");
    let wrapped_boxes = drawn_boxes.iter().filter(|(j, _)| *j == wrapped_k).count();
    assert!(wrapped_boxes >= 2, "expected wrapped link to span at least 2 line boxes, got {wrapped_boxes}");

    for (id, want_help) in [("l10", "Help for l10"), ("wrapped", "Help for wrapped")] {
        let k = link_index(&rt, id);
        let first = drawn_boxes.iter().find(|(j, _)| *j == k).unwrap().1;
        let expected_anchor = drawn_boxes.iter().filter(|(j, _)| *j == k).map(|(_, r)| *r)
            .reduce(|u, r| [u[0].min(r[0]), u[1].min(r[1]), u[2].max(r[2]), u[3].max(r[3])])
            .unwrap();

        rt.pointer_move((first[0] + first[2]) * 0.5, (first[1] + first[3]) * 0.5);
        rt.frame(1.0 / 60.0);
        rt.frame(rt.help_delay + 0.1);
        let (tip_text, _) = rt.help_tooltip().expect("tooltip open");
        assert_eq!(tip_text, want_help);

        let ((), queries) = crate::runtime::count_span_box_queries(|| rt.core_mut().update_help());
        assert_eq!(queries, 1, "update_help should lay out only hovered link {id}");
        let got_anchor = rt.pointer.help.drawn.as_ref().expect("drawn tooltip").anchor;
        assert!(
            got_anchor.iter().zip(expected_anchor).all(|(a, b)| (a - b).abs() < 0.01),
            "{id}: got anchor {got_anchor:?}, want {expected_anchor:?}"
        );
    }

    // A non-link element tooltip lays out 0 link spans.
    rt.pointer_move(240.0, 30.0);
    rt.frame(1.0 / 60.0);
    rt.frame(rt.help_delay + 0.1);
    assert_eq!(rt.help_tooltip().map(|(t, _)| t), Some("Plain"));
    let ((), queries) = crate::runtime::count_span_box_queries(|| rt.core_mut().update_help());
    assert_eq!(queries, 0, "non-link tooltip should query 0 link spans");
}

#[test]
fn link_help_hits_lay_out_only_links_with_help() {
    struct Doc {
        body: String,
        links: Vec<(usize, usize, String, Option<String>)>,
        plain_body: String,
        plain_links: Vec<(usize, usize, String)>,
    }
    let mut body = String::new();
    let mut links = Vec::with_capacity(40);
    for idx in 0..40 {
        if idx > 0 { body.push(' '); }
        let start = body.len();
        let (label, id, help) = if idx == 15 {
            ("long_wrapped_link_phrase across_two_lines".to_owned(), "wrapped".to_owned(), Some("Help for wrapped".to_owned()))
        } else if idx == 10 {
            (format!("link{idx:02}"), format!("l{idx}"), Some("Help for l10".to_owned()))
        } else if idx == 30 {
            (format!("link{idx:02}"), format!("l{idx}"), Some("Help for l30".to_owned()))
        } else {
            (format!("link{idx:02}"), format!("l{idx}"), None)
        };
        body.push_str(&label);
        links.push((start, body.len(), id, help));
    }
    let mut plain_body = String::new();
    let mut plain_links = Vec::with_capacity(10);
    for idx in 0..10 {
        if idx > 0 { plain_body.push(' '); }
        let start = plain_body.len();
        plain_body.push_str(&format!("plain{idx:02}"));
        plain_links.push((start, plain_body.len(), format!("p{idx}")));
    }
    fn view(s: &Doc, _: &mut Cx<Doc>) -> El {
        let mut t = crate::text(s.body.clone())
            .key("doc")
            .font(crate::TextStyle::new(14.0, 400.0).lh(20.0))
            .wrap()
            .selectable(true)
            .w_full();
        for (start, end, id, help) in &s.links {
            let mut l = crate::Link::new(*start..*end, id.as_str());
            if let Some(h) = help { l = l.help(h.as_str()); }
            t = t.link(l);
        }
        let mut p = crate::text(s.plain_body.clone())
            .key("no-help")
            .font(crate::TextStyle::new(14.0, 400.0).lh(20.0))
            .wrap()
            .selectable(true)
            .w_full();
        for (start, end, id) in &s.plain_links {
            p = p.link(crate::Link::new(*start..*end, id.as_str()));
        }
        el().w_full().h_full()
            .child(el().pin(Some(10.0), Some(10.0), None, None).w(180.0).child(t))
            .child(el().pin(Some(210.0), Some(10.0), None, None).w(100.0).child(p))
    }
    let mut rt = Runtime::new(Doc { body, links, plain_body, plain_links }, view, (340.0, 600.0), Env::new(true, 1.0));
    rt.frame(0.0);

    let doc_i = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("doc")).expect("doc text node");
    let plain_i = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("no-help")).expect("plain text node");
    let doc_id = rt.tree.nodes[doc_i].id;
    let doc_rect = rt.drawn_box(doc_i);
    let plain_rect = rt.drawn_box(plain_i);
    let all_boxes = rt.link_boxes(doc_i, doc_rect);

    let wrapped_k = rt.tree.nodes[doc_i].text.as_ref().unwrap().cold.links.iter()
        .position(|l| l.id == "wrapped").unwrap();
    let wrapped_boxes: Vec<[f32; 4]> = all_boxes.iter().filter(|(j, _)| *j == wrapped_k).map(|(_, r)| *r).collect();
    assert!(wrapped_boxes.len() >= 2, "expected wrapped link to span at least 2 line boxes, got {}", wrapped_boxes.len());

    // Expected help hits computed from laying out every link and filtering to those with help.
    let has_help: Vec<bool> = rt.tree.nodes[doc_i].text.as_ref().unwrap().cold.links.iter()
        .map(|l| l.help.is_some())
        .collect();
    let expected_for_clip = |clip: [f32; 4], seq: usize| -> Vec<(super::NodeId, [f32; 4], usize, Option<usize>)> {
        all_boxes.iter()
            .filter(|(k, _)| has_help[*k])
            .filter_map(|&(k, r)| {
                let c = [r[0].max(clip[0]), r[1].max(clip[1]), r[2].min(clip[2]), r[3].min(clip[3])];
                (c[2] > c[0] && c[3] > c[1]).then_some((doc_id, c, seq, Some(k)))
            })
            .collect()
    };

    // Unclipped pass: only the 3 links with help are laid out, with identical hits.
    let full_clip = [0.0, 0.0, 1000.0, 1000.0];
    let want_full = expected_for_clip(full_clip, 7);
    assert!(want_full.len() >= 4, "3 help links including a 2+-line wrapped link should emit at least 4 boxes");
    rt.core_mut().clear_help_hits();
    let ((), queries) = crate::runtime::count_span_box_queries(|| {
        rt.core_mut().push_link_help_hits(doc_i, doc_rect, full_clip, 7);
    });
    assert_eq!(queries, 3, "push_link_help_hits should lay out only the 3 links with help, not all 40");
    assert_eq!(rt.hit.help_hits(), want_full.as_slice(), "unclipped link help hits must match");

    // Partial clip rect cutting through the wrapped link: identical clipped hits, still 3 queries.
    let mid_y = (wrapped_boxes[0][1] + wrapped_boxes[1][3]) * 0.5;
    let partial_clip = [doc_rect[0] + 10.0, wrapped_boxes[0][1], doc_rect[2] - 10.0, mid_y];
    let want_partial = expected_for_clip(partial_clip, 11);
    assert!(!want_partial.is_empty(), "partial clip should keep part of the wrapped link");
    rt.core_mut().clear_help_hits();
    let ((), partial_queries) = crate::runtime::count_span_box_queries(|| {
        rt.core_mut().push_link_help_hits(doc_i, doc_rect, partial_clip, 11);
    });
    assert_eq!(partial_queries, 3, "clipped push_link_help_hits should still lay out only the 3 links with help");
    assert_eq!(rt.hit.help_hits(), want_partial.as_slice(), "clipped link help hits must match");

    // A text node whose links have no help queries 0 spans and pushes 0 hits.
    rt.core_mut().clear_help_hits();
    let ((), plain_queries) = crate::runtime::count_span_box_queries(|| {
        rt.core_mut().push_link_help_hits(plain_i, plain_rect, full_clip, 0);
    });
    assert_eq!(plain_queries, 0, "links without help should query 0 spans");
    assert!(rt.hit.help_hits().is_empty());

    // A subsequent painted frame (no rebuild) re-runs push_link_help_hits because linked text
    // nodes are not paint-reused; on selectable text it lays out only the 3 links with help.
    let (_, frame_queries) = crate::runtime::count_span_box_queries(|| rt.frame(1.0 / 60.0));
    assert_eq!(frame_queries, 3, "painted frame on selectable linked text should lay out only the 3 help links");

    // End-to-end tooltip hit-testing still finds every line of a wrapped help link and skips plain links.
    let plain_box = all_boxes.iter().find(|(j, _)| *j == 0).unwrap().1;
    rt.pointer_move((plain_box[0] + plain_box[2]) * 0.5, (plain_box[1] + plain_box[3]) * 0.5);
    rt.frame(1.0 / 60.0);
    rt.frame(rt.help_delay + 0.1);
    assert!(rt.help_tooltip().is_none(), "link without help must not open a tooltip");

    for line_box in &wrapped_boxes {
        rt.pointer_move(330.0, 590.0);
        rt.frame(1.0 / 60.0);
        rt.pointer_move((line_box[0] + line_box[2]) * 0.5, (line_box[1] + line_box[3]) * 0.5);
        rt.frame(1.0 / 60.0);
        rt.frame(rt.help_delay + 0.1);
        assert_eq!(rt.help_tooltip().map(|(t, _)| t), Some("Help for wrapped"));
    }
}

#[test]
fn focused_link_shows_its_help_under_its_glyphs_when_on_focus_is_enabled() {
    use crate::{Key, KeyEvent, Mods};

    fn view(on_focus: &bool, cx: &mut Cx<bool>) -> El {
        cx.help_look(crate::HelpLook::system(true).on_focus(*on_focus));
        let first = crate::text("AAAA Local app CCCC")
            .key("links")
            .font(crate::TextStyle::new(16.0, 400.0))
            .link(crate::Link::new(0..4, "https://example.com/a"))
            .link(crate::Link::new(5..14, "http://localhost:8123/").help("This address opens on your device."))
            .link(crate::Link::new(15..19, "https://example.com/c").help("Opens docs"))
            .on_link(|_: &mut bool, _, _| {});
        let fallback = crate::text("Fallback")
            .key("fallback")
            .font(crate::TextStyle::new(16.0, 400.0))
            .help("Paragraph help")
            .link(crate::Link::new(0..8, "https://example.com/f"))
            .on_link(|_: &mut bool, _, _| {});
        el().w_full().h_full()
            .child(el().pin(Some(10.0), Some(10.0), None, None).child(first))
            .child(el().pin(Some(10.0), Some(80.0), None, None).child(fallback))
    }

    let press = |rt: &mut Runtime<bool>, key: Key, shift: bool| {
        rt.key(KeyEvent { key, mods: Mods { shift, ..Default::default() } });
        rt.frame(1.0 / 60.0);
    };

    let mut rt = Runtime::new(true, view, (400.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let i = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("links")).unwrap();
    let boxes = rt.link_boxes(i, rt.drawn_box(i));
    let b1 = boxes.iter().find(|(j, _)| *j == 1).unwrap().1;
    let b2 = boxes.iter().find(|(j, _)| *j == 2).unwrap().1;

    // Tab to link 0 (no help): no tooltip.
    press(&mut rt, Key::Tab, false);
    assert!(rt.help_tooltip().is_none(), "focused link without help shows no tooltip");

    // Tab to link 1 (has help): tooltip opens immediately under link 1's glyphs, laying out only link 1.
    press(&mut rt, Key::Tab, false);
    let (tip, r1) = rt.help_tooltip().expect("focused link with help opens tooltip immediately");
    assert_eq!(tip, "This address opens on your device.");
    assert!(r1[1] >= b1[3] && r1[0] < b1[2] && r1[0] + r1[2] > b1[0], "tip {r1:?} sits under link 1 {b1:?}");
    let ((), queries) = crate::runtime::count_span_box_queries(|| rt.core_mut().update_help());
    assert_eq!(queries, 1, "focused link tooltip should lay out only the focused link");

    // Tab to link 2: tooltip switches immediately to link 2's help under link 2's glyphs.
    press(&mut rt, Key::Tab, false);
    let (tip, r2) = rt.help_tooltip().expect("switching focus to next link updates tooltip");
    assert_eq!(tip, "Opens docs");
    assert!(r2[1] >= b2[3] && r2[0] < b2[2] && r2[0] + r2[2] > b2[0], "tip {r2:?} sits under link 2 {b2:?}");

    // Escape dismisses the tooltip while link 2 keeps focus.
    press(&mut rt, Key::Escape, false);
    assert!(rt.help_tooltip().is_none(), "Escape suppresses the focused link tooltip");

    // Shift+Tab back to link 1 reopens link 1's tooltip.
    press(&mut rt, Key::Tab, true);
    assert_eq!(rt.help_tooltip().map(|(t, _)| t), Some("This address opens on your device."));

    // Tab past link 2 onto the fallback node's link (no link help): falls back to node help.
    press(&mut rt, Key::Tab, false);
    press(&mut rt, Key::Tab, false);
    assert_eq!(rt.help_tooltip().map(|(t, _)| t), Some("Paragraph help"));

    // With HelpLook::on_focus(false), keyboard focus onto a link does not open a tooltip.
    let mut off = Runtime::new(false, view, (400.0, 300.0), Env::new(true, 2.0));
    off.frame(0.0);
    press(&mut off, Key::Tab, false);
    press(&mut off, Key::Tab, false);
    assert!(off.help_tooltip().is_none(), "on_focus(false) keeps link focus tooltips off");
}

#[test]
fn help_look_side_places_by_default_flips_and_is_overridden_by_el_help_side() {
    use crate::{Direction, HelpLook, Side};

    struct Cfg {
        side: Side,
        rtl: bool,
    }

    fn view(c: &Cfg, cx: &mut Cx<Cfg>) -> El {
        cx.help_look(HelpLook::system(true).side(c.side));
        let mut root = el().w_full().h_full()
            .child(el().key("mid").size(40.0, 20.0).pin(Some(140.0), Some(140.0), None, None).help("Middle"))
            .child(el().key("near_top").size(40.0, 20.0).pin(Some(140.0), Some(6.0), None, None).help("Near top"))
            .child(el().key("near_left").size(40.0, 20.0).pin(Some(6.0), Some(140.0), None, None).help("Near left"))
            .child(el().key("force_bottom").size(40.0, 20.0).pin(Some(140.0), Some(200.0), None, None).help("Force bottom").help_side(Side::Bottom))
            .child(el().key("force_right").size(40.0, 20.0).pin(Some(80.0), Some(200.0), None, None).help("Force right").help_side(Side::Right));
        if c.rtl {
            root = root.dir(Direction::Rtl);
        }
        root
    }

    let hover = |rt: &mut Runtime<Cfg>, key: &str| -> ([f32; 4], [f32; 4]) {
        let i = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some(key)).unwrap();
        let b = rt.drawn_box(i);
        rt.pointer_move(0.0, 0.0);
        rt.frame(1.0 / 60.0);
        rt.pointer_move((b[0] + b[2]) * 0.5, (b[1] + b[3]) * 0.5);
        rt.frame(1.0 / 60.0);
        rt.frame(rt.help_delay + 0.1);
        let (_, tip) = rt.help_tooltip().expect("tooltip open");
        (b, tip)
    };

    // HelpLook::side(Side::Top) opens above by default, flips below near the top edge, and
    // yields to per-element El::help_side (including an explicit Side::Bottom override).
    let mut rt = Runtime::new(Cfg { side: Side::Top, rtl: false }, view, (320.0, 320.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let (mid, tip) = hover(&mut rt, "mid");
    assert!(tip[1] + tip[3] <= mid[1], "Side::Top opens above: tip {tip:?}, anchor {mid:?}");

    let (top, tip) = hover(&mut rt, "near_top");
    assert!(tip[1] >= top[3], "Side::Top flips below near top edge: tip {tip:?}, anchor {top:?}");

    let (fb, tip) = hover(&mut rt, "force_bottom");
    assert!(tip[1] >= fb[3], "El::help_side(Side::Bottom) overrides HelpLook::side(Side::Top): tip {tip:?}, anchor {fb:?}");

    let (fr, tip) = hover(&mut rt, "force_right");
    assert!(tip[0] >= fr[2], "El::help_side(Side::Right) overrides HelpLook::side(Side::Top): tip {tip:?}, anchor {fr:?}");

    // HelpLook::side(Side::Left) opens left by default and flips right near the left edge.
    let mut rt = Runtime::new(Cfg { side: Side::Left, rtl: false }, view, (320.0, 320.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let (mid, tip) = hover(&mut rt, "mid");
    assert!(tip[0] + tip[2] <= mid[0], "Side::Left opens left: tip {tip:?}, anchor {mid:?}");

    let (nl, tip) = hover(&mut rt, "near_left");
    assert!(tip[0] >= nl[2], "Side::Left flips right near left edge: tip {tip:?}, anchor {nl:?}");

    // In RTL layout, HelpLook::side(Side::Right) mirrors to open on the left of its anchor.
    let mut rt = Runtime::new(Cfg { side: Side::Right, rtl: true }, view, (320.0, 320.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let (mid, tip) = hover(&mut rt, "mid");
    assert!(tip[0] + tip[2] <= mid[0], "RTL mirrors Side::Right to left: tip {tip:?}, anchor {mid:?}");
}

#[test]
fn help_look_arrow_emits_triangle_facing_anchor_and_omits_when_disabled() {
    use crate::render::{KIND_RECT, KIND_TRIANGLE};
    use crate::{Color, HelpLook, Side};

    struct Cfg {
        side: Side,
        arrow: f32,
        gap: f32,
    }

    fn view(c: &Cfg, cx: &mut Cx<Cfg>) -> El {
        cx.help_look(
            HelpLook::system(true)
                .fill(Color { r: 0.12, g: 0.14, b: 0.18, a: 1.0 })
                .ring(0.0, Color::TRANSPARENT)
                .gap(c.gap)
                .side(c.side)
                .arrow(c.arrow),
        );
        el().w_full().h_full()
            .child(el().key("mid").size(40.0, 20.0).pin(Some(140.0), Some(140.0), None, None).help("Arrow tooltip"))
            .child(el().key("near_top").size(40.0, 20.0).pin(Some(140.0), Some(6.0), None, None).help("Flipped"))
    }

    let open_at = |cfg: Cfg, key: &str, scale: f32| {
        let mut rt = Runtime::new(cfg, view, (360.0, 320.0), Env::new(true, scale));
        rt.frame(0.0);
        let i = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some(key)).unwrap();
        let b = rt.drawn_box(i);
        rt.pointer_move((b[0] + b[2]) * 0.5, (b[1] + b[3]) * 0.5);
        rt.frame(1.0 / 60.0);
        rt.frame(rt.help_delay + 0.1);
        (rt, b)
    };

    // Disabled, negative and non-finite arrow sizes emit zero KIND_TRIANGLE instances.
    for size in [0.0, -4.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let (rt, _) = open_at(Cfg { side: Side::Top, arrow: size, gap: 4.0 }, "mid", 2.0);
        assert!(rt.help_tooltip().is_some());
        let tris = rt.instances.iter().filter(|i| i.params[1] == KIND_TRIANGLE).count();
        assert_eq!(tris, 0, "arrow({size}) must emit no KIND_TRIANGLE");
    }

    // All four sides at 2x scale: one KIND_TRIANGLE facing the anchor, flush with the card edge,
    // tip staying gap clear of the anchor and colour matching the card fill.
    let s = 2.0;
    for side in [Side::Top, Side::Bottom, Side::Left, Side::Right] {
        let (rt, b) = open_at(Cfg { side, arrow: 5.0, gap: 4.0 }, "mid", s);
        let (_, tip) = rt.help_tooltip().expect("tooltip open");
        let tris: Vec<_> = rt.instances.iter().filter(|i| i.params[1] == KIND_TRIANGLE).collect();
        assert_eq!(tris.len(), 1, "side {side:?}: expected 1 triangle");
        let tri = tris[0];
        assert_eq!(tri.params[0], 6.0, "side {side:?}: base hard, slanted legs AA");
        let card = rt.instances.iter().rev().find(|i| i.params[1] == KIND_RECT && i.fill == tri.fill).expect("matching card rect");
        assert_eq!(tri.fill, card.fill);
        let [ax, ay, bx, by] = tri.radii;
        let [cx, cy] = [tri.uv[0], tri.uv[1]];
        let expected_h = 5.0 * s;
        match side {
            Side::Top => {
                assert!((ay - (card.rect[1] + card.rect[3])).abs() < 0.01 && (by - ay).abs() < 0.01);
                assert!((cy - (ay + expected_h)).abs() < 0.01, "Top arrow points down toward anchor");
                assert!(cy <= b[1] * s - 4.0 * s + 1.0, "tip keeps gap clear of anchor");
                assert!(tip[1] + tip[3] <= b[1] - 8.9);
            }
            Side::Bottom => {
                assert!((ay - card.rect[1]).abs() < 0.01 && (by - ay).abs() < 0.01);
                assert!((cy - (ay - expected_h)).abs() < 0.01, "Bottom arrow points up toward anchor");
                assert!(cy >= b[3] * s + 4.0 * s - 1.0, "tip keeps gap clear of anchor");
                assert!(tip[1] >= b[3] + 8.9);
            }
            Side::Left => {
                assert!((ax - (card.rect[0] + card.rect[2])).abs() < 0.01 && (bx - ax).abs() < 0.01);
                assert!((cx - (ax + expected_h)).abs() < 0.01, "Left arrow points right toward anchor");
                assert!(cx <= b[0] * s - 4.0 * s + 1.0, "tip keeps gap clear of anchor");
            }
            Side::Right => {
                assert!((ax - card.rect[0]).abs() < 0.01 && (bx - ax).abs() < 0.01);
                assert!((cx - (ax - expected_h)).abs() < 0.01, "Right arrow points left toward anchor");
                assert!(cx >= b[2] * s + 4.0 * s - 1.0, "tip keeps gap clear of anchor");
            }
        }
    }

    // When Side::Top flips to Side::Bottom near the top edge, the arrow flips to point up.
    let (rt, b) = open_at(Cfg { side: Side::Top, arrow: 5.0, gap: 4.0 }, "near_top", s);
    let (_, tip) = rt.help_tooltip().expect("flipped tooltip");
    assert!(tip[1] >= b[3], "flipped below anchor");
    let tri = rt.instances.iter().find(|i| i.params[1] == KIND_TRIANGLE).expect("flipped arrow");
    assert!(tri.uv[1] < tri.radii[1], "flipped arrow points upward toward anchor");
}

#[test]
fn help_look_arrow_clamps_inside_corner_radii_and_handles_edge_cases() {
    use crate::render::{KIND_RECT, KIND_TRIANGLE};
    use crate::{HelpLook, Side};

    struct Cfg {
        side: Side,
        radius: f32,
        arrow: f32,
        anchor_x: f32,
    }

    fn view(c: &Cfg, cx: &mut Cx<Cfg>) -> El {
        cx.help_look(HelpLook::system(true).radius(c.radius).side(c.side).arrow(c.arrow).gap(4.0).margin(4.0));
        el().w_full().h_full()
            .child(el().key("a").size(16.0, 20.0).pin(Some(c.anchor_x), Some(140.0), None, None).help("A fairly wide tooltip label"))
    }

    let open = |cfg: Cfg| {
        let mut rt = Runtime::new(cfg, view, (320.0, 300.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let i = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("a")).unwrap();
        let b = rt.drawn_box(i);
        rt.pointer_move((b[0] + b[2]) * 0.5, (b[1] + b[3]) * 0.5);
        rt.frame(1.0 / 60.0);
        rt.frame(rt.help_delay + 0.1);
        rt
    };

    // Anchor pinned at the left window edge: card clamps to margin, arrow clamps inside
    // [card.x + radius, card.x + card.w - radius].
    let rt = open(Cfg { side: Side::Top, radius: 8.0, arrow: 5.0, anchor_x: 2.0 });
    let tri = rt.instances.iter().find(|i| i.params[1] == KIND_TRIANGLE).expect("clamped arrow");
    let card = rt.instances.iter().rev().find(|i| i.params[1] == KIND_RECT && i.fill == tri.fill).unwrap();
    assert!(tri.radii[0] >= card.rect[0] + 8.0 - 0.01, "arrow left base stays clear of corner radius: {tri:?} vs {card:?}");
    assert!(tri.radii[2] <= card.rect[0] + card.rect[2] - 8.0 + 0.01, "arrow right base stays clear of corner radius");

    // Huge arrow clamps cleanly to the flat span between corner radii without panicking.
    let rt = open(Cfg { side: Side::Top, radius: 6.0, arrow: 500.0, anchor_x: 140.0 });
    let tri = rt.instances.iter().find(|i| i.params[1] == KIND_TRIANGLE).expect("huge arrow clamped");
    let card = rt.instances.iter().rev().find(|i| i.params[1] == KIND_RECT && i.fill == tri.fill).unwrap();
    assert!(tri.radii.iter().chain(&tri.uv).all(|v| v.is_finite()));
    assert!(tri.radii[0] >= card.rect[0] + 6.0 - 0.01);
    assert!(tri.radii[2] <= card.rect[0] + card.rect[2] - 6.0 + 0.01);

    // Pill radius on a vertical edge leaves zero flat span: no degenerate triangle emitted.
    let rt = open(Cfg { side: Side::Right, radius: 40.0, arrow: 5.0, anchor_x: 40.0 });
    assert!(rt.help_tooltip().is_some());
    assert_eq!(rt.instances.iter().filter(|i| i.params[1] == KIND_TRIANGLE).count(), 0);
}

#[test]
fn help_look_arrow_fades_scales_and_respects_reduced_motion() {
    use crate::render::{KIND_RECT, KIND_TRIANGLE};
    use crate::{HelpLook, Side};

    fn view(reduced: &bool, cx: &mut Cx<bool>) -> El {
        cx.reduced_motion(*reduced);
        cx.help_look(HelpLook::system(true).side(Side::Top).arrow(5.0).fade(0.2, 0.2, 0.8));
        el().w_full().h_full()
            .child(el().key("btn").size(40.0, 20.0).pin(Some(140.0), Some(140.0), None, None).help("Fade"))
    }

    // Mid fade-in: arrow shares card alpha and stays flush with the scaled card bottom edge.
    let mut rt = Runtime::new(false, view, (320.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(160.0, 150.0);
    rt.frame(1.0 / 60.0);
    rt.frame(rt.help_delay);
    rt.frame(0.05);
    let tri = rt.instances.iter().find(|i| i.params[1] == KIND_TRIANGLE).expect("fading arrow");
    let card = rt.instances.iter().rev().find(|i| i.params[1] == KIND_RECT && i.fill == tri.fill).expect("fading card");
    assert!(tri.fill[3] > 0.0 && tri.fill[3] < 1.0, "arrow fades in: alpha {}", tri.fill[3]);
    assert!((tri.radii[1] - (card.rect[1] + card.rect[3])).abs() < 0.01, "arrow base stays flush with scaled card edge");
    assert!(tri.rect[3] < 5.0, "arrow height scales down with fade_scale");

    // Reduced motion: full alpha and full scale on the first open frame.
    let mut rt = Runtime::new(true, view, (320.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(160.0, 150.0);
    rt.frame(1.0 / 60.0);
    rt.frame(rt.help_delay);
    let tri = rt.instances.iter().find(|i| i.params[1] == KIND_TRIANGLE).expect("reduced motion arrow");
    assert!((tri.fill[3] - 1.0).abs() < 1e-4, "reduced motion opens at full alpha");
    assert!((tri.rect[3] - 5.0).abs() < 0.01, "reduced motion opens at full arrow scale");
}

#[test]
fn help_look_side_and_arrow_update_a11y_tooltip_on_keyboard_focus() {
    use crate::render::KIND_TRIANGLE;
    use crate::{HelpLook, Key, KeyEvent, Mods, Side};

    fn view(_: &(), cx: &mut Cx<()>) -> El {
        cx.help_look(HelpLook::system(true).side(Side::Top).arrow(5.0).on_focus(true));
        el().w_full().h_full().child(
            el().key("save").role("button").label("Save").size(60.0, 24.0)
                .pin(Some(120.0), Some(140.0), None, None)
                .focusable(true)
                .on_press(|_: &mut ()| {})
                .help("Save changes"),
        )
    }

    let mut rt = Runtime::new((), view, (320.0, 300.0), Env::new(true, 1.0));
    rt.a11y_activate("test");
    rt.frame(0.0);
    let _ = rt.a11y_update();
    let i = rt.tree.nodes.iter().position(|n| n.key.as_deref() == Some("save")).unwrap();
    let anchor = rt.drawn_box(i);

    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    let (tip_text, tip_rect) = rt.help_tooltip().expect("keyboard focus opens tooltip");
    assert_eq!(tip_text, "Save changes");
    assert!(tip_rect[1] + tip_rect[3] <= anchor[1], "keyboard tooltip respects HelpLook::side(Side::Top)");
    assert_eq!(rt.instances.iter().filter(|i| i.params[1] == KIND_TRIANGLE).count(), 1);

    let update = rt.a11y_update().expect("a11y tree updated for tooltip");
    let tip_node = update.nodes.iter().find(|(_, n)| n.role() == accesskit::Role::Tooltip).expect("Tooltip role in a11y tree");
    let bounds = tip_node.1.bounds().expect("Tooltip bounds");
    assert!(bounds.y1 <= anchor[1] as f64, "a11y tooltip bounds reflect Top placement: {bounds:?}");
}
