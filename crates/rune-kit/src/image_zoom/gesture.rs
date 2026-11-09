//! The preview's gestures (`@opencode/ui` `image-zoom-machine.ts`): whether an eased zoom is
//! gliding, and the press under way. Scale and offsets live on springs in the driver
//! ([`super::glide`]), not here, so a frame of a glide or a pan changes no state.

use rune::machine::Effects;

/// The press under way. A `BackdropPress` began on the backdrop around the image and stayed put,
/// so the click that ends it closes the preview. A pan's click lands where it began, so
/// `on_backdrop` says whether that was the backdrop, and `moved` whether it has moved: the click
/// that ends a moved pan doesn't close the preview.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Gesture {
    Idle,
    BackdropPress,
    Panning {
        on_backdrop: bool,
        moved: bool,
        /// Where the pan last saw its pointer.
        cursor: [f32; 2],
    },
}

/// `Gliding` is an eased zoom under way, stepped from the frame clock since `last`. A press
/// doesn't stop a glide, so a double-click's third click leaves it running; the hand moving does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum State {
    Still(Gesture),
    Gliding { gesture: Gesture, last: f64 },
}

impl Default for State {
    fn default() -> State { State::Still(Gesture::Idle) }
}

/// `zoomed` is whether the image shows above fit once the event's change applies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    /// A primary press: on the body the image zooms in (`on_body`), and on the backdrop around
    /// the image rather than the image itself (`on_backdrop`).
    PointerDown { on_body: bool, on_backdrop: bool, zoomed: bool, at: [f32; 2] },
    PointerMoved { at: [f32; 2] },
    PointerUp,
    /// The press ended unreleased (a cancelled drag).
    PointerCancel,
    /// The click a release makes, landing on the backdrop or not.
    Click { on_backdrop: bool },
    /// Pinch, wheel, drag or a resize set the view directly.
    Follow { zoomed: bool },
    GlideStart { now: f64 },
    GlideSettled { zoomed: bool },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    /// Move the view by `[dx, dy]` points, following the pan's pointer.
    Pan([f32; 2]),
    /// Step the glide from the frame clock, a frame at a time.
    Frames,
    Close,
}

pub fn step(state: &mut State, event: Event) -> Effects<Effect> {
    let (gesture, gliding) = match *state {
        State::Still(g) => (g, None),
        State::Gliding { gesture, last } => (gesture, Some(last)),
    };
    let set = |state: &mut State, g: Gesture| {
        *state = match gliding {
            Some(last) => State::Gliding { gesture: g, last },
            None => State::Still(g),
        };
    };
    let pan = matches!(gesture, Gesture::Panning { .. });
    match event {
        Event::PointerDown { on_body, on_backdrop, zoomed, at } => {
            if zoomed && on_body {
                set(state, Gesture::Panning { on_backdrop, moved: false, cursor: at });
            } else {
                set(state, if on_backdrop { Gesture::BackdropPress } else { Gesture::Idle });
            }
            Effects::none()
        }
        Event::PointerMoved { at } => {
            let Gesture::Panning { cursor, .. } = gesture else { return Effects::none() };
            let delta = [at[0] - cursor[0], at[1] - cursor[1]];
            if delta == [0.0, 0.0] { return Effects::none(); }
            set(state, Gesture::Panning { on_backdrop: false, moved: true, cursor: at });
            Effects::one(Effect::Pan(delta))
        }
        Event::PointerUp => {
            if let Gesture::Panning { on_backdrop, moved, .. } = gesture {
                set(state, if !moved && on_backdrop { Gesture::BackdropPress } else { Gesture::Idle });
            }
            Effects::none()
        }
        Event::PointerCancel => {
            if pan { set(state, Gesture::Idle); }
            Effects::none()
        }
        Event::Click { on_backdrop } => {
            if gesture != Gesture::BackdropPress { return Effects::none(); }
            set(state, Gesture::Idle);
            if on_backdrop { Effects::one(Effect::Close) } else { Effects::none() }
        }
        // An eased zoom takes over from a pan, so a pan never outlives a zoom back to fit.
        Event::GlideStart { now } => {
            let gesture = if pan { Gesture::Idle } else { gesture };
            let started = gliding.is_none();
            *state = State::Gliding { gesture, last: gliding.unwrap_or(now) };
            if started { Effects::one(Effect::Frames) } else { Effects::none() }
        }
        // The hand takes over from an eased zoom; a pan ends once the image is back at fit.
        Event::Follow { zoomed } => {
            let gesture = if pan && !zoomed { Gesture::Idle } else { gesture };
            *state = State::Still(gesture);
            Effects::none()
        }
        Event::GlideSettled { zoomed } => {
            if gliding.is_none() { return Effects::none(); }
            *state = State::Still(if pan && !zoomed { Gesture::Idle } else { gesture });
            Effects::none()
        }
    }
}

/// Whether a pan holds the pointer, which shows the grabbing cursor.
pub fn dragging(state: &State) -> bool {
    matches!(state, State::Still(Gesture::Panning { .. }) | State::Gliding { gesture: Gesture::Panning { .. }, .. })
}
