//! A clip's phases as one machine, apart from AVFoundation so they're plain data: whether it
//! plays ([`Run`]), and what stands behind its surface ([`Stage`]): nothing yet, a poster, or a
//! native player and where that player's last seek stands ([`Settle`]). The handle, the clocks,
//! the poster worker and the native player all report to [`step`], and what it asks for comes
//! back as [`Effect`]s, carried out by `turn` or the caller.

use super::{Control, Kind, Meta, PlayerState};
use crate::machine::{Attempt, Effects};
use crate::time::Instant;
use std::time::Duration;

/// A seek that never lands stops requesting redraws (or ticks) after this long.
const SEEK_PATIENCE: Duration = Duration::from_secs(2);
/// An audio seek has landed once the player reports a time this close to the target.
const AUDIO_SEEK_SLOP: f64 = 0.25;
/// A player item that never becomes ready fails after this long.
const LOAD_PATIENCE: Duration = Duration::from_secs(30);

/// Whether the clip plays. Loading is apart from this: `play` is intent until the clip opens.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) enum Run {
    #[default]
    Paused,
    /// Headless, `anchor` is (runtime clock, playback time) when playback last (re)started.
    Playing { anchor: Option<(f64, f64)> },
    /// Reached the end without looping.
    Ended,
    /// `drawn` once a video's placeholder is on its surface.
    Failed { error: String, drawn: bool },
}

/// What stands behind the clip's surface.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum Stage {
    /// No player, and no poster asked for since the clip was made or since a player that never
    /// showed a frame was released.
    #[default]
    Unasked,
    /// No player, and a poster asked for: a finished one replaces the surface's frame.
    Poster,
    /// No player, and a finished load brings only the clip's facts: the clip is audio, or a
    /// released player's frame stays on the surface.
    Bare,
    /// A player that can't play yet.
    Loading { created: Instant, muted: bool, volume: f32 },
    /// A player that can. `rate` was last applied to it (0 = paused); `shown` is the item time
    /// of the frame on the surface.
    Ready { created: Instant, muted: bool, volume: f32, rate: f32, settle: Settle, shown: Option<f64> },
}

impl Stage {
    pub(super) fn has_player(self) -> bool {
        matches!(self, Stage::Loading { .. } | Stage::Ready { .. })
    }

    pub(super) fn ready(self) -> bool {
        matches!(self, Stage::Ready { .. })
    }

    /// The player was last told to run.
    pub(super) fn running(self) -> bool {
        matches!(self, Stage::Ready { rate, .. } if rate > 0.0)
    }
}

/// Where the player's last seek stands.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum Settle {
    #[default]
    Still,
    /// Waiting for the frame at `to` (for audio, the position), since `since`.
    Seeking { since: Instant, to: f64 },
    /// A paused seek within the frame on screen: nothing to wait for, but the player may report
    /// the old position until it lands, so the handle's time holds.
    Holding { to: f64 },
}

impl Settle {
    /// A seek to `to`, `same_frame` when it's paused within the frame on screen. It replaces
    /// any earlier seek, as the player cancels that one.
    pub(super) fn seek(&mut self, to: f64, same_frame: bool, now: Instant) {
        *self = if same_frame { Settle::Holding { to } } else { Settle::Seeking { since: now, to } };
    }

    /// A new frame reached the surface.
    pub(super) fn framed(&mut self) {
        if let Settle::Seeking { .. } = self {
            *self = Settle::Still;
        }
    }

    /// The player reports `current`. Audio has no frames, so its seek lands at the position;
    /// a hold ends once the player catches up, or when playback moves on from it.
    pub(super) fn observe(&mut self, current: f64, playing: bool, audio: bool) {
        match *self {
            Settle::Seeking { to, .. } if audio && (current - to).abs() < AUDIO_SEEK_SLOP => *self = Settle::Still,
            Settle::Holding { to } if playing || (current - to).abs() < 1e-3 => *self = Settle::Still,
            _ => {}
        }
    }

    /// A seek that hasn't landed by `now` stops being waited for.
    pub(super) fn expire(&mut self, now: Instant) {
        if let Settle::Seeking { since, .. } = *self
            && now.saturating_duration_since(since) > SEEK_PATIENCE
        {
            *self = Settle::Still;
        }
    }

    /// The player may still report the position from before the seek.
    pub(super) fn pending(self) -> bool {
        self != Settle::Still
    }

    /// A frame (or position) is still to come.
    pub(super) fn waiting(self) -> bool {
        matches!(self, Settle::Seeking { .. })
    }
}

/// What the player item reports.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Item {
    Loading,
    Ready,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Event {
    // The handle.
    Play,
    Pause,
    Seek(f64),
    Rate(f32),
    // The clip's facts, or why there are none.
    Open(Result<Meta, String>),
    Fail(String),
    /// The headless clock reads this.
    Clock(f64),
    /// A pull drew the clip.
    Drawn,
    /// An app warms the page it shows next.
    Preload,
    /// The window loop can load an audio clip.
    LoadAudio,
    /// The poster worker finished the request made under `attempt`: the clip's facts, or why it
    /// couldn't open.
    PosterDone { attempt: Attempt, result: Result<Meta, String> },
    /// A pull or tick reached a clip that hasn't failed, on the main thread.
    Pull,
    PlayerMade { created: Instant },
    Item { item: Item, now: Instant },
    /// The handle's intent is due at the player.
    Reconcile { now: Instant },
    /// A new frame reached the surface, at this item time.
    Framed { shown: f64 },
    /// The player reports its position, and that it stopped though told to run; `frame_time`
    /// is the frame's item time, for video.
    Follow { current: f64, stopped: bool, frame_time: Option<f64>, now: Instant },
    /// The turn is over.
    Finish { now: Instant },
    /// The player goes because the clip failed.
    Release,
    /// The player goes because parked players are over their limit.
    Parked,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Effect {
    /// Draw the neutral placeholder.
    Placeholder,
    /// Have the worker open the clip and decode this time, answering under this attempt.
    AskPoster(f64, Attempt),
    /// A finished poster may land on the surface.
    ShowPoster,
    MakePlayer,
    DropPlayer,
    /// The item just became ready: read how its buffers turn upright.
    Orient,
    /// Nothing opened the clip before its item did: read the facts from the item.
    ReadFacts,
    SetMuted(bool),
    SetVolume(f32),
    SeekTo(f64),
    /// 0 pauses.
    SetRate(f32),
    /// Back to zero at this rate, for a loop.
    Restart(f32),
}

/// The one place a clip's phases change.
pub(super) fn step(c: &mut Control, event: Event) -> Effects<Effect> {
    match event {
        Event::Play => {
            match c.run {
                Run::Failed { .. } => return Effects::none(),
                Run::Ended => {
                    c.seek = Some(0.0);
                    c.time = 0.0;
                }
                Run::Paused | Run::Playing { .. } => {}
            }
            c.run = Run::Playing { anchor: None };
        }
        Event::Pause => {
            if let Run::Playing { .. } = c.run {
                c.run = Run::Paused;
            }
        }
        Event::Seek(seconds) => {
            let seconds = if seconds.is_finite() { seconds.max(0.0) } else { 0.0 };
            let seconds = c.duration.map_or(seconds, |d| seconds.min(d));
            c.seek = Some(seconds);
            c.time = seconds;
            match c.run {
                Run::Ended => c.run = Run::Paused,
                Run::Playing { .. } => c.run = Run::Playing { anchor: None },
                Run::Paused | Run::Failed { .. } => {}
            }
        }
        Event::Rate(rate) => {
            c.rate = rate;
            if let Run::Playing { .. } = c.run {
                c.run = Run::Playing { anchor: None };
            }
        }
        Event::Open(Ok(meta)) => opened(c, meta),
        Event::Open(Err(error)) | Event::Fail(error) => failed(c, error),
        Event::Clock(now) => advance(c, now),
        Event::Drawn => {
            if let Run::Failed { drawn, .. } = &mut c.run
                && !*drawn
            {
                *drawn = true;
                return Effects::one(Effect::Placeholder);
            }
        }
        Event::Preload => {
            if c.kind != Kind::Audio && c.stage == Stage::Unasked && !c.failed() {
                let at = c.time;
                return ask_poster(c, at);
            }
        }
        Event::LoadAudio => {
            if c.stage == Stage::Unasked && !c.opened() {
                return ask_poster(c, 0.0);
            }
        }
        Event::PosterDone { attempt, result } => {
            // A failed clip keeps its placeholder.
            if c.failed() {
                return Effects::none();
            }
            // A request a player or a newer request took over from still brings the clip's facts
            // (its true frame timing), but its frame and its errors are for a time or a surface
            // that has moved on.
            let current = attempt == c.posters.current();
            match result {
                Ok(meta) => {
                    if !c.opened() {
                        opened(c, meta);
                    }
                    // A late poster never covers a frame a player made.
                    if current && c.stage == Stage::Poster {
                        return Effects::one(Effect::ShowPoster);
                    }
                }
                // Audio's load only opens it: once another open did, its error is old news.
                Err(error) if current && (c.kind == Kind::Video || !c.opened()) => failed(c, error),
                Err(_) => {}
            }
        }
        Event::Pull => return pull(c),
        Event::PlayerMade { created } => {
            // A fresh player starts at the handle's position.
            if c.time > 0.0 && c.seek.is_none() {
                c.seek = Some(c.time);
            }
            // Posters still in flight are for a surface the player now owns. Audio's load has
            // no surface to lose, so its answer always counts.
            if c.kind == Kind::Video {
                c.posters.next();
                c.poster_at = None;
            }
            c.stage = Stage::Loading { created, muted: false, volume: 1.0 };
        }
        Event::Item { item, now } => return item_reports(c, item, now),
        Event::Reconcile { now } => return reconcile(c, now),
        Event::Framed { shown: at } => {
            if let Stage::Ready { shown, settle, .. } = &mut c.stage {
                *shown = Some(at);
                settle.framed();
            }
        }
        Event::Follow { current, stopped, frame_time, now } => return follow(c, current, stopped, frame_time, now),
        Event::Finish { now } => {
            if let Stage::Ready { settle, .. } = &mut c.stage {
                settle.expire(now);
            }
        }
        Event::Release => {
            if c.stage.has_player() {
                c.stage = Stage::Bare;
                return Effects::one(Effect::DropPlayer);
            }
        }
        Event::Parked => {
            // A video that never showed a frame shows a poster instead.
            let shown = matches!(c.stage, Stage::Ready { shown: Some(_), .. });
            if c.stage.has_player() {
                c.stage = if c.kind == Kind::Video && !shown { Stage::Unasked } else { Stage::Bare };
            }
        }
    }
    Effects::none()
}

fn opened(c: &mut Control, meta: Meta) {
    c.meta = Some(meta);
    c.duration = Some(meta.duration);
    c.size = meta.size;
    c.time = c.time.min(meta.duration);
    c.seek = c.seek.map(|t| t.min(meta.duration));
    c.generation += 1;
}

fn failed(c: &mut Control, error: String) {
    if !c.failed() {
        c.run = Run::Failed { error, drawn: false };
        c.generation += 1;
    }
}

/// Headless clock: playback time follows the runtime clock from the first pull after `play`,
/// `seek` or a rate change.
fn advance(c: &mut Control, now: f64) {
    let Some(meta) = c.meta else { return };
    if let Some(t) = c.seek.take() {
        c.time = t.min(meta.duration);
        if let Run::Playing { anchor } = &mut c.run {
            *anchor = None;
        }
    }
    let Run::Playing { anchor } = &mut c.run else { return };
    // A clock behind the anchor (another runtime on this thread) restarts from here.
    let (start_clock, start_time) = match *anchor {
        Some((clock, time)) if clock <= now => (clock, time),
        _ => *anchor.insert((now, c.time)),
    };
    let t = start_time + (now - start_clock).max(0.0) * c.rate as f64;
    if t < meta.duration {
        c.time = t;
    } else if c.looping && meta.duration > 0.0 {
        c.time = t % meta.duration;
    } else {
        c.time = meta.duration;
        c.run = Run::Ended;
        c.generation += 1;
    }
}

/// Audio has no poster, so its worker only opens it. Called only while no player runs. A video's
/// request for a new time supersedes the ones before it, which may still be decoding an older
/// time; one for the same time shares their attempt.
fn ask_poster(c: &mut Control, at: f64) -> Effects<Effect> {
    if c.kind == Kind::Video && c.poster_at != Some(at) {
        c.posters.next();
        c.poster_at = Some(at);
    }
    c.stage = if c.kind == Kind::Video { Stage::Poster } else { Stage::Bare };
    Effects::one(Effect::AskPoster(at, c.posters.current()))
}

/// Nothing to do with a player already there. Without one, a paused video shows a poster, with
/// no player and no decoder session; a clip that should play gets one.
fn pull(c: &mut Control) -> Effects<Effect> {
    if c.stage.has_player() {
        return Effects::none();
    }
    if !c.playing() {
        if c.kind == Kind::Video
            && let Some(at) = c.seek.take().or_else(|| (c.stage == Stage::Unasked).then_some(c.time))
        {
            return ask_poster(c, at);
        }
        return Effects::none();
    }
    // Audio opens on the load worker first, which may find it needs decoding (an Ogg clip at a
    // URL with no extension); the worker then drives the clip again, starting a queued play.
    if c.kind == Kind::Audio && !c.opened() {
        if c.stage == Stage::Unasked {
            return ask_poster(c, 0.0);
        }
        return Effects::none();
    }
    Effects::one(Effect::MakePlayer)
}

/// Fails the clip if its item failed or never loaded; marks it ready once it can play.
fn item_reports(c: &mut Control, item: Item, now: Instant) -> Effects<Effect> {
    let (Stage::Loading { created, muted, volume } | Stage::Ready { created, muted, volume, .. }) = c.stage else { return Effects::none() };
    let failure = match &item {
        Item::Failed(error) => Some(error.clone()),
        Item::Loading if now.saturating_duration_since(created) > LOAD_PATIENCE => {
            Some(format!("timed out loading after {} s", LOAD_PATIENCE.as_secs()))
        }
        _ => None,
    };
    if let Some(error) = failure {
        failed(c, error);
        c.stage = Stage::Bare;
        return Effects::one(Effect::DropPlayer);
    }
    if item == Item::Ready && !c.stage.ready() {
        c.stage = Stage::Ready { created, muted, volume, rate: 0.0, settle: Settle::Still, shown: None };
        let mut effects = Effects::one(Effect::Orient);
        if !c.opened() {
            effects.push(Effect::ReadFacts);
        }
        return effects;
    }
    Effects::none()
}

/// Brings the player in line with the handle: mute, volume, seek, rate. Seeks wait for the item to be
/// ready, which drops earlier ones.
fn reconcile(c: &mut Control, now: Instant) -> Effects<Effect> {
    let mut effects = Effects::none();
    if let Stage::Loading { muted, .. } | Stage::Ready { muted, .. } = &mut c.stage
        && *muted != c.muted
    {
        *muted = c.muted;
        effects.push(Effect::SetMuted(c.muted));
    }
    if let Stage::Loading { volume, .. } | Stage::Ready { volume, .. } = &mut c.stage
        && *volume != c.volume
    {
        *volume = c.volume;
        effects.push(Effect::SetVolume(c.volume));
    }
    let playing = c.playing();
    if let Stage::Ready { rate, settle, shown, .. } = &mut c.stage {
        if let Some(t) = c.seek.take() {
            effects.push(Effect::SeekTo(t));
            // A paused seek within the frame on screen brings no new frame to wait for.
            let same_frame = !playing && c.meta.zip(*shown).is_some_and(|(m, shown)| m.frame_index(shown) == m.frame_index(t));
            settle.seek(t, same_frame, now);
        }
        let want = if playing { c.rate } else { 0.0 };
        if *rate != want {
            *rate = want;
            effects.push(Effect::SetRate(want));
        }
    }
    effects
}

/// Folds the player's position into the state: seeks landing, the end, loops.
fn follow(c: &mut Control, current: f64, stopped: bool, frame_time: Option<f64>, now: Instant) -> Effects<Effect> {
    let playing = c.playing();
    let Stage::Ready { settle, .. } = &mut c.stage else { return Effects::none() };
    settle.observe(current, playing, frame_time.is_none());
    // Seeks land asynchronously: until then the player still reports the old position, which
    // may be the end.
    let seeking = settle.pending();
    let at_end = !seeking && (c.duration.is_some_and(|d| current >= d - 1e-3) || stopped);
    let time = if seeking {
        c.time
    } else if playing && !at_end {
        frame_time.unwrap_or(current).max(0.0)
    } else {
        current
    };
    let restart = c.observe(time, at_end);
    let playing = c.playing();
    let Stage::Ready { rate, settle, .. } = &mut c.stage else { return Effects::none() };
    if restart {
        *rate = c.rate;
        settle.seek(0.0, false, now);
        // A loop is news for anything showing play counts or progress from zero.
        c.generation += 1;
        return Effects::one(Effect::Restart(c.rate));
    }
    if !playing {
        *rate = 0.0;
    }
    Effects::none()
}

impl Control {
    pub(crate) fn state(&self) -> PlayerState {
        if let Run::Failed { error, .. } = &self.run {
            PlayerState::Failed(error.clone())
        } else if self.meta.is_none() {
            PlayerState::Loading
        } else {
            match self.run {
                Run::Playing { .. } => PlayerState::Playing,
                Run::Ended => PlayerState::Ended,
                _ => PlayerState::Ready,
            }
        }
    }
    pub(crate) fn playing(&self) -> bool {
        matches!(self.run, Run::Playing { .. })
    }
    pub(crate) fn failed(&self) -> bool {
        matches!(self.run, Run::Failed { .. })
    }
    pub(crate) fn opened(&self) -> bool {
        self.meta.is_some() || self.failed()
    }
    pub(crate) fn open(&mut self, meta: Result<Meta, String>) {
        step(self, Event::Open(meta));
    }
    pub(crate) fn fail(&mut self, error: String) {
        step(self, Event::Fail(error));
    }
    pub(crate) fn advance_virtual(&mut self, now: f64) {
        step(self, Event::Clock(now));
    }
    pub(crate) fn next_frame_id(&mut self) -> u64 {
        self.frame_id += 1;
        self.frame_id
    }

    /// Whether the player's own position is the one to read: it runs, and no seek is under way.
    pub(super) fn live(&self) -> bool {
        matches!(self.stage, Stage::Ready { rate, settle, .. } if rate > 0.0 && !settle.waiting())
    }

    /// Whether the clip is playing, seeking or loading (a seek waits for it).
    pub(super) fn active(&self) -> bool {
        match self.stage {
            Stage::Ready { settle, .. } => self.playing() || settle.waiting(),
            _ => true,
        }
    }

    /// Whether the player may be kept only as a parked one.
    pub(super) fn parkable(&self) -> bool {
        self.stage.ready() && !self.active()
    }

    /// The player reports `time`, at the end or not. Returns true when it should restart from
    /// zero.
    pub(super) fn observe(&mut self, time: f64, at_end: bool) -> bool {
        self.time = self.duration.map_or(time, |d| time.clamp(0.0, d));
        if !(self.playing() && at_end) {
            return false;
        }
        if self.looping {
            self.time = 0.0;
            return true;
        }
        self.run = Run::Ended;
        self.generation += 1;
        false
    }
}

#[cfg(test)]
#[path = "phases_tests.rs"]
mod tests;
