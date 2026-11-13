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
    rt.nodes.iter().find(|n| n.key.as_deref() == Some(key)).and_then(|n| n.text_layout.as_ref()?.single()).unwrap()
}

/// A commit rebuilds the view but leaves a still text's layout alone: its quads are copied, as
/// they are on a frame that commits nothing.
#[test]
fn a_still_text_across_a_commit_copies_its_quads() {
    let mut rt = Runtime::new(0u32, label, (400.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.frame(1.0 / 120.0);
    let copies = rt.quads_copied;
    rt.update(|s, _| *s += 1);
    rt.frame(2.0 / 120.0);
    assert!(rt.quads_copied > copies, "the label copied its quads across a commit");
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
    let (before, copies, kept) = (instance_bytes(&rt), rt.quads_copied, Rc::as_ptr(layout_of(&rt, "para")));
    rt.set_viewport((300.0, 900.0), 2.0);
    rt.frame(2.0 / 120.0);
    assert_eq!(Rc::as_ptr(layout_of(&rt, "para")), kept, "re-broken in place");
    assert_eq!(rt.quads_copied, copies, "a re-broken layout copies nothing");
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
    let copies = rt.quads_copied;
    rt.update(|s, _| s[0] = "Hide");
    rt.frame(2.0 / 120.0);
    assert_eq!(rt.quads_copied, copies, "a replaced layout copies nothing");

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
    let copies = rt.quads_copied;
    let layout = layout_of(&rt, "note").clone();
    rt.text.restamp_for_test(&layout);
    rt.frame(2.0 / 120.0);
    assert_eq!(rt.quads_copied, copies, "a restamped address copies nothing");
    assert!(!rt.instances.is_empty(), "the text still draws");
}

/// A new text system's ids never match the quads the one it replaced drew.
#[test]
fn a_replaced_text_system_copies_nothing() {
    let mut rt = Runtime::new(vec!["Show"], note, (400.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.frame(1.0 / 120.0);
    let copies = rt.quads_copied;
    rt.text = TextSystem::default();
    rt.update(|s, _| s[0] = "Different words");
    rt.frame(2.0 / 120.0);
    assert_eq!(rt.quads_copied, copies, "the new text system's ids never match the old one's");
}

/// A layout whose stamp the id table dropped misses every paint until it is built again: a
/// lookup never stamps it back, so the cache can't refill itself frame after frame.
#[test]
fn a_layout_whose_stamp_went_copies_nothing() {
    let mut rt = Runtime::new(vec!["Show"], note, (400.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.frame(1.0 / 120.0);
    let copies = rt.quads_copied;
    let layout = layout_of(&rt, "note").clone();
    rt.text.forget_stamp_for_test(&layout);
    rt.frame(2.0 / 120.0);
    assert_eq!(rt.quads_copied, copies, "a dropped stamp doesn't copy");
    rt.frame(3.0 / 120.0);
    assert_eq!(rt.quads_copied, copies, "a lookup never stamps the layout back");
    assert!(!rt.instances.is_empty(), "the text still draws");
}
