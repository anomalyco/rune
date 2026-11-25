use crate::{Cx, El, Env, Runtime, ScrollSpec, col, el};

#[derive(Default)]
struct S {
    rows: Vec<u32>,
    layout: Option<Option<[f32; 4]>>,
    presented: Option<Option<([f32; 4], f32)>>,
}

/// Twenty 20-point rows in a 100-point scrolling viewport.
fn view(s: &S, _: &mut Cx<S>) -> El {
    let rows = col().key("rows").w_full().children(s.rows.iter().map(|n| el().key(format!("row-{n}")).w_full().h(20.0)));
    col().size(100.0, 100.0).child(col().key("scroll").size(100.0, 100.0).scroll(ScrollSpec { stick_bottom: false }).child(rows))
}

fn runtime() -> Runtime<S> {
    let mut rt = Runtime::new(S { rows: (0..20).collect(), ..S::default() }, view, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn read(rt: &mut Runtime<S>, key: &'static str, remove: Option<u32>) {
    rt.update(move |s, tx| {
        if let Some(n) = remove { s.rows.retain(|&r| r != n); }
        tx.read_layout(key, |s: &mut S, rect, _| s.layout = Some(rect));
        tx.read_presented(key, |s: &mut S, drawn, _| s.presented = Some(drawn));
    });
    rt.frame(0.0);
}

#[test]
fn a_handler_reads_a_row_it_removes_where_it_stood() {
    let mut rt = runtime();
    let before = rt.presented("row-3");
    read(&mut rt, "row-3", Some(3));
    assert!(!rt.state.rows.contains(&3), "the handler removed the row from the model");
    assert_eq!(rt.state.layout, Some(Some([0.0, 60.0, 100.0, 20.0])));
    assert_eq!(rt.state.presented, Some(before));
    // A removed row is drawn until its exit ends (`Runtime::presented` answers "exiting or not"):
    // after the commit it is on its way out, and gone once the exit is over.
    assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("row-3") && e.exiting), "the row is leaving after the commit");
    for _ in 0..180 {
        if rt.presented("row-3").is_none() { break; }
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.presented("row-3").is_none(), "the row is gone once its exit ends");
}

#[test]
fn an_instant_removal_reads_where_it_stood_and_is_unmounted_from_presented() {
    let mut rt = runtime();
    let before = rt.presented("row-3");
    rt.update(|s, tx| {
        s.rows.retain(|&r| r != 3);
        tx.instant();
        tx.read_layout("row-3", |s: &mut S, rect, _| s.layout = Some(rect));
        tx.read_presented("row-3", |s: &mut S, drawn, _| s.presented = Some(drawn));
    });
    rt.frame(0.0);
    assert!(rt.find("row-3").is_none(), "the row left the live tree after the commit");
    assert!(rt.presented("row-3").is_none(), "an instant-removed unmounted row must not be reported as presented");
    assert_eq!(rt.state.layout, Some(Some([0.0, 60.0, 100.0, 20.0])));
    assert_eq!(rt.state.presented, Some(before));
}

#[test]
fn layout_is_read_after_scrolling_and_key_paths_resolve() {
    let mut rt = runtime();
    rt.update(|_, tx| tx.scroll_to_instant("scroll", 0.0, 40.0));
    rt.frame(0.0);
    let drawn = rt.presented("rows/row-5");
    read(&mut rt, "rows/row-5", None);
    assert_eq!(rt.state.layout, Some(Some([0.0, 60.0, 100.0, 20.0])), "where the scrolled window shows it");
    assert_eq!(rt.state.presented, Some(drawn));
}

#[test]
fn a_missing_key_reads_none() {
    let mut rt = runtime();
    read(&mut rt, "nowhere", None);
    assert_eq!(rt.state.layout, Some(None));
    assert_eq!(rt.state.presented, Some(None));
}
