//! Every short sequence through a clip's machine, since the handle, the clocks, the poster worker
//! and the native player report in whatever order they like. The real [`step`] and the real
//! [`turn`](super::super::turn::turn) run against a scripted native player; beside them runs
//! `Old`, a transcription of the fields and branches the machine replaced, which calls none of it.

use super::super::turn::{Port, Turn, reconcile, turn};
use super::*;
use rune_core::machine::Attempt;

#[path = "phases_old_tests.rs"]
mod old;
use old::Old;

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
        Some((attempt, Ok(Meta { duration: 1.0, size: Some((64, 36)), frame: 0.25, sound: false })))
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
        Meta { duration: 2.0, size: Some((32, 18)), frame: 1.0 / 30.0, sound: false }
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
            Sym::Open => self.c.open(Ok(Meta { duration: 1.0, size: Some((64, 36)), frame: 0.25, sound: false })),
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
    let now = rune_core::time::now();
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
    let now = rune_core::time::now();
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
    let now = rune_core::time::now();
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
    let now = rune_core::time::now();
    let mut s = Settle::default();
    s.seek(0.5, true, now);
    s.framed();
    assert_eq!(s, Settle::Holding { to: 0.5 });
    s.observe(0.4, true, false);
    assert_eq!(s, Settle::Still);
}

#[test]
fn a_seek_that_never_lands_expires() {
    let now = rune_core::time::now();
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
