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

#[test]
fn native_regions_carry_a_links_help() {
    let mut rt = Runtime::new(1.0, linked, (400.0, 300.0), Env::new(true, 2.0));
    rt.pointer.help.native = true;
    rt.frame(0.0);
    let texts: Vec<&str> = rt.help_regions().iter().map(|(_, t)| t.as_str()).collect();
    assert_eq!(texts, ["Goes to b"]);
}
