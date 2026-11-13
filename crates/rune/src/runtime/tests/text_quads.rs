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

fn layout_of<'a>(rt: &'a Runtime<bool>) -> &'a Rc<Layout<Ink>> {
    rt.nodes.iter().find(|n| n.key.as_deref() == Some("para")).and_then(|n| n.text_layout.as_ref()?.single()).unwrap()
}

/// A text re-broken in place keeps its address but not its identity: the next frame draws its
/// new lines instead of copying the quads of its old ones.
#[test]
fn a_rebroken_layout_draws_its_own_quads() {
    let mut rt = Runtime::new(true, paragraph, (700.0, 900.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.frame(1.0 / 120.0);
    let (before, copies, kept) = (instance_bytes(&rt), rt.quads_copied, Rc::as_ptr(layout_of(&rt)));
    rt.set_viewport((300.0, 900.0), 2.0);
    rt.frame(2.0 / 120.0);
    assert_eq!(Rc::as_ptr(layout_of(&rt)), kept, "re-broken in place");
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

fn secret(ts: &mut TextSystem, text: &str) -> Rc<Layout<Ink>> {
    ts.layout_blocks_inner(text, &[], &[], &[], crate::TextStyle::new(14.0, 400.0), 2.0, None, parley::Alignment::Start, false, false)
}

/// An address is no identity: a layout built where a freed one stood is stamped anew.
#[test]
fn a_layout_built_where_a_freed_one_stood_gets_a_new_id() {
    let mut ts = TextSystem::default();
    let first = secret(&mut ts, "Show");
    let (id, address) = (ts.layout_id(&first), Rc::as_ptr(&first) as usize);
    drop(first);
    let second = (0..64)
        .map(|_| secret(&mut ts, "Hide"))
        .find(|l| Rc::as_ptr(l) as usize == address)
        .expect("a freed layout's address is handed to the next one");
    assert_ne!(ts.layout_id(&second), id, "the freed layout's id never comes back");
}
