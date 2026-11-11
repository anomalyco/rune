//! Sound for a clip: a WASAPI shared-mode stream on the default output device, fed by the
//! decoder and timed by the player's clock. The stream follows the clock rather than leading
//! it: each time the device asks for more, it writes the samples for the moment they'll be
//! heard, padding with silence or skipping ahead when the clock moved (a start, a seek, a stall,
//! the device's own drift), so picture and sound stay together without either waiting.

use super::machine::Anchor;
use super::mf::Sound;
use crate::time::Instant;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::*;
use windows::Win32::Media::MediaFoundation::IMFSample;
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

/// The device's buffer: long enough to ride out a busy decoder, short enough to react.
const BUFFER: Duration = Duration::from_millis(100);
/// Sound decoded ahead of what's heard; the decoder waits beyond it.
const AHEAD: f64 = 1.0;
/// How far written sound may drift from the clock before it jumps back in line.
const SLOP: f64 = 0.04;

/// The playback clock the UI sets and the sound follows: none while paused, stalled or ended.
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
    /// Sleeps until the clock runs at 1x (or patience passes) while it doesn't: sound plays
    /// only at normal speed.
    fn wait_while_stopped(&self, patience: Duration) {
        let anchor = self.anchor.lock().unwrap();
        if anchor.is_none_or(|a| (a.rate - 1.0).abs() >= 1e-6) {
            let _ = self.changed.wait_timeout(anchor, patience);
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
    stop: bool,
    /// Media time of the next sample the device gets, once it's writing in line with the clock.
    cursor: Option<f64>,
}

/// A clip's sound output; dropping it stops the stream.
pub(super) struct Output {
    feed: Arc<Mutex<Feed>>,
    clock: Arc<Clock>,
    sound: Sound,
}

impl Output {
    pub(super) fn open(sound: Sound, clock: Arc<Clock>) -> Result<Self, String> {
        let feed = Arc::new(Mutex::new(Feed { gain: 1.0, ..Feed::default() }));
        let (thread_feed, thread_clock) = (feed.clone(), clock.clone());
        let (ready, started) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("rune-video-sound".into())
            .spawn(move || {
                // SAFETY: COM for this thread, matched below.
                let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
                match Stream::new(sound) {
                    Ok(stream) => {
                        let _ = ready.send(Ok(()));
                        stream.run(&thread_feed, &thread_clock, sound);
                    }
                    Err(e) => {
                        let _ = ready.send(Err(e));
                    }
                }
                if com {
                    // SAFETY: matches the initialisation above.
                    unsafe { CoUninitialize() };
                }
            })
            .map_err(|e| e.to_string())?;
        started.recv().map_err(|_| "the sound thread stopped".to_string())??;
        Ok(Self { feed, clock, sound })
    }

    /// Queues a decoded float sample at `pts` seconds; sound before `skip_before` (a seek's
    /// pre-roll) is dropped.
    pub(super) fn push(&self, sample: &IMFSample, pts: f64, skip_before: f64, _generation: u32) {
        let Some(samples) = samples(sample) else { return };
        let channels = usize::from(self.sound.channels);
        let rate = f64::from(self.sound.rate);
        let end = pts + (samples.len() / channels) as f64 / rate;
        if end <= skip_before {
            return;
        }
        let mut feed = self.feed.lock().unwrap();
        feed.samples += samples.len();
        feed.chunks.push_back(Chunk { pts, samples });
    }

    /// Forgets queued sound (a seek).
    pub(super) fn flush(&self) {
        let mut feed = self.feed.lock().unwrap();
        feed.chunks.clear();
        feed.samples = 0;
        feed.cursor = None;
    }

    pub(super) fn set_gain(&self, gain: f32) {
        self.feed.lock().unwrap().gain = gain.clamp(0.0, 1.0);
    }

    /// Enough is queued; the decoder may wait.
    pub(super) fn full(&self) -> bool {
        let per_second = f64::from(self.sound.rate) * f64::from(self.sound.channels);
        self.feed.lock().unwrap().samples as f64 >= AHEAD * per_second
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
        self.feed.lock().unwrap().stop = true;
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
        let _ = buffer.Unlock();
        Some(values)
    }
}

struct Stream {
    client: IAudioClient,
    render: IAudioRenderClient,
    event: HANDLE,
    frames: u32,
    latency: Duration,
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
            Ok(Self { client, render, event, frames, latency })
        }
    }

    fn run(&self, feed: &Mutex<Feed>, clock: &Clock, sound: Sound) {
        let channels = usize::from(sound.channels);
        let rate = f64::from(sound.rate);
        let mut started = false;
        loop {
            if !started {
                // Stopped: sleep until the clock runs, not on the device's period.
                clock.wait_while_stopped(Duration::from_secs(1));
            }
            // SAFETY: a wait on this stream's own event.
            let woke = started && unsafe { WaitForSingleObject(self.event, 20) } == WAIT_OBJECT_0;
            if feed.lock().unwrap().stop {
                break;
            }
            let anchor = clock.get().filter(|a| (a.rate - 1.0).abs() < 1e-6);
            let Some(anchor) = anchor else {
                if started {
                    // SAFETY: stops and empties this stream.
                    unsafe {
                        let _ = self.client.Stop();
                        let _ = self.client.Reset();
                    }
                    started = false;
                    feed.lock().unwrap().cursor = None;
                }
                continue;
            };
            if started && !woke {
                continue;
            }
            // SAFETY: buffer calls on this stream; the device buffer holds `free` frames.
            unsafe {
                let padding = self.client.GetCurrentPadding().unwrap_or(self.frames);
                let free = self.frames.saturating_sub(padding);
                if free == 0 {
                    continue;
                }
                let Ok(data) = self.render.GetBuffer(free) else { continue };
                let out = std::slice::from_raw_parts_mut(data.cast::<f32>(), free as usize * channels);
                let heard = Instant::now() + self.latency + Duration::from_secs_f64(f64::from(padding) / rate);
                fill(&mut feed.lock().unwrap(), out, channels, rate, anchor.position(heard));
                let _ = self.render.ReleaseBuffer(free, 0);
                if !started {
                    started = self.client.Start().is_ok();
                }
            }
        }
        // SAFETY: stops this stream and closes its event.
        unsafe {
            let _ = self.client.Stop();
            let _ = CloseHandle(self.event);
        }
    }
}

/// Writes `out` with the sound from media time `want` on, in line with the clock.
fn fill(feed: &mut Feed, out: &mut [f32], channels: usize, rate: f64, want: f64) {
    let mut cursor = match feed.cursor {
        Some(c) if (c - want).abs() < SLOP => c,
        _ => want,
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
}
