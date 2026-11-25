//! `El::shrink` drops only an automatic minimum width, so an explicit `min_w` holds in either order.
use super::*;
use crate::{Cx, El, Env, col, el, row};

fn child_width(view: fn(&(), &mut Cx<()>) -> El) -> f32 {
    let mut rt = Runtime::new((), view, (500.0, 600.0), Env::new(false, 1.0));
    rt.frame(1.0 / 60.0);
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some("child")).unwrap().rect[2]
}

fn in_narrow_row(child: El) -> El {
    col().w_full().h_full().child(row().w(100.0).child(child.key("child").w(300.0).h(10.0)))
}

#[test]
fn shrink_keeps_an_explicit_min_width_set_before_or_after_it() {
    fn plain(_: &(), _: &mut Cx<()>) -> El { in_narrow_row(el().shrink()) }
    fn min_first(_: &(), _: &mut Cx<()>) -> El { in_narrow_row(el().min_w(160.0).shrink()) }
    fn min_after(_: &(), _: &mut Cx<()>) -> El { in_narrow_row(el().shrink().min_w(160.0)) }
    assert_eq!(child_width(plain), 100.0);
    assert_eq!(child_width(min_first), 160.0);
    assert_eq!(child_width(min_after), 160.0);
}
