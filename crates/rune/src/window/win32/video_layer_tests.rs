use super::*;

/// What a fake DirectComposition tree did: the visuals made (by surface handle) and removed.
#[derive(Default)]
struct Tree {
    made: std::cell::RefCell<Vec<usize>>,
    removed: std::cell::RefCell<Vec<u32>>,
}

impl Tree {
    /// `visual_for` with visuals that are just their number in the order they were made.
    fn show(&self, kept: &mut Option<(u32, u64)>, surface: LayerSurface) -> u32 {
        let make = |handle| {
            self.made.borrow_mut().push(handle);
            Ok(self.made.borrow().len() as u32)
        };
        *visual_for(kept, surface, make, |old| self.removed.borrow_mut().push(*old)).unwrap()
    }
}

#[test]
fn a_layer_made_again_gets_a_new_visual_even_with_the_closed_handles_value() {
    let (tree, mut kept) = (Tree::default(), None);
    // The layer the clip plays on.
    let first = LayerSurface { id: 1, handle: 0xbe4 };
    assert_eq!(tree.show(&mut kept, first), 1);
    assert_eq!(tree.show(&mut kept, first), 1, "the same layer keeps its visual");
    // A seek: the decoder closes that layer and makes a new one, and Windows hands its surface
    // the handle value it just closed.
    let second = LayerSurface { id: 2, handle: 0xbe4 };
    assert_eq!(tree.show(&mut kept, second), 2, "the visual for the closed surface stayed");
    assert_eq!(*tree.removed.borrow(), vec![1]);
    assert_eq!(*tree.made.borrow(), vec![0xbe4, 0xbe4]);
    assert_eq!(kept, Some((2, 2)));
}

#[test]
fn a_visual_that_cant_be_made_leaves_none_kept() {
    let mut kept = Some((1u32, 1));
    let removed = std::cell::Cell::new(false);
    let made = visual_for(&mut kept, LayerSurface { id: 2, handle: 8 }, |_| Err("no surface".to_string()), |_| removed.set(true));
    assert_eq!(made.err().as_deref(), Some("no surface"));
    assert!(removed.get() && kept.is_none());
}
