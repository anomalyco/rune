//! Windows: Media Foundation. Each clip that plays gets a decoder thread (`decoder`) that opens
//! it with the source reader, decodes on the GPU where the adapter has a hardware decoder (else
//! on the CPU) and copies each picture into a shared NV12 texture Rune samples in place
//! (`gpu`). Sound goes to the default output through WASAPI (`audio`), following the clock.
//!
//! The UI thread's part, in each frame that draws the clip: fold the handle's intent and the
//! decoder's news into the player's phase (`machine`), and swap in the picture for the refresh
//! the frame presents at (`vsync`). It then asks for its next frame when the next picture is
//! due, not every refresh. A clip nobody draws isn't pulled, so its decoder fills its queue and
//! sleeps; a muted one picks up where it left off when drawn again, and after a few seconds
//! undrawn the decoder lets go of everything but the picture on screen.
//!
//! Nothing here runs until a clip opens: Media Foundation, D3D11 and the audio stack load then.

mod audio;
mod decoder;
mod gpu;
mod inbox;
mod layer;
mod machine;
mod mf;
mod still;
mod vsync;

use super::*;
use decoder::{Facts, Job};
use inbox::{Decoded, News, Pic, Pick, Pipe, SeekAsk};
use machine::{Effect, Event, Phase};
use std::time::Duration;

/// A muted clip undrawn this long picks up at its picture when drawn again, rather than
/// skipping what played while nobody watched.
const RESUME_GAP: Duration = Duration::from_millis(250);

#[derive(Default)]
pub(crate) struct Native {
    phase: Phase,
    pipe: Option<Arc<Pipe>>,
    /// Seeks issued; the decoder's pictures carry the one they belong to.
    seeks: rune_core::machine::Seq,
    clock: Arc<audio::Clock>,
    facts: Option<Facts>,
    /// The picture on screen for the current seek: its unwrapped time.
    shown: Option<f64>,
    /// The last unwrapped picture time shown on screen, kept across in-flight seeks until
    /// replaced by a newer picture.
    last_shown: Option<f64>,
    /// When a frame last drew the clip.
    pulled: Option<Instant>,
    /// The intent last folded into the phase.
    playing: bool,
    rate: f32,
    /// When the frame that draws the clip next should come.
    due: Option<Instant>,
    /// Headless pictures.
    still: Option<still::Still>,
    /// When the decoder was last started, until its first picture reached the surface
    /// (`RUNE_VIDEO_LOG`).
    opened: Option<Instant>,
    screen: Screen,
}

/// What puts the picture at `Native::shown` on screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Screen {
    /// The window draws the surface, which holds that picture.
    #[default]
    Surface,
    /// The clip's layer shows the pictures (`layer`): frames don't pull them one by one, and the
    /// surface keeps the picture it had when the layer took over.
    Layer,
    /// The layer showed pictures and let go: the surface is behind, until the frame that draws
    /// the clip next puts that picture on it (`show`).
    Behind,
}

/// A clip's layer as the window shows it: the DirectComposition surface handle to wrap in a
/// visual, and the layer's id. A clip's layer is closed and made again at every pause, seek and
/// move, and Windows often gives the new surface handle the closed one's value, so the handle
/// alone can't tell the window that its visual shows a layer that is gone: the id can.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayerSurface {
    /// Unique among the process's layers, never 0.
    pub id: u64,
    pub handle: usize,
}

/// The window's side of a clip's layer (`layer`): what the decoder has made of its ask.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LayerStatus {
    /// The layer's surface, once made.
    pub surface: Option<LayerSurface>,
    /// A picture of the current seek is on the layer at the asked size, and the clock runs.
    pub showing: bool,
    /// The clock runs: only then does the layer take the pictures.
    pub running: bool,
    /// The clip can't have a layer; the window draws it.
    pub refused: bool,
}

/// The window asks for the clip's layer at `ask` device pixels (None: it draws the pictures),
/// and hears what the decoder has done.
pub(crate) use gpu::release_removed_device;

pub(crate) fn layer(shared: &Arc<Shared>, ask: Option<(u32, u32)>) -> LayerStatus {
    let mut native = shared.native.lock().unwrap();
    let running = matches!(native.phase, Phase::Playing { .. });
    let generation = native.seeks.current().0;
    let Some(pipe) = native.pipe.clone() else {
        let _ = native.leave_layer(None);
        return LayerStatus { running, ..LayerStatus::default() };
    };
    let mut s = pipe.lock();
    let changed = s.layer != ask;
    s.layer = ask;
    let showing = running && ask.is_some_and(|size| s.layer_shown == Some((size, generation)));
    let status = LayerStatus { surface: s.layer_surface, showing, running, refused: s.layer_refused };
    let presented = s.presented(generation);
    drop(s);
    if changed {
        pipe.poke();
    }
    if showing {
        native.screen = Screen::Layer;
    } else if native.leave_layer(presented)
        && let Some(shown) = native.shown
    {
        // No frame put the layer's pictures on the surface, which sets the handle's time.
        let mut c = shared.ctl.lock().unwrap();
        let at = clip_time(&c, shown);
        c.time = at;
        c.presented = Some(at);
    }
    status
}

/// Read-only check of whether the clip's layer is still running and showing at `size` for the
/// current seek generation, without mutating the decoder's ask or `native.screen`.
pub(crate) fn layer_showing(shared: &Arc<Shared>, size: (u32, u32)) -> bool {
    let native = shared.native.lock().unwrap();
    if !matches!(native.phase, Phase::Playing { .. }) {
        return false;
    }
    let generation = native.seeks.current().0;
    let Some(pipe) = native.pipe.as_ref() else {
        return false;
    };
    let s = pipe.lock();
    !s.layer_refused && s.layer_shown == Some((size, generation))
}

// SAFETY: the reader in `still` stays on the headless thread that made it, under the Shared mutex.
#[allow(unsafe_code)]
unsafe impl Send for Native {}

impl Drop for Native {
    /// The clip's last handle and element went: its decoder stops now, not when it would park.
    fn drop(&mut self) {
        self.clock.set(None);
        if let Some(pipe) = self.pipe.take() {
            pipe.lock().stop = true;
            pipe.poke();
        }
    }
}

pub(super) fn created(shared: &Arc<Shared>) {
    // Media Foundation's network source sends no headers of ours.
    if !shared.headers.is_empty() {
        shared.ctl.lock().unwrap().fail("request headers aren't supported for clips on Windows yet".into());
    }
}

pub(super) fn enable() {}

pub(super) fn probe(native: &mut Native, source: &MediaSource, _: &[(String, String)], kind: Kind) -> Result<Meta, String> {
    let (still, meta) = still::open(source, kind)?;
    native.still = still;
    Ok(meta)
}

pub(super) fn decode(native: &mut Native, source: &MediaSource, headers: &[(String, String)], at: f64) -> Result<(ByteFrame, f64), String> {
    if native.still.is_none() {
        probe(native, source, headers, Kind::Video)?;
    }
    native.still.as_mut().ok_or("no video track")?.frame(at)
}

/// A handle edit: fold it in now, so pausing or muting a clip nobody draws goes quiet at once.
pub(super) fn apply(shared: &Arc<Shared>) {
    if !playback_enabled() {
        return;
    }
    if shared.kind == Kind::Audio {
        return drive(shared);
    }
    // Being pulled further up this stack: that pull folds it in.
    let Ok(mut native) = shared.native.try_lock() else { return };
    let mut c = shared.ctl.lock().unwrap();
    let target = vsync::snap(rune_core::time::now());
    // Video opens when a frame draws it.
    native.reconcile(shared, &mut c, target, false);
    drop(c);
    drop(native);
    shared.settle(false);
}

/// Audio, from any thread (an edit, or its decoder's news): fold everything in. Sound has no
/// pictures to land, so a clip that opened plays from where it was asked to, and one whose
/// clock passed its end ends.
pub(super) fn drive(shared: &Arc<Shared>) {
    let Ok(mut native) = shared.native.try_lock() else { return };
    let mut c = shared.ctl.lock().unwrap();
    let now = rune_core::time::now();
    native.reconcile(shared, &mut c, now, true);
    match native.phase {
        Phase::Buffering { at, .. } => {
            native.shown = Some(at);
            native.event(shared, &c, Event::Shown { pts: at, target: now, lead: 0.0, rate: f64::from(c.rate) });
        }
        Phase::Playing { clock } => {
            let generation = native.seeks.current().0;
            let (ended, audio_end) = native.pipe.as_ref().map_or((false, None), |p| {
                let inbox = p.lock();
                (inbox.ended == Some(generation), inbox.audio_end)
            });
            let duration = c.duration.unwrap_or(0.0);
            let pos = clock.position(now);
            let duration_end = (duration > 0.0).then(|| (clock.pos / duration).floor() * duration + duration);
            let end = match (duration_end, audio_end.filter(|_| native.sounding(&c))) {
                (Some(d), Some(a)) => Some(d.max(a)),
                (a, b) => a.or(b),
            };
            if ended && end.is_some_and(|end| pos >= end) {
                native.end(shared, &mut c, duration, duration);
            }
        }
        _ => {}
    }
    drop(c);
    drop(native);
    shared.settle(false);
}

pub(super) fn preload(shared: &Arc<Shared>) {
    let mut native = shared.native.lock().unwrap();
    let c = shared.ctl.lock().unwrap();
    if !c.failed() && native.pipe.is_none() {
        let play = c.playing();
        native.event(shared, &c, Event::Open { play });
    }
}

/// The window takes the pictures back from the clip's layer: the picture for now goes on the
/// surface at once, so the frame that draws it isn't one from before the layer showed.
pub(crate) fn unlayer(shared: &Arc<Shared>, surface: &NativeSurface) {
    let mut native = shared.native.lock().unwrap();
    let mut c = shared.ctl.lock().unwrap();
    let presented = native.presented();
    let _ = native.leave_layer(presented);
    let lead = vsync::period().as_secs_f64() / 2.0;
    native.show(shared, &mut c, surface, vsync::snap(rune_core::time::now()), lead);
}

/// Whether a seek or step is still waiting for its picture to land.
pub(super) fn is_seeking(shared: &Shared) -> bool {
    let native = shared.native.lock().unwrap();
    let c = shared.ctl.lock().unwrap();
    if c.failed() {
        return false;
    }
    if !playback_enabled() && native.pipe.is_none() {
        return false;
    }
    c.pending.is_some() || native.phase.is_seeking()
}

/// Media presentation timestamp of the picture currently on screen (surface or layer).
pub(super) fn presented_time(shared: &Shared) -> Option<f64> {
    if shared.kind == Kind::Audio {
        return None;
    }
    let native = shared.native.lock().unwrap();
    let c = shared.ctl.lock().unwrap();
    if c.failed() {
        return None;
    }
    if !playback_enabled() && native.pipe.is_none() {
        return c.presented;
    }
    native.presented_clip_time(&c)
}

/// Audio, and video on its layer: where the clock has it now. The clock runs only while the sound
/// does (or the layer shows the pictures), so it's always the one to read.
pub(super) fn live_time(shared: &Shared, _: bool) -> Option<f64> {
    let native = shared.native.try_lock().ok()?;
    if shared.kind != Kind::Audio && native.screen != Screen::Layer {
        return None;
    }
    let clock = native.clock.get()?;
    let duration = native.facts.map(|f| f.meta.duration).filter(|d| *d > 0.0)?;
    Some(clock.position(rune_core::time::now()).rem_euclid(duration))
}

/// When the frame that draws the clip next should come; None while nothing is due.
pub(super) fn due(shared: &Shared) -> Option<Instant> {
    shared.native.try_lock().ok()?.due
}

pub(super) fn pull(shared: &Arc<Shared>, surface: &NativeSurface, target: Instant) -> bool {
    let now = rune_core::time::now();
    let target = vsync::snap(target.max(now));
    let mut native = shared.native.lock().unwrap();
    let mut c = shared.ctl.lock().unwrap();
    let gap = native.pulled.map(|p| now.saturating_duration_since(p));
    native.pulled = Some(now);
    if let Some(pipe) = &native.pipe {
        pipe.lock().touched = Some(now);
    }
    native.reconcile(shared, &mut c, target, true);
    if c.failed() {
        native.due = None;
        return false;
    }
    let lead = vsync::period().as_secs_f64() / 2.0;
    // A clip on its layer played on between pulls, as the window saw it.
    if let (Phase::Playing { .. }, Some(gap), Some(at)) = (native.phase, gap, native.shown)
        && gap > RESUME_GAP
        && !native.sounding(&c)
        && native.screen != Screen::Layer
    {
        native.event(shared, &c, Event::Resumed { target, at, lead });
    }
    let shown = native.shown;
    native.show(shared, &mut c, surface, target, lead);
    let (active, due) = native.next(&c, now);
    native.due = due;
    let cost = now.elapsed().as_nanos();
    if decoder::logging_frames() {
        let new = native.shown.filter(|s| shown != Some(*s)).map_or(String::new(), |s| format!(" picture {s:.4}"));
        let at = |t: Instant| t.saturating_duration_since(now).as_micros();
        decoder::log!("pull {cost} ns, target +{} us{new}, due +{:?} us", at(target), due.map(at));
    }
    active
}

impl Native {
    /// Steps the phase and carries out what it asks.
    fn event(&mut self, shared: &Arc<Shared>, c: &Control, event: Event) {
        let before = self.phase;
        let effects = machine::step(&mut self.phase, event);
        if decoder::logging() && std::mem::discriminant(&before) != std::mem::discriminant(&self.phase) {
            decoder::log!("{:?} -> {:?}", before, self.phase);
        }
        if self.phase == Phase::Failed {
            self.shown = None;
            self.last_shown = None;
        }
        for effect in effects {
            match effect {
                Effect::Open { at, exact } => self.open(shared, c, at, exact),
                Effect::Seek { to, exact } => {
                    let generation = self.seeks.next().0;
                    if let Some(pipe) = &self.pipe {
                        pipe.lock().seek = Some(SeekAsk { to, generation, exact });
                        pipe.poke();
                    }
                    if let Some(prev) = self.shown.take() {
                        self.last_shown = Some(prev);
                    }
                }
                Effect::Clock(anchor) => self.clock.set(anchor),
            }
        }
    }

    fn open(&mut self, shared: &Arc<Shared>, c: &Control, at: f64, exact: bool) {
        let weak = Arc::downgrade(shared);
        let job = Job {
            source: shared.source.clone(),
            kind: shared.kind,
            // Picture times count on through loops; the clip starts at its own time.
            start: c.duration.filter(|d| *d > 0.0).map_or(at, |d| at.rem_euclid(d)),
            exact,
            generation: self.seeks.current().0,
            looping: c.looping,
            sound: (!c.muted).then_some(c.volume),
            notify: Box::new(move || {
                let Some(shared) = weak.upgrade() else { return };
                match shared.kind {
                    Kind::Video => shared.wake_windows(),
                    Kind::Audio => drive(&shared),
                }
            }),
            clock: self.clock.clone(),
            started: rune_core::time::now(),
        };
        self.opened = Some(job.started);
        self.pipe = Some(decoder::start(job));
    }

    /// Sound is playing: the clip's clock runs on whether or not it's drawn.
    fn sounding(&self, c: &Control) -> bool {
        !c.muted && self.facts.is_some_and(|f| f.sound.is_some())
    }

    /// Folds the decoder's news and the handle's intent into the phase.
    fn reconcile(&mut self, shared: &Arc<Shared>, c: &mut Control, target: Instant, may_open: bool) {
        let news = self.pipe.as_ref().map(|p| std::mem::take(&mut p.lock().news)).unwrap_or_default();
        for news in news {
            match news {
                News::Opened(facts) => {
                    self.facts = Some(facts);
                    if !c.opened() {
                        c.open(Ok(facts.meta));
                    }
                    self.event(shared, c, Event::Opened);
                }
                News::Failed(e) => {
                    if decoder::logging() {
                        decoder::log!("failed: {e}");
                    }
                    c.fail(e);
                    self.pipe = None;
                    self.event(shared, c, Event::Failed);
                }
                News::Parked => {
                    self.pipe = None;
                    // Let go while it waited for a seek's picture: it opens again at the seek.
                    let at = self.shown.unwrap_or_else(|| self.at());
                    self.event(shared, c, Event::Released { at });
                }
            }
        }
        if c.failed() {
            self.pipe = None;
            self.event(shared, c, Event::Failed);
            return;
        }
        if let (Phase::Playing { clock }, Some(heard)) = (self.phase, self.clock.get())
            && heard != clock
        {
            self.event(shared, c, Event::Synced { clock: heard });
        }
        let lead = vsync::period().as_secs_f64() / 2.0;
        match c.pending.take() {
            Some(Pending::Seek(seek)) => {
                self.event(shared, c, Event::Seek { to: seek.at, exact: seek.exact });
            }
            Some(Pending::Step(frames)) => {
                let was_playing = self.playing;
                self.playing = false;
                if let Some(meta) = c.meta {
                    let base = if self.phase.is_seeking() {
                        clip_time(c, self.at())
                    } else if was_playing {
                        clip_time(c, self.held())
                    } else {
                        self.presented_clip_time(c).unwrap_or(c.time)
                    };
                    if let Some((next, _)) = meta.step_from(base, frames) {
                        let to = meta.frame_start(next);
                        c.time = to;
                        self.event(shared, c, Event::Step { to });
                    } else if was_playing {
                        let at = self.held();
                        self.event(shared, c, Event::Pause { at });
                        if let (Phase::Paused { at }, true) = (self.phase, self.screen != Screen::Surface) {
                            let clamped = clip_time(c, at);
                            c.time = clamped;
                            c.presented = Some(clamped);
                        }
                    }
                } else {
                    // Clip has not opened yet: keep the step until News::Opened delivers Meta.
                    c.pending = Some(Pending::Step(frames));
                    if was_playing {
                        let at = self.held();
                        self.event(shared, c, Event::Pause { at });
                    }
                }
            }
            None => {}
        }
        if c.playing() != self.playing {
            self.playing = c.playing();
            let event = if self.playing {
                Event::Play { target, lead, rate: f64::from(c.rate) }
            } else {
                Event::Pause { at: self.held() }
            };
            self.event(shared, c, event);
            // Paused where the layer stood: no frame put that picture on the surface, which
            // sets the handle's time, so it's set here. The surface gets the picture when the
            // window draws the clip again (`show`).
            if let (Phase::Paused { at }, true) = (self.phase, self.screen != Screen::Surface) {
                let clamped = clip_time(c, at);
                c.time = clamped;
                c.presented = Some(clamped);
            }
        }
        if c.rate != self.rate {
            self.rate = c.rate;
            self.event(shared, c, Event::Rate { target, rate: f64::from(c.rate) });
        }
        if let Some(pipe) = &self.pipe {
            let mut inbox = pipe.lock();
            // Until the clip has opened, whether it has sound isn't known: the decoder keeps
            // the ask it started with. Taking it for "no sound" closed the output the decoder
            // had just opened and deselected the audio stream, which reads from the clip's
            // start when it's selected again.
            let sound = if self.facts.is_some() { self.sounding(c).then_some(c.volume) } else { inbox.sound };
            if inbox.looping != c.looping || inbox.sound != sound {
                inbox.looping = c.looping;
                inbox.sound = sound;
                drop(inbox);
                pipe.poke();
            }
        }
        if let Phase::Idle { .. } = self.phase
            && may_open
            && self.wants_decoder(c.playing(), shared.kind)
        {
            self.event(shared, c, Event::Open { play: c.playing() });
        }
    }

    /// Whether a frame that draws a clip with no decoder starts one: a clip that plays, one with
    /// no picture yet, and one whose surface is behind what its layer showed. A paused clip with
    /// its picture on the surface keeps it.
    fn wants_decoder(&self, playing: bool, kind: Kind) -> bool {
        playing || (kind == Kind::Video && (self.shown.is_none() || self.screen == Screen::Behind))
    }

    /// The newest picture of the current seek the layer showed.
    fn presented(&self) -> Option<f64> {
        let generation = self.seeks.current().0;
        self.pipe.as_ref().and_then(|pipe| pipe.lock().presented(generation))
    }

    /// Clip-relative presentation timestamp of the picture currently shown on screen (surface or
    /// layer), falling back to the last shown picture while a seek is in flight.
    fn presented_clip_time(&self, c: &Control) -> Option<f64> {
        let live = if self.screen == Screen::Layer { self.presented() } else { None };
        let pts = match (self.shown, live) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        }
        .or(self.last_shown);
        pts.map(|p| clip_time(c, p)).or(c.presented)
    }

    /// The picture on screen is at least the one the layer showed last (`presented`).
    fn follow_layer(&mut self, presented: Option<f64>) {
        if let Some(pts) = presented {
            let newest = self.shown.map_or(pts, |shown| shown.max(pts));
            self.shown = Some(newest);
            self.last_shown = Some(newest);
        }
    }

    /// The layer no longer shows the pictures. If it did (true), the picture on screen is the
    /// last it showed, and the surface is behind.
    fn leave_layer(&mut self, presented: Option<f64>) -> bool {
        let left = self.screen == Screen::Layer;
        if left {
            self.follow_layer(presented);
            self.screen = Screen::Behind;
        }
        left
    }

    /// Where a pause holds: at the picture on screen. While the layer shows the pictures, and
    /// until the surface has one again, that is the decoder's to say, so the clock stops first:
    /// the decoder presents a picture only under the inbox lock and with the clock running, so
    /// nothing is presented after its answer.
    fn held(&mut self) -> f64 {
        if self.screen != Screen::Surface {
            self.clock.set(None);
            let presented = self.presented();
            self.follow_layer(presented);
        }
        self.shown.unwrap_or_else(|| self.at())
    }

    /// The phase's time, for a pause before any picture showed.
    fn at(&self) -> f64 {
        match self.phase {
            Phase::Idle { at, .. } | Phase::Opening { at, .. } | Phase::Buffering { at, .. } | Phase::Paused { at } | Phase::Ended { at } => at,
            Phase::Playing { clock } => clock.pos,
            Phase::Failed => 0.0,
        }
    }

    /// Puts the picture for `target` on the surface and notes where playback stands.
    fn show(&mut self, shared: &Arc<Shared>, c: &mut Control, surface: &NativeSurface, target: Instant, lead: f64) {
        let Some(pipe) = self.pipe.clone() else { return };
        let generation = self.seeks.current().0;
        // The layer shows the pictures: the surface keeps the one it has, so the window has
        // nothing to draw, and playback stands at the newest picture presented.
        if self.screen == Screen::Layer {
            let (presented, ended, audio_end) = {
                let inbox = pipe.lock();
                (inbox.presented(generation), inbox.ended == Some(generation), inbox.audio_end)
            };
            self.follow_layer(presented);
            if let (true, Phase::Playing { clock }, Some(shown)) = (ended, self.phase, self.shown) {
                let duration = c.duration.unwrap_or(0.0);
                if clock.position(target) >= self.end_pos(c, shown, audio_end) {
                    self.end(shared, c, shown, duration);
                }
            }
            return;
        }
        let behind = self.screen == Screen::Behind;
        let pick = match self.phase {
            Phase::Buffering { .. } => Pick::First,
            Phase::Paused { .. } if self.shown.is_none() => Pick::First,
            // The layer showed pictures the surface never got: it gets the one that holds.
            Phase::Paused { at } | Phase::Ended { at } if behind => Pick::Due(at),
            Phase::Playing { clock } => Pick::Due(clock.position(target)),
            _ => Pick::Hold,
        };
        let (picked, dropped) = {
            let mut inbox = pipe.lock();
            let picked = inbox.take(generation, pick);
            (picked, !inbox.unread.is_empty())
        };
        // Room in the queue, or textures to free: the decoder goes on.
        if picked.is_some() || dropped {
            pipe.poke();
        }
        let Some(frame) = picked else {
            // The picture a paused clip holds left the queue: the decoder gets it again.
            if let (true, Phase::Paused { at }) = (behind, self.phase) {
                return self.event(shared, c, Event::Seek { to: clip_time(c, at), exact: true });
            }
            self.check_end(shared, c, &pipe, generation, target);
            return;
        };
        let pts = frame.pts;
        self.put(c, surface, frame);
        if let Phase::Buffering { .. } = self.phase {
            self.event(shared, c, Event::Shown { pts, target, lead, rate: f64::from(c.rate) });
        }
    }

    /// Unwrapped media time at which playback ends once the clip's streams reached EOS.
    fn end_pos(&self, c: &Control, shown: f64, audio_end: Option<f64>) -> f64 {
        let frame = self.facts.map_or(DEFAULT_FRAME, |f| f.meta.frame_step());
        let duration = c.duration.unwrap_or(0.0);
        let end = if duration > 0.0 { (shown / duration).floor() * duration + duration } else { shown + frame };
        let end = end.max(shown + frame);
        match audio_end.filter(|_| self.sounding(c)) {
            Some(a) => end.max(a),
            None => end,
        }
    }

    /// Playing with nothing due: the end, or a decoder that fell behind. Waiting for a seek's
    /// picture when the decoder ended with none: the clip ends, or holds, where it was asked to
    /// stand.
    fn check_end(&mut self, shared: &Arc<Shared>, c: &mut Control, pipe: &Pipe, generation: u32, target: Instant) {
        let (empty, video_ended, ended, audio_end) = {
            let inbox = pipe.lock();
            (
                inbox.frames.is_empty(),
                inbox.video_ended == Some(generation),
                inbox.ended == Some(generation),
                inbox.audio_end,
            )
        };
        if !empty {
            return;
        }
        let duration = c.duration.unwrap_or(0.0);
        let clock = match self.phase {
            Phase::Playing { clock } => clock,
            Phase::Buffering { at, play: true, .. } if ended => return self.end(shared, c, at, duration),
            Phase::Buffering { at, play: false, .. } if ended => return self.event(shared, c, Event::Ended { at }),
            _ => return,
        };
        // Played from a hold with no picture: nothing is left to show.
        if ended && self.shown.is_none() {
            return self.end(shared, c, clock.pos, duration);
        }
        let pos = clock.position(target);
        let frame = self.facts.map_or(DEFAULT_FRAME, |f| f.meta.frame_step());
        let shown = self.shown.unwrap_or(clock.pos);
        if ended {
            if pos >= self.end_pos(c, shown, audio_end) {
                self.end(shared, c, shown, duration);
            }
        } else if !video_ended && pos > shown + frame * 2.0 + 0.1 {
            self.event(shared, c, Event::Stalled { at: shown + frame });
        }
    }

    /// The last picture showed (or the last sound played) and the decoder didn't loop. Looping
    /// turned on since starts the clip again from zero.
    fn end(&mut self, shared: &Arc<Shared>, c: &mut Control, at: f64, duration: f64) {
        if c.observe(duration, true) {
            c.pending = Some(Pending::Seek(Seek::exact(0.0)));
            return;
        }
        self.event(shared, c, Event::Ended { at });
        self.playing = c.playing();
    }

    /// Puts `frame` on the surface.
    fn put(&mut self, c: &mut Control, surface: &NativeSurface, frame: Decoded) {
        let Some(facts) = self.facts else { return };
        let Some(picture) = facts.picture else { return };
        let id = c.next_frame_id();
        match frame.picture {
            Pic::Gpu(planes, value) => surface.push_decoded(planes, value, id, picture.format, picture.color, picture.orient),
            Pic::Cpu(bytes) => {
                if let Err(e) = surface.push_bytes_oriented(bytes, id, picture.orient) {
                    c.fail(e);
                    return;
                }
            }
        }
        let duration = facts.meta.duration;
        if duration > 0.0 && self.shown.is_some_and(|s| (s / duration).floor() < (frame.pts / duration).floor()) {
            // A loop is news for anything showing progress from zero.
            c.generation += 1;
        }
        self.shown = Some(frame.pts);
        self.last_shown = Some(frame.pts);
        self.screen = Screen::Surface;
        let at = clip_time(c, frame.pts);
        c.time = at;
        c.presented = Some(at);
        if let Some(opened) = self.opened.take() {
            rune_core::marks::mark("video.first_frame.shown");
            if decoder::logging() {
                decoder::log!("first picture on the surface at {} us", decoder::us(opened));
            }
        }
    }

    /// Whether frames should keep coming, and when the next is due.
    fn next(&self, c: &Control, now: Instant) -> (bool, Option<Instant>) {
        let Phase::Playing { clock } = self.phase else { return (false, None) };
        let frame = self.facts.map_or(DEFAULT_FRAME, |f| f.meta.frame_step());
        let shown = self.shown.unwrap_or(clock.pos);
        let generation = self.seeks.current().0;
        let (next_frame, video_ended, ended, audio_end) = self.pipe.as_ref().map_or((None, false, false, None), |p| {
            let inbox = p.lock();
            (
                inbox.frames.front().map(|f| f.pts),
                inbox.video_ended == Some(generation),
                inbox.ended == Some(generation),
                inbox.audio_end,
            )
        });
        // The layer shows the pictures: a frame is due only to hear that the clip ended.
        if self.screen == Screen::Layer {
            if !ended {
                return (false, None);
            }
            let end = self.end_pos(c, shown, audio_end);
            if self.sounding(c) && audio_end.is_some() && clock.when(end) > now + Duration::from_millis(50) {
                return (false, None);
            }
            return (true, Some(clock.when(end).max(now)));
        }
        if let Some(pts) = next_frame {
            return (true, Some(clock.when(pts).max(now)));
        }
        if video_ended {
            if !ended {
                return (false, None);
            }
            let end = self.end_pos(c, shown, audio_end);
            if self.sounding(c) && audio_end.is_some() && clock.when(end) > now + Duration::from_millis(50) {
                return (false, None);
            }
            return (true, Some(clock.when(end).max(now)));
        }
        (true, Some(clock.when(shown + frame).max(now)))
    }
}

/// Unwrapped seconds (counting on through loops) as the clip's own time.
fn clip_time(c: &Control, pts: f64) -> f64 {
    match c.duration.filter(|d| *d > 0.0) {
        Some(duration) => pts.rem_euclid(duration).min(duration),
        None => pts,
    }
}

#[cfg(test)]
mod tests;