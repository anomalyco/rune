//! A transcription of the pre-machine fields and branches, which calls none of `phases` or `turn`,
//! used as an oracle by the exhaustive sequence tests in `phases_tests.rs`.

use super::*;

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
pub(super) struct Old {
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
    // A finished poster answers the request that asked for it.
    done: Option<(u32, Done)>,
    // Each video player fences the poster requests made before it.
    fence: u32,
    // The fence the newest request was made under.
    asked: u32,
    // differs from dev: the time the fence's requests asked for, until a player takes over.
    fence_at: Option<f64>,
    // The requests the worker still owes, oldest (in flight) first.
    pending: Vec<u32>,
    now: Instant,
    pub(super) log: Vec<Cmd>,
}

impl Old {
    pub(super) fn new(audio: bool, now: Instant) -> Self {
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
            fence_at: None,
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
        // differs from dev: a video request for a new time fences the ones before it.
        if !self.audio && self.fence_at != Some(at) {
            self.fence += 1;
            self.fence_at = Some(at);
        }
        // The request remembers the fence it was made under.
        self.asked = self.fence;
        queue(&mut self.pending, self.fence);
        self.log.push(Cmd::Ask(at));
    }
    fn take_poster(&mut self, surface: bool) {
        let Some((asked, done)) = self.done.take() else { return };
        // A failed clip ignores the answer.
        if self.failed.is_some() {
            return;
        }
        // A request a player fenced brings facts, but no error or frame.
        let current = asked == self.fence;
        match done {
            Done::Err => {
                // differs from dev: an audio load error after the clip opened is ignored.
                if current && !self.overtaken() && (!self.audio || !self.opened()) {
                    self.fail("load failed".into());
                }
            }
            Done::Facts | Done::Frame | Done::FrameFails => {
                if !self.opened() {
                    self.open(Ok(old_meta()));
                }
                if current && !self.overtaken() && done != Done::Facts && surface && self.player.is_none() && self.poster == OldPoster::Due {
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
    /// differs from dev: a seek made since the request names another time, so its answer is for
    /// a time the clip has left even before the next pull asks for the new one. Both clamp to
    /// the clip's length, so a seek past the end is the poster's own time.
    fn overtaken(&self) -> bool {
        let clamp = |t: f64| self.duration.map_or(t, |d| t.min(d));
        self.fence_at.is_some_and(|asked| self.seek.is_some_and(|target| clamp(target) != clamp(asked)))
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
        // A video player fences the poster requests before it.
        if !self.audio {
            self.fence += 1;
            // differs from dev: the next request is new, whatever its time.
            self.fence_at = None;
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

    pub(super) fn run(&mut self, sym: Sym) {
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

    pub(super) fn view(&self) -> View {
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
