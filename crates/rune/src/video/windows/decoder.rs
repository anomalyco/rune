//! One clip's decoder thread: opens the source with Media Foundation, decodes a few pictures
//! ahead into the player's GPU ring, hands audio to the sound output, loops without a gap by
//! seeking back while the last pictures still wait in the queue, and stops when told or when no
//! frame has drawn the clip for a while. The UI thread only takes pictures off the queue.
//!
//! The queue is the decoder's only clock: it decodes while there's room, so a clip nobody
//! draws fills it and sleeps, costing nothing until a frame takes a picture again.

use super::audio::{self, Output};
use super::gpu::{self, Ring};
use super::mf::{self, Decode, Picture, Sound};
use super::super::{Kind, MediaSource, Meta};
use crate::native_surface::ByteFrame;
use crate::native_surface::windows::DecodedPlanes;
use crate::time::Instant;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
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

/// What the UI asks of the decoder and what the decoder hands back, under one lock.
#[derive(Default)]
pub(super) struct Inbox {
    /// A seek to land: clip seconds and the generation its pictures carry.
    pub seek: Option<(f64, u32)>,
    pub looping: bool,
    /// Play sound, at this gain (unmuted, and the clip has some).
    pub sound: Option<f32>,
    pub stop: bool,
    /// When a frame last drew the clip.
    pub touched: Option<Instant>,
    pub news: Vec<News>,
    pub frames: VecDeque<Decoded>,
    /// The generation whose last picture is in the queue: the clip ended there.
    pub ended: Option<u32>,
}

pub(super) enum News {
    Opened(Facts),
    Failed(String),
    /// Let go after `PARK` undrawn.
    Parked,
}

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

pub(super) struct Decoded {
    pub generation: u32,
    /// Seconds from the clip's start, counting on through loops.
    pub pts: f64,
    pub picture: Pic,
}

pub(super) enum Pic {
    Gpu(Arc<DecodedPlanes>, u64),
    Cpu(ByteFrame),
}

pub(super) struct Pipe {
    inbox: Mutex<Inbox>,
    wake: Condvar,
}

impl Pipe {
    pub(super) fn lock(&self) -> MutexGuard<'_, Inbox> {
        self.inbox.lock().unwrap()
    }
    /// Tell the decoder something changed (a seek, a picture taken, a stop).
    pub(super) fn poke(&self) {
        self.wake.notify_all();
    }
}

/// What a decoder starts with.
pub(super) struct Job {
    pub source: MediaSource,
    pub kind: Kind,
    /// Clip seconds to start at, and that start's generation.
    pub start: f64,
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
    let pipe = Arc::new(Pipe {
        inbox: Mutex::new(Inbox { looping: job.looping, sound: job.sound, touched: Some(Instant::now()), ..Inbox::default() }),
        wake: Condvar::new(),
    });
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
    ($($arg:tt)*) => {{
        static T0: std::sync::OnceLock<crate::time::Instant> = std::sync::OnceLock::new();
        let t0 = *T0.get_or_init(crate::time::Instant::now);
        eprintln!("rune video {:>10} us: {}", t0.elapsed().as_micros(), format!($($arg)*));
    }};
}
pub(super) use log;

/// Microseconds from `t0` to now.
pub(super) fn us(t0: Instant) -> u128 {
    t0.elapsed().as_micros()
}

fn run(pipe: &Pipe, job: Job) {
    crate::marks::mark("video.open");
    let mut d = match Decoder::open(&job) {
        Ok(d) => d,
        Err(e) => {
            pipe.lock().news.push(News::Failed(e));
            (job.notify)();
            return;
        }
    };
    crate::marks::mark("video.opened");
    pipe.lock().news.push(News::Opened(d.facts));
    (job.notify)();
    if let Err(e) = d.decode(pipe, &job) {
        pipe.lock().news.push(News::Failed(e));
        (job.notify)();
    }
}

struct Decoder {
    reader: IMFSourceReader,
    facts: Facts,
    ring: Option<Ring>,
    output: Option<Output>,
    video: Option<u32>,
    audio: Option<u32>,
    /// Selected streams not at their end yet.
    video_open: bool,
    audio_open: bool,
    /// For `RUNE_VIDEO_LOG`: microseconds to open, and each step's.
    opened_us: u128,
    steps: String,
}

impl Decoder {
    fn open(job: &Job) -> Result<Self, String> {
        let t0 = job.started;
        let lib = mf::library()?;
        let t_mf = us(t0);
        crate::marks::mark("video.mf");
        let software = std::env::var_os("RUNE_VIDEO_DECODE").is_some_and(|v| v == "software");
        // The decoders' D3D11 device (made once per process) comes up on a thread of its own
        // while this one resolves the source: the two don't depend on each other.
        let (device, source, t_device) = std::thread::scope(|s| {
            let device = (job.kind == Kind::Video).then(|| s.spawn(|| (gpu::device().ok(), us(t0))));
            let source = lib.resolve(&job.source);
            let (device, t_device) = device.and_then(|d| d.join().ok()).unwrap_or((None, 0));
            (device, source, t_device)
        });
        let source = source?;
        let t_source = us(t0);
        crate::marks::mark("video.device");
        // Software decoding still uploads its pictures from this thread, not the UI's.
        let manager = device.as_ref().filter(|_| !software).map(|d| d.manager(lib)).transpose()?;
        let decode = manager.as_ref().map_or(Decode::Cpu, Decode::Gpu);
        let opened = mf::open(lib, &source, job.kind, decode, true)?;
        let t_reader = us(t0);
        let duration = opened.duration;
        let picture = opened.picture;
        let ring = match (&device, &picture) {
            (Some(device), Some(picture)) => Some(Ring::new(device.clone(), picture)?),
            _ => None,
        };
        let hardware = false;
        if job.start > 0.0 {
            mf::seek(&opened.reader, job.start)?;
        }
        let meta = Meta { duration, size: picture.map(|p| p.natural), frame: picture.map_or(1.0 / 30.0, |p| p.frame) };
        let facts = Facts { meta, picture, sound: opened.sound, hardware, gpu: ring.is_some() };
        let opened_us = us(t0);
        let steps = format!("Media Foundation at {t_mf}, D3D11 at {t_device}, source at {t_source}, reader at {t_reader}, ring at {opened_us}");
        let mut d = Self { reader: opened.reader, facts, ring, output: None, video: opened.video, audio: opened.audio, video_open: picture.is_some(), audio_open: false, opened_us, steps };
        d.select_sound(job.sound.is_some() || job.kind == Kind::Audio, &job.clock);
        Ok(d)
    }

    /// Turns the audio stream on (opening the output) or off (so it isn't decoded at all).
    fn select_sound(&mut self, on: bool, clock: &Arc<audio::Clock>) {
        let Some(sound) = self.facts.sound else { return };
        if on && self.output.is_none() {
            match Output::open(sound, clock.clone()) {
                Ok(output) => self.output = Some(output),
                Err(e) => eprintln!("rune: no sound for the clip: {e}"),
            }
        }
        if !on {
            self.output = None;
        }
        if let Some(index) = self.audio {
            // SAFETY: stream selection on this thread's reader.
            let _ = unsafe { self.reader.SetStreamSelection(index, self.output.is_some()) };
        }
        self.audio_open = self.output.is_some();
    }

    fn decode(&mut self, pipe: &Pipe, job: &Job) -> Result<(), String> {
        let mut generation = job.generation;
        // Pictures before a seek's target are decoded only to reach it.
        let mut skip_before = job.start;
        // Added to stream times: the loops so far times the duration.
        let mut epoch = 0.0;
        let mut first = true;
        let mut waiting_since = Instant::now();
        // Audio alone: the clip's end, until the clock passes it and the clip hears so.
        let mut end_due: Option<f64> = None;
        loop {
            // Take the UI's asks, then wait for room in the queue and a free texture.
            {
                let mut s = pipe.lock();
                loop {
                    if s.stop {
                        return Ok(());
                    }
                    if let Some((to, g)) = s.seek.take() {
                        end_due = None;
                        s.frames.clear();
                        s.ended = None;
                        generation = g;
                        skip_before = to;
                        epoch = 0.0;
                        mf::seek(&self.reader, to)?;
                        self.reopen_streams();
                        if let Some(o) = &self.output {
                            o.flush();
                        }
                    }
                    if self.output.is_some() != s.sound.is_some() {
                        let on = s.sound.is_some();
                        drop(s);
                        self.select_sound(on, &job.clock);
                        s = pipe.lock();
                    }
                    if let (Some(o), Some(gain)) = (&self.output, s.sound) {
                        o.set_gain(gain);
                    }
                    let ended = s.ended == Some(generation);
                    let room = s.frames.len() < AHEAD || !self.video_open;
                    let texture = self.ring.as_ref().is_none_or(Ring::free);
                    let sound_full = self.output.as_ref().is_some_and(Output::full);
                    if !ended && room && texture && !(sound_full && !self.video_open) {
                        break;
                    }
                    let touched = s.touched.unwrap_or(waiting_since);
                    let playing_sound = self.output.as_ref().is_some_and(|o| !o.idle());
                    let heard_end = end_due.and_then(|end| job.clock.get().map(|clock| clock.when(end)));
                    if let Some(at) = heard_end.filter(|at| *at <= Instant::now()) {
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
                    let patience = if (room && !texture) || sound_full { RING_POLL } else { PARK };
                    let patience = heard_end.map_or(patience, |at| patience.min(at.saturating_duration_since(Instant::now()) + Duration::from_millis(1)));
                    s = pipe.wake.wait_timeout(s, patience).unwrap().0;
                    waiting_since = waiting_since.max(touched);
                }
            }
            let read = mf::read(&self.reader, MF_SOURCE_READER_ANY_STREAM.0 as u32)?;
            if read.flags & MF_SOURCE_READERF_ERROR.0 != 0 {
                return Err("the decoder stopped with an error".into());
            }
            let video = self.video == Some(read.stream);
            if read.flags & MF_SOURCE_READERF_ENDOFSTREAM.0 != 0 {
                if video { self.video_open = false } else { self.audio_open = false }
            }
            if let Some(sample) = &read.sample {
                if video {
                    if read.time + self.facts.meta.frame * 0.5 >= skip_before {
                        let picture = self.picture(sample)?;
                        let mut s = pipe.lock();
                        let was_empty = s.frames.is_empty();
                        s.frames.push_back(Decoded { generation, pts: epoch + read.time, picture });
                        drop(s);
                        if first {
                            crate::marks::mark("video.first_frame");
                            crate::marks::note("video.decoder", if self.facts.hardware { "hardware" } else { "software" });
                            crate::marks::note("video.pictures", if self.facts.gpu { "gpu" } else { "cpu" });
                            if logging() {
                                let f = &self.facts;
                                log!(
                                    "{:?} opened in {} us ({}), first picture decoded at {} us; {} decoder, {} pictures",
                                    job.source, self.opened_us, self.steps, us(job.started), if f.hardware { "hardware" } else { "software" }, if f.gpu { "GPU" } else { "CPU" }
                                );
                            }
                            first = false;
                        }
                        if was_empty {
                            (job.notify)();
                        }
                    }
                } else if let Some(o) = &self.output {
                    o.push(sample, epoch + read.time, skip_before, generation);
                }
            }
            if !self.video_open && !self.audio_open {
                let looping = pipe.lock().looping;
                if looping && self.facts.meta.duration > 0.0 {
                    epoch += self.facts.meta.duration;
                    skip_before = 0.0;
                    mf::seek(&self.reader, 0.0)?;
                    self.reopen_streams();
                } else {
                    pipe.lock().ended = Some(generation);
                    if let Some(o) = &self.output {
                        o.end(generation);
                    }
                    if job.kind == Kind::Audio {
                        end_due = Some(epoch + self.facts.meta.duration);
                    }
                    (job.notify)();
                }
            }
        }
    }

    /// After a seek or a loop, every selected stream reads again.
    fn reopen_streams(&mut self) {
        self.video_open = self.facts.picture.is_some();
        self.audio_open = self.output.is_some();
    }


    fn picture(&mut self, sample: &IMFSample) -> Result<Pic, String> {
        let picture = self.facts.picture.ok_or("no video stream")?;
        // SAFETY: COM reads on a sample this thread owns.
        let texture = unsafe {
            sample.GetBufferByIndex(0).ok().and_then(|b| b.cast::<IMFDXGIBuffer>().ok()).and_then(|b| {
                let mut raw = std::ptr::null_mut();
                b.GetResource(&ID3D11Texture2D::IID, &mut raw).ok()?;
                Some((ID3D11Texture2D::from_raw(raw), b.GetSubresourceIndex().ok()?))
            })
        };
        let Some(ring) = &mut self.ring else {
            return mf::nv12_bytes(sample, &picture).map(Pic::Cpu);
        };
        let slot = ring.take()?.ok_or("no free video texture")?;
        let (planes, value) = match texture {
            Some((texture, index)) => {
                // DXVA decoders write into textures bound for decoding.
                let mut desc = Default::default();
                // SAFETY: a plain query on a live texture.
                unsafe { texture.GetDesc(&mut desc) };
                self.facts.hardware = desc.BindFlags & D3D11_BIND_DECODER.0 as u32 != 0;
                ring.copy(slot, &texture, index)
            }
            None => ring.upload(slot, &mf::nv12_bytes(sample, &picture)?),
        };
        Ok(Pic::Gpu(planes, value))
    }
}
