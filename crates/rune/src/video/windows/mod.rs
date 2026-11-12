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
mod layer;
mod machine;
mod mf;
mod still;
mod vsync;

use super::*;
use decoder::{Decoded, Facts, Job, News, Pic, Pipe};
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
    seeks: crate::machine::Seq,
    clock: Arc<audio::Clock>,
    facts: Option<Facts>,
    /// The picture on the surface: its unwrapped time.
    shown: Option<f64>,
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
    /// The clip's layer shows its pictures (`layer`): frames don't pull them one by one.
    layered: bool,
}

/// The window's side of a clip's layer (`layer`): what the decoder has made of its ask.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct LayerStatus {
    /// The layer's DirectComposition surface handle, once made.
    pub handle: Option<usize>,
    /// A picture of the current seek is on the layer at the asked size, and the clock runs.
    pub showing: bool,
    /// The clock runs: only then does the layer take the pictures.
    pub running: bool,
    /// The clip can't have a layer; the window draws it.
    pub refused: bool,
}

/// The window asks for the clip's layer at `ask` device pixels (None: it draws the pictures),
/// and hears what the decoder has done.
pub(crate) fn layer(shared: &Arc<Shared>, ask: Option<(u32, u32)>) -> LayerStatus {
    let mut native = shared.native.lock().unwrap();
    let running = matches!(native.phase, Phase::Playing { .. });
    let generation = native.seeks.current().0;
    let Some(pipe) = native.pipe.clone() else {
        native.layered = false;
        return LayerStatus { running, ..LayerStatus::default() };
    };
    let mut s = pipe.lock();
    let changed = s.layer != ask;
    s.layer = ask;
    let showing = running && ask.is_some_and(|size| s.layer_shown == Some((size, generation)));
    let status = LayerStatus { handle: s.layer_handle, showing, running, refused: s.layer_refused };
    drop(s);
    if changed {
        pipe.poke();
    }
    native.layered = showing;
    status
}

// The reader in `still` stays on the headless thread that made it, under the Shared mutex.
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

pub(super) fn decode(native: &mut Native, source: &MediaSource, headers: &[(String, String)], at: f64) -> Result<ByteFrame, String> {
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
    let target = vsync::snap(Instant::now());
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
    let now = Instant::now();
    native.reconcile(shared, &mut c, now, true);
    match native.phase {
        Phase::Buffering { at, .. } => {
            native.shown = Some(at);
            native.event(shared, &c, Event::Shown { pts: at, target: now, lead: 0.0, rate: f64::from(c.rate) });
        }
        Phase::Playing { clock } => {
            let generation = native.seeks.current().0;
            let ended = native.pipe.as_ref().is_some_and(|p| p.lock().ended == Some(generation));
            let duration = c.duration.unwrap_or(0.0);
            let pos = clock.position(now);
            if ended && duration > 0.0 && pos >= (clock.pos / duration).floor() * duration + duration {
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
    native.layered = false;
    let lead = vsync::period().as_secs_f64() / 2.0;
    native.show(shared, &mut c, surface, vsync::snap(Instant::now()), lead);
}

/// Audio, and video on its layer: where the clock has it now. The clock runs only while the sound
/// does (or the layer shows the pictures), so it's always the one to read.
pub(super) fn live_time(shared: &Shared, _: bool) -> Option<f64> {
    let native = shared.native.try_lock().ok()?;
    if shared.kind != Kind::Audio && !native.layered {
        return None;
    }
    let clock = native.clock.get()?;
    let duration = native.facts.map(|f| f.meta.duration).filter(|d| *d > 0.0)?;
    Some(clock.position(Instant::now()).rem_euclid(duration))
}

/// When the frame that draws the clip next should come; None while nothing is due.
pub(super) fn due(shared: &Shared) -> Option<Instant> {
    shared.native.try_lock().ok()?.due
}

pub(super) fn pull(shared: &Arc<Shared>, surface: &NativeSurface, target: Instant) -> bool {
    let now = Instant::now();
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
        && !native.layered
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
        for effect in effects {
            match effect {
                Effect::Open { at } => self.open(shared, c, at),
                Effect::Seek { to } => {
                    let generation = self.seeks.next().0;
                    if let Some(pipe) = &self.pipe {
                        pipe.lock().seek = Some((to, generation));
                        pipe.poke();
                    }
                    self.shown = None;
                }
                Effect::Clock(anchor) => self.clock.set(anchor),
            }
        }
    }

    fn open(&mut self, shared: &Arc<Shared>, c: &Control, at: f64) {
        let weak = Arc::downgrade(shared);
        let job = Job {
            source: shared.source.clone(),
            kind: shared.kind,
            // Picture times count on through loops; the clip starts at its own time.
            start: c.duration.filter(|d| *d > 0.0).map_or(at, |d| at.rem_euclid(d)),
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
            started: Instant::now(),
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
                    let at = self.shown.unwrap_or(0.0);
                    self.event(shared, c, Event::Released { at });
                }
            }
        }
        if c.failed() {
            self.pipe = None;
            self.event(shared, c, Event::Failed);
            return;
        }
        let lead = vsync::period().as_secs_f64() / 2.0;
        if let Some(to) = c.seek.take() {
            self.event(shared, c, Event::Seek { to });
        }
        if c.playing() != self.playing {
            self.playing = c.playing();
            let event = if self.playing {
                Event::Play { target, lead, rate: f64::from(c.rate) }
            } else {
                Event::Pause { at: self.shown.unwrap_or_else(|| self.at()) }
            };
            self.event(shared, c, event);
        }
        if c.rate != self.rate {
            self.rate = c.rate;
            self.event(shared, c, Event::Rate { target, rate: f64::from(c.rate) });
        }
        let sound = self.sounding(c).then_some(c.volume);
        if let Some(pipe) = &self.pipe {
            let mut inbox = pipe.lock();
            if inbox.looping != c.looping || inbox.sound != sound {
                inbox.looping = c.looping;
                inbox.sound = sound;
                drop(inbox);
                pipe.poke();
            }
        }
        // A clip with no picture yet, or one that plays, opens; a paused one keeps its picture.
        if let Phase::Idle { .. } = self.phase
            && may_open
            && (c.playing() || (self.shown.is_none() && shared.kind == Kind::Video))
        {
            self.event(shared, c, Event::Open { play: c.playing() });
        }
    }

    /// The phase's time, for a pause before any picture showed.
    fn at(&self) -> f64 {
        match self.phase {
            Phase::Idle { at } | Phase::Opening { at, .. } | Phase::Buffering { at, .. } | Phase::Paused { at } | Phase::Ended { at } => at,
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
        if self.layered {
            let (newest, ended) = {
                let inbox = pipe.lock();
                (inbox.frames.back().filter(|f| f.generation == generation).map(|f| f.pts), inbox.ended == Some(generation))
            };
            if let Some(pts) = newest {
                self.shown = Some(pts);
            }
            if let (true, Phase::Playing { clock }, Some(shown)) = (ended, self.phase, self.shown) {
                let frame = self.facts.map_or(1.0 / 30.0, |f| f.meta.frame);
                let duration = c.duration.unwrap_or(0.0);
                let end = if duration > 0.0 { (shown / duration).floor() * duration + duration } else { shown + frame };
                if clock.position(target) >= end.max(shown + frame) {
                    self.end(shared, c, shown, duration);
                }
            }
            return;
        }
        let picked = {
            let mut inbox = pipe.lock();
            while inbox.frames.front().is_some_and(|f| f.generation != generation) {
                inbox.frames.pop_front();
            }
            match self.phase {
                Phase::Buffering { .. } => inbox.frames.pop_front(),
                Phase::Paused { .. } if self.shown.is_none() => inbox.frames.pop_front(),
                Phase::Playing { clock } => {
                    let pos = clock.position(target);
                    let mut newest = None;
                    while inbox.frames.front().is_some_and(|f| f.pts <= pos) {
                        newest = inbox.frames.pop_front();
                    }
                    newest
                }
                _ => None,
            }
        };
        let Some(frame) = picked else {
            self.check_end(shared, c, &pipe, generation, target);
            return;
        };
        pipe.poke();
        let pts = frame.pts;
        self.put(c, surface, frame);
        if let Phase::Buffering { .. } = self.phase {
            self.event(shared, c, Event::Shown { pts, target, lead, rate: f64::from(c.rate) });
        }
    }

    /// Playing with nothing due: the end, or a decoder that fell behind.
    fn check_end(&mut self, shared: &Arc<Shared>, c: &mut Control, pipe: &Pipe, generation: u32, target: Instant) {
        let Phase::Playing { clock } = self.phase else { return };
        let (empty, ended) = {
            let inbox = pipe.lock();
            (inbox.frames.is_empty(), inbox.ended == Some(generation))
        };
        if !empty {
            return;
        }
        let pos = clock.position(target);
        let frame = self.facts.map_or(1.0 / 30.0, |f| f.meta.frame);
        let duration = c.duration.unwrap_or(0.0);
        let shown = self.shown.unwrap_or(clock.pos);
        if ended {
            let end = if duration > 0.0 { (shown / duration).floor() * duration + duration } else { shown + frame };
            if pos >= end.max(shown + frame) {
                self.end(shared, c, shown, duration);
            }
        } else if pos > shown + frame * 2.0 + 0.1 {
            self.event(shared, c, Event::Stalled { at: shown + frame });
        }
    }

    /// The last picture showed (or the last sound played) and the decoder didn't loop. Looping
    /// turned on since starts the clip again from zero.
    fn end(&mut self, shared: &Arc<Shared>, c: &mut Control, at: f64, duration: f64) {
        if c.observe(duration, true) {
            c.seek = Some(0.0);
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
        c.time = if duration > 0.0 { frame.pts.rem_euclid(duration).min(duration) } else { frame.pts };
        if let Some(opened) = self.opened.take() {
            crate::marks::mark("video.first_frame.shown");
            if decoder::logging() {
                decoder::log!("first picture on the surface at {} us", decoder::us(opened));
            }
        }
    }

    /// Whether frames should keep coming, and when the next is due.
    fn next(&self, c: &Control, now: Instant) -> (bool, Option<Instant>) {
        let Phase::Playing { clock } = self.phase else { return (false, None) };
        let frame = self.facts.map_or(1.0 / 30.0, |f| f.meta.frame);
        let shown = self.shown.unwrap_or(clock.pos);
        // The layer shows the pictures: a frame is due only to hear that the clip ended.
        if self.layered {
            let generation = self.seeks.current().0;
            let ended = self.pipe.as_ref().is_some_and(|p| p.lock().ended == Some(generation));
            return if ended { (true, Some(clock.when(shown + frame).max(now))) } else { (false, None) };
        }
        let next = self.pipe.as_ref().and_then(|p| p.lock().frames.front().map(|f| f.pts));
        let pos = next.unwrap_or(shown + frame);
        let _ = c;
        (true, Some(clock.when(pos).max(now)))
    }
}
