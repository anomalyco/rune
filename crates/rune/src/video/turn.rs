//! The order of one video pull or audio tick, apart from AVFoundation: each piece of news goes
//! to [`step`], and what it asks for goes to a [`Port`], so the order runs in tests with no
//! native player.

use super::Control;
use super::Kind;
use super::Meta;
use super::phases::{Effect, Event, Item, step};
use crate::machine::Effects;
use crate::time::Instant;

/// What a clip's native side can read and do. Holds no phase of its own.
pub(super) trait Port {
    fn on_main(&mut self) -> bool;
    /// A finished poster load, keeping its frame for [`show_poster`](Self::show_poster).
    fn take_poster(&mut self) -> Option<Result<Meta, String>>;
    /// Puts the kept frame on the surface, if there is one and a surface to take it.
    fn show_poster(&mut self, next_id: &mut dyn FnMut() -> u64) -> Result<(), String>;
    fn ask_poster(&mut self, at: f64);
    /// Returns when the player was made.
    fn make_player(&mut self) -> Result<Instant, String>;
    fn drop_player(&mut self);
    fn unpark(&mut self);
    fn item(&self) -> Item;
    fn orient(&mut self);
    fn item_facts(&self) -> Meta;
    /// Whether the player still holds its objects.
    fn alive(&self) -> bool;
    fn set_muted(&mut self, muted: bool);
    fn seek_to(&mut self, at: f64);
    fn set_rate(&mut self, rate: f32);
    fn restart(&mut self, rate: f32);
    /// The item time for this frame, and the new frame's item time if one reached the surface.
    fn frame(&mut self, next_id: &mut dyn FnMut() -> u64) -> (f64, Result<Option<f64>, String>);
    /// The player's position.
    fn current(&self) -> f64;
    /// Whether the player has stopped. Asked only of one told to run.
    fn stopped(&mut self) -> bool;
}

/// How a turn ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Turn {
    /// Nothing more to do this frame.
    Idle,
    /// Audio still loading: tick until it does, or times out, so a pending seek or play lands.
    Loading,
    /// The player was driven: `Control::active` and `Control::parkable` say what's next.
    Finished,
}

/// Carries out what `step` asked for. [`Effect::Placeholder`] is drawn by the pull itself.
pub(super) fn carry(c: &mut Control, port: &mut impl Port, effects: Effects<Effect>) {
    for effect in effects {
        match effect {
            Effect::Placeholder => {}
            Effect::AskPoster(at) => port.ask_poster(at),
            Effect::ShowPoster => {
                if let Err(e) = port.show_poster(&mut || c.next_frame_id()) {
                    c.fail(e);
                }
            }
            Effect::MakePlayer => match port.make_player() {
                Ok(created) => {
                    step(c, Event::PlayerMade { created });
                }
                Err(e) => c.fail(e),
            },
            Effect::DropPlayer => port.drop_player(),
            Effect::Orient => port.orient(),
            Effect::ReadFacts => c.open(Ok(port.item_facts())),
            Effect::SetMuted(muted) => port.set_muted(muted),
            Effect::SeekTo(at) => port.seek_to(at),
            Effect::SetRate(rate) => port.set_rate(rate),
            Effect::Restart(rate) => port.restart(rate),
        }
    }
}

fn send(c: &mut Control, port: &mut impl Port, event: Event) {
    let effects = step(c, event);
    carry(c, port, effects);
}

/// Bring the player in line with the handle.
pub(super) fn reconcile(c: &mut Control, port: &mut impl Port, now: Instant) {
    if port.alive() {
        send(c, port, Event::Reconcile { now });
    }
}

/// Takes a finished poster load: the clip's facts and, for video, its poster frame.
fn take_poster(c: &mut Control, port: &mut impl Port) {
    if let Some(done) = port.take_poster() {
        send(c, port, Event::PosterDone(done));
    }
}

fn follow(c: &mut Control, port: &mut impl Port, frame_time: Option<f64>, now: Instant) {
    let current = port.current();
    let stopped = c.stage.running() && port.stopped();
    send(c, port, Event::Follow { current, stopped, frame_time, now });
}

/// One pull of a video, or one tick of audio: take a finished load, make or update the player,
/// pull its frame and follow it to the end.
pub(super) fn turn(c: &mut Control, port: &mut impl Port, now: Instant) -> Turn {
    take_poster(c, port);
    if c.failed() {
        send(c, port, Event::Release);
        return Turn::Idle;
    }
    if !port.on_main() {
        return Turn::Idle;
    }
    let fresh = !c.stage.has_player();
    send(c, port, Event::Pull);
    if !c.stage.has_player() {
        return Turn::Idle;
    }
    if fresh {
        port.unpark();
    }
    let item = port.item();
    send(c, port, Event::Item { item, now });
    if !c.stage.has_player() {
        return Turn::Idle;
    }
    reconcile(c, port, now);
    match (c.kind, c.stage.ready()) {
        (Kind::Video, true) => {
            let (item_time, landed) = port.frame(&mut || c.next_frame_id());
            match landed {
                Ok(Some(shown)) => {
                    step(c, Event::Framed { shown });
                }
                Ok(None) => {}
                Err(e) => {
                    c.fail(e);
                    send(c, port, Event::Release);
                    return Turn::Idle;
                }
            }
            follow(c, port, Some(item_time), now);
        }
        (Kind::Video, false) => {}
        (Kind::Audio, false) => return Turn::Loading,
        (Kind::Audio, true) => follow(c, port, None, now),
    }
    step(c, Event::Finish { now });
    Turn::Finished
}
