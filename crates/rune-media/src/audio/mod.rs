//! Cross-platform streaming PCM audio output: a cheap, cloneable [`AudioOutput`] handle that
//! accepts interleaved `i16` or `f32` PCM without blocking the caller, tracks a monotonic
//! device-latency-adjusted heard-frame clock, applies volume and mute after the ring buffer, and
//! reports underrun, end-of-stream, and device-lost transitions through a [`Waker`](rune_core::Waker).
//!
//! Callers normally reach these types through `rune`, which re-exports them.

mod buffer;
#[cfg(target_os = "linux")]
mod linux;
mod machine;
mod silent;
#[cfg(not(target_os = "linux"))]
mod unsupported;
mod worker;

use buffer::{DEFAULT_BUFFER_DURATION, bounded_capacity_frames};
use machine::{Event, frames_to_duration, step};
use rune_core::time::Instant;
use std::fmt;
use std::sync::{Arc, PoisonError};
use std::time::Duration;
use worker::{Shared, apply_event};

#[cfg(target_os = "linux")]
use linux as backend;
#[cfg(not(target_os = "linux"))]
use unsupported as backend;

/// Minimum supported sample rate in Hz.
pub(crate) const MIN_SAMPLE_RATE: u32 = 8_000;
/// Maximum supported sample rate in Hz.
pub(crate) const MAX_SAMPLE_RATE: u32 = 192_000;
/// Minimum supported channel count.
pub(crate) const MIN_CHANNELS: u16 = 1;
/// Maximum supported channel count.
pub(crate) const MAX_CHANNELS: u16 = 8;
/// Maximum unread events retained before dropping the oldest.
pub(super) const MAX_EVENTS: usize = 64;
/// Latency-polling cadence during non-blocking drain or while waiting for device dry-out.
pub(super) const DRAIN_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Injectable monotonic clock used by the state machine and silent sink.
pub(crate) type ClockFn = Arc<dyn Fn() -> Instant + Send + Sync>;

/// Sample encoding for interleaved PCM frames pushed into an [`AudioOutput`].
///
/// ```
/// use rune_media::SampleFormat;
///
/// assert_eq!(SampleFormat::I16.bytes_per_sample(), 2);
/// assert_eq!(SampleFormat::F32.bytes_per_sample(), 4);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SampleFormat {
    /// Interleaved signed 16-bit little-endian PCM (`i16`).
    I16,
    /// Interleaved 32-bit IEEE 754 float little-endian PCM in `[-1.0, 1.0]` (`f32`).
    F32,
}

impl SampleFormat {
    /// Size in bytes of one channel sample.
    pub const fn bytes_per_sample(self) -> usize {
        match self {
            Self::I16 => 2,
            Self::F32 => 4,
        }
    }
}

/// Which platform audio backend drives an [`AudioOutput`].
///
/// ```
/// use rune_media::{AudioBackend, AudioOutput, AudioOutputSpec, SampleFormat};
///
/// let audio = AudioOutput::open_silent(AudioOutputSpec::new(SampleFormat::I16, 48_000, 2))?;
/// assert_eq!(audio.backend(), AudioBackend::Silent);
/// # Ok::<(), rune_media::AudioOutputError>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AudioBackend {
    /// PulseAudio (including PipeWire via `pipewire-pulse`).
    PulseAudio,
    /// Advanced Linux Sound Architecture (ALSA).
    Alsa,
    /// Silent clock-paced virtual sink (no OS audio device).
    Silent,
}

/// Lifecycle state of an [`AudioOutput`].
///
/// ```
/// use rune_media::{AudioOutput, AudioOutputSpec, AudioOutputState, SampleFormat};
///
/// let audio = AudioOutput::open_silent(AudioOutputSpec::new(SampleFormat::I16, 48_000, 2))?;
/// assert_eq!(audio.state(), AudioOutputState::Paused);
/// # Ok::<(), rune_media::AudioOutputError>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AudioOutputState {
    /// Output is paused; no audio is being written and the worker is parked.
    Paused,
    /// Actively streaming queued PCM to the output device.
    Playing,
    /// [`AudioOutput::finish`] was called; draining queued and device-buffered audio.
    Draining,
    /// All queued and device-buffered audio finished playing cleanly.
    Ended,
    /// The output device disconnected or failed unrecoverably.
    DeviceLost(String),
}

/// Discrete state-transition events delivered via [`AudioOutput::take_events`].
///
/// ```
/// use rune_media::{AudioEvent, AudioOutput, AudioOutputSpec, SampleFormat};
///
/// let audio = AudioOutput::open_silent(AudioOutputSpec::new(SampleFormat::I16, 48_000, 2))?;
/// audio.finish();
/// assert_eq!(audio.take_events(), vec![AudioEvent::Ended]);
/// # Ok::<(), rune_media::AudioOutputError>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AudioEvent {
    /// Playback starved because the buffer and device ran dry while playing. Reported once per
    /// underrun transition until new samples arrive and play again.
    Underrun,
    /// A [`finish`](AudioOutput::finish) drain completed cleanly.
    Ended,
    /// The output device was lost unrecoverably.
    DeviceLost(String),
}

/// Monotonic playback position snapshot reported by [`AudioOutput::position`].
///
/// ```
/// use rune_media::{AudioOutput, AudioOutputSpec, SampleFormat};
///
/// let audio = AudioOutput::open_silent(AudioOutputSpec::new(SampleFormat::I16, 48_000, 2))?;
/// let pos = audio.position();
/// assert_eq!(pos.heard_frames(), 0);
/// assert_eq!(pos.queued_frames(), 0);
/// # Ok::<(), rune_media::AudioOutputError>(())
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AudioPosition {
    pub(crate) heard_frames: u64,
    pub(crate) sample_rate: u32,
    pub(crate) queued_frames: u64,
    pub(crate) latency: Duration,
}

impl AudioPosition {
    /// Monotonic count of frames heard at the speakers since open or the last
    /// [`flush`](AudioOutput::flush).
    pub fn heard_frames(&self) -> u64 {
        self.heard_frames
    }

    /// Playback time corresponding to [`heard_frames`](Self::heard_frames).
    pub fn duration(&self) -> Duration {
        frames_to_duration(self.heard_frames, self.sample_rate)
    }

    /// Unread frames still buffered in the ring waiting to be written to the device.
    pub fn queued_frames(&self) -> u64 {
        self.queued_frames
    }

    /// Estimated latency of audio already accepted by the device and not yet heard.
    pub fn latency(&self) -> Duration {
        self.latency
    }
}

/// Error returned when opening an [`AudioOutput`] fails.
///
/// ```
/// use rune_media::{AudioOutput, AudioOutputError, AudioOutputSpec, SampleFormat};
///
/// let bad = AudioOutputSpec::new(SampleFormat::I16, 0, 2);
/// assert!(matches!(AudioOutput::open_silent(bad), Err(AudioOutputError::InvalidSpec(_))));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AudioOutputError {
    /// Sample rate or channel count is outside the supported range (`8_000..=192_000` Hz, `1..=8`
    /// channels).
    InvalidSpec(String),
    /// No real audio output backend is available on this platform yet.
    Unsupported,
    /// Opening the platform audio device failed.
    OpenFailed(String),
}

impl fmt::Display for AudioOutputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSpec(msg) => write!(f, "invalid audio output spec: {msg}"),
            Self::Unsupported => write!(f, "audio output isn't supported on this platform yet"),
            Self::OpenFailed(msg) => write!(f, "couldn't open audio output: {msg}"),
        }
    }
}

impl std::error::Error for AudioOutputError {}

/// Error returned when pushing PCM into an [`AudioOutput`] cannot accept any frames.
///
/// ```
/// use rune_media::{AudioOutput, AudioOutputSpec, AudioPushError, SampleFormat};
///
/// let audio = AudioOutput::open_silent(AudioOutputSpec::new(SampleFormat::I16, 48_000, 2))?;
/// assert_eq!(audio.push_f32(&[0.0, 0.0]), Err(AudioPushError::FormatMismatch));
/// assert_eq!(audio.push_bytes(&[0u8; 3]), Err(AudioPushError::Unaligned));
/// # Ok::<(), rune_media::AudioOutputError>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AudioPushError {
    /// The ring buffer has no room for a whole frame right now; wait for capacity and retry.
    Full,
    /// [`AudioOutput::finish`] has already been called; call [`flush`](AudioOutput::flush) to
    /// reset before pushing again.
    Draining,
    /// The output device was lost unrecoverably.
    DeviceLost,
    /// Typed push (`push_i16` / `push_f32`) did not match the stream's [`SampleFormat`].
    FormatMismatch,
    /// Input slice length was not a whole multiple of one interleaved frame.
    Unaligned,
}

impl fmt::Display for AudioPushError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full => write!(f, "audio output buffer is full"),
            Self::Draining => write!(f, "audio output is draining or ended"),
            Self::DeviceLost => write!(f, "audio output device was lost"),
            Self::FormatMismatch => write!(f, "sample format mismatch"),
            Self::Unaligned => write!(f, "PCM slice length is not aligned to whole frames"),
        }
    }
}

impl std::error::Error for AudioPushError {}

/// Configuration builder for opening an [`AudioOutput`].
///
/// ```
/// use rune_media::{AudioOutput, AudioOutputSpec, SampleFormat};
/// use std::time::Duration;
///
/// let spec = AudioOutputSpec::new(SampleFormat::I16, 48_000, 2)
///     .buffer_duration(Duration::from_millis(2_400))
///     .volume(0.8)
///     .muted(false)
///     .app_name("Preview")
///     .stream_name("Media audio");
/// let audio = AudioOutput::open_silent(spec)?;
/// assert_eq!(audio.sample_rate(), 48_000);
/// assert_eq!(audio.channels(), 2);
/// # Ok::<(), rune_media::AudioOutputError>(())
/// ```
#[derive(Clone)]
pub struct AudioOutputSpec {
    format: SampleFormat,
    sample_rate: u32,
    channels: u16,
    buffer_duration: Duration,
    volume: f32,
    muted: bool,
    app_name: String,
    stream_name: String,
    waker: Option<rune_core::Waker>,
}

impl fmt::Debug for AudioOutputSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AudioOutputSpec")
            .field("format", &self.format)
            .field("sample_rate", &self.sample_rate)
            .field("channels", &self.channels)
            .field("buffer_duration", &self.buffer_duration)
            .field("volume", &self.volume)
            .field("muted", &self.muted)
            .field("app_name", &self.app_name)
            .field("stream_name", &self.stream_name)
            .field("waker", &self.waker.is_some())
            .finish()
    }
}

impl AudioOutputSpec {
    /// Creates a spec with the required PCM format, sample rate (`8_000..=192_000` Hz), and
    /// interleaved channel count (`1..=8`). Defaults to a 2.5 s buffer, volume `1.0`, unmuted,
    /// app name `"Rune"`, and stream name `"Audio"`.
    pub fn new(format: SampleFormat, sample_rate: u32, channels: u16) -> Self {
        Self {
            format,
            sample_rate,
            channels,
            buffer_duration: DEFAULT_BUFFER_DURATION,
            volume: 1.0,
            muted: false,
            app_name: "Rune".to_owned(),
            stream_name: "Audio".to_owned(),
            waker: None,
        }
    }

    /// Sets the queued ring-buffer duration. Clamped to `100 ms..=10 s` and capped at 16 MiB.
    pub fn buffer_duration(mut self, duration: Duration) -> Self {
        self.buffer_duration = duration;
        self
    }

    /// Sets the initial linear volume gain in `0.0..=1.0`. Non-finite values are ignored.
    pub fn volume(mut self, volume: f32) -> Self {
        if volume.is_finite() {
            self.volume = volume.clamp(0.0, 1.0);
        }
        self
    }

    /// Sets the initial mute state.
    pub fn muted(mut self, muted: bool) -> Self {
        self.muted = muted;
        self
    }

    /// Sets the application name reported to the system audio mixer.
    pub fn app_name(mut self, name: impl Into<String>) -> Self {
        self.app_name = name.into();
        self
    }

    /// Sets the stream description reported to the system audio mixer.
    pub fn stream_name(mut self, name: impl Into<String>) -> Self {
        self.stream_name = name.into();
        self
    }

    /// Installs a waker notified when discrete [`AudioEvent`] transitions occur.
    pub fn waker(mut self, waker: rune_core::Waker) -> Self {
        self.waker = Some(waker);
        self
    }

    pub(crate) fn validate(self) -> Result<ValidatedSpec, AudioOutputError> {
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&self.sample_rate) {
            return Err(AudioOutputError::InvalidSpec(format!(
                "sample rate {} Hz outside {MIN_SAMPLE_RATE}..={MAX_SAMPLE_RATE}",
                self.sample_rate
            )));
        }
        if !(MIN_CHANNELS..=MAX_CHANNELS).contains(&self.channels) {
            return Err(AudioOutputError::InvalidSpec(format!(
                "channel count {} outside {MIN_CHANNELS}..={MAX_CHANNELS}",
                self.channels
            )));
        }
        let frame_bytes = self.format.bytes_per_sample() * usize::from(self.channels);
        let capacity_frames = bounded_capacity_frames(self.sample_rate, frame_bytes, self.buffer_duration);
        Ok(ValidatedSpec {
            format: self.format,
            sample_rate: self.sample_rate,
            channels: self.channels,
            frame_bytes,
            capacity_frames,
            volume: self.volume,
            muted: self.muted,
            app_name: self.app_name,
            stream_name: self.stream_name,
            waker: self.waker,
        })
    }
}

/// Validated stream parameters passed to platform [`SinkDevice`] constructors.
#[allow(dead_code)] // `app_name` and `stream_name` are read by platform backends (Linux PulseAudio).
pub(crate) struct ValidatedSpec {
    pub(crate) format: SampleFormat,
    pub(crate) sample_rate: u32,
    pub(crate) channels: u16,
    pub(crate) frame_bytes: usize,
    pub(crate) capacity_frames: usize,
    pub(crate) volume: f32,
    pub(crate) muted: bool,
    pub(crate) app_name: String,
    pub(crate) stream_name: String,
    pub(crate) waker: Option<rune_core::Waker>,
}

/// Recoverable or terminal error returned by a platform [`SinkDevice`].
#[allow(dead_code)] // `Underrun` and `Lost` are produced by platform backends and test fakes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SinkError {
    /// Recoverable underrun (e.g. ALSA `-EPIPE` after `snd_pcm_prepare`).
    Underrun,
    /// Unrecoverable device loss; the output transitions to `AudioOutputState::DeviceLost`.
    Lost(String),
}

/// Internal platform audio sink seam implemented by `silent`, OS backends (such as Linux
/// PulseAudio/ALSA), and deterministic test fakes.
///
/// Methods are invoked from a single dedicated worker thread on native targets (or inline under
/// the device mutex on `wasm32`). Implementations must never call blocking drain APIs; the
/// shared worker polls [`latency_frames`](Self::latency_frames) in ~10 ms quanta so `pause`,
/// `flush`, and `drop` remain responsive.
pub(crate) trait SinkDevice: Send {
    /// Which backend drives this sink.
    fn backend(&self) -> AudioBackend;

    /// Writes interleaved post-gain PCM bytes (a whole number of frames, typically one ~10 ms
    /// quantum). Returns the number of frames accepted by the device.
    fn write(&mut self, pcm: &[u8]) -> Result<usize, SinkError>;

    /// Frames already accepted by [`write`](Self::write) that have not yet reached the speakers.
    fn latency_frames(&mut self) -> Result<u64, SinkError>;

    /// Discards all buffered unheard audio inside the device immediately.
    fn flush(&mut self) -> Result<(), SinkError>;

    /// Optional sleep duration when the sink does not pace writes by blocking on its own.
    fn pace(&self) -> Duration {
        Duration::ZERO
    }
}

/// Cloneable, thread-safe streaming PCM audio output with a monotonic playback clock.
///
/// All post-open methods are non-blocking and safe to call from any thread. Volume and mute are
/// applied after the ring buffer in ~10 ms write quanta so UI slider changes take effect
/// immediately even with seconds of audio buffered.
///
/// ```
/// use rune_media::{AudioEvent, AudioOutput, AudioOutputSpec, AudioPushError, SampleFormat};
/// use std::sync::Arc;
/// use std::time::Duration;
///
/// let spec = AudioOutputSpec::new(SampleFormat::I16, 48_000, 2)
///     .buffer_duration(Duration::from_millis(2_400))
///     .app_name("Preview")
///     .stream_name("Media audio")
///     .volume(0.8);
/// let audio = AudioOutput::open(spec.clone())
///     .or_else(|_| AudioOutput::open_silent(spec))?;
/// audio.set_waker(Arc::new(|| {}));
///
/// // Typical streaming loop: push until full, wait for capacity, pace video on `position()`.
/// let pcm_chunk = vec![0u8; 960 * 4]; // 20 ms of stereo S16LE
/// let mut pushed_chunks = 0usize;
/// while audio.has_capacity(960) && pushed_chunks < 4 {
///     match audio.push_bytes(&pcm_chunk) {
///         Ok(_accepted_bytes) => pushed_chunks += 1,
///         Err(AudioPushError::Full) => break,
///         Err(err) => return Err(rune_media::AudioOutputError::OpenFailed(err.to_string())),
///     }
/// }
/// audio.play();
/// let pos = audio.position();
/// let _video_clock = pos.duration();
/// let _spectrum_index = pos.heard_frames();
/// for event in audio.take_events() {
///     let _ = matches!(event, AudioEvent::Underrun | AudioEvent::Ended | AudioEvent::DeviceLost(_));
/// }
/// # Ok::<(), rune_media::AudioOutputError>(())
/// ```
#[derive(Clone)]
pub struct AudioOutput {
    shared: Arc<Shared>,
}

impl fmt::Debug for AudioOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let g = self.shared.inner.lock().unwrap_or_else(PoisonError::into_inner);
        f.debug_struct("AudioOutput")
            .field("backend", &self.shared.backend)
            .field("format", &self.shared.format)
            .field("sample_rate", &self.shared.sample_rate)
            .field("channels", &self.shared.channels)
            .field("state", &g.phase.public_state())
            .finish()
    }
}

impl AudioOutput {
    /// Opens a real OS audio output stream for `spec`.
    ///
    /// Returns [`AudioOutputError::Unsupported`] on platforms without a native backend yet; call
    /// [`AudioOutput::open_silent`] when a silent virtual sink is an acceptable fallback.
    ///
    /// ```
    /// use rune_media::{AudioOutput, AudioOutputSpec, SampleFormat};
    ///
    /// let spec = AudioOutputSpec::new(SampleFormat::I16, 48_000, 2);
    /// let audio = AudioOutput::open(spec.clone())
    ///     .or_else(|_| AudioOutput::open_silent(spec))?;
    /// assert_eq!(audio.sample_rate(), 48_000);
    /// # Ok::<(), rune_media::AudioOutputError>(())
    /// ```
    pub fn open(spec: AudioOutputSpec) -> Result<Self, AudioOutputError> {
        let validated = spec.validate()?;
        let device = backend::open(&validated)?;
        Self::from_device(validated, device, Arc::new(rune_core::time::now))
    }

    /// Opens a silent clock-paced virtual output that works on every platform (including `wasm32`)
    /// without opening any OS audio device or producing sound.
    ///
    /// ```
    /// use rune_media::{AudioBackend, AudioOutput, AudioOutputSpec, SampleFormat};
    ///
    /// let audio = AudioOutput::open_silent(AudioOutputSpec::new(SampleFormat::F32, 48_000, 2))?;
    /// assert_eq!(audio.backend(), AudioBackend::Silent);
    /// # Ok::<(), rune_media::AudioOutputError>(())
    /// ```
    pub fn open_silent(spec: AudioOutputSpec) -> Result<Self, AudioOutputError> {
        let validated = spec.validate()?;
        let clock: ClockFn = Arc::new(rune_core::time::now);
        let device = Box::new(silent::SilentDevice::new(&validated, clock.clone()));
        Self::from_device(validated, device, clock)
    }

    pub(crate) fn from_device(
        spec: ValidatedSpec,
        device: Box<dyn SinkDevice>,
        clock: ClockFn,
    ) -> Result<Self, AudioOutputError> {
        Ok(Self { shared: Shared::new(spec, device, clock)? })
    }

    /// Sample encoding of this stream.
    pub fn format(&self) -> SampleFormat { self.shared.format }

    /// Sample rate in Hz.
    pub fn sample_rate(&self) -> u32 { self.shared.sample_rate }

    /// Number of interleaved channels per frame.
    pub fn channels(&self) -> u16 { self.shared.channels }

    /// Backend driving this output.
    pub fn backend(&self) -> AudioBackend { self.shared.backend }

    /// Total ring-buffer capacity in frames.
    pub fn capacity_frames(&self) -> usize { self.shared.capacity_frames }

    /// Frames that can be pushed right now without returning [`AudioPushError::Full`].
    pub fn free_frames(&self) -> usize {
        self.shared.sync_inline();
        self.shared.inner.lock().unwrap_or_else(PoisonError::into_inner).buffer.free_frames()
    }

    /// Bytes that can be pushed right now without returning [`AudioPushError::Full`].
    pub fn free_bytes(&self) -> usize {
        self.shared.sync_inline();
        self.shared.inner.lock().unwrap_or_else(PoisonError::into_inner).buffer.free_bytes()
    }

    /// Whether the ring buffer currently has room for at least `frames` whole frames.
    pub fn has_capacity(&self, frames: usize) -> bool {
        self.free_frames() >= frames
    }

    /// Pushes interleaved little-endian PCM bytes without blocking. Returns the number of bytes
    /// accepted (always a whole number of frames).
    pub fn push_bytes(&self, pcm: &[u8]) -> Result<usize, AudioPushError> {
        let frame_bytes = self.shared.frame_bytes;
        self.shared.push_with(
            || pcm.len().is_multiple_of(frame_bytes).then_some(()).ok_or(AudioPushError::Unaligned),
            |buf| buf.push_bytes(pcm),
        )
    }

    /// Pushes interleaved `i16` samples without blocking. Returns the number of frames accepted.
    pub fn push_i16(&self, samples: &[i16]) -> Result<usize, AudioPushError> {
        let format = self.shared.format;
        let channels = usize::from(self.shared.channels);
        self.shared.push_with(
            || {
                if format != SampleFormat::I16 {
                    return Err(AudioPushError::FormatMismatch);
                }
                samples.len().is_multiple_of(channels).then_some(()).ok_or(AudioPushError::Unaligned)
            },
            |buf| buf.push_i16(samples),
        )
    }

    /// Pushes interleaved `f32` samples without blocking. Returns the number of frames accepted.
    pub fn push_f32(&self, samples: &[f32]) -> Result<usize, AudioPushError> {
        let format = self.shared.format;
        let channels = usize::from(self.shared.channels);
        self.shared.push_with(
            || {
                if format != SampleFormat::F32 {
                    return Err(AudioPushError::FormatMismatch);
                }
                samples.len().is_multiple_of(channels).then_some(()).ok_or(AudioPushError::Unaligned)
            },
            |buf| buf.push_f32(samples),
        )
    }

    /// Starts or resumes playback without blocking.
    pub fn play(&self) {
        self.shared.sync_inline();
        let now = (self.shared.clock)();
        let outcome = {
            let mut g = self.shared.inner.lock().unwrap_or_else(PoisonError::into_inner);
            apply_event(&mut g, self.shared.sample_rate, Event::Play { now })
        };
        self.shared.finish_outcome(outcome);
        self.shared.sync_inline();
    }

    /// Pauses playback without dropping queued audio: flushes the device immediately and rewinds
    /// unheard device-buffered frames back ahead of the unread ring so resuming continues from the
    /// exact heard frame with current volume and mute.
    pub fn pause(&self) {
        self.shared.sync_inline();
        let now = (self.shared.clock)();
        let outcome = {
            let mut g = self.shared.inner.lock().unwrap_or_else(PoisonError::into_inner);
            apply_event(&mut g, self.shared.sample_rate, Event::Pause { now })
        };
        self.shared.finish_outcome(outcome);
        self.shared.sync_inline();
    }

    /// Discards all queued and device-buffered audio and resets the playback clock to zero.
    /// Preserves `Playing` (suppressing spurious underruns until new samples arrive) and returns
    /// `Draining` or `Ended` outputs back to `Paused`.
    pub fn flush(&self) {
        self.shared.sync_inline();
        let now = (self.shared.clock)();
        let outcome = {
            let mut g = self.shared.inner.lock().unwrap_or_else(PoisonError::into_inner);
            g.flush_seq = g.flush_seq.wrapping_add(1);
            apply_event(&mut g, self.shared.sample_rate, Event::Flush { now })
        };
        self.shared.finish_outcome(outcome);
        self.shared.sync_inline();
    }

    /// Signals end of stream and drains all queued and device-buffered audio without blocking.
    /// Rejects subsequent pushes with [`AudioPushError::Draining`] until [`flush`](Self::flush)
    /// resets the output, and emits [`AudioEvent::Ended`] when drain completes.
    pub fn finish(&self) {
        self.shared.sync_inline();
        let now = (self.shared.clock)();
        let outcome = {
            let mut g = self.shared.inner.lock().unwrap_or_else(PoisonError::into_inner);
            let has_buffered = !g.buffer.is_empty();
            apply_event(&mut g, self.shared.sample_rate, Event::Finish { now, has_buffered })
        };
        self.shared.finish_outcome(outcome);
        self.shared.sync_inline();
    }

    /// Sets the post-buffer linear gain in `0.0..=1.0`. Applied on the next ~10 ms write quantum.
    /// Non-finite values are ignored.
    pub fn set_volume(&self, volume: f32) {
        if !volume.is_finite() {
            return;
        }
        let mut g = self.shared.inner.lock().unwrap_or_else(PoisonError::into_inner);
        g.volume = volume.clamp(0.0, 1.0);
    }

    /// Current linear volume gain in `0.0..=1.0`.
    pub fn volume(&self) -> f32 {
        self.shared.inner.lock().unwrap_or_else(PoisonError::into_inner).volume
    }

    /// Mutes or unmutes playback after the ring buffer. Takes effect on the next ~10 ms write
    /// quantum without dropping queued audio.
    pub fn set_muted(&self, muted: bool) {
        let mut g = self.shared.inner.lock().unwrap_or_else(PoisonError::into_inner);
        g.muted = muted;
    }

    /// Whether playback is currently muted.
    pub fn muted(&self) -> bool {
        self.shared.inner.lock().unwrap_or_else(PoisonError::into_inner).muted
    }

    /// Current lifecycle state of this output.
    pub fn state(&self) -> AudioOutputState {
        self.shared.sync_inline();
        self.shared.inner.lock().unwrap_or_else(PoisonError::into_inner).phase.public_state()
    }

    /// Whether audio is currently playing or draining out to the device.
    pub fn is_playing(&self) -> bool {
        self.shared.sync_inline();
        self.shared.inner.lock().unwrap_or_else(PoisonError::into_inner).phase.is_playing()
    }

    /// Monotonic playback position snapshot (heard frames, heard duration, queued frames, and
    /// device latency).
    pub fn position(&self) -> AudioPosition {
        self.shared.sync_inline();
        let now = (self.shared.clock)();
        let mut g = self.shared.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = step(&mut g.phase, self.shared.sample_rate, Event::Observe { now });
        let queued = g.buffer.queued_frames() as u64;
        g.phase.position(now, self.shared.sample_rate, queued)
    }

    /// Monotonic count of frames heard at the speakers since open or the last
    /// [`flush`](Self::flush).
    pub fn heard_frames(&self) -> u64 {
        self.position().heard_frames()
    }

    /// Monotonic playback duration corresponding to [`heard_frames`](Self::heard_frames).
    pub fn heard_duration(&self) -> Duration {
        self.position().duration()
    }

    /// Installs a waker called once per discrete [`AudioEvent`] transition. If unread events are
    /// already queued, the waker is notified once immediately.
    pub fn set_waker(&self, waker: rune_core::Waker) {
        self.shared.waker.set(waker);
        let generation = {
            let g = self.shared.inner.lock().unwrap_or_else(PoisonError::into_inner);
            (!g.events.is_empty()).then_some(g.event_generation)
        };
        if let Some(gen_id) = generation {
            self.shared.waker.tell(gen_id);
        }
    }

    /// Drains and returns all queued discrete [`AudioEvent`] notifications in order.
    pub fn take_events(&self) -> Vec<AudioEvent> {
        self.shared.sync_inline();
        let mut g = self.shared.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if g.events.is_empty() {
            return Vec::new();
        }
        g.events.drain(..).collect()
    }
}

#[cfg(test)]
mod buffer_tests;
#[cfg(test)]
mod machine_tests;
#[cfg(test)]
mod tests;
