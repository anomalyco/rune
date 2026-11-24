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

fn test_entry(phase: Phase) -> Entry {
    let player = crate::Player::file("missing.mp4");
    let video = player.surface().video().expect("player surface carries video");
    Entry {
        key: player.surface().key(),
        video,
        phase,
        last: None,
        isolated: false,
        visual: None,
        logged: None,
    }
}

#[test]
fn needs_frame_flags_placing_pending_alpha_and_stopped_layered_clips() {
    let mut layers = VideoLayers::default();
    assert!(!layers.needs_frame(), "empty layers need no frame");

    layers.pending_alpha = Some(true);
    assert!(layers.needs_frame(), "pending swap-chain alpha change needs a frame");
    layers.pending_alpha = None;

    layers.entries.push(test_entry(Phase::Drawn));
    assert!(!layers.needs_frame(), "a Drawn clip needs no extra layer frame");

    layers.entries[0].phase = Phase::Placing { size: (640, 360) };
    assert!(layers.needs_frame(), "a Placing clip must run VideoLayers::frame");

    layers.entries[0].phase = Phase::Layered { size: (640, 360), at: (0, 0) };
    assert!(
        !layers.needs_frame_with(|_, size| size == (640, 360)),
        "a Layered clip that is still running and showing at its size needs no frame"
    );
    assert!(
        layers.needs_frame_with(|_, _| false),
        "a Layered clip that stopped running or showing needs a frame to unlayer"
    );
    // The real Player::file handle is idle (not playing), so needs_frame() itself must return true.
    assert!(layers.needs_frame(), "a layered clip that is no longer playing marks needs_frame");
}
