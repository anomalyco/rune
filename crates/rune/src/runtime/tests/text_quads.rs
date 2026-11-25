use super::*;
use parley::Layout;
use crate::text::{Ink, TextSystem};

fn label(s: &u32, _: &mut Cx<u32>) -> El {
    col().w_full().child(crate::el::text("a label that stays put").key("label")).child(crate::el::text(format!("token {s}")).key("body"))
}

/// `a` and `b` draw the same placements, whatever atlas pages their masks landed on.
fn draws_alike<S>(a: &Runtime<S>, b: &Runtime<S>) -> bool {
    a.instances.len() == b.instances.len()
        && a.instances.iter().zip(&b.instances).all(|(x, y)| {
            x.rect == y.rect && x.radii == y.radii && x.fill == y.fill && x.border == y.border
                && x.params == y.params && x.clip == y.clip && x.fade == y.fade
        })
}

fn instance_bytes<S>(rt: &Runtime<S>) -> Vec<u8> {
    bytemuck::cast_slice(&rt.instances).to_vec()
}

fn layout_of<'a, S>(rt: &'a Runtime<S>, key: &str) -> &'a Rc<Layout<Ink>> {
    rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some(key)).and_then(|n| n.text_layout.as_ref()?.single()).unwrap()
}

/// A commit rebuilds the view but leaves a still text's layout alone: its quads are copied, as
/// they are on a frame that commits nothing.
#[test]
fn a_still_text_across_a_commit_copies_its_quads() {
    let mut rt = Runtime::new(0u32, label, (400.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.frame(1.0 / 120.0);
    let copies = rt.paint.quads_copied();
    rt.update(|s, _| *s += 1);
    rt.frame(2.0 / 120.0);
    assert!(rt.paint.quads_copied() > copies, "the label copied its quads across a commit");
}

fn paragraph(_: &bool, _: &mut Cx<bool>) -> El {
    col().w_full().child(crate::el::text("Paragraph text long enough to wrap over several lines as the window narrows.").w_full().wrap().key("para"))
}

/// A text re-broken in place keeps its address but not its identity: the next frame draws its
/// new lines instead of copying the quads of its old ones.
#[test]
fn a_rebroken_layout_draws_its_own_quads() {
    let mut rt = Runtime::new(true, paragraph, (700.0, 900.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.frame(1.0 / 120.0);
    let (before, copies, kept) = (instance_bytes(&rt), rt.paint.quads_copied(), Rc::as_ptr(layout_of(&rt, "para")));
    rt.set_viewport((300.0, 900.0), 2.0);
    rt.frame(2.0 / 120.0);
    assert_eq!(Rc::as_ptr(layout_of(&rt, "para")), kept, "re-broken in place");
    assert_eq!(rt.paint.quads_copied(), copies, "a re-broken layout copies nothing");
    assert!(instance_bytes(&rt) != before, "wrapped at a new width, the text draws new quads");
}

fn note(s: &Vec<&'static str>, _: &mut Cx<Vec<&'static str>>) -> El {
    col().w_full().child(crate::el::text(s[0]).key("note"))
}

/// A layout replaced by another text's is a new layout with a new id: the frame after the swap
/// never copies the old text's quads, and draws what a fresh window does.
#[test]
fn a_replaced_layout_copies_nothing() {
    let mut rt = Runtime::new(vec!["Show"], note, (400.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.frame(1.0 / 120.0);
    let copies = rt.paint.quads_copied();
    rt.update(|s, _| s[0] = "Hide");
    rt.frame(2.0 / 120.0);
    assert_eq!(rt.paint.quads_copied(), copies, "a replaced layout copies nothing");

    let mut fresh = Runtime::new(vec!["Hide"], note, (400.0, 200.0), Env::new(true, 2.0));
    fresh.frame(0.0);
    fresh.frame(1.0 / 120.0);
    assert!(draws_alike(&rt, &fresh), "the swap draws the new text as a fresh window does");
}

/// An address handed to another layout is stamped anew: the quads drawn at that address before
/// are not copied, even though the address still holds a layout.
#[test]
fn a_restamped_address_copies_nothing() {
    let mut rt = Runtime::new(vec!["Show"], note, (400.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.frame(1.0 / 120.0);
    let copies = rt.paint.quads_copied();
    let layout = layout_of(&rt, "note").clone();
    rt.text.restamp_for_test(&layout);
    rt.frame(2.0 / 120.0);
    assert_eq!(rt.paint.quads_copied(), copies, "a restamped address copies nothing");
    assert!(!rt.instances.is_empty(), "the text still draws");
}

/// A new text system's ids never match the quads the one it replaced drew.
#[test]
fn a_replaced_text_system_copies_nothing() {
    let mut rt = Runtime::new(vec!["Show"], note, (400.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.frame(1.0 / 120.0);
    let copies = rt.paint.quads_copied();
    rt.text = TextSystem::default();
    rt.update(|s, _| s[0] = "Different words");
    rt.frame(2.0 / 120.0);
    assert_eq!(rt.paint.quads_copied(), copies, "the new text system's ids never match the old one's");
}

/// A layout whose stamp the id table dropped misses every paint until it is built again: a
/// lookup never stamps it back, so the cache can't refill itself frame after frame.
#[test]
fn a_layout_whose_stamp_went_copies_nothing() {
    let mut rt = Runtime::new(vec!["Show"], note, (400.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.frame(1.0 / 120.0);
    let copies = rt.paint.quads_copied();
    let layout = layout_of(&rt, "note").clone();
    rt.text.forget_stamp_for_test(&layout);
    rt.frame(2.0 / 120.0);
    assert_eq!(rt.paint.quads_copied(), copies, "a dropped stamp doesn't copy");
    rt.frame(3.0 / 120.0);
    assert_eq!(rt.paint.quads_copied(), copies, "a lookup never stamps the layout back");
    assert!(!rt.instances.is_empty(), "the text still draws");
}

/// Updating a text across commits drops its superseded cached quads instead of retaining every
/// recent value until the 64-frame sweep, while a still sibling continues copying its quads.
#[test]
fn a_changing_text_replaces_its_previous_quads() {
    let mut rt = Runtime::new(0u32, label, (400.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.frame(1.0 / 120.0);
    let copies = rt.paint.quads_copied();
    for step in 1..=40u32 {
        rt.update(|s, _| *s = step);
        rt.frame((step as f64 + 1.0) / 120.0);
    }
    assert!(rt.paint.text_quads.len() <= 2, "expected at most one entry per text node, got {}", rt.paint.text_quads.len());
    assert!(rt.paint.text_quads.bytes() <= paint::quads::MAX_TEXT_QUAD_BYTES, "tracked bytes stay within budget");
    assert!(rt.paint.quads_copied() > copies, "still sibling still copies its quads across commits");

    let mut fresh = Runtime::new(40u32, label, (400.0, 200.0), Env::new(true, 2.0));
    fresh.frame(0.0);
    fresh.frame(1.0 / 120.0);
    assert!(draws_alike(&rt, &fresh), "final frame matches a fresh runtime");
}

/// Publishing new values through `LiveText` replaces the node's cached quads in place rather than
/// accumulating one entry per published value.
#[test]
fn a_changing_live_text_replaces_its_previous_quads() {
    struct LiveState {
        reading: crate::LiveText,
    }
    fn live_view(s: &LiveState, _: &mut Cx<LiveState>) -> El {
        col().w_full()
            .child(crate::el::text("pressure").key("tag"))
            .child(crate::el::text("0.00 bar").key("val").live_text(&s.reading))
    }

    let mut rt = Runtime::new(
        LiveState { reading: crate::LiveText::new("0.00 bar") },
        live_view,
        (400.0, 200.0),
        Env::new(true, 2.0),
    );
    rt.frame(0.0);
    rt.frame(1.0 / 120.0);
    for step in 1..=40u32 {
        rt.state.reading.set(format!("{step}.25 bar"));
        rt.frame((step as f64 + 1.0) / 120.0);
    }
    assert!(rt.paint.text_quads.len() <= 2, "expected at most one entry per node, got {}", rt.paint.text_quads.len());
    assert!(rt.paint.text_quads.bytes() <= paint::quads::MAX_TEXT_QUAD_BYTES, "tracked bytes stay within budget");

    let mut fresh = Runtime::new(
        LiveState { reading: crate::LiveText::new("40.25 bar") },
        live_view,
        (400.0, 200.0),
        Env::new(true, 2.0),
    );
    fresh.frame(0.0);
    fresh.frame(1.0 / 120.0);
    assert!(draws_alike(&rt, &fresh), "final live_text frame matches a fresh runtime");
}

/// A multi-paragraph text node caches each visible paragraph under its own piece index so sibling
/// paragraphs do not evict one another within a frame.
#[test]
fn multi_paragraph_field_caches_each_visible_paragraph() {
    fn multi(_: &(), _: &mut Cx<()>) -> El {
        col().w_full().child(
            crate::el::text("first paragraph\nsecond paragraph\nthird paragraph")
                .w_full()
                .wrap()
                .by_line()
                .key("multi"),
        )
    }

    let mut rt = Runtime::new((), multi, (400.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    assert_eq!(rt.paint.text_quads.len(), 3, "each paragraph keeps its own quad cache slot");
    let copies = rt.paint.quads_copied();
    // Force a paint pass without changing the text so paint_content runs again.
    rt.invalidate();
    rt.frame(1.0 / 120.0);
    assert!(rt.paint.quads_copied() >= copies + 3, "all three paragraphs copied their quads on the next paint");
}

/// Enforcing tight entry and byte limits evicts the least-recently-used entries and skips entries
/// larger than the byte budget.
#[test]
fn entry_and_byte_caps_evict_least_recently_used_quads() {
    fn many(count: &usize, _: &mut Cx<usize>) -> El {
        let mut c = col().w_full();
        for i in 0..*count {
            c = c.child(crate::el::text(format!("sensor {i:02}: 123.45 bar")).key(format!("s{i}")));
        }
        c
    }

    let mut rt = Runtime::new(12usize, many, (400.0, 800.0), Env::new(true, 2.0));
    rt.paint.text_quads.set_limits(4, usize::MAX);
    rt.frame(0.0);
    assert!(rt.paint.text_quads.len() <= 4, "entry cap enforced: got {}", rt.paint.text_quads.len());
    assert!(rt.paint.text_quads.len() >= 1, "cache retains newest entries within entry cap");

    // Measure a single cached entry's footprint, then set a byte cap holding at most two entries.
    let single_bytes = rt.paint.text_quads.bytes() / rt.paint.text_quads.len();
    let two_cap = single_bytes * 2 + single_bytes / 2;
    rt.paint.text_quads.set_limits(usize::MAX, two_cap);
    rt.invalidate();
    rt.frame(1.0 / 120.0);
    assert!(rt.paint.text_quads.bytes() <= two_cap, "byte cap enforced: {} > {}", rt.paint.text_quads.bytes(), two_cap);
    assert!(rt.paint.text_quads.len() <= 2, "byte cap bounded entry count to {}", rt.paint.text_quads.len());

    // A single entry larger than the budget is not cached and never pushes bytes past the cap.
    let tiny_cap = single_bytes / 2;
    rt.paint.text_quads.set_limits(16, tiny_cap);
    rt.invalidate();
    rt.frame(2.0 / 120.0);
    assert_eq!(rt.paint.text_quads.len(), 0, "oversized entries are skipped");
    assert_eq!(rt.paint.text_quads.bytes(), 0, "byte count stays zero when all entries exceed cap");
}

/// Unmounted text nodes stop touching their entries and are reclaimed by the 64-frame sweep along
/// with their tracked byte count.
#[test]
fn unused_entries_are_pruned_after_64_frames() {
    fn rows(count: &usize, _: &mut Cx<usize>) -> El {
        let mut c = col().w_full();
        for i in 0..*count {
            c = c.child(crate::el::text(format!("row {i}")).key(format!("r{i}")));
        }
        c
    }

    let mut rt = Runtime::new(4usize, rows, (400.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    assert_eq!(rt.paint.text_quads.len(), 4);
    let four_bytes = rt.paint.text_quads.bytes();
    assert!(four_bytes > 0);

    rt.update(|n, _| *n = 1);
    for f in 1..=128u32 {
        rt.invalidate();
        rt.frame(f as f64 / 120.0);
    }
    assert_eq!(rt.paint.text_quads.len(), 1, "unmounted nodes pruned after 64 unused frames");
    assert!(rt.paint.text_quads.bytes() > 0 && rt.paint.text_quads.bytes() < four_bytes, "tracked bytes decreased after prune");
}
