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
