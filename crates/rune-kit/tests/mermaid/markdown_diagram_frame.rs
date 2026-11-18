//! [`Markdown::diagram_frame`] wraps each drawn diagram's card, and
//! [`MarkdownStyle::diagram_min_scale`] lets a wide one shrink until it fits.

use std::cell::RefCell;
use std::rc::Rc;

use rune::{Cx, El, Env, Runtime, col, el};
use rune_kit::{Markdown, MarkdownStyle};

const TWO: &str = "Code first:\n\n```rust\nfn main() {}\n```\n\n```mermaid\ngraph LR\n  A --> B\n```\n\nThen a second:\n\n```mermaid\nsequenceDiagram\n  A->>B: hi\n```\n\n```mermaid\nkanban\n  Todo\n```\n";

type Seen = Rc<RefCell<Vec<(usize, String, bool)>>>;

thread_local! {
    static SEEN: Seen = Rc::default();
}

fn view(s: &String, _: &mut Cx<String>) -> El {
    let md = Markdown::new(s.clone()).diagram_frame(|card, d| {
        SEEN.with(|seen| seen.borrow_mut().push((d.index, d.source.to_owned(), d.open)));
        col().key(format!("frame-{}", d.index)).child(card).child(el().key("open").size(20.0, 20.0))
    });
    col().w_full().child(md.render(&MarkdownStyle::neutral(true)))
}

#[test]
fn each_drawn_diagram_comes_to_the_frame_with_its_place_and_source() {
    SEEN.with(|s| s.borrow_mut().clear());
    let mut rt = Runtime::new(TWO.to_owned(), view, (600.0, 900.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let seen = SEEN.with(|s| s.borrow().clone());
    assert_eq!(seen.first().map(|s| (s.0, s.2)), Some((1, false)), "the code block counts, so the flowchart is fence 1: {seen:?}");
    assert!(seen.iter().any(|s| s.0 == 2 && s.1.starts_with("sequenceDiagram")), "{seen:?}");
    assert!(!seen.iter().any(|s| s.1.contains("kanban")), "a fence drawn as its source isn't framed: {seen:?}");
    let els = rt.elements();
    for i in [1, 2] {
        let frame = els.iter().find(|e| e.key.as_deref() == Some(&format!("frame-{i}"))).unwrap_or_else(|| panic!("frame-{i}"));
        assert!(els.iter().any(|e| e.key.as_deref() == Some("card") && e.path.starts_with(&frame.path)), "the card sits in frame-{i}");
        assert!(els.iter().any(|e| e.key.as_deref() == Some("open") && e.path.starts_with(&frame.path)), "with what the app added");
    }
}

const WIDE: &str = "```mermaid\nsequenceDiagram\n  participant B as Browser on a laptop\n  participant E as Edge cache in front\n  participant A as Auth service cluster\n  participant P as Payments API server\n  participant W as Billing worker pool\n  participant S as Search index shard\n  B->>E: a request arrives at the edge\n  E->>A: is this session token still valid?\n  P->>W: enqueue the invoice with the scope\n  W->>S: index the invoice\n```\n";

fn wide_view(min: &Option<f32>, _: &mut Cx<Option<f32>>) -> El {
    let style = MarkdownStyle::neutral(true).diagram_max_width(420.0);
    let style = match *min { Some(k) => style.diagram_min_scale(k), None => style };
    col().w(420.0).child(Markdown::new(WIDE).render(&style))
}

/// The card's and the drawing's rects, with `min_scale` set or left at its default.
fn wide(min_scale: Option<f32>) -> ([f32; 4], [f32; 4]) {
    let mut rt = Runtime::new(min_scale, wide_view, (600.0, 900.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let els = rt.elements();
    let rect = |k: &str| els.iter().find(|e| e.key.as_deref() == Some(k)).unwrap_or_else(|| panic!("{k}")).rect;
    (rect("card"), rect("diagram"))
}

#[test]
fn a_wide_diagram_fits_its_card_with_min_scale_zero() {
    let (card, diagram) = wide(None);
    assert!(diagram[2] > card[2], "by default it stops shrinking and overflows: card {card:?} diagram {diagram:?}");
    let (card, diagram) = wide(Some(0.0));
    assert!(diagram[0] >= card[0] && diagram[0] + diagram[2] <= card[0] + card[2], "it fits: card {card:?} diagram {diagram:?}");
}
