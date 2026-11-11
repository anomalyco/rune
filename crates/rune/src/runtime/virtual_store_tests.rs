//! A commit's kept offscreen rows go before that commit's row builders run
//! (`commit/measure.rs`), so a row holding a resource doesn't keep it through building its
//! replacement.

use std::cell::Cell;
use std::rc::Rc;

use super::*;
use crate::{GrowthSpec, ScrollSpec, col, el};

const COUNT: usize = 30;
const ROW_H: f32 = 40.0;
/// Far below the 200 pt viewport, so this row is kept, not mounted.
const HELD_ROW: usize = 25;

struct List {
    /// Build rows with [`El::virtual_rows`] instead of children.
    indexed: bool,
    /// A resource the kept row holds while it lives; only the row clones it.
    held: Rc<()>,
    /// Row builders that found the resource still alive.
    saw_held: Cell<usize>,
}

impl List {
    fn new() -> Self {
        List { indexed: false, held: Rc::new(()), saw_held: Cell::new(0) }
    }
}

fn list(s: &List, _: &mut Cx<List>) -> El {
    let stack = if s.indexed {
        col().key("stack").w_full().growth(GrowthSpec::default()).virtual_rows(
            COUNT,
            ROW_H,
            |_: &List, i| format!("r{i}").into(),
            |s: &List, i| {
                if Rc::strong_count(&s.held) > 1 {
                    s.saw_held.set(s.saw_held.get() + 1);
                }
                el().key(format!("r{i}")).w_full().h(ROW_H).fill(Color::WHITE)
            },
        )
    } else {
        let held = s.held.clone();
        col().key("stack").w_full().growth(GrowthSpec::default()).virtualize(ROW_H).children((0..COUNT).map(|i| {
            let row = el().key(format!("r{i}")).w_full().h(ROW_H).fill(Color::WHITE);
            if i == HELD_ROW {
                let held = held.clone();
                row.on_press(move |_: &mut List| { let _ = &held; })
            } else {
                row
            }
        }))
    };
    col().w_full().h_full()
        .child(col().key("scroll").w_full().h(200.0).scroll(ScrollSpec { stick_bottom: false }).child(stack))
}

#[test]
fn stored_rows_drop_before_the_row_builders_run() {
    let mut rt = Runtime::new(List::new(), list, (400.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    while rt.frame(1.0 / 120.0) {}
    let kept = &rt.virtuals["#0/scroll/stack"].selection.stored;
    assert!(kept.contains_key(&HELD_ROW), "a row below the viewport is kept: {:?}", kept.keys().collect::<Vec<_>>());
    assert_eq!(Rc::strong_count(&rt.state.held), 2, "the kept row holds the resource");
    rt.update(|s, _| s.indexed = true);
    while rt.frame(1.0 / 120.0) {}
    assert_eq!(rt.state.saw_held.get(), 0, "the kept rows went before any builder ran");
}
