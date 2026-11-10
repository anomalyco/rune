//! X11 autorepeat filter for grabbed shortcuts. Without detectable autorepeat the server sends
//! each repeat as a KeyRelease and a KeyPress with the same timestamp; with it, as KeyPresses
//! with no release between. Either way only the first press of a hold fires, as on macOS and
//! Windows. A release is held back until the next event shows whether a repeat follows it.

use crate::machine::Effects;

/// The keys a hold has down, and the release waiting to see whether a repeat follows it.
#[derive(Default)]
pub(super) enum Held {
    /// No key is down and no release waits.
    #[default]
    Up,
    /// These keys are down, the first one having started the active grab.
    Down { keys: Vec<u8> },
    /// `code` was released at `time`; the next event says whether a repeat follows it.
    AwaitingRepeat { keys: Vec<u8>, code: u8, time: u32 },
}

/// A grabbed key the server reported.
#[derive(Clone, Copy, Debug)]
pub(super) enum Event {
    Press { code: u8, time: u32 },
    Release { code: u8, time: u32 },
}

/// What a transition asks its caller to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Effect {
    /// This press starts a hold, so the shortcut fires.
    Fire,
}

/// The hold after `event`, and what the caller carries out.
pub(super) fn step(state: &mut Held, event: Event) -> Effects<Effect> {
    match event {
        Event::Press { code, time } => {
            let mut keys = match std::mem::take(state) {
                Held::Up => Vec::new(),
                Held::Down { keys } => keys,
                // A release with this press's own timestamp is the repeat itself: the key stayed
                // down, and the release is spent rather than replayed.
                Held::AwaitingRepeat { keys, code: released, time: released_at } if (released, released_at) == (code, time) => {
                    *state = held(keys);
                    return Effects::none();
                }
                Held::AwaitingRepeat { keys, code: released, .. } => forget(keys, released),
            };
            if keys.contains(&code) {
                *state = held(keys);
                return Effects::none();
            }
            keys.push(code);
            *state = Held::Down { keys };
            Effects::one(Effect::Fire)
        }
        Event::Release { code, time } => {
            let keys = match std::mem::take(state) {
                Held::Up => Vec::new(),
                Held::Down { keys } => keys,
                Held::AwaitingRepeat { keys, code: released, .. } => forget(keys, released),
            };
            *state = Held::AwaitingRepeat { keys, code, time };
            Effects::none()
        }
    }
}

/// A hold with these keys down, or `Up` when there are none.
fn held(keys: Vec<u8>) -> Held {
    if keys.is_empty() { Held::Up } else { Held::Down { keys } }
}

/// The first key down started the active grab, which ends with its release; keys pressed
/// during the grab send their releases elsewhere, so they are forgotten with it.
fn forget(mut keys: Vec<u8>, released: u8) -> Vec<u8> {
    if keys.first() == Some(&released) { keys.clear(); } else { keys.retain(|&c| c != released); }
    keys
}

#[cfg(test)]
mod tests {
    use super::{Event, Held, step};

    /// Whether a press fires.
    fn press(r: &mut Held, code: u8, time: u32) -> bool {
        !step(r, Event::Press { code, time }).is_empty()
    }

    fn release(r: &mut Held, code: u8, time: u32) {
        assert!(step(r, Event::Release { code, time }).is_empty(), "a release never fires");
    }

    #[test]
    fn a_held_key_fires_once_in_either_autorepeat_mode() {
        let mut r = Held::default();
        // Release/press pairs with matching timestamps.
        assert!(press(&mut r, 38, 10));
        release(&mut r, 38, 40); assert!(!press(&mut r, 38, 40));
        release(&mut r, 38, 70); assert!(!press(&mut r, 38, 70));
        release(&mut r, 38, 90);
        assert!(press(&mut r, 38, 300), "a new press after a real release fires");
        // Detectable autorepeat: presses without releases.
        assert!(!press(&mut r, 38, 330));
        assert!(!press(&mut r, 38, 360));
        release(&mut r, 38, 400);
        assert!(press(&mut r, 39, 410), "another key fires and settles the earlier release");
        assert!(press(&mut r, 38, 420));
    }

    #[test]
    fn keys_pressed_during_a_grab_are_forgotten_when_the_grab_key_is_released() {
        let mut r = Held::default();
        assert!(press(&mut r, 65, 10)); // the grabbed chord's key starts the active grab
        assert!(press(&mut r, 38, 20)); // another key arrives through the grab
        release(&mut r, 65, 30); // the grab ends; 38's release goes to the focused window instead
        assert!(press(&mut r, 38, 400), "a later fresh press of the other key still fires");
        assert!(press(&mut r, 65, 410));
    }
}
