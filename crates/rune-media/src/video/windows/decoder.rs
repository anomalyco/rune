//! One clip's decoder thread: opens the source with Media Foundation, decodes a few pictures
//! ahead into the player's GPU ring, hands audio to the sound output, loops without a gap by
//! seeking back while the last pictures still wait in the queue, and stops when told or when no
//! frame has drawn the clip for a while. The UI thread only takes pictures off the queue.
//!
//! The queue is the decoder's only clock: it decodes while there's room, so a clip nobody
//! draws fills it and sleeps, costing nothing until a frame takes a picture again.

#![allow(unsafe_code)]

use super::audio::{self, Output, Queue};
use super::gpu::{self, Ring};
use super::inbox::{Decoded, Inbox, News, Pic, Pipe};
use super::layer::Layer;
use super::machine::Anchor;
use super::mf::{self, Decode, Picture, Sound};
use super::super::{Kind, MediaSource, Meta};
use rune_core::time::Instant;
use std::sync::{Arc, MutexGuard};
use std::time::Duration;
use windows::Win32::Graphics::Direct3D11::{D3D11_BIND_DECODER, ID3D11Texture2D};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::core::Interface;

/// Pictures decoded ahead of the one on screen.
const AHEAD: usize = 3;
/// A decoder no frame has taken a picture from for this long lets go of its source, decoder and
/// textures; the clip keeps its last picture and reopens when drawn again.
const PARK: Duration = Duration::from_secs(3);
/// How often a decoder with room but no free texture looks at the release fence again.
const RING_POLL: Duration = Duration::from_millis(2);

/// How long before a picture's time the decoder presents it on the clip's layer: the display
/// shows it at the first refresh after.
const LAYER_LEAD: Duration = Duration::from_millis(2);

/// Sound samples read in a row while a picture could be read instead. Enough for the sound to
/// gain on the clock at any picture rate (8 AAC packets are about 170 ms of sound), and few
/// enough that sound with a lot to catch up on (after a seek, or right after unmute) never holds
/// the pictures back.
const SOUND_BURST: u32 = 8;

/// While muted, the audio stream stays selected and advances this far behind playback so
/// unmuting mid-clip starts at once and in sync without opening a WASAPI output while muted, and
/// without discarding a sample before it would be heard.
const MUTE_MARGIN: f64 = 0.1;

/// A layer that presented nothing for this long shows its decoder's next picture however late
/// (`superseded`), so a decoder that can't keep up with the clip still moves.
const LAYER_STARVED: Duration = Duration::from_millis(250);

/// How long a clip that opens before Rune has a GPU waits for it (`rune_device`).
const GPU_WAIT: Duration = Duration::from_secs(3);

/// `decode`'s answer when Rune's device was replaced under the ring: not a failure.
const DEVICE_CHANGED: &str = "Rune's GPU device changed";

/// What opening found.
#[derive(Clone, Copy, Debug)]
pub(super) struct Facts {
    pub meta: Meta,
    pub picture: Option<Picture>,
    pub sound: Option<Sound>,
    /// Pictures come out of a hardware decoder.
    pub hardware: bool,
    /// Pictures reach the screen on the GPU (false: through system memory).
    pub gpu: bool,
}

/// Where a seek lands. The reader starts at the key frame before the target, so the pictures
/// before the target are decoded only to reach it. A target after the clip's last picture keeps
/// none of them: at the stream's end the decoder goes back, once, for the newest one it passed,
/// so the clip shows its last picture instead of waiting for one that never comes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Landing {
    /// Clip seconds: pictures before this are dropped, and sound that ends before it.
    pub skip_before: f64,
    /// The newest picture dropped on the way.
    passed: Option<f64>,
    /// A picture was kept.
    kept: bool,
    /// The decoder already went back for the last picture.
    went_back: bool,
}

impl Landing {
    pub(super) fn at(skip_before: f64) -> Self {
        Self { skip_before, passed: None, kept: false, went_back: false }
    }

    /// Keeps the first decoded picture returned after `mf::seek` without decoding forward to a
    /// target time.
    pub(super) fn keyframe() -> Self {
        Self::at(f64::NEG_INFINITY)
    }

    /// Whether the picture at clip second `time` is kept: the one nearest the target, and all
    /// after it. `frame` is the seconds a picture lasts.
    pub(super) fn keeps(&mut self, time: f64, frame: f64) -> bool {
        let keep = time + frame * 0.5 >= self.skip_before;
        if keep {
            self.kept = true;
        } else {
            self.passed = Some(self.passed.map_or(time, |passed| passed.max(time)));
        }
        keep
    }

    /// A picture was kept: the seek has landed.
    pub(super) fn landed(&self) -> bool {
        self.kept
    }

    /// The video stream ended. When no picture was kept, the time of the last one to seek back
    /// to; the landing then keeps that picture. None once a picture was kept, or the second time.
    pub(super) fn ended(&mut self) -> Option<f64> {
        if self.kept || self.went_back {
            return None;
        }
        let last = self.passed?;
        *self = Self { skip_before: last, passed: None, kept: false, went_back: true };
        Some(last)
    }
}

/// A stream of the clip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Stream {
    Sound,
    Picture,
}

/// Which stream the decoder reads next; None when it waits. `sound`: the sound output wants
/// more (it holds less than a second ahead). `picture`: the queue has room and a texture is
/// free. Each stream is read by its own index and for its own need: read as they come in the
/// file (any stream, only while the queue has room for a picture), the sound was never more
/// than the picture queue ahead of the clock, three pictures or one on the layer, while the
/// sound output writes about 100 ms ahead of it, so every sample came too late to be heard.
/// `landed`: a picture of the current seek was kept, before which nothing is heard and the
/// pictures come first. `burst`: sound samples read since the last picture (`SOUND_BURST`).
pub(super) fn next_read(sound: bool, picture: bool, landed: bool, burst: u32) -> Option<Stream> {
    match (sound, picture) {
        (false, false) => None,
        (true, false) => Some(Stream::Sound),
        (false, true) => Some(Stream::Picture),
        (true, true) if !landed || burst >= SOUND_BURST => Some(Stream::Picture),
        (true, true) => Some(Stream::Sound),
    }
}

/// Unwrapped media time up to which muted audio may advance without discarding unplayed sound.
/// Caps at both the running clock (so a low-fps clip whose picture queue sits seconds ahead never
/// drops future sound) and the screen's video position (so an undrawn muted clip whose picture
/// queue froze stops advancing and stays ready for resume or parking). `front` is the first
/// queued picture (one frame ahead of screen off the layer; the presented picture on it).
pub(super) fn muted_audio_target(clock_pos: Option<f64>, front: Option<f64>, frame: f64, layered: bool, has_video: bool) -> Option<f64> {
    let video_pos = front.map(|pts| if layered { pts } else { (pts - frame).max(0.0) });
    let pos = match (clock_pos, video_pos) {
        (Some(c), Some(v)) => Some(c.min(v)),
        (None, Some(v)) => Some(v),
        (Some(c), None) if !has_video => Some(c),
        _ => None,
    };
    pos.map(|p| p - MUTE_MARGIN)
}

/// Whether muted audio should read the next sample to stay near playback: true while the stream
/// is still behind `target` (defaulting to `epoch` before any sample was read after a seek or
/// loop, so a clip at the start of a loop never discards its first packet).
pub(super) fn muted_audio_want(audio_pts: Option<f64>, epoch: f64, target: Option<f64>) -> bool {
    target.is_some_and(|t| audio_pts.unwrap_or(epoch) < t)
}

/// Whether every stream that controls the clip's end or loop has reached its end. A muted video
/// keeps its audio stream selected to stay in position for unmute, but still ends or loops with
/// its pictures so trailing muted audio never stalls the end.
pub(super) fn clip_done(video_open: bool, audio_open: bool, has_video: bool, output_open: bool) -> bool {
    !video_open && (!audio_open || (has_video && !output_open))
}

/// Next loop's unwrapped start: at least `epoch + duration`, and past any trailing audio.
pub(super) fn next_loop_epoch(epoch: f64, duration: f64, audio_end: Option<f64>) -> f64 {
    audio_end.map_or(epoch + duration, |end| (epoch + duration).max(end))
}

/// Unwrapped time when trailing audio or an audio-only clip finishes draining.
pub(super) fn drain_end(epoch: f64, duration: f64, video_end: Option<f64>, audio_end: Option<f64>) -> Option<f64> {
    let duration_end = (duration > 0.0).then_some(epoch + duration);
    [duration_end, video_end, audio_end].into_iter().flatten().reduce(f64::max)
}

/// When `pts` is due on the layer for `current`, or `None` if stopped or rate changed.
pub(super) fn layer_due(initial: Anchor, current: Option<Anchor>, pts: f64) -> Option<Instant> {
    let due = current.filter(|c| c.rate == initial.rate)?.when(pts);
    Some(due.checked_sub(LAYER_LEAD).unwrap_or(due))
}

/// Whether the layer skips the picture at `pts`: on the clock, now at `pos`, its successor is
/// due too (`frame` is the seconds a picture lasts). The window's own path shows only the
/// newest picture whose time has come; the layer presents each picture it's given, one per
/// refresh, so without this a decoder that fell behind plays its backlog fast, and a clip with
/// more pictures a second than the display has refreshes never catches up with its clock.
/// `since` is how long ago the layer last presented (None: not yet): after `LAYER_STARVED` it
/// shows the picture anyway.
pub(super) fn superseded(pos: f64, pts: f64, frame: f64, since: Option<Duration>) -> bool {
    pos >= pts + frame && since.is_some_and(|since| since < LAYER_STARVED)
}

/// What a decoder starts with.
pub(super) struct Job {
    pub source: MediaSource,
    pub kind: Kind,
    /// Clip seconds to start at, whether that start is exact (vs keyframe-only), and that
    /// start's generation.
    pub start: f64,
    pub exact: bool,
    pub generation: u32,
    pub looping: bool,
    pub sound: Option<f32>,
    /// Wakes the windows drawing the clip (news, or a picture after a wait).
    pub notify: Box<dyn Fn() + Send>,
    /// The clock audio follows.
    pub clock: Arc<audio::Clock>,
    /// When the UI asked for the clip.
    pub started: Instant,
}

/// Starts a decoder thread; it runs until `stop` or parking.
pub(super) fn start(job: Job) -> Arc<Pipe> {
    let pipe = Pipe::new(Inbox { looping: job.looping, sound: job.sound, touched: Some(rune_core::time::now()), ..Inbox::default() });
    let thread_pipe = pipe.clone();
    let spawned = std::thread::Builder::new().name("rune-video-decode".into()).spawn(move || {
        // SAFETY: COM for this thread, matched below.
        let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
        run(&thread_pipe, job);
        if com {
            // SAFETY: matches the initialisation above; every COM object of this thread is gone.
            unsafe { CoUninitialize() };
        }
    });
    if spawned.is_err() {
        pipe.lock().news.push(News::Failed("couldn't start a video decoder thread".into()));
    }
    pipe
}

/// `RUNE_VIDEO_LOG=1` prints each open's steps in microseconds.
pub(super) fn logging() -> bool {
    log_level() > 0
}

/// `RUNE_VIDEO_LOG=frames`: every picture shown and every pull's cost too.
pub(super) fn logging_frames() -> bool {
    log_level() > 1
}

fn log_level() -> u8 {
    static LEVEL: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *LEVEL.get_or_init(|| match std::env::var_os("RUNE_VIDEO_LOG") {
        Some(v) if v == "frames" => 2,
        Some(v) if v != "0" => 1,
        _ => 0,
    })
}

/// A `RUNE_VIDEO_LOG` line, stamped with microseconds since the first.
macro_rules! log {
    ($($arg:tt)*) => {
        eprintln!("rune video {:>10} us: {}", $crate::video::windows::decoder::log_clock(), format!($($arg)*))
    };
}
pub(super) use log;

/// Microseconds since the first `log!` line: one clock for every line, wherever it's written.
pub(super) fn log_clock() -> u128 {
    static T0: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    T0.get_or_init(rune_core::time::now).elapsed().as_micros()
}

/// Microseconds from `t0` to now.
pub(super) fn us(t0: Instant) -> u128 {
    t0.elapsed().as_micros()
}

/// The decoders' device on Rune's GPU. A window drawn on the CPU first (`window::cpu_frame`)
/// makes its GPU after its first frame, and a clip that opens before then would decode on the CPU
/// for good: it waits for the GPU as long as one takes to come, while the source resolves.
fn rune_device() -> Option<Arc<gpu::Device>> {
    let deadline = rune_core::time::now() + GPU_WAIT;
    loop {
        match gpu::device() {
            Ok(device) => return Some(device),
            Err(_) if crate::native_surface::windows::presenting_device().is_none() && rune_core::time::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(_) => return None,
        }
    }
}

fn run(pipe: &Pipe, job: Job) {
    rune_core::marks::mark("video.open");
    let generation = crate::native_surface::windows::device_generation();
    let mut d = match Decoder::open(&job) {
        Ok(d) => d,
        // An open that failed as Rune lost its device opens again on the next one.
        Err(_) if gpu::lost_since(generation) => {
            if logging() {
                log!("Rune's GPU device was lost while the clip opened: it opens again on the next one");
            }
            pipe.lock().news.push(News::Parked);
            (job.notify)();
            return;
        }
        Err(e) => {
            pipe.lock().news.push(News::Failed(e));
            (job.notify)();
            return;
        }
    };
    rune_core::marks::mark("video.opened");
    pipe.lock().news.push(News::Opened(d.facts));
    (job.notify)();
    match d.decode(pipe, &job) {
        Err(e) if e == DEVICE_CHANGED || gpu::lost_since(generation) => {
            if logging() {
                log!("Rune's GPU device changed: the clip opens again on the new one");
            }
            pipe.lock().news.push(News::Parked);
            (job.notify)();
        }
        Err(e) => {
            pipe.lock().news.push(News::Failed(e));
            (job.notify)();
        }
        Ok(()) => {}
    }
}

struct Decoder {
    reader: mf::Reader,
    facts: Facts,
    ring: Option<Ring>,
    audio_queue: Option<Queue>,
    output: Option<Output>,
    /// Whether the UI currently asks for sound output; tracks transitions so a failed device
    /// open isn't retried on every decode step.
    output_on: bool,
    video: Option<u32>,
    audio: Option<u32>,
    /// Selected streams not at their end yet.
    video_open: bool,
    audio_open: bool,
    /// For `RUNE_VIDEO_LOG`: microseconds to open, and each step's.
    opened_us: u128,
    steps: String,
    layer: Option<Layer>,
    /// When the layer last presented a picture (`superseded`).
    presented_at: Option<Instant>,
    /// Decoding on the CPU only because Rune had no GPU yet when the clip opened: once it has one,
    /// the clip opens again there (`decode`).
    awaiting_gpu: bool,
}

impl Decoder {
    fn open(job: &Job) -> Result<Self, String> {
        let t0 = job.started;
        let target = mf::Target::from_source(&job.source)?;
        let lib = mf::library()?;
        let t_mf = us(t0);
        rune_core::marks::mark("video.mf");
        let software = std::env::var_os("RUNE_VIDEO_DECODE").is_some_and(|v| v == "software");
        // The decoders' D3D11 device (made once per process) comes up on a thread of its own
        // while this one resolves the source: the two don't depend on each other.
        let (device, source, t_device) = std::thread::scope(|s| {
            let device = (job.kind == Kind::Video).then(|| s.spawn(|| (rune_device(), us(t0))));
            let source = lib.resolve(&target);
            let (device, t_device) = device.and_then(|d| d.join().ok()).unwrap_or((None, 0));
            (device, source, t_device)
        });
        let source = source?;
        let t_source = us(t0);
        rune_core::marks::mark("video.device");
        // Software decoding still uploads its pictures from this thread, not the UI's.
        let manager = device.as_ref().filter(|_| !software).map(|d| d.manager(lib)).transpose()?;
        let decode = manager.as_ref().map_or(Decode::Cpu, Decode::Gpu);
        let mut opened = mf::open(lib, source, job.kind, decode, true)?;
        let t_reader = us(t0);
        let duration = opened.duration;
        let picture = opened.picture;
        let awaiting_gpu = !software && device.is_none() && picture.is_some() && crate::native_surface::windows::presenting_device().is_none();
        let ring = match (&device, &picture) {
            (Some(device), Some(picture)) => Some(Ring::new(device.clone(), picture)?),
            _ => None,
        };
        let hardware = false;
        if job.start > 0.0 {
            mf::seek(&mut opened.reader, job.start)?;
        }
        let meta = Meta { duration, size: picture.map(|p| p.natural), frame: picture.and_then(|p| p.frame_duration), sound: opened.has_sound };
        let facts = Facts { meta, picture, sound: opened.sound, hardware, gpu: ring.is_some() };
        let opened_us = us(t0);
        let steps = format!("Media Foundation at {t_mf}, D3D11 at {t_device}, source at {t_source}, reader at {t_reader}, ring at {opened_us}");
        let audio_queue = opened.sound.map(Queue::new);
        let audio_open = opened.audio.is_some();
        let mut d = Self { reader: opened.reader, facts, ring, audio_queue, output: None, output_on: false, video: opened.video, audio: opened.audio, video_open: picture.is_some(), audio_open, opened_us, steps, layer: None, presented_at: None, awaiting_gpu };
        d.set_output(job.sound, &job.clock);
        Ok(d)
    }

    /// Opens or closes the WASAPI sound output while leaving the Media Foundation audio stream
    /// selected, so muted playback holds no OS audio stream and unmuting mid-clip continues at
    /// the current playback position instead of restarting from the clip's start.
    fn set_output(&mut self, sound: Option<f32>, clock: &Arc<audio::Clock>) {
        self.output_on = sound.is_some();
        let Some(queue) = &self.audio_queue else { return };
        let Some(gain) = sound else {
            self.output = None;
            return;
        };
        queue.set_gain(gain);
        if self.output.is_none() {
            match Output::open(queue, clock.clone()) {
                Ok(output) => self.output = Some(output),
                Err(e) => eprintln!("rune: no sound for the clip: {e}"),
            }
        }
    }

    fn decode(&mut self, pipe: &Pipe, job: &Job) -> Result<(), String> {
        let mut generation = job.generation;
        // Pictures before a seek's target are decoded only to reach it; a keyframe seek keeps the
        // first picture returned after seeking.
        let mut landing = if job.exact { Landing::at(job.start) } else { Landing::keyframe() };
        // Added to stream times: the loops so far times the duration.
        let mut epoch = 0.0;
        // Unwrapped timestamps of audio read/pushed and video kept since open, seek or loop.
        let mut audio_pts: Option<f64> = None;
        let mut audio_end: Option<f64> = None;
        let mut last_video_pts: Option<f64> = None;
        // Unwrapped media time before which decoded audio samples are dropped on push.
        let mut audio_skip_before = if job.exact || self.video.is_none() { job.start } else { f64::NEG_INFINITY };
        let mut first = true;
        let mut waiting_since = rune_core::time::now();
        // Audio-only or active sound drain: the clip's end, until the clock passes it.
        let mut end_due: Option<f64> = None;
        // Sound samples read since the last picture (`next_read`).
        let mut burst = 0;
        loop {
            let stream;
            // Take the UI's asks, then wait for room in the queue and a free texture.
            {
                let mut s = pipe.lock();
                loop {
                    if s.stop {
                        return Ok(());
                    }
                    if s.seek.is_some() {
                        end_due = None;
                        // The source moves with the inbox unlocked: the UI thread never waits
                        // for it.
                        s = pipe.land_seeks(s, |ask| {
                            generation = ask.generation;
                            landing = if ask.exact { Landing::at(ask.to) } else { Landing::keyframe() };
                            epoch = 0.0;
                            audio_pts = None;
                            audio_end = None;
                            last_video_pts = None;
                            audio_skip_before = if ask.exact || self.video.is_none() { ask.to } else { f64::NEG_INFINITY };
                            mf::seek(&mut self.reader, ask.to)?;
                            self.reopen_streams();
                            if let Some(q) = &self.audio_queue {
                                q.flush();
                            }
                            Ok::<(), String>(())
                        })?;
                        // The UI may have asked for a stop meanwhile.
                        continue;
                    }
                    if self.output_on != s.sound.is_some() {
                        let want = s.sound;
                        let wake = want.is_none() && s.audio_end.take().is_some() && s.ended == Some(generation);
                        if wake { end_due = None; }
                        drop(s);
                        if want.is_some()
                            && let Some(clock) = job.clock.get()
                        {
                            audio_skip_before = audio_skip_before.max(clock.position(rune_core::time::now()));
                        }
                        self.set_output(want, &job.clock);
                        if wake { (job.notify)(); }
                        s = pipe.lock();
                    }
                    if let (Some(q), Some(gain)) = (&self.audio_queue, s.sound) {
                        q.set_gain(gain);
                    }
                    self.follow_layer(&mut s);
                    self.reclaim(&mut s);
                    // Rune's GPU came after the clip opened on the CPU: it opens again on the GPU,
                    // at its picture, as for a replaced device.
                    if self.awaiting_gpu && crate::native_surface::windows::presenting_device().is_some() {
                        return Err(DEVICE_CHANGED.into());
                    }
                    let ended = s.ended == Some(generation);
                    if !ended && self.video.is_some() && !self.video_open {
                        s.video_ended = Some(generation);
                    }
                    if !ended && clip_done(self.video_open, self.audio_open, self.video.is_some(), self.output.is_some()) {
                        let active_audio_end = self.output.is_some().then_some(audio_end).flatten();
                        if s.looping && self.facts.meta.duration > 0.0 {
                            epoch = next_loop_epoch(epoch, self.facts.meta.duration, active_audio_end);
                            landing = Landing::at(0.0);
                            (audio_pts, audio_end, last_video_pts) = (None, None, None);
                            audio_skip_before = epoch;
                            (s.video_ended, s.audio_end) = (None, None);
                            drop(s);
                            mf::seek(&mut self.reader, 0.0)?;
                            self.reopen_streams();
                            s = pipe.lock();
                            continue;
                        }
                        s.ended = Some(generation);
                        if let Some(o) = &self.output {
                            o.end(generation);
                            s.audio_end = audio_end;
                        }
                        if job.kind == Kind::Audio || self.output.is_some() {
                            let video_end = last_video_pts.map(|pts| pts + self.facts.meta.frame_step());
                            end_due = drain_end(epoch, self.facts.meta.duration, video_end, active_audio_end);
                        }
                        drop(s);
                        (job.notify)();
                        s = pipe.lock();
                        continue;
                    }
                    let room = s.frames.len() < AHEAD;
                    let texture = self.ring.as_ref().is_none_or(Ring::free);
                    let (sound, sound_full, muted_due) = if self.output.is_some() {
                        let full = self.audio_queue.as_ref().is_some_and(Queue::full);
                        (!ended && self.audio_open && !full, full, None)
                    } else {
                        let clock_pos = job.clock.get().map(|c| c.position(rune_core::time::now()));
                        let front = s.frames.front().map(|f| f.pts);
                        let layered = s.presented(generation).is_some_and(|p| front == Some(p));
                        let target = muted_audio_target(clock_pos, front, self.facts.meta.frame_step(), layered, self.video.is_some());
                        if let (Some(q), Some(t)) = (&self.audio_queue, target) {
                            q.discard_before(t);
                        }
                        let want = !ended && self.audio_open && muted_audio_want(audio_pts, epoch, target);
                        let due = (!ended && self.audio_open && self.video.is_none())
                            .then(|| job.clock.get().map(|c| c.when(audio_pts.unwrap_or(epoch) + MUTE_MARGIN + 0.05))).flatten();
                        (want, false, due)
                    };
                    let picture = !ended && self.video_open && room && texture;
                    if let Some(next) = next_read(sound, picture, landing.landed(), burst) {
                        stream = next;
                        break;
                    }
                    let touched = s.touched.unwrap_or(waiting_since);
                    let playing_sound = self.output.as_ref().is_some_and(|o| !o.idle());
                    let heard_end = end_due.and_then(|end| job.clock.get().map(|clock| clock.when(end)));
                    if let Some(at) = heard_end.filter(|at| *at <= rune_core::time::now()) {
                        let _ = at;
                        end_due = None;
                        drop(s);
                        (job.notify)();
                        s = pipe.lock();
                        continue;
                    }
                    if job.kind == Kind::Video && !playing_sound && touched.elapsed() > PARK {
                        s.news.push(News::Parked);
                        drop(s);
                        (job.notify)();
                        return Ok(());
                    }
                    let patience = if (self.video_open && room && !texture) || sound_full { RING_POLL } else { PARK };
                    let patience = heard_end.map_or(patience, |at| patience.min(at.saturating_duration_since(rune_core::time::now()) + Duration::from_millis(1)));
                    let patience = muted_due.map_or(patience, |at| patience.min(at.saturating_duration_since(rune_core::time::now()) + Duration::from_millis(1)));
                    s = pipe.wait(s, patience);
                    waiting_since = waiting_since.max(touched);
                }
            }
            let index = match stream {
                Stream::Sound => { burst += 1; self.audio }
                Stream::Picture => { burst = 0; self.video }
            };
            let mut read = mf::read(&mut self.reader, index.unwrap_or(MF_SOURCE_READER_ANY_STREAM.0 as u32))?;
            if read.flags & MF_SOURCE_READERF_ERROR.0 != 0 {
                return Err("the decoder stopped with an error".into());
            }
            let video = self.video == Some(read.stream);
            if read.flags & MF_SOURCE_READERF_ENDOFSTREAM.0 != 0 {
                if video { self.video_open = false } else { self.audio_open = false }
            }
            if let Some(sample) = read.sample.take() {
                if video {
                    let pts = epoch + read.time;
                    // A picture the layer would only show late is decoded and no more: not
                    // copied, not queued, not presented.
                    let keep = landing.keeps(read.time, self.facts.meta.frame_step());
                    if keep {
                        if audio_skip_before.is_infinite() {
                            audio_skip_before = pts;
                        }
                        last_video_pts = Some(last_video_pts.map_or(pts, |prev| prev.max(pts)));
                    }
                    if keep && !self.late_on_layer(pipe, job, pts) {
                        let (picture, texture) = self.picture(&sample)?;
                        let blitted = self.blit_on_layer(pipe, job, texture);
                        // The decoder's surface goes back before the wait for the picture's time:
                        // held through it, Media Foundation's decoder ran out of surfaces and spun
                        // in the driver with the device's lock, and the next copy here never got it.
                        drop(sample);
                        let decoded = Decoded { generation, pts, picture };
                        match blitted.and_then(|clock| self.wait_until_due(pipe, job, clock, pts)) {
                            // The window takes no pictures while its layer shows them: only this
                            // one stays, and the others' textures are freed before the next
                            // picture (`reclaim`). It is queued, and noted as the one the layer
                            // shows, under the lock that saw the clock still run: a pause stops
                            // the clock and then asks what the layer showed, so it hears of
                            // every picture presented after it stopped.
                            Some(mut s) => {
                                s.push_presented(decoded, rune_core::time::now());
                                drop(s);
                                // A layer that can't present leaves the picture to the window.
                                if !self.present(pipe, generation) {
                                    (job.notify)();
                                }
                            }
                            None => {
                                if pipe.lock().push(decoded) {
                                    (job.notify)();
                                }
                            }
                        }
                        if first {
                            rune_core::marks::mark("video.first_frame");
                            rune_core::marks::note("video.decoder", if self.facts.hardware { "hardware" } else { "software" });
                            rune_core::marks::note("video.pictures", if self.facts.gpu { "gpu" } else { "cpu" });
                            if logging() {
                                let f = &self.facts;
                                log!(
                                    "{:?} opened in {} us ({}), first picture decoded at {} us; {} decoder, {} pictures",
                                    job.source, self.opened_us, self.steps, us(job.started), if f.hardware { "hardware" } else { "software" }, if f.gpu { "GPU" } else { "CPU" }
                                );
                            }
                            first = false;
                        }
                    }
                } else {
                    let pts = epoch + read.time;
                    audio_pts = Some(pts);
                    if self.output.is_some()
                        && let Some(q) = &self.audio_queue
                        && let Some(end) = q.push(&sample, pts, audio_skip_before)
                    {
                        audio_end = Some(audio_end.map_or(end, |prev| prev.max(end)));
                    }
                }
            }
            // A seek past the last picture kept none: back for the last one, so the clip shows
            // it and ends (or holds) there.
            if !self.video_open
                && let Some(last) = landing.ended()
            {
                mf::seek(&mut self.reader, last)?;
                self.reopen_streams();
                (audio_pts, audio_end, last_video_pts) = (None, None, None);
                audio_skip_before = last;
                if let Some(q) = &self.audio_queue {
                    q.flush();
                }
                continue;
            }
        }
    }

    /// Frees the textures of pictures dropped before they reached the surface. Rune never draws
    /// them, so no draw signals their release: left alone, they'd hold their textures until a
    /// later picture is drawn, and when none can be (a burst of seeks drops every picture as an
    /// earlier seek's), the ring runs out and the last seek's picture is never decoded.
    fn reclaim(&mut self, s: &mut Inbox) {
        for dropped in s.unread.drain(..) {
            if let (Some(ring), Pic::Gpu(planes, value)) = (&mut self.ring, &dropped.picture) {
                ring.unread(planes, *value);
            }
        }
    }

    /// After a seek or a loop, every selected stream reads again.
    fn reopen_streams(&mut self) {
        self.video_open = self.facts.picture.is_some();
        self.audio_open = self.audio.is_some();
    }


    /// Makes, resizes or lets go of the clip's layer as the window asks (`Inbox::layer`).
    fn follow_layer(&mut self, s: &mut Inbox) {
        let Some(size) = s.layer.filter(|_| !s.layer_refused) else {
            if s.layer.is_none() && self.layer.take().is_some() {
                s.layer_surface = None;
                s.layer_shown = None;
            }
            return;
        };
        let made = match &mut self.layer {
            Some(layer) => layer.resize(size),
            None => self.make_layer(size).map(|layer| {
                s.layer_surface = Some(layer.surface());
                self.layer = Some(layer);
                self.presented_at = None;
            }),
        };
        if let Err(e) = made {
            if logging() {
                log!("no layer for the clip: {e}; the window draws its pictures");
            }
            s.layer_refused = true;
            s.layer_surface = None;
            s.layer_shown = None;
            self.layer = None;
        }
    }

    fn make_layer(&self, size: (u32, u32)) -> Result<Layer, String> {
        let picture = self.facts.picture.ok_or("no video stream")?;
        // The layer scales the decoder's own textures; software pictures have none.
        let device = self.ring.as_ref().filter(|_| self.facts.hardware).map(Ring::device).ok_or("software pictures draw in the UI")?;
        Layer::new(device, picture, size)
    }

    /// Whether the layer shows the pictures now (as `blit_on_layer` would find) and the one at
    /// `pts` is too late for it (`superseded`).
    fn late_on_layer(&self, pipe: &Pipe, job: &Job, pts: f64) -> bool {
        let (Some(_), Some(clock)) = (&self.layer, job.clock.get()) else { return false };
        {
            let s = pipe.lock();
            if s.layer.is_none() || s.layer_refused {
                return false;
            }
        }
        let now = rune_core::time::now();
        superseded(clock.position(now), pts, self.facts.meta.frame_step(), self.presented_at.map(|at| now.saturating_duration_since(at)))
    }

    /// Scales a picture into the clip's layer when the window has the layer up and the clock
    /// runs, and returns that clock; None leaves the picture to the window.
    fn blit_on_layer(&mut self, pipe: &Pipe, job: &Job, texture: Option<(ID3D11Texture2D, u32)>) -> Option<Anchor> {
        let (Some(layer), Some((texture, index)), Some(clock)) = (&mut self.layer, texture, job.clock.get()) else { return None };
        if pipe.lock().layer.is_none() {
            return None;
        }
        if let Err(e) = layer.blit(&texture, index) {
            if logging() {
                log!("the layer couldn't take a picture: {e}; the window draws them");
            }
            pipe.lock().layer_refused = true;
            return None;
        }
        Some(clock)
    }

    /// Waits for the time on the clock of the picture `blit_on_layer` scaled, and returns the
    /// locked inbox when the layer should present it now; None leaves it to the window. Follows
    /// audio-clock nudges at the same rate, and ends early for pause, rate change, seek, stop or
    /// the window taking the pictures back.
    fn wait_until_due<'a>(&self, pipe: &'a Pipe, job: &Job, clock: Anchor, pts: f64) -> Option<MutexGuard<'a, Inbox>> {
        self.layer.as_ref()?;
        let mut s = pipe.lock();
        loop {
            if s.stop || s.seek.is_some() || s.layer.is_none() {
                return None;
            }
            // Paused, or the rate changed, while it waited: the window decides again.
            let at = layer_due(clock, job.clock.get(), pts)?;
            let now = rune_core::time::now();
            if now >= at {
                break;
            }
            s = pipe.wait(s, at - now);
        }
        Some(s)
    }

    /// Presents the picture `blit_on_layer` scaled; false when the layer couldn't.
    fn present(&mut self, pipe: &Pipe, generation: u32) -> bool {
        let Some(layer) = &self.layer else { return false };
        if let Err(e) = layer.present() {
            if logging() {
                log!("{e}; the window draws the pictures");
            }
            pipe.lock().layer_refused = true;
            return false;
        }
        pipe.lock().layer_shown = Some((layer.size, generation));
        self.presented_at = Some(rune_core::time::now());
        true
    }
    fn picture(&mut self, sample: &IMFSample) -> Result<(Pic, Option<(ID3D11Texture2D, u32)>), String> {
        let picture = self.facts.picture.ok_or("no video stream")?;
        // SAFETY: COM reads on a sample this thread owns.
        let texture = unsafe {
            sample.GetBufferByIndex(0).ok().and_then(|b| b.cast::<IMFDXGIBuffer>().ok()).and_then(|b| {
                let mut raw = std::ptr::null_mut();
                b.GetResource(&ID3D11Texture2D::IID, &mut raw).ok()?;
                Some((ID3D11Texture2D::from_raw(raw), b.GetSubresourceIndex().ok()?))
            })
        };
        // Rune replaced its device (a lost one, a driver update): the ring's textures and fences
        // were opened on the old one, and the decoders' D3D11 device went with it, so the decoder
        // lets go and the clip opens again at its picture on the new one.
        if self.ring.as_ref().is_some_and(Ring::stale) {
            let device = gpu::device()?;
            if !self.ring.as_mut().is_some_and(|r| r.adopt(&device)) {
                return Err(DEVICE_CHANGED.into());
            }
        }
        let Some(ring) = &mut self.ring else {
            return mf::nv12_bytes(sample, &picture).map(|bytes| (Pic::Cpu(bytes), None));
        };
        let slot = ring.take()?.ok_or("no free video texture")?;
        let (planes, value) = match &texture {
            Some((texture, index)) => {
                // DXVA decoders write into textures bound for decoding.
                let mut desc = Default::default();
                // SAFETY: a plain query on a live texture.
                unsafe { texture.GetDesc(&mut desc) };
                self.facts.hardware = desc.BindFlags & D3D11_BIND_DECODER.0 as u32 != 0;
                if self.facts.hardware {
                    self.reader.confirm_hardware();
                }
                ring.copy(slot, texture, *index)
            }
            None => ring.upload(slot, &mf::nv12_bytes(sample, &picture)?),
        };
        Ok((Pic::Gpu(planes, value), texture))
    }
}

#[cfg(test)]
mod tests;