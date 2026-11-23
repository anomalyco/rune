//! Sound for a clip: a WASAPI shared-mode stream on the default output device, fed by the
//! decoder and timed by the player's clock. Each time the device asks for more, it writes the
//! samples for the moment they'll be heard (from `IAudioClock::GetPosition` when running, else
//! the buffer and stream latency), jumping on large discontinuities (start, seek, stall). While
//! playing on, sound is the master clock: when device clock drift passes `NUDGE`, the player's
//! clock moves to where sound plays so pictures follow what is heard without audio skips.

#![allow(unsafe_code)]

use super::machine::Anchor;
use super::mf::Sound;
use rune_core::time::Instant;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::*;
use windows::Win32::Media::MediaFoundation::IMFSample;
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

/// The device's buffer: long enough to ride out a busy decoder, short enough to react.
const BUFFER: Duration = Duration::from_millis(100);
/// Sound decoded ahead of what's heard; the decoder waits beyond it.
const AHEAD: f64 = 1.0;
/// How far written sound may be from the clock before it jumps back in line (start, seek, stall).
const SLOP: f64 = 0.04;
/// How far the sound may run from the clock, on average, before the clock moves to the sound:
/// a quarter of a picture at 60 fps, so one move never crosses a picture's boundary.
const NUDGE: f64 = 0.004;
/// Each write's weight in the smoothed drift average.
const SMOOTH: f64 = 0.1;
/// Nudges this soon after `Start()` also refine the start-latency bias.
const SETTLE: Duration = Duration::from_secs(1);
/// The device clock's wait is trusted only this far past the buffer estimate.
const POSITION_SLACK: f64 = 0.5;

/// The playback clock the UI sets and the sound leads while playing: none while paused, stalled
/// or ended.
#[derive(Default)]
pub(super) struct Clock {
    anchor: Mutex<Option<Anchor>>,
    changed: Condvar,
}

impl Clock {
    pub(super) fn set(&self, anchor: Option<Anchor>) {
        *self.anchor.lock().unwrap() = anchor;
        self.changed.notify_all();
    }
    pub(super) fn get(&self) -> Option<Anchor> {
        *self.anchor.lock().unwrap()
    }
    /// Moves the clock `by` seconds to where the sound plays, unless the UI replaced it since
    /// `anchor` was read.
    pub(super) fn nudge(&self, anchor: Anchor, by: f64) {
        let mut current = self.anchor.lock().unwrap();
        if *current == Some(anchor) {
            *current = Some(Anchor { pos: anchor.pos + by, ..anchor });
        }
    }
    /// Sleeps until the clock runs at 1x (or patience passes) while it doesn't: sound plays
    /// only at normal speed.
    fn wait_while_stopped(&self, patience: Duration) {
        let anchor = self.anchor.lock().unwrap();
        if anchor.is_none_or(|a| (a.rate - 1.0).abs() >= 1e-6) {
            self.changed.wait_timeout(anchor, patience).ok();
        }
    }
}

struct Chunk {
    pts: f64,
    samples: Vec<f32>,
}

#[derive(Default)]
struct Feed {
    chunks: VecDeque<Chunk>,
    /// Interleaved samples buffered.
    samples: usize,
    gain: f32,
    /// Identifies the active WASAPI stream; bumped on open and drop so an older stream exits
    /// without touching chunks kept across mute and unmute.
    stream: u64,
    /// Media time of the next sample the device gets, once it's writing in line with the clock.
    cursor: Option<f64>,
}

impl Feed {
    /// Drops buffered chunks that end at or before `pos`.
    fn discard_before(&mut self, pos: f64, channels: usize, rate: f64) {
        while let Some(c) = self.chunks.front() {
            let end = c.pts + (c.samples.len() / channels) as f64 / rate;
            if end > pos {
                break;
            }
            self.samples -= c.samples.len();
            self.chunks.pop_front();
        }
    }
}

/// Decoded sound shared between the decoder and whichever WASAPI [`Output`] is open. Kept across
/// mute and unmute so muting closes the audio device without losing sound already decoded ahead
/// of the clock.
pub(super) struct Queue {
    feed: Arc<Mutex<Feed>>,
    sound: Sound,
}

impl Queue {
    pub(super) fn new(sound: Sound) -> Self {
        Self { feed: Arc::new(Mutex::new(Feed { gain: 1.0, ..Feed::default() })), sound }
    }

    /// Queues a decoded float sample at `pts` seconds and returns its unwrapped end time; sound
    /// ending at or before `skip_before` (a seek's pre-roll, or behind the clock on unmute) is
    /// dropped.
    pub(super) fn push(&self, sample: &IMFSample, pts: f64, skip_before: f64) -> Option<f64> {
        let samples = samples(sample)?;
        self.push_pcm(pts, samples, skip_before)
    }

    fn push_pcm(&self, pts: f64, samples: Vec<f32>, skip_before: f64) -> Option<f64> {
        let channels = usize::from(self.sound.channels);
        let rate = f64::from(self.sound.rate);
        let end = pts + (samples.len() / channels) as f64 / rate;
        if end <= skip_before {
            return None;
        }
        let mut feed = self.feed.lock().unwrap();
        feed.samples += samples.len();
        feed.chunks.push_back(Chunk { pts, samples });
        Some(end)
    }

    /// Forgets queued sound (a seek).
    pub(super) fn flush(&self) {
        let mut feed = self.feed.lock().unwrap();
        feed.chunks.clear();
        feed.samples = 0;
        feed.cursor = None;
    }

    /// Drops queued sound that playback has already passed while muted.
    pub(super) fn discard_before(&self, pos: f64) {
        let mut feed = self.feed.lock().unwrap();
        if feed.chunks.is_empty() {
            return;
        }
        feed.discard_before(pos, usize::from(self.sound.channels), f64::from(self.sound.rate));
    }

    pub(super) fn set_gain(&self, gain: f32) {
        self.feed.lock().unwrap().gain = gain.clamp(0.0, 1.0);
    }

    /// Enough is queued; the decoder may wait.
    pub(super) fn full(&self) -> bool {
        let per_second = f64::from(self.sound.rate) * f64::from(self.sound.channels);
        self.feed.lock().unwrap().samples as f64 >= AHEAD * per_second
    }
}

/// A clip's sound output; dropping it stops the WASAPI stream and thread while leaving its
/// [`Queue`] intact for a prompt unmute.
pub(super) struct Output {
    feed: Arc<Mutex<Feed>>,
    clock: Arc<Clock>,
    stream: u64,
}

impl Output {
    pub(super) fn open(queue: &Queue, clock: Arc<Clock>) -> Result<Self, String> {
        let stream = {
            let mut feed = queue.feed.lock().unwrap();
            feed.stream = feed.stream.wrapping_add(1).max(1);
            feed.cursor = None;
            feed.stream
        };
        let (thread_feed, thread_clock, sound) = (queue.feed.clone(), clock.clone(), queue.sound);
        let (ready, started) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("rune-video-sound".into())
            .spawn(move || {
                // SAFETY: COM for this thread, matched below.
                let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
                match Stream::new(sound) {
                    Ok(s) => {
                        ready.send(Ok(())).ok();
                        s.run(&thread_feed, stream, &thread_clock, sound);
                    }
                    Err(e) => {
                        ready.send(Err(e)).ok();
                    }
                }
                if com {
                    // SAFETY: matches the initialisation above.
                    unsafe { CoUninitialize() };
                }
            })
            .map_err(|e| e.to_string())?;
        started.recv().map_err(|_| "the sound thread stopped".to_string())??;
        Ok(Self { feed: queue.feed.clone(), clock, stream })
    }

    /// Nothing plays: paused, stalled or ended.
    pub(super) fn idle(&self) -> bool {
        self.clock.get().is_none()
    }

    /// The decoder reached the end; what's queued still plays out.
    pub(super) fn end(&self, _generation: u32) {}
}

impl Drop for Output {
    fn drop(&mut self) {
        {
            let mut feed = self.feed.lock().unwrap();
            if feed.stream == self.stream {
                feed.stream = feed.stream.wrapping_add(1);
            }
        }
        self.clock.changed.notify_all();
    }
}

/// A decoded float sample's interleaved values.
fn samples(sample: &IMFSample) -> Option<Vec<f32>> {
    // SAFETY: the buffer is locked for the copy and unlocked after it.
    unsafe {
        let buffer = sample.ConvertToContiguousBuffer().ok()?;
        let (mut base, mut len) = (std::ptr::null_mut(), 0u32);
        buffer.Lock(&mut base, None, Some(&mut len)).ok()?;
        let values = std::slice::from_raw_parts(base.cast::<f32>(), len as usize / 4).to_vec();
        buffer.Unlock().ok();
        Some(values)
    }
}

struct Stream {
    client: IAudioClient,
    render: IAudioRenderClient,
    event: HANDLE,
    frames: u32,
    latency: Duration,
    /// The device's position counter and its ticks per second, when available.
    position: Option<(IAudioClock, u64)>,
}

/// Smoothed audio-vs-clock drift and learned start-latency bias.
#[derive(Default)]
struct Drift {
    /// Exponentially smoothed `cursor - want` offset, in seconds.
    avg: f64,
    /// Start-latency correction learned during the first `SETTLE` after `Start()`.
    bias: f64,
}

impl Drift {
    fn reset(&mut self) {
        self.avg = 0.0;
    }

    /// Folds one write's offset (`Some(cursor - want)` within `SLOP`, or `None` on jump).
    /// Returns `Some(nudge)` once the smoothed offset passes `NUDGE`, updating `bias` while
    /// `since_start` is within `SETTLE`.
    fn observe(&mut self, off: Option<f64>, since_start: Option<Duration>) -> Option<f64> {
        let Some(off) = off else {
            self.avg = 0.0;
            return None;
        };
        self.avg += (off - self.avg) * SMOOTH;
        if self.avg.abs() > NUDGE {
            let nudge = self.avg;
            if since_start.is_some_and(|d| d < SETTLE) {
                self.bias = (self.bias - nudge).clamp(-POSITION_SLACK, POSITION_SLACK);
            }
            self.avg = 0.0;
            return Some(nudge);
        }
        None
    }
}

impl Stream {
    fn new(sound: Sound) -> Result<Self, String> {
        // SAFETY: COM calls on this thread's objects; the format describes interleaved float.
        unsafe {
            let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| e.message())?;
            let device = devices.GetDefaultAudioEndpoint(eRender, eConsole).map_err(|_| "no sound output device".to_string())?;
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None).map_err(|e| e.message())?;
            let block = 4 * sound.channels;
            let format = WAVEFORMATEX {
                wFormatTag: 3, // WAVE_FORMAT_IEEE_FLOAT
                nChannels: sound.channels,
                nSamplesPerSec: sound.rate,
                nAvgBytesPerSec: sound.rate * u32::from(block),
                nBlockAlign: block,
                wBitsPerSample: 32,
                cbSize: 0,
            };
            let flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
            client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, BUFFER.as_nanos() as i64 / 100, 0, &format, None).map_err(|e| e.message())?;
            let event = CreateEventW(None, false, false, None).map_err(|e| e.message())?;
            client.SetEventHandle(event).map_err(|e| e.message())?;
            let render: IAudioRenderClient = client.GetService().map_err(|e| e.message())?;
            let frames = client.GetBufferSize().map_err(|e| e.message())?;
            let latency = Duration::from_nanos(client.GetStreamLatency().unwrap_or(0).max(0) as u64 * 100);
            let position = client
                .GetService::<IAudioClock>()
                .ok()
                .and_then(|c| c.GetFrequency().ok().filter(|f| *f > 0).map(|f| (c, f)));
            Ok(Self { client, render, event, frames, latency, position })
        }
    }

    /// When the next sample written will be heard. Running, uses `IAudioClock::GetPosition`
    /// (written frames minus played, extrapolated with QPC); else falls back to the buffer
    /// padding, stream latency, and learned start bias.
    fn heard(&self, now: Instant, written: u64, padding: u32, rate: f64, running: bool, bias: f64) -> Instant {
        let reading = self.position.as_ref().filter(|_| running).and_then(|(clock, frequency)| {
            let (mut position, mut counter) = (0u64, 0u64);
            // SAFETY: reads into locals from this stream's clock.
            unsafe { clock.GetPosition(&mut position, Some(&mut counter)) }.ok()?;
            Some((position, *frequency, counter))
        });
        let qpc = reading.map(|_| query_performance_counter());
        let wait = heard_wait(self.latency, padding, rate, bias, written, reading, qpc);
        now + Duration::from_secs_f64(wait)
    }

    fn run(&self, feed: &Mutex<Feed>, stream: u64, clock: &Clock, sound: Sound) {
        let channels = usize::from(sound.channels);
        let rate = f64::from(sound.rate);
        let mut started = false;
        let mut written = 0u64;
        let mut since: Option<Instant> = None;
        let mut drift = Drift::default();
        loop {
            if !started {
                // Stopped: sleep until the clock runs, not on the device's period.
                clock.wait_while_stopped(Duration::from_secs(1));
            }
            // SAFETY: a wait on this stream's own event.
            let woke = started && unsafe { WaitForSingleObject(self.event, 20) } == WAIT_OBJECT_0;
            if feed.lock().unwrap().stream != stream {
                break;
            }
            let anchor = clock.get().filter(|a| (a.rate - 1.0).abs() < 1e-6);
            let Some(anchor) = anchor else {
                if started {
                    // SAFETY: stops and empties this stream.
                    unsafe {
                        self.client.Stop().ok();
                        self.client.Reset().ok();
                    }
                    started = false;
                    written = 0;
                    since = None;
                    let mut guard = feed.lock().unwrap();
                    if guard.stream == stream {
                        guard.cursor = None;
                    }
                }
                drift.reset();
                continue;
            };
            if started && !woke {
                continue;
            }
            // SAFETY: buffer calls on this stream; the device buffer holds `free` frames.
            let (alive, off) = unsafe {
                let padding = self.client.GetCurrentPadding().unwrap_or(self.frames);
                let free = self.frames.saturating_sub(padding);
                if free == 0 {
                    continue;
                }
                let Ok(data) = self.render.GetBuffer(free) else { continue };
                let out = std::slice::from_raw_parts_mut(data.cast::<f32>(), free as usize * channels);
                let (alive, off) = {
                    let mut guard = feed.lock().unwrap();
                    if guard.stream != stream {
                        out.fill(0.0);
                        (false, None)
                    } else {
                        let heard = self.heard(rune_core::time::now(), written, padding, rate, started, drift.bias);
                        let off = fill(&mut guard, out, channels, rate, anchor.position(heard));
                        (true, off)
                    }
                };
                if self.render.ReleaseBuffer(free, 0).is_ok() {
                    written += u64::from(free);
                }
                if alive && !started {
                    started = self.client.Start().is_ok();
                    if started {
                        since = Some(rune_core::time::now());
                    }
                }
                (alive, off)
            };
            if !alive {
                break;
            }
            let since_start = since.map(|t| rune_core::time::now().saturating_duration_since(t));
            if let Some(nudge) = drift.observe(off, since_start) {
                clock.nudge(anchor, nudge);
            }
        }
        // SAFETY: stops this stream and closes its event.
        unsafe {
            self.client.Stop().ok();
            CloseHandle(self.event).ok();
        }
    }
}

/// Current performance counter and its frequency, for extrapolating `IAudioClock::GetPosition`.
fn query_performance_counter() -> (i64, i64) {
    let (mut count, mut frequency) = (0i64, 0i64);
    // SAFETY: plain reads of the performance counter into locals.
    unsafe {
        QueryPerformanceCounter(&mut count);
        QueryPerformanceFrequency(&mut frequency);
    }
    (count, frequency)
}

/// Seconds until the next sample written to the device buffer is heard. Running, computes
/// `written / rate - position / frequency - age` from `IAudioClock::GetPosition`
/// (`qpc_100ns` is in 100 ns units) and QPC; falls back to `(latency + padding / rate + bias).max(0)`
/// when stopped, unavailable, or outside `[0, buffered + POSITION_SLACK]`.
fn heard_wait(
    latency: Duration,
    padding: u32,
    rate: f64,
    bias: f64,
    written: u64,
    reading: Option<(u64, u64, u64)>,
    qpc: Option<(i64, i64)>,
) -> f64 {
    let buffered = (latency.as_secs_f64() + f64::from(padding) / rate + bias).max(0.0);
    let (Some((position, frequency, qpc_100ns)), Some((qpc_now, qpc_freq))) = (reading, qpc) else {
        return buffered;
    };
    if frequency == 0 || qpc_freq <= 0 {
        return buffered;
    }
    let played = position as f64 / frequency as f64;
    let now_100ns = (i128::from(qpc_now) * 10_000_000) / i128::from(qpc_freq);
    let age = (now_100ns - i128::from(qpc_100ns)) as f64 / 1e7;
    let wait = written as f64 / rate - played - age;
    if (0.0..=buffered + POSITION_SLACK).contains(&wait) { wait } else { buffered }
}

/// Writes `out` with the sound from media time `want` on. Within `SLOP` of the last write's
/// end, continues seamlessly and returns `Some(cursor - want)`; further off (start, seek,
/// stall), jumps to `want` and returns `None`.
fn fill(feed: &mut Feed, out: &mut [f32], channels: usize, rate: f64, want: f64) -> Option<f64> {
    let (mut cursor, off) = match feed.cursor {
        Some(c) if (c - want).abs() < SLOP => (c, Some(c - want)),
        _ => (want, None),
    };
    let frames = out.len() / channels;
    let mut written = 0;
    while written < frames {
        while let Some(c) = feed.chunks.front() {
            let end = c.pts + (c.samples.len() / channels) as f64 / rate;
            if end > cursor + 0.5 / rate {
                break;
            }
            feed.samples -= c.samples.len();
            feed.chunks.pop_front();
        }
        let Some(chunk) = feed.chunks.front() else {
            out[written * channels..].fill(0.0);
            cursor += (frames - written) as f64 / rate;
            break;
        };
        let start = ((cursor - chunk.pts) * rate).round() as i64;
        let n = if start < 0 {
            let n = ((-start) as usize).min(frames - written);
            out[written * channels..(written + n) * channels].fill(0.0);
            n
        } else {
            let start = start as usize;
            let n = (chunk.samples.len() / channels - start).min(frames - written);
            let gain = feed.gain;
            for (o, s) in out[written * channels..(written + n) * channels].iter_mut().zip(&chunk.samples[start * channels..]) {
                *o = s * gain;
            }
            n
        };
        written += n;
        cursor += n as f64 / rate;
    }
    feed.cursor = Some(cursor);
    off
}

#[cfg(test)]
mod tests;
