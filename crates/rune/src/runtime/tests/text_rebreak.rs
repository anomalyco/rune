use super::*;
use parley::Layout;
use crate::text::Ink;

fn resize_view(s: &bool, _: &mut Cx<bool>) -> El {
    let para = "Paragraph text long enough to wrap over several lines as the window narrows.";
    col().w_full().children((0..3).filter(|&i| *s || i != 2).map(|i| crate::el::text(format!("{i}. {para}")).w_full().wrap().key(format!("t{i}"))))
}

fn layout_of<'a>(rt: &'a Runtime<bool>, key: &str) -> &'a Rc<Layout<Ink>> {
    rt.nodes.iter().find(|n| n.key.as_deref() == Some(key)).and_then(|n| n.text_layout.as_ref()).unwrap()
}

/// A commit lets go of the last commit's text layouts before measuring, so on a resize each
/// wrapped text re-breaks its own layout in place; a text that leaves keeps its layout to draw
/// its exit.
#[test]
fn resizing_rebreaks_kept_texts_in_place_and_leaving_ones_keep_their_layout() {
    let mut rt = Runtime::new(true, resize_view, (700.0, 900.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.set_viewport((600.0, 900.0), 2.0);
    rt.frame(1.0 / 120.0);
    let kept = Rc::as_ptr(layout_of(&rt, "t0"));
    rt.set_viewport((300.0, 900.0), 2.0);
    rt.frame(2.0 / 120.0);
    assert_eq!(Rc::as_ptr(layout_of(&rt, "t0")), kept, "re-broken in place");
    assert!(layout_of(&rt, "t0").len() > 1);
    let leaving = layout_of(&rt, "t2").clone();
    rt.update(|s, _| *s = false);
    rt.frame(3.0 / 120.0);
    assert!(rt.is_exiting(rt.nodes.iter().find(|n| n.key.as_deref() == Some("t2")).unwrap().id));
    assert!(Rc::ptr_eq(layout_of(&rt, "t2"), &leaving), "the leaving text draws its exit from its own layout");
}

/// Measuring the content's height without committing leaves the drawn nodes' layouts alone.
#[test]
fn measuring_content_height_keeps_the_drawn_layouts() {
    let mut rt = Runtime::new(true, resize_view, (700.0, 900.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let l = layout_of(&rt, "t0").clone();
    rt.fit_height = true;
    rt.content_height();
    assert!(Rc::ptr_eq(layout_of(&rt, "t0"), &l));
}
