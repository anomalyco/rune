//! `El::data` read back through `Picked::data`: one value per type, memo reuse, virtual rows,
//! and `Tx::element_at`.

use std::borrow::Cow;

use crate::{Cx, El, Env, GrowthSpec, Runtime, ScrollSpec, col, el, memo};

#[derive(Debug, PartialEq)]
struct Folder(u32);

#[derive(Debug, PartialEq)]
struct Tag(&'static str);

#[test]
fn a_second_value_of_a_type_replaces_the_first_and_other_types_sit_beside_it() {
    let view = |_: &(), _: &mut Cx<()>| {
        col().size(200.0, 40.0).child(
            el().key("zone").size(200.0, 40.0).data(Folder(1)).data(Tag("tile")).data(Folder(2)),
        )
    };
    let mut rt = Runtime::new((), view, (200.0, 40.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let hit = rt.element_at(10.0, 10.0).expect("zone is hit");
    assert_eq!(hit.key(), Some("zone"));
    assert_eq!(hit.data::<Folder>(), Some(&Folder(2)));
    assert_eq!(hit.data::<Tag>(), Some(&Tag("tile")));
    assert_eq!(hit.data::<u32>(), None);
}

#[test]
fn data_is_read_from_the_nearest_ancestor_that_holds_the_type() {
    let view = |_: &(), _: &mut Cx<()>| {
        col().size(200.0, 40.0).data(Folder(1)).data(Tag("window")).child(
            col().size(200.0, 40.0).data(Folder(9)).child(el().key("label").size(100.0, 20.0)),
        )
    };
    let mut rt = Runtime::new((), view, (200.0, 40.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let hit = rt.element_at(10.0, 10.0).expect("label is hit");
    assert_eq!(hit.key(), Some("label"));
    assert_eq!(hit.data::<Folder>(), Some(&Folder(9)));
    assert_eq!(hit.data::<Tag>(), Some(&Tag("window")));
}

#[derive(Default)]
struct MemoState {
    version: u32,
    folder: u32,
    other: u32,
}

fn memo_view(s: &MemoState, _: &mut Cx<MemoState>) -> El {
    let folder = s.folder;
    col().size(200.0, 80.0)
        .child(memo("zone", s.version, move || {
            col().size(200.0, 40.0).data(Folder(folder)).child(el().key("zone-label").size(100.0, 20.0))
        }))
        .child(el().key("other").size(200.0, 40.0).data(Tag(if s.other == 0 { "a" } else { "b" })))
}

#[test]
fn data_survives_memo_reuse_and_follows_a_new_version() {
    let mut rt = Runtime::new(MemoState { version: 0, folder: 3, other: 0 }, memo_view, (200.0, 80.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(rt.element_at(10.0, 10.0).unwrap().data::<Folder>(), Some(&Folder(3)));

    // Something outside the memo changes: the memo's cached subtree is reused, and its value
    // with it, even though the state now holds another folder under the same version.
    rt.update(|s, _| {
        s.other = 1;
        s.folder = 4;
    });
    rt.frame(0.016);
    assert_eq!(rt.element_at(10.0, 10.0).unwrap().data::<Folder>(), Some(&Folder(3)));
    assert_eq!(rt.element_at(10.0, 50.0).unwrap().data::<Tag>(), Some(&Tag("b")));

    // A new version rebuilds the subtree with the new value.
    rt.update(|s, _| s.version = 1);
    rt.frame(0.016);
    assert_eq!(rt.element_at(10.0, 10.0).unwrap().data::<Folder>(), Some(&Folder(4)));
}

#[test]
fn data_on_virtual_rows_is_read_from_a_hit() {
    let view = |_: &(), _: &mut Cx<()>| {
        col().key("scroller").size(200.0, 90.0).scroll(ScrollSpec::default()).child(
            col().key("vlist").w_full().growth(GrowthSpec::default()).virtual_rows(
                50,
                30.0,
                |_: &(), i| Cow::Owned(format!("row-{i}")),
                |_: &(), i| el().w_full().h(30.0).data(Folder(i as u32)),
            ),
        )
    };
    let mut rt = Runtime::new((), view, (200.0, 90.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.frame(0.016);
    let hit = rt.element_at(10.0, 45.0).expect("a row is hit");
    assert_eq!(hit.data::<Folder>(), Some(&Folder(1)), "hit {hit:?}");
    assert_eq!(rt.element_at(10.0, 75.0).unwrap().data::<Folder>(), Some(&Folder(2)));
}

#[test]
fn tx_element_at_delivers_data_to_its_callback() {
    #[derive(Default)]
    struct State {
        seen: Option<u32>,
        calls: usize,
    }
    let view = |_: &State, _: &mut Cx<State>| {
        col().size(200.0, 80.0)
            .child(el().key("zone").size(200.0, 40.0).data(Folder(5)))
            .child(el().key("button").size(200.0, 40.0).on_press_tx(|_: &mut State, tx| {
                tx.element_at::<State>(10.0, 10.0, |s, picked, _| {
                    s.calls += 1;
                    s.seen = picked.and_then(|p| p.data::<Folder>().map(|f| f.0));
                });
            }))
    };
    let mut rt = Runtime::new(State::default(), view, (200.0, 80.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let (x, y) = rt.find("button").unwrap();
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.016);
    assert_eq!(rt.state.calls, 1);
    assert_eq!(rt.state.seen, Some(5));
}
