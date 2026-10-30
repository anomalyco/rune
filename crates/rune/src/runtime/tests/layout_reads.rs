//! Layout reads find their element whether a few reads scan the tree or many index it.

use super::*;
use crate::el::row;

struct Cells { width: f32, reads: usize, seen: std::rc::Rc<std::cell::RefCell<Vec<Option<[f32; 2]>>>> }

fn view(s: &Cells, cx: &mut Cx<Cells>) -> El {
    let mut seen: Vec<Option<[f32; 2]>> = (0..s.reads).map(|i| cx.layout_size(&format!("cell{i}"))).collect();
    seen.push(cx.layout_size("panel/cell3"));
    seen.push(cx.layout_size("missing"));
    *s.seen.borrow_mut() = seen;
    let cells = |w: f32| (0..12).map(move |i| el().key(format!("cell{i}")).w(w + i as f32).h(10.0));
    col()
        .child(row().key("other").children(cells(1.0)))
        .child(row().key("panel").children(cells(s.width)))
}

#[test]
fn few_and_many_reads_see_the_same_rects() {
    for reads in [2, 12] {
        let seen = std::rc::Rc::default();
        let mut rt = Runtime::new(Cells { width: 20.0, reads, seen: std::rc::Rc::clone(&seen) }, view, (400.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        rt.update(|s, _| s.width = 30.0);
        for _ in 0..3 { rt.frame(1.0 / 60.0); }
        let seen = seen.borrow();
        assert_eq!(seen.len(), reads + 2);
        for (i, size) in seen[..reads].iter().enumerate() {
            assert_eq!(*size, Some([1.0 + i as f32, 10.0]), "{reads} reads: cell{i} is the first one keyed so");
        }
        assert_eq!(seen[reads], Some([33.0, 10.0]), "{reads} reads: a scoped selector finds the panel's cell");
        assert_eq!(seen[reads + 1], None, "{reads} reads");
    }
}
