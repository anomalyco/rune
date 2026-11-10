//! A pointer drag held from its press to its release, with what the press started. Its release
//! can be lost: the dragged element unmounts, or a modal takes the pointer, and no `Phase::Up`
//! comes. A new press then replaces the stale grip, and so does a pointer move the drag never
//! heard, so nothing stays lifted, pressed or resizing.
//!
//! The runtime hands every move of a held pointer to the drag (`Phase::Move`) and to whatever
//! observes the pointer there (`on_pointer_move`), in either order and to any number of
//! observers. So one move the drag hasn't heard yet is normal; a second at another point means
//! the drag is gone.

/// `Idle`, or `Held` since a press, with `data`. `heard` is the last point the drag saw;
/// `unheard` a pointer move at another point that it hasn't seen yet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Grip<T> {
    Idle,
    Held { data: T, heard: Option<[f32; 2]>, unheard: Option<[f32; 2]> },
}

impl<T> Default for Grip<T> {
    fn default() -> Self {
        Grip::Idle
    }
}

impl<T> Grip<T> {
    pub(crate) fn get(&self) -> Option<&T> {
        match self {
            Grip::Held { data, .. } => Some(data),
            Grip::Idle => None,
        }
    }

    pub(crate) fn is_held(&self) -> bool {
        matches!(self, Grip::Held { .. })
    }

    /// A press at `at` (window points) starts `data`. Returns what a grip still held had: its
    /// release was lost.
    pub(crate) fn press(&mut self, data: T, at: Option<[f32; 2]>) -> Option<T> {
        self.replace(Grip::Held { data, heard: at, unheard: None })
    }

    /// The drag's own move at `at`: what the press started, while held.
    pub(crate) fn moved(&mut self, at: [f32; 2]) -> Option<&mut T> {
        match self {
            Grip::Held { data, heard, unheard } => {
                (*heard, *unheard) = (Some(at), None);
                Some(data)
            }
            Grip::Idle => None,
        }
    }

    /// The held grip now carries `data`; an idle one starts holding it (a move came with no
    /// press seen).
    pub(crate) fn hold(&mut self, data: T) {
        match self {
            Grip::Held { data: held, .. } => *held = data,
            Grip::Idle => *self = Grip::Held { data, heard: None, unheard: None },
        }
    }

    /// The release, dropped or cancelled: what the press started.
    pub(crate) fn release(&mut self) -> Option<T> {
        self.replace(Grip::Idle)
    }

    /// The pointer moved to `at`, as an `on_pointer_move` sees it. Returns what the grip held
    /// when this shows its release was lost.
    pub(crate) fn pointer(&mut self, at: [f32; 2]) -> Option<T> {
        let Grip::Held { heard, unheard, .. } = self else { return None };
        if *heard == Some(at) {
            return None;
        }
        match *unheard {
            Some(u) if u != at => self.release(),
            _ => {
                *unheard = Some(at);
                None
            }
        }
    }

    /// The window lost focus, or the gesture was dropped: what the grip held.
    pub(crate) fn lose(&mut self) -> Option<T> {
        self.release()
    }

    fn replace(&mut self, next: Grip<T>) -> Option<T> {
        match std::mem::replace(self, next) {
            Grip::Held { data, .. } => Some(data),
            Grip::Idle => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_live_drag_hears_each_move_in_either_order() {
        let mut g = Grip::default();
        assert_eq!(g.press(7, Some([0.0, 0.0])), None);
        for x in 1..5 {
            let at = [x as f32, 0.0];
            // The observer first, as the runtime queues them, then the drag.
            assert_eq!(g.pointer(at), None);
            assert_eq!(g.pointer(at), None, "a second observer at the same point");
            assert_eq!(g.moved(at).copied(), Some(7));
        }
        for x in 5..9 {
            let at = [x as f32, 0.0];
            assert_eq!(g.moved(at).copied(), Some(7));
            assert_eq!(g.pointer(at), None, "the drag first");
        }
        assert_eq!(g.release(), Some(7));
        assert_eq!(g.release(), None, "one release");
    }

    #[test]
    fn a_move_the_drag_never_hears_lets_go() {
        let mut g = Grip::default();
        g.press(7, Some([0.0, 0.0]));
        assert_eq!(g.pointer([1.0, 0.0]), None, "one move the drag hasn't heard yet");
        assert_eq!(g.pointer([2.0, 0.0]), Some(7), "the second: its release was lost");
        assert!(!g.is_held());
        assert_eq!(g.pointer([3.0, 0.0]), None, "idle, a move is nothing");
        assert_eq!(g.moved([3.0, 0.0]), None, "nor is a stray drag move");
    }

    #[test]
    fn a_new_press_replaces_a_grip_whose_release_was_lost() {
        let mut g = Grip::default();
        g.press(1, Some([0.0, 0.0]));
        assert_eq!(g.press(2, Some([9.0, 9.0])), Some(1));
        assert_eq!(g.get(), Some(&2));
        assert_eq!(g.lose(), Some(2));
        assert_eq!(g.lose(), None);
    }
}
