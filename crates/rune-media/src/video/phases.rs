//! A clip's phases as one machine, apart from AVFoundation so they're plain data: whether it
//! plays ([`Run`]), and what stands behind its surface ([`Stage`]): nothing yet, a poster, or a
//! native player and where that player's last seek stands ([`Settle`]). The handle, the clocks,
//! the poster worker and the native player all report to [`step`], and what it asks for comes
//! back as [`Effect`]s, carried out by `turn` or the caller.

use super::{Control, DEFAULT_FRAME, Kind, Meta, Pending, PlayerState, Seek};
use rune_core::machine::{Attempt, Effects};
use rune_core::time::Instant;
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
    Loading { created: Instant, muted: bool, volume: f32, output: u64 },
    /// A player that can. `rate` was last applied to it (0 = paused); `shown` is the item time
    /// of the frame on the surface.
    Ready { created: Instant, muted: bool, volume: f32, output: u64, rate: f32, settle: Settle, shown: Option<f64> },
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
    Seek { to: f64, exact: bool },
    /// Pause and step by `frames` video pictures, clamped at the clip's ends.
    Step(i32),
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
    /// A poster frame reached the surface, at this presentation timestamp.
    PosterShown { shown: f64 },
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
    SetOutputDevice,
    SeekTo { at: f64, exact: bool },
    /// Step by `frames` on the native item when supported, else fall back to an exact seek to `fallback`.
    StepBy { frames: i32, fallback: f64 },
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
                    c.pending = Some(Pending::Seek(Seek::exact(0.0)));
                    c.time = 0.0;
                }
                Run::Paused | Run::Playing { .. } => {
                    // Playing overrides any unapplied paused step.
                    if matches!(c.pending, Some(Pending::Step(_))) {
                        c.pending = None;
                    }
                }
            }
            c.run = Run::Playing { anchor: None };
        }
        Event::Pause => {
            if let Run::Playing { .. } = c.run {
                c.run = Run::Paused;
            }
        }
        Event::Seek { to: seconds, exact } => {
            let seconds = if seconds.is_finite() { seconds.max(0.0) } else { 0.0 };
            let seconds = c.duration.map_or(seconds, |d| seconds.min(d));
            c.pending = Some(Pending::Seek(Seek { at: seconds, exact }));
            c.time = seconds;
            match c.run {
                Run::Ended => c.run = Run::Paused,
                Run::Playing { .. } => c.run = Run::Playing { anchor: None },
                Run::Paused | Run::Failed { .. } => {}
            }
        }
        Event::Step(frames) => {
            if c.kind == Kind::Audio || c.failed() {
                return Effects::none();
            }
            let was_playing = c.playing();
            if matches!(c.run, Run::Playing { .. } | Run::Ended) {
                c.run = Run::Paused;
            }
            if frames == 0 {
                return Effects::none();
            }
            match c.pending {
                Some(Pending::Step(prev)) => {
                    let total = prev.saturating_add(frames);
                    c.pending = (total != 0).then_some(Pending::Step(total));
                }
                Some(Pending::Seek(seek)) => {
                    if let Some(meta) = c.meta {
                        if let Some((next, _)) = meta.step_from(seek.at, frames) {
                            let at = meta.frame_time(next);
                            c.pending = Some(Pending::Seek(Seek::exact(at)));
                            c.time = at;
                        }
                    } else {
                        let at = (seek.at + f64::from(frames) * DEFAULT_FRAME).max(0.0);
                        c.pending = Some(Pending::Seek(Seek::exact(at)));
                        c.time = at;
                    }
                }
                None => {
                    // When already paused on a known shown frame and not seeking, stepping past
                    // a clip end is an immediate no-op (never enters is_seeking).
                    if !was_playing
                        && !c.is_seeking_stage()
                        && let (Some(meta), Some(shown)) = (c.meta, c.presented)
                        && meta.step_from(shown, frames).is_none()
                    {
                        return Effects::none();
                    }
                    c.pending = Some(Pending::Step(frames));
                }
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
                    // A late poster never covers a frame a player made, or one a seek moved past.
                    if current && !overtaken(c) && c.stage == Stage::Poster {
                        if c.poster_seeking == Some(attempt) {
                            c.poster_seeking = None;
                        }
                        return Effects::one(Effect::ShowPoster);
                    }
                }
                // Audio's load only opens it: once another open did, its error is old news.
                Err(error) if current && !overtaken(c) && (c.kind == Kind::Video || !c.opened()) => failed(c, error),
                Err(_) => {}
            }
        }
        Event::PosterShown { shown: at } => {
            if c.kind == Kind::Video && !c.failed() {
                c.presented = Some(c.clamp_time(at));
            }
        }
        Event::Pull => return pull(c),
        Event::PlayerMade { created } => {
            // A fresh player starts at the handle's position.
            if c.time > 0.0 && c.pending.is_none() {
                c.pending = Some(Pending::Seek(Seek::exact(c.time)));
            }
            // Posters still in flight are for a surface the player now owns. Audio's load has
            // no surface to lose, so its answer always counts.
            if c.kind == Kind::Video {
                c.posters.next();
                c.poster_at = None;
                c.poster_seeking = None;
            }
            // A fresh player starts on the system output; only a chosen device needs applying.
            let output = if c.output_device.is_some() { c.output_generation.wrapping_sub(1) } else { c.output_generation };
            c.stage = Stage::Loading { created, muted: false, volume: 1.0, output };
        }
        Event::Item { item, now } => return item_reports(c, item, now),
        Event::Reconcile { now } => return reconcile(c, now),
        Event::Framed { shown: at } => {
            if let Stage::Ready { shown, settle, .. } = &mut c.stage {
                *shown = Some(at);
                settle.framed();
            }
            if c.kind == Kind::Video && !c.failed() {
                c.presented = Some(c.clamp_time(at));
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

/// A seek made since the poster request names another time, so its answer is for a time the
/// clip has left even before the next pull asks for the new one. Both times clamp to the clip's
/// length first: a seek past the end lands on the poster's own time, which is the same picture.
/// // differs from dev: dev had no fence, and failed the clip on the old answer.
fn overtaken(c: &Control) -> bool {
    let clamp = |t: f64| c.duration.map_or(t, |d| t.min(d));
    c.poster_at.is_some_and(|asked| match c.pending {
        Some(Pending::Seek(target)) => clamp(target.at) != clamp(asked),
        Some(Pending::Step(frames)) => {
            c.meta.is_none_or(|meta| {
                meta.step_from(c.presented.unwrap_or(c.time), frames)
                    .is_some_and(|(next, _)| next != meta.frame_index(asked))
            })
        }
        None => false,
    })
}

fn opened(c: &mut Control, meta: Meta) {
    c.meta = Some(meta);
    c.duration = Some(meta.duration);
    c.size = meta.size;
    c.time = c.time.min(meta.duration);
    c.pending = match c.pending {
        Some(Pending::Seek(s)) => Some(Pending::Seek(Seek { at: s.at.min(meta.duration), ..s })),
        Some(Pending::Step(frames)) => {
            meta.step_from(c.presented.unwrap_or(c.time), frames).map(|_| Pending::Step(frames))
        }
        None => None,
    };
    c.generation += 1;
}

fn failed(c: &mut Control, error: String) {
    if !c.failed() {
        c.run = Run::Failed { error, drawn: false };
        c.poster_seeking = None;
        c.presented = None;
        c.generation += 1;
    }
}

/// Headless clock: playback time follows the runtime clock from the first pull after `play`,
/// `seek` or a rate change.
fn advance(c: &mut Control, now: f64) {
    let Some(meta) = c.meta else { return };
    match c.pending.take() {
        Some(Pending::Seek(seek)) => {
            c.time = seek.at.min(meta.duration);
            if let Run::Playing { anchor } = &mut c.run {
                *anchor = None;
            }
        }
        Some(Pending::Step(frames)) => {
            let base = c.presented.unwrap_or(c.time);
            if let Some((next, _)) = meta.step_from(base, frames) {
                c.time = meta.frame_time(next);
            }
        }
        None => {}
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
        if c.kind == Kind::Video {
            match c.pending.take() {
                Some(Pending::Seek(seek)) => {
                    let effects = ask_poster(c, seek.at);
                    c.poster_seeking = Some(c.posters.current());
                    return effects;
                }
                Some(Pending::Step(frames)) => {
                    if let Some(meta) = c.meta {
                        let base = if c.poster_seeking.is_some() { c.time } else { c.presented.unwrap_or(c.time) };
                        if let Some((next, _)) = meta.step_from(base, frames) {
                            let target = meta.frame_time(next);
                            c.time = target;
                            let effects = ask_poster(c, target);
                            c.poster_seeking = Some(c.posters.current());
                            return effects;
                        }
                    } else {
                        c.pending = Some(Pending::Step(frames));
                        if c.stage == Stage::Unasked {
                            let effects = ask_poster(c, c.time);
                            c.poster_seeking = Some(c.posters.current());
                            return effects;
                        }
                        return Effects::none();
                    }
                }
                None => {}
            }
            if c.stage == Stage::Unasked {
                return ask_poster(c, c.time);
            }
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
    let (Stage::Loading { created, muted, volume, output } | Stage::Ready { created, muted, volume, output, .. }) = c.stage else { return Effects::none() };
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
        c.stage = Stage::Ready { created, muted, volume, output, rate: 0.0, settle: Settle::Still, shown: None };
        let mut effects = Effects::one(Effect::Orient);
        if !c.opened() {
            effects.push(Effect::ReadFacts);
        }
        return effects;
    }
    Effects::none()
}

/// Brings the player in line with the handle: mute, volume, output device, seek, rate. Seeks wait
/// for the item to be ready, which drops earlier ones.
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
    if let Stage::Loading { output, .. } | Stage::Ready { output, .. } = &mut c.stage
        && *output != c.output_generation
    {
        *output = c.output_generation;
        effects.push(Effect::SetOutputDevice);
    }
    let playing = c.playing();
    if let Stage::Ready { rate, settle, shown, .. } = &mut c.stage {
        match c.pending.take() {
            Some(Pending::Seek(seek)) => {
                effects.push(Effect::SeekTo { at: seek.at, exact: seek.exact });
                // A paused seek within the frame on screen brings no new frame to wait for.
                let same_frame = !playing && c.meta.zip(*shown).is_some_and(|(m, shown)| m.frame_index(shown) == m.frame_index(seek.at));
                settle.seek(seek.at, same_frame, now);
            }
            Some(Pending::Step(frames)) => {
                if *rate != 0.0 {
                    *rate = 0.0;
                    effects.push(Effect::SetRate(0.0));
                }
                let was_pending = settle.pending();
                let base = if was_pending { c.time } else { (*shown).or(c.presented).unwrap_or(c.time) };
                if let Some(meta) = c.meta {
                    if let Some((next, delta)) = meta.step_from(base, frames) {
                        let target = meta.frame_time(next);
                        c.time = target;
                        if !was_pending && shown.is_some() {
                            effects.push(Effect::StepBy { frames: delta, fallback: target });
                        } else {
                            effects.push(Effect::SeekTo { at: target, exact: true });
                        }
                        settle.seek(target, false, now);
                    }
                } else {
                    let target = (base + f64::from(frames) * DEFAULT_FRAME).max(0.0);
                    c.time = target;
                    effects.push(Effect::SeekTo { at: target, exact: true });
                    settle.seek(target, false, now);
                }
            }
            None => {}
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
    pub(crate) fn set_output_device(&mut self, uid: Option<&str>) {
        let uid = uid.filter(|s| !s.is_empty());
        if self.output_device.as_deref() != uid {
            self.output_device = uid.map(str::to_owned);
            self.output_generation = self.output_generation.wrapping_add(1);
        }
    }
    pub(crate) fn next_frame_id(&mut self) -> u64 {
        self.frame_id += 1;
        self.frame_id
    }

    pub(super) fn clamp_time(&self, t: f64) -> f64 {
        let t = if t.is_finite() { t.max(0.0) } else { 0.0 };
        self.duration.filter(|d| *d > 0.0).map_or(t, |d| t.min(d))
    }

    pub(super) fn is_seeking_stage(&self) -> bool {
        self.poster_seeking.is_some()
            || matches!(self.stage, Stage::Ready { settle, .. } if settle.waiting())
    }

    /// Whether a seek or step is still waiting to be carried out or for its picture (or position,
    /// for audio) to land.
    pub(super) fn is_seeking(&self) -> bool {
        if self.failed() {
            return false;
        }
        self.pending.is_some() || self.is_seeking_stage()
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
