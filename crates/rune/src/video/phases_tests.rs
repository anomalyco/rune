//! Every short sequence through a clip's machine, since the handle, the clocks, the poster worker
//! and the native player report in whatever order they like. The real [`step`] and the real
//! [`turn`](super::super::turn::turn) run against a scripted native player; beside them runs
//! `Old`, a transcription of the fields and branches the machine replaced, which calls none of it.

use super::super::turn::{Port, Turn, reconcile, turn};
use super::*;
use crate::machine::Attempt;

// The old code.

#[derive(Clone, Copy, Debug, PartialEq)]
struct OldMeta {
    duration: f64,
    size: Option<(u32, u32)>,
    frame: f64,
}

impl OldMeta {
    fn frames(&self) -> i64 {
        ((self.duration / self.frame).round() as i64).max(1)
    }
    fn frame_index(&self, t: f64) -> i64 {
        ((t / self.frame + 1e-6).floor() as i64).clamp(0, self.frames() - 1)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum OldSettle {
    Still,
    Seeking { since: Instant, to: f64 },
    Holding { to: f64 },
}

impl OldSettle {
    fn seek(&mut self, to: f64, same_frame: bool, now: Instant) {
        *self = if same_frame { OldSettle::Holding { to } } else { OldSettle::Seeking { since: now, to } };
    }
    fn framed(&mut self) {
        if let OldSettle::Seeking { .. } = self {
            *self = OldSettle::Still;
        }
    }
    fn observe(&mut self, current: f64, playing: bool, audio: bool) {
        match *self {
            OldSettle::Seeking { to, .. } if audio && (current - to).abs() < 0.25 => *self = OldSettle::Still,
            OldSettle::Holding { to } if playing || (current - to).abs() < 1e-3 => *self = OldSettle::Still,
            _ => {}
        }
    }
    fn expire(&mut self, now: Instant) {
        if let OldSettle::Seeking { since, .. } = *self
            && now.saturating_duration_since(since) > Duration::from_secs(2)
        {
            *self = OldSettle::Still;
        }
    }
    fn pending(self) -> bool {
        self != OldSettle::Still
    }
    fn waiting(self) -> bool {
        matches!(self, OldSettle::Seeking { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OldPoster {
    Unasked,
    Due,
    Withheld,
}

#[derive(Clone, Copy, Debug)]
struct OldPlayer {
    rate: f32,
    muted: bool,
    ready: bool,
    created: Instant,
    settle: OldSettle,
    shown: Option<f64>,
}

#[derive(Clone)]
struct Old {
    audio: bool,
    playing: bool,
    rate: f32,
    looping: bool,
    muted: bool,
    seek: Option<f64>,
    time: f64,
    duration: Option<f64>,
    size: Option<(u32, u32)>,
    meta: Option<OldMeta>,
    ended: bool,
    failed: Option<String>,
    placeholder: bool,
    generation: u64,
    frame_id: u64,
    anchor: Option<(f64, f64)>,
    player: Option<OldPlayer>,
    poster: OldPoster,
    // differs from dev: a finished poster answers the request that asked for it.
    done: Option<(u32, Done)>,
    // differs from dev: each video player fences the poster requests made before it.
    fence: u32,
    // differs from dev: the fence the newest request was made under.
    asked: u32,
    // differs from dev: the requests the worker still owes, oldest (in flight) first.
    pending: Vec<u32>,
    now: Instant,
    log: Vec<Cmd>,
}

impl Old {
    fn new(audio: bool, now: Instant) -> Self {
        Self {
            audio,
            playing: false,
            rate: 1.0,
            looping: false,
            muted: !audio,
            seek: None,
            time: 0.0,
            duration: None,
            size: None,
            meta: None,
            ended: false,
            failed: None,
            placeholder: false,
            generation: 0,
            frame_id: 0,
            anchor: None,
            player: None,
            poster: OldPoster::Unasked,
            done: None,
            fence: 0,
            asked: 0,
            pending: Vec::new(),
            now,
            log: Vec::new(),
        }
    }

    fn play(&mut self) {
        if self.failed.is_some() {
            return;
        }
        if self.ended {
            self.ended = false;
            self.seek = Some(0.0);
            self.time = 0.0;
        }
        self.playing = true;
        self.anchor = None;
    }
    fn pause(&mut self) {
        self.playing = false;
        self.anchor = None;
    }
    fn seek(&mut self, seconds: f64) {
        let seconds = if seconds.is_finite() { seconds.max(0.0) } else { 0.0 };
        let seconds = self.duration.map_or(seconds, |d| seconds.min(d));
        self.seek = Some(seconds);
        self.time = seconds;
        self.ended = false;
        self.anchor = None;
    }
    fn set_rate(&mut self, rate: f32) {
        self.rate = rate;
        self.anchor = None;
    }
    fn state(&self) -> PlayerState {
        if let Some(e) = &self.failed {
            PlayerState::Failed(e.clone())
        } else if self.meta.is_none() {
            PlayerState::Loading
        } else if self.playing {
            PlayerState::Playing
        } else if self.ended {
            PlayerState::Ended
        } else {
            PlayerState::Ready
        }
    }
    fn opened(&self) -> bool {
        self.meta.is_some() || self.failed.is_some()
    }
    fn open(&mut self, meta: Result<OldMeta, String>) {
        match meta {
            Ok(meta) => {
                self.meta = Some(meta);
                self.duration = Some(meta.duration);
                self.size = meta.size;
                self.time = self.time.min(meta.duration);
                self.seek = self.seek.map(|t| t.min(meta.duration));
                self.generation += 1;
            }
            Err(e) => self.fail(e),
        }
    }
    fn fail(&mut self, error: String) {
        if self.failed.is_none() {
            self.failed = Some(error);
            self.playing = false;
            self.generation += 1;
        }
    }
    fn next_frame_id(&mut self) -> u64 {
        self.frame_id += 1;
        self.frame_id
    }
    fn advance_virtual(&mut self, now: f64) {
        let Some(meta) = self.meta else { return };
        if let Some(t) = self.seek.take() {
            self.time = t.min(meta.duration);
            self.anchor = None;
        }
        if !self.playing {
            return;
        }
        let (start_clock, start_time) = match self.anchor {
            Some((clock, time)) if clock <= now => (clock, time),
            _ => *self.anchor.insert((now, self.time)),
        };
        let t = start_time + (now - start_clock).max(0.0) * self.rate as f64;
        if t < meta.duration {
            self.time = t;
        } else if self.looping && meta.duration > 0.0 {
            self.time = t % meta.duration;
        } else {
            self.time = meta.duration;
            self.playing = false;
            self.ended = true;
            self.anchor = None;
            self.generation += 1;
        }
    }
    fn observe(&mut self, time: f64, at_end: bool) -> bool {
        self.time = self.duration.map_or(time, |d| time.clamp(0.0, d));
        if !(self.playing && at_end) {
            return false;
        }
        if self.looping {
            self.time = 0.0;
            return true;
        }
        self.playing = false;
        self.ended = true;
        self.generation += 1;
        false
    }

    fn load_audio(&mut self) {
        if self.poster == OldPoster::Unasked && !self.opened() {
            self.request_poster(0.0);
        }
    }
    fn preload(&mut self) {
        if self.audio || self.player.is_some() || self.poster != OldPoster::Unasked || self.failed.is_some() {
            return;
        }
        let at = self.time;
        self.request_poster(at);
    }
    fn request_poster(&mut self, at: f64) {
        self.poster = if !self.audio { OldPoster::Due } else { OldPoster::Withheld };
        // differs from dev: the request remembers the fence it was made under.
        self.asked = self.fence;
        queue(&mut self.pending, self.fence);
        self.log.push(Cmd::Ask(at));
    }
    fn take_poster(&mut self, surface: bool) {
        let Some((asked, done)) = self.done.take() else { return };
        // differs from dev: a failed clip ignores the answer.
        if self.failed.is_some() {
            return;
        }
        // differs from dev: a request a player fenced brings facts, but no error or frame.
        let current = asked == self.fence;
        match done {
            Done::Err => {
                if current {
                    self.fail("load failed".into());
                }
            }
            Done::Facts | Done::Frame | Done::FrameFails => {
                if !self.opened() {
                    self.open(Ok(old_meta()));
                }
                if current && done != Done::Facts && surface && self.player.is_none() && self.poster == OldPoster::Due {
                    let id = self.next_frame_id();
                    if done == Done::FrameFails {
                        self.fail("poster push failed".into());
                    } else {
                        self.log.push(Cmd::Poster(id));
                    }
                }
            }
        }
    }
    fn drop_player(&mut self) {
        if self.player.take().is_some() {
            self.log.push(Cmd::Drop);
        }
    }
    fn check_item(&mut self, s: &Script, now: Instant) -> bool {
        let p = self.player.as_mut().unwrap();
        let failure = if s.item == Reports::Failed {
            Some("item failed".to_string())
        } else if s.item != Reports::Ready && now.saturating_duration_since(p.created) > Duration::from_secs(30) {
            Some("timed out loading after 30 s".to_string())
        } else {
            None
        };
        if let Some(e) = failure {
            self.fail(e);
            self.drop_player();
            return false;
        }
        if s.item == Reports::Ready && !p.ready {
            p.ready = true;
            self.log.push(Cmd::Orient);
            if !self.opened() {
                self.open(Ok(old_item_meta()));
            }
        }
        true
    }
    fn ensure_player(&mut self, s: &Script, now: Instant) -> bool {
        if self.player.is_some() {
            return true;
        }
        if self.audio && !self.opened() {
            if self.poster == OldPoster::Unasked {
                self.request_poster(0.0);
            }
            return false;
        }
        if s.make_fails {
            self.fail("couldn't make a player".into());
            return false;
        }
        self.log.push(Cmd::Make);
        if self.time > 0.0 && self.seek.is_none() {
            self.seek = Some(self.time);
        }
        self.player = Some(OldPlayer { rate: 0.0, muted: false, ready: false, created: now, settle: OldSettle::Still, shown: None });
        // differs from dev: a video player fences the poster requests before it.
        if !self.audio {
            self.fence += 1;
        }
        self.poster = OldPoster::Withheld;
        true
    }
    fn reconcile(&mut self, now: Instant) {
        let p = self.player.as_mut().unwrap();
        if p.muted != self.muted {
            self.log.push(Cmd::Muted(self.muted));
            p.muted = self.muted;
        }
        if p.ready
            && let Some(t) = self.seek.take()
        {
            self.log.push(Cmd::Seek(t));
            let same_frame = !self.playing && self.meta.zip(p.shown).is_some_and(|(m, shown)| m.frame_index(shown) == m.frame_index(t));
            p.settle.seek(t, same_frame, now);
        }
        if p.ready {
            let rate = if self.playing { self.rate } else { 0.0 };
            if p.rate != rate {
                self.log.push(Cmd::Rate(rate));
                p.rate = rate;
            }
        }
    }
    fn follow(&mut self, s: &Script, now: Instant, frame_time: Option<f64>) {
        let mut p = self.player.unwrap();
        p.settle.observe(s.current, self.playing, frame_time.is_none());
        let seeking = p.settle.pending();
        let stopped = p.rate > 0.0 && {
            self.log.push(Cmd::ReadRate);
            s.player_rate == 0.0
        };
        let at_end = !seeking && (self.duration.is_some_and(|d| s.current >= d - 1e-3) || stopped);
        let time = if seeking {
            self.time
        } else if self.playing && !at_end {
            frame_time.unwrap_or(s.current).max(0.0)
        } else {
            s.current
        };
        if self.observe(time, at_end) {
            self.log.push(Cmd::Restart(self.rate));
            p.rate = self.rate;
            p.settle.seek(0.0, false, now);
            self.generation += 1;
        } else if !self.playing {
            p.rate = 0.0;
        }
        self.player = Some(p);
    }
    fn finish(&mut self, now: Instant) -> bool {
        let p = self.player.as_mut().unwrap();
        p.settle.expire(now);
        let active = self.playing || p.settle.waiting() || !p.ready;
        let park = p.ready && !active;
        self.log.push(if park { Cmd::Park } else { Cmd::Unpark });
        active
    }
    fn pull(&mut self, s: &Script) -> bool {
        let now = self.now;
        self.take_poster(true);
        if self.failed.is_some() {
            self.drop_player();
            return false;
        }
        self.log.push(Cmd::OnMain);
        if !s.on_main {
            return false;
        }
        if self.player.is_none() && !self.playing {
            let want = self.seek.take().or_else(|| (self.poster == OldPoster::Unasked).then_some(self.time));
            if let Some(at) = want {
                self.request_poster(at);
            }
            return false;
        }
        let fresh = self.player.is_none();
        if !self.ensure_player(s, now) {
            return false;
        }
        if fresh {
            self.log.push(Cmd::Unpark);
        }
        if !self.check_item(s, now) {
            return false;
        }
        self.reconcile(now);
        if self.player.unwrap().ready {
            match s.frame {
                Frame::Same => {}
                Frame::New(shown) => {
                    let id = self.next_frame_id();
                    self.log.push(Cmd::Frame(id));
                    let p = self.player.as_mut().unwrap();
                    p.shown = Some(shown);
                    p.settle.framed();
                }
                Frame::PushFails => {
                    self.next_frame_id();
                    self.fail("frame push failed".into());
                    self.drop_player();
                    return false;
                }
            }
            self.follow(s, now, Some(s.item_time));
        }
        self.finish(now)
    }
    fn step_audio(&mut self, s: &Script) -> bool {
        self.log.push(Cmd::OnMain);
        if !s.on_main {
            return false;
        }
        let now = self.now;
        self.take_poster(false);
        if self.failed.is_some() {
            self.drop_player();
            return false;
        }
        if self.player.is_none() && !self.playing {
            return false;
        }
        let fresh = self.player.is_none();
        if !self.ensure_player(s, now) {
            return false;
        }
        if fresh {
            self.log.push(Cmd::Unpark);
        }
        if !self.check_item(s, now) {
            return false;
        }
        self.reconcile(now);
        if !self.player.unwrap().ready {
            return true;
        }
        self.follow(s, now, None);
        self.finish(now)
    }
    /// One element's pull, as the runtime made it.
    fn shared_pull(&mut self, s: &Script) -> bool {
        if self.audio {
            return self.step_audio(s);
        }
        let active = self.pull(s);
        if self.failed.is_some() && !self.placeholder {
            self.placeholder = true;
            self.log.push(Cmd::Placeholder);
        }
        active && self.failed.is_none()
    }
    fn apply(&mut self, s: &Script) {
        if self.failed.is_some() {
            return;
        }
        if self.audio {
            let needs_tick = self.step_audio(s);
            self.log.push(Cmd::Returned(needs_tick));
            return;
        }
        if self.player.is_none() {
            return;
        }
        let now = self.now;
        self.reconcile(now);
        if self.playing {
            self.log.push(Cmd::Unpark);
        }
    }
    fn park(&mut self) {
        let player = self.player.take();
        if !self.audio && player.as_ref().is_some_and(|p| p.shown.is_none()) {
            self.poster = OldPoster::Unasked;
        }
    }

    fn run(&mut self, sym: Sym) {
        match sym {
            Sym::Play => self.play(),
            Sym::Pause => self.pause(),
            Sym::Seek(t) => self.seek(t),
            Sym::Rate(r) => self.set_rate(r),
            Sym::Looping(l) => self.looping = l,
            Sym::Muted(m) => self.muted = m,
            Sym::Open => self.open(Ok(old_meta())),
            Sym::OpenFails => self.open(Err("open failed".into())),
            Sym::Fail => self.fail("failed".into()),
            Sym::Clock(t) => self.advance_virtual(t),
            Sym::Preload => self.preload(),
            Sym::LoadAudio => self.load_audio(),
            Sym::Done(done) => self.done = Some((answer(&mut self.pending, self.asked), done)),
            Sym::Wait(s) => self.now += Duration::from_secs(s),
            Sym::Park => self.park(),
            Sym::Apply => self.apply(&Script::READY),
            Sym::Pull(s) => {
                let active = self.shared_pull(&s);
                self.log.push(Cmd::Returned(active));
            }
        }
    }

    fn view(&self) -> View {
        let p = self.player.as_ref();
        View {
            state: self.state(),
            playing: self.playing,
            ended: self.ended && self.failed.is_none(),
            drawn: self.placeholder && self.failed.is_some(),
            anchor: if self.playing { self.anchor } else { None },
            rate: self.rate,
            seek: self.seek,
            time: self.time,
            duration: self.duration,
            size: self.size,
            generation: self.generation,
            frame_id: self.frame_id,
            poster: match self.poster {
                OldPoster::Unasked => 0,
                OldPoster::Due => 1,
                OldPoster::Withheld => 2,
            },
            fence: self.fence,
            player: p.is_some(),
            ready: p.is_some_and(|p| p.ready),
            created: p.map(|p| p.created),
            player_muted: p.is_some_and(|p| p.muted),
            player_rate: p.map_or(0.0, |p| p.rate),
            settle: p.map_or(SettleView::Still, |p| match p.settle {
                OldSettle::Still => SettleView::Still,
                OldSettle::Seeking { since, to } => SettleView::Seeking(since, to),
                OldSettle::Holding { to } => SettleView::Holding(to),
            }),
            shown: p.and_then(|p| p.shown),
            live: p.is_some_and(|p| p.ready && p.rate > 0.0 && !p.settle.waiting()),
            log: self.log.clone(),
        }
    }
}

fn old_meta() -> OldMeta {
    OldMeta { duration: 1.0, size: Some((64, 36)), frame: 0.25 }
}

fn old_item_meta() -> OldMeta {
    OldMeta { duration: 2.0, size: Some((32, 18)), frame: 1.0 / 30.0 }
}

// The script both sides follow.

#[derive(Clone, Copy, Debug, PartialEq)]
enum Reports {
    Loading,
    Ready,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Frame {
    Same,
    New(f64),
    PushFails,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Script {
    on_main: bool,
    make_fails: bool,
    item: Reports,
    frame: Frame,
    item_time: f64,
    current: f64,
    player_rate: f32,
}

impl Script {
    const LOADING: Script = Script { on_main: true, make_fails: false, item: Reports::Loading, frame: Frame::Same, item_time: 0.0, current: 0.0, player_rate: 0.0 };
    const READY: Script = Script { item: Reports::Ready, player_rate: 1.0, item_time: 0.4, current: 0.4, ..Script::LOADING };
    const FRAME: Script = Script { frame: Frame::New(0.3), item_time: 0.3, current: 0.3, ..Script::READY };
    const AT_SEEK: Script = Script { item_time: 0.3, current: 0.3, ..Script::READY };
    const END: Script = Script { item_time: 1.0, current: 1.0, ..Script::READY };
    const STOPPED: Script = Script { player_rate: 0.0, ..Script::READY };
    const ITEM_FAILS: Script = Script { item: Reports::Failed, ..Script::LOADING };
    const MAKE_FAILS: Script = Script { make_fails: true, ..Script::LOADING };
    const PUSH_FAILS: Script = Script { frame: Frame::PushFails, ..Script::READY };
    const OFF_MAIN: Script = Script { on_main: false, ..Script::READY };
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Done {
    Facts,
    Frame,
    FrameFails,
    Err,
}

#[derive(Clone, Copy, Debug)]
enum Sym {
    Play,
    Pause,
    Seek(f64),
    Rate(f32),
    Looping(bool),
    Muted(bool),
    Open,
    OpenFails,
    Fail,
    Clock(f64),
    Preload,
    LoadAudio,
    Done(Done),
    Wait(u64),
    Park,
    Apply,
    Pull(Script),
}

/// What the native side was told to do, in order.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Cmd {
    Ask(f64),
    Poster(u64),
    Make,
    Drop,
    Unpark,
    Park,
    Orient,
    Muted(bool),
    Seek(f64),
    Rate(f32),
    Restart(f32),
    Frame(u64),
    Placeholder,
    Returned(bool),
    /// Checked for the main thread.
    OnMain,
    /// Read the player's rate.
    ReadRate,
}

#[derive(Clone, Debug, PartialEq)]
enum SettleView {
    Still,
    Seeking(Instant, f64),
    Holding(f64),
}

/// Everything either side can show.
#[derive(Clone, Debug, PartialEq)]
struct View {
    state: PlayerState,
    playing: bool,
    ended: bool,
    drawn: bool,
    anchor: Option<(f64, f64)>,
    rate: f32,
    seek: Option<f64>,
    time: f64,
    duration: Option<f64>,
    size: Option<(u32, u32)>,
    generation: u64,
    frame_id: u64,
    poster: u8,
    fence: u32,
    player: bool,
    ready: bool,
    created: Option<Instant>,
    player_muted: bool,
    player_rate: f32,
    settle: SettleView,
    shown: Option<f64>,
    live: bool,
    log: Vec<Cmd>,
}

// The new code, on a scripted native player.

#[derive(Clone)]
struct Fake {
    script: Script,
    /// The caller already checked for the main thread, as an audio tick does.
    checked: bool,
    surface: bool,
    done: Option<(Attempt, Done)>,
    kept: Option<Done>,
    /// The attempt of the newest poster request.
    asked: Attempt,
    /// The requests the worker still owes, oldest (in flight) first.
    pending: Vec<Attempt>,
    player: bool,
    now: Instant,
    log: Vec<Cmd>,
}

impl Port for Fake {
    fn on_main(&mut self) -> bool {
        if !self.checked {
            self.checked = true;
            self.log.push(Cmd::OnMain);
        }
        self.script.on_main
    }
    fn take_poster(&mut self) -> Option<(Attempt, Result<Meta, String>)> {
        let (attempt, done) = self.done.take()?;
        if done == Done::Err {
            return Some((attempt, Err("load failed".into())));
        }
        self.kept = Some(done);
        Some((attempt, Ok(Meta { duration: 1.0, size: Some((64, 36)), frame: 0.25 })))
    }
    fn show_poster(&mut self, next_id: &mut dyn FnMut() -> u64) -> Result<(), String> {
        match (self.kept.take(), self.surface) {
            (Some(Done::Frame), true) => {
                let id = next_id();
                self.log.push(Cmd::Poster(id));
                Ok(())
            }
            (Some(Done::FrameFails), true) => {
                next_id();
                Err("poster push failed".into())
            }
            _ => Ok(()),
        }
    }
    fn ask_poster(&mut self, at: f64, attempt: Attempt) {
        self.asked = attempt;
        queue(&mut self.pending, attempt);
        self.log.push(Cmd::Ask(at));
    }
    fn make_player(&mut self) -> Result<Instant, String> {
        if self.script.make_fails {
            return Err("couldn't make a player".into());
        }
        self.log.push(Cmd::Make);
        self.player = true;
        Ok(self.now)
    }
    fn drop_player(&mut self) {
        self.player = false;
        self.log.push(Cmd::Drop);
    }
    fn unpark(&mut self) {
        self.log.push(Cmd::Unpark);
    }
    fn item(&self) -> Item {
        match self.script.item {
            Reports::Loading => Item::Loading,
            Reports::Ready => Item::Ready,
            Reports::Failed => Item::Failed("item failed".into()),
        }
    }
    fn orient(&mut self) {
        self.log.push(Cmd::Orient);
    }
    fn item_facts(&self) -> Meta {
        Meta { duration: 2.0, size: Some((32, 18)), frame: 1.0 / 30.0 }
    }
    fn alive(&self) -> bool {
        self.player
    }
    fn set_muted(&mut self, muted: bool) {
        self.log.push(Cmd::Muted(muted));
    }
    fn set_volume(&mut self, _: f32) {}
    fn seek_to(&mut self, at: f64) {
        self.log.push(Cmd::Seek(at));
    }
    fn set_rate(&mut self, rate: f32) {
        self.log.push(Cmd::Rate(rate));
    }
    fn restart(&mut self, rate: f32) {
        self.log.push(Cmd::Restart(rate));
    }
    fn frame(&mut self, next_id: &mut dyn FnMut() -> u64) -> (f64, Result<Option<f64>, String>) {
        let landed = match self.script.frame {
            Frame::Same => Ok(None),
            Frame::New(shown) => {
                let id = next_id();
                self.log.push(Cmd::Frame(id));
                Ok(Some(shown))
            }
            Frame::PushFails => {
                next_id();
                Err("frame push failed".into())
            }
        };
        (self.script.item_time, landed)
    }
    fn current(&self) -> f64 {
        self.script.current
    }
    fn stopped(&mut self) -> bool {
        self.log.push(Cmd::ReadRate);
        self.script.player_rate == 0.0
    }
}

#[derive(Clone)]
struct New {
    c: Control,
    fake: Fake,
}

impl New {
    fn new(audio: bool, now: Instant) -> Self {
        let kind = if audio { Kind::Audio } else { Kind::Video };
        Self {
            c: Control { muted: !audio, kind, ..Control::default() },
            fake: Fake { script: Script::READY, checked: false, surface: !audio, done: None, kept: None, asked: Attempt::default(), pending: Vec::new(), player: false, now, log: Vec::new() },
        }
    }

    /// `macos::step_audio` and `macos::finish`, around the real turn.
    fn tick(&mut self, script: Script) -> bool {
        let audio = self.c.kind == Kind::Audio;
        if audio {
            self.fake.log.push(Cmd::OnMain);
            if !script.on_main {
                return false;
            }
        }
        self.fake.script = script;
        self.fake.checked = audio;
        let now = self.fake.now;
        match turn(&mut self.c, &mut self.fake, now) {
            Turn::Idle => false,
            Turn::Loading => true,
            Turn::Finished => {
                let (active, park) = (self.c.active(), self.c.parkable());
                self.fake.log.push(if park { Cmd::Park } else { Cmd::Unpark });
                active
            }
        }
    }

    /// `Shared::pull`, around the real turn.
    fn shared_pull(&mut self, script: Script) -> bool {
        let active = self.tick(script);
        if self.c.kind == Kind::Audio {
            return active;
        }
        for effect in step(&mut self.c, Event::Drawn) {
            if effect == Effect::Placeholder {
                self.fake.log.push(Cmd::Placeholder);
            }
        }
        active && !self.c.failed()
    }

    /// `macos::apply`, around the real reconcile.
    fn apply(&mut self) {
        if self.c.failed() {
            return;
        }
        if self.c.kind == Kind::Audio {
            let needs_tick = self.tick(Script::READY);
            self.fake.log.push(Cmd::Returned(needs_tick));
            return;
        }
        if !self.fake.player {
            return;
        }
        let now = self.fake.now;
        self.fake.script = Script::READY;
        reconcile(&mut self.c, &mut self.fake, now);
        if self.c.playing() {
            self.fake.log.push(Cmd::Unpark);
        }
    }

    fn send(&mut self, event: Event) {
        let effects = step(&mut self.c, event);
        super::super::turn::carry(&mut self.c, &mut self.fake, effects);
    }

    fn run(&mut self, sym: Sym) {
        match sym {
            Sym::Play => self.send(Event::Play),
            Sym::Pause => self.send(Event::Pause),
            Sym::Seek(t) => self.send(Event::Seek(t)),
            Sym::Rate(r) => self.send(Event::Rate(r)),
            Sym::Looping(l) => self.c.looping = l,
            Sym::Muted(m) => self.c.muted = m,
            Sym::Open => self.c.open(Ok(Meta { duration: 1.0, size: Some((64, 36)), frame: 0.25 })),
            Sym::OpenFails => self.c.open(Err("open failed".into())),
            Sym::Fail => self.c.fail("failed".into()),
            Sym::Clock(t) => self.c.advance_virtual(t),
            Sym::Preload => self.send(Event::Preload),
            Sym::LoadAudio => self.send(Event::LoadAudio),
            Sym::Done(done) => self.fake.done = Some((answer(&mut self.fake.pending, self.fake.asked), done)),
            Sym::Wait(s) => self.fake.now += Duration::from_secs(s),
            Sym::Park => {
                step(&mut self.c, Event::Parked);
                self.fake.player = false;
            }
            Sym::Apply => self.apply(),
            Sym::Pull(s) => {
                let active = self.shared_pull(s);
                self.fake.log.push(Cmd::Returned(active));
            }
        }
    }

    fn view(&self) -> View {
        let c = &self.c;
        assert_eq!(c.stage.has_player(), self.fake.player, "the stage says what the native side holds");
        let (created, player_muted, player_rate, settle, shown) = match c.stage {
            Stage::Loading { created, muted, .. } => (Some(created), muted, 0.0, Settle::Still, None),
            Stage::Ready { created, muted, rate, settle, shown, .. } => (Some(created), muted, rate, settle, shown),
            _ => (None, false, 0.0, Settle::Still, None),
        };
        View {
            state: c.state(),
            playing: c.playing(),
            ended: c.run == Run::Ended,
            drawn: matches!(c.run, Run::Failed { drawn: true, .. }),
            anchor: if let Run::Playing { anchor } = c.run { anchor } else { None },
            rate: c.rate,
            seek: c.seek,
            time: c.time,
            duration: c.duration,
            size: c.size,
            generation: c.generation,
            frame_id: c.frame_id,
            poster: match c.stage {
                Stage::Unasked => 0,
                Stage::Poster => 1,
                _ => 2,
            },
            fence: c.posters.current().0,
            player: c.stage.has_player(),
            ready: c.stage.ready(),
            created,
            player_muted,
            player_rate,
            settle: match settle {
                Settle::Still => SettleView::Still,
                Settle::Seeking { since, to } => SettleView::Seeking(since, to),
                Settle::Holding { to } => SettleView::Holding(to),
            },
            shown,
            live: c.live(),
            log: self.fake.log.clone(),
        }
    }
}

/// The poster worker's queue: the request in flight, then the newest one waiting, which a
/// later request replaces.
fn queue<T>(pending: &mut Vec<T>, request: T) {
    if pending.len() == 2 {
        pending.pop();
    }
    pending.push(request);
}

/// The request a finished load answers: the one in flight, else (a load the script made up) the
/// newest.
fn answer<T: Copy>(pending: &mut Vec<T>, newest: T) -> T {
    if pending.is_empty() { newest } else { pending.remove(0) }
}

/// Walks every sequence up to `depth` from `(new, old)`, comparing after every symbol.
fn descend(new: &New, old: &Old, alphabet: &[Sym], depth: usize, seq: &mut Vec<Sym>, walked: &mut u64) {
    if depth == 0 {
        return;
    }
    for &sym in alphabet {
        let (mut n, mut o) = (new.clone(), old.clone());
        seq.push(sym);
        n.run(sym);
        o.run(sym);
        assert_eq!(n.view(), o.view(), "after {seq:?}");
        n.fake.log.clear();
        o.log.clear();
        *walked += 1;
        descend(&n, &o, alphabet, depth - 1, seq, walked);
        seq.pop();
    }
}

/// Each kind of clip, from the state `prefix` leaves it in.
fn walk(prefix: &[Sym], alphabet: &[Sym], depth: usize) {
    let now = Instant::now();
    for audio in [false, true] {
        let (mut new, mut old) = (New::new(audio, now), Old::new(audio, now));
        let mut seq = Vec::new();
        for &sym in prefix {
            seq.push(sym);
            new.run(sym);
            old.run(sym);
            assert_eq!(new.view(), old.view(), "after {seq:?}");
            new.fake.log.clear();
            old.log.clear();
        }
        let mut walked = 0;
        descend(&new, &old, alphabet, depth, &mut seq, &mut walked);
        assert!(walked >= alphabet.len().pow(depth as u32) as u64);
    }
}

use Sym::{Apply, Done as Land, Fail, LoadAudio, Looping, Muted, Open, OpenFails, Park, Pause, Play, Preload, Pull, Rate, Seek, Wait};

#[test]
fn the_handle_and_the_headless_clock_match_the_old_flags() {
    let clock = Sym::Clock;
    walk(&[], &[Play, Pause, Seek(0.3), Seek(9.0), Seek(f64::NAN), Rate(2.0), Looping(true), Open, OpenFails, Fail, clock(0.0), clock(0.4), clock(2.0)], 5);
}

#[test]
fn a_playing_headless_clip_ends_loops_and_rewinds_like_the_old_flags() {
    let clock = Sym::Clock;
    walk(&[Open, Play], &[Play, Pause, Seek(0.3), Rate(2.0), Looping(true), Looping(false), clock(0.0), clock(0.4), clock(2.0)], 6);
}

#[test]
fn every_report_in_any_order_matches_the_old_flags() {
    walk(
        &[],
        &[
            Play,
            Pause,
            Seek(0.3),
            Muted(false),
            Land(Done::Frame),
            Land(Done::Facts),
            Land(Done::FrameFails),
            Land(Done::Err),
            Preload,
            LoadAudio,
            Fail,
            Park,
            Apply,
            Wait(31),
            Pull(Script::LOADING),
            Pull(Script::FRAME),
            Pull(Script::END),
            Pull(Script::STOPPED),
            Pull(Script::ITEM_FAILS),
            Pull(Script::MAKE_FAILS),
            Pull(Script::PUSH_FAILS),
            Pull(Script::OFF_MAIN),
        ],
        5,
    );
}

#[test]
fn posters_and_players_race_like_the_old_flags() {
    walk(
        &[],
        &[Play, Pause, Seek(0.3), Land(Done::Frame), Land(Done::Facts), Preload, Pull(Script::LOADING), Pull(Script::FRAME), Park],
        7,
    );
}

#[test]
fn a_failing_poster_or_frame_stops_the_clip_like_the_old_flags() {
    walk(&[], &[Play, Land(Done::FrameFails), Land(Done::Frame), Pull(Script::LOADING), Pull(Script::PUSH_FAILS), Pull(Script::FRAME), Park], 6);
}

#[test]
fn a_ready_player_seeks_ends_and_parks_like_the_old_flags() {
    walk(
        &[Open, Play, Pull(Script::READY)],
        &[Play, Pause, Seek(0.3), Seek(0.31), Rate(2.0), Looping(true), Wait(3), Apply, Park, Pull(Script::FRAME), Pull(Script::AT_SEEK), Pull(Script::END), Pull(Script::STOPPED)],
        6,
    );
}

#[test]
fn a_loading_player_times_out_like_the_old_flags() {
    walk(&[Play, Land(Done::Facts), Pull(Script::LOADING)], &[Seek(0.3), Muted(false), Pause, Wait(31), Wait(3), Apply, Pull(Script::LOADING), Pull(Script::READY), Park], 5);
}

#[path = "phases_poster_tests.rs"]
mod poster;

#[test]
fn the_newest_seek_replaces_an_earlier_one() {
    let now = Instant::now();
    let mut s = Settle::default();
    s.seek(3.0, false, now);
    assert!(s.waiting());
    // Paused, back within the frame on screen before the far seek landed: that frame is
    // already showing, so there's nothing left to wait for.
    s.seek(0.5, true, now);
    assert!(!s.waiting() && s.pending());
    s.observe(0.5, false, false);
    assert_eq!(s, Settle::Still);
}

#[test]
fn a_seek_lands_with_a_frame_or_for_audio_at_its_position() {
    let now = Instant::now();
    let mut video = Settle::default();
    video.seek(2.0, false, now);
    video.observe(2.0, false, false);
    assert!(video.waiting(), "video waits for the frame, not the position");
    video.framed();
    assert_eq!(video, Settle::Still);

    let mut audio = Settle::default();
    audio.seek(2.0, false, now);
    audio.observe(1.0, true, true);
    assert!(audio.waiting());
    audio.observe(2.1, true, true);
    assert_eq!(audio, Settle::Still);
}

#[test]
fn a_hold_ends_when_playback_moves_on_and_a_frame_does_not_end_it() {
    let now = Instant::now();
    let mut s = Settle::default();
    s.seek(0.5, true, now);
    s.framed();
    assert_eq!(s, Settle::Holding { to: 0.5 });
    s.observe(0.4, true, false);
    assert_eq!(s, Settle::Still);
}

#[test]
fn a_seek_that_never_lands_expires() {
    let now = Instant::now();
    let mut s = Settle::default();
    s.seek(2.0, false, now);
    s.expire(now + SEEK_PATIENCE);
    assert!(s.waiting());
    s.expire(now + SEEK_PATIENCE + Duration::from_millis(1));
    assert_eq!(s, Settle::Still);
    // A hold has nothing to wait for, so nothing expires it.
    s.seek(1.0, true, now);
    s.expire(now + Duration::from_secs(60));
    assert_eq!(s, Settle::Holding { to: 1.0 });
}
