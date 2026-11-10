//! Audio recording: a cheap, cloneable [`Recorder`] handle that records the microphone to a
//! file, with elapsed time and a smoothed input level to draw.
//!
//! In a window on macOS, AVAudioRecorder records on its own thread (AAC or WAV, metering built
//! in) and publishes the level 30 times a second while recording. Elsewhere starting fails with
//! a clear state.
//!
//! Headless runs (no window event loop yet, as with `Player`) never open a device: recorders
//! hear a [`FakeMicrophone`], elapsed time follows the virtual clock of the runtime stepping
//! on the thread that started them, levels come from the fake samples in 10 ms blocks (so they
//! don't depend on the frame rate), and `stop` writes a real WAV.

mod control;
mod fake;
mod meter;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as backend;
#[cfg(not(target_os = "macos"))]
mod unsupported;
#[cfg(not(target_os = "macos"))]
use unsupported as backend;

pub use fake::FakeMicrophone;
pub use meter::Level;

use crate::live::Live;
use crate::machine::Attempt;
use crate::video::{WakerSlot, headless_clock};
use control::{Control, Effect, Event, FakeSession, Phase, Source};
#[cfg(test)]
use meter::Meter;
use std::path::{Path, PathBuf};
use crate::time::Instant;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

/// The file format a recorder writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RecordFormat {
    /// AAC in an MPEG-4 container (`.m4a`), high quality: about 1 MB a minute, mono.
    Aac,
    /// 16-bit PCM WAV: about 5 MB a minute at 44.1 kHz mono.
    Wav,
}

/// Where a recorder is.
///
/// The `Interrupted` and `Failed` messages start lower case and have no final period, so they
/// read after a colon ("Couldn't record: {message}"). Capitalize the first letter to show one
/// on its own. Later versions keep this form.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RecorderState {
    /// Not started, or cancelled.
    Idle,
    /// Asking for permission or opening the input.
    Starting,
    Recording,
    Paused,
    /// Stopped with the file written.
    Stopped,
    /// The input went away mid-recording (unplugged, or the default input changed). What was
    /// captured is saved; `stop` returns it and `cancel` deletes it.
    Interrupted(String),
    /// Couldn't record: no permission, no input, an unwritable path, or an unsupported
    /// platform. `start` tries again.
    Failed(String),
}

/// Microphone access, as the system reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MicPermission {
    Granted,
    /// The user said no; only System Settings can change it.
    Denied,
    /// Not asked yet: [`Recorder::request_permission`] (or `start`) asks.
    Undetermined,
    /// The system won't allow recording (device management, parental controls), or this
    /// platform can't record.
    Restricted,
}

/// A finished recording.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Recording {
    /// The file, ready for [`Player::audio`](crate::Player::audio). Its extension follows
    /// the format, and a [fake microphone](Recorder::fake) always writes `.wav`, so read the
    /// type from this path rather than from the one given to [`Recorder::to_file`].
    pub path: PathBuf,
    /// Seconds of audio, pauses excluded.
    pub duration: f64,
}

/// Records the microphone to a file. Clones share one recorder; dropping the last clone while
/// recording stops it and keeps the file.
///
/// ```no_run
/// let rec = rune::Recorder::to_file("note.m4a"); // AAC; "note.wav" records WAV
/// rec.start();
/// // In a view, a level bar that redraws without re-running the view:
/// let bar = rune::el().w(4.0).h(40.0).live_scale(&rec.live_level());
/// // or read `rec.level().meter()` and `rec.elapsed()` from `poll`.
/// let note = rec.stop()?;
/// let clip = rune::Player::audio(note.path);
/// # Ok::<(), String>(())
/// ```
///
/// macOS asks for microphone access the first time an app records, and ends an app whose
/// Info.plist has no `NSMicrophoneUsageDescription`. Rune checks first: a bundled app without
/// the key fails to start with a clear message instead. An unbundled binary run from a
/// terminal is attributed to the terminal, which asks on its behalf. Hardened-runtime apps
/// also need the `com.apple.security.device.audio-input` entitlement (sandboxed apps
/// `com.apple.security.device.microphone`); an ad-hoc signed dev build may be denied without
/// a prompt.
#[derive(Clone)]
pub struct Recorder {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for Recorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let c = self.shared.ctl.lock().unwrap();
        f.debug_struct("Recorder").field("path", &self.shared.path).field("state", &c.status()).field("elapsed", &c.elapsed).finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Options {
    pub format: RecordFormat,
    pub sample_rate: u32,
    pub channels: u16,
    fake: Option<FakeMicrophone>,
}

pub(crate) struct Shared {
    path: PathBuf,
    opts: Mutex<Options>,
    ctl: Mutex<Control>,
    /// The published level: peak and RMS f32 bits in one word, so reads take no lock and
    /// always see a matching pair.
    level: AtomicU64,
    /// Whether a fake recording is in progress, so `level` knows to catch it up without
    /// locking for a device recording.
    faked: AtomicBool,
    live: Live<f32>,
    waker: WakerSlot,
}

/// Whether recorders use the device backend: in a window, and always in a browser (which has
/// no headless mode and no file system for a fake microphone's WAV).
fn devices() -> bool {
    cfg!(target_arch = "wasm32") || crate::video::playback_enabled()
}

/// Seconds since the first recorder looked, for elapsed time in a window.
fn wall_now() -> f64 {
    static ORIGIN: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    ORIGIN.get_or_init(Instant::now).elapsed().as_secs_f64()
}

thread_local! {
    /// Fake recorders following this thread's headless clock.
    static HEADLESS: std::cell::RefCell<Vec<Weak<Shared>>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// A headless runtime starts a frame: fake recorders started on this thread catch up to it,
/// so bound level meters move and an unplugged fake microphone interrupts on time.
pub(crate) fn headless_frame(_clock: f64) {
    let recorders: Vec<Arc<Shared>> = HEADLESS.with_borrow_mut(|h| {
        h.retain(|w| w.strong_count() > 0);
        h.iter().filter_map(Weak::upgrade).collect()
    });
    for r in recorders {
        r.follow();
    }
}

/// A new runtime on this thread starts its clock at 0: fake recordings keep their elapsed
/// time and go on from there.
pub(crate) fn headless_reset() {
    let recorders: Vec<Arc<Shared>> = HEADLESS.with_borrow(|h| h.iter().filter_map(Weak::upgrade).collect());
    for r in recorders {
        r.apply(Event::Tick);
        r.ctl.lock().unwrap().restart_clock();
    }
}

impl Recorder {
    /// How often a recording publishes its level in a window. A `poll` that draws elapsed
    /// time can ask for frames at this rate while recording with `tx.wake_after`.
    pub const METER_INTERVAL: Duration = Duration::from_millis(33);

    /// A recorder writing `path`: WAV when it ends in `.wav`, otherwise AAC, at 44.1 kHz
    /// mono. The format decides the file's extension: AAC writes `.m4a` (or keeps `.mp4`) and
    /// WAV writes `.wav`, so `to_file("note")` writes `note.m4a`, and
    /// [`Recording::path`] is always the file written. Nothing happens until
    /// [`start`](Self::start); each start overwrites the file.
    pub fn to_file(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let wav = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("wav"));
        let format = if wav { RecordFormat::Wav } else { RecordFormat::Aac };
        let opts = Options { format, sample_rate: 44_100, channels: 1, fake: None };
        Self {
            shared: Arc::new(Shared {
                opts: Mutex::new(opts),
                ctl: Mutex::new(Control::new(path.clone())),
                path,
                level: AtomicU64::new(pack(Level::SILENT)),
                faked: AtomicBool::new(false),
                live: Live::new(0.0),
                waker: WakerSlot::default(),
            }),
        }
    }
    /// The format for the next start, overriding the extension's. The file's extension
    /// follows it: `to_file("note.m4a").format(RecordFormat::Wav)` writes `note.wav`.
    pub fn format(self, format: RecordFormat) -> Self {
        self.shared.opts.lock().unwrap().format = format;
        self
    }
    /// Device sample rate for the next start (default 44 100 Hz).
    pub fn sample_rate(self, hz: u32) -> Self {
        self.shared.opts.lock().unwrap().sample_rate = hz.clamp(8_000, 192_000);
        self
    }
    /// Device channels for the next start: 1 (default) or 2.
    pub fn channels(self, channels: u16) -> Self {
        self.shared.opts.lock().unwrap().channels = channels.clamp(1, 2);
        self
    }
    /// Record from `mic` instead of a device, even in a window (where its level and elapsed
    /// time update when read). A fake microphone always writes 16-bit mono WAV at its input's
    /// rate: for an AAC recorder at `note.m4a` it writes `note.wav`, and [`Recording::path`]
    /// says so.
    pub fn fake(self, mic: FakeMicrophone) -> Self {
        self.shared.opts.lock().unwrap().fake = Some(mic);
        self
    }

    /// The file a device recording writes: the path given to [`to_file`](Self::to_file),
    /// with the format's extension. A fake microphone writes the `.wav` beside it.
    pub fn path(&self) -> PathBuf {
        with_format_extension(&self.shared.path, self.shared.opts.lock().unwrap().format)
    }

    /// Microphone access now. Headless: the installed [`FakeMicrophone`]'s (default
    /// `Granted`). Never asks.
    pub fn permission() -> MicPermission {
        if devices() { backend::permission() } else { FakeMicrophone::installed().permission }
    }
    /// Asks for microphone access if it's `Undetermined`, then calls `callback` with the
    /// answer, from any thread (at once when already decided, and always at once headless).
    /// macOS shows its prompt with the app's `NSMicrophoneUsageDescription`; a bundled app
    /// without one gets `Denied` rather than the crash macOS would cause.
    pub fn request_permission(callback: impl FnOnce(MicPermission) + Send + 'static) {
        if devices() { backend::request_permission(Box::new(callback)) } else { callback(FakeMicrophone::request_installed()) }
    }

    /// Starts recording, from `Idle`, `Stopped`, `Interrupted` or `Failed` (an interrupted
    /// recording's file is overwritten). Asks for permission first when it's `Undetermined`
    /// (the state is `Starting` meanwhile); without it the state becomes `Failed`.
    pub fn start(&self) {
        let shared = &self.shared;
        let fake = shared.fake_mic().is_some();
        let format = if fake { RecordFormat::Wav } else { shared.opts.lock().unwrap().format };
        let path = with_format_extension(&shared.path, format);
        shared.apply(Event::Start { path, virtual_clock: fake && !devices() });
        shared.settle();
    }

    /// Pauses a recording; elapsed time and the file stop growing.
    pub fn pause(&self) {
        self.shared.apply(Event::Pause);
        self.shared.settle();
    }

    /// Resumes a paused recording into the same file.
    pub fn resume(&self) {
        self.shared.apply(Event::Resume);
        self.shared.settle();
    }

    /// Stops and finalizes the file (blocking for the few milliseconds that takes), returning
    /// it. After an interruption, returns what was saved. Stopping while `Starting` abandons
    /// the start; stopping when not recording returns an error and changes nothing.
    pub fn stop(&self) -> Result<Recording, String> {
        let result = self.shared.apply(Event::Stop).unwrap_or_else(|| Err(control::NOT_RECORDING.into()));
        self.shared.settle();
        result
    }

    /// Stops and deletes the file of a recording in progress (or interrupted), back to
    /// `Idle`. Only a file this recording created is deleted: a stopped recording's file is
    /// the app's and stays, and cancelling a start that hasn't opened the input yet leaves an
    /// earlier file at the same path alone.
    pub fn cancel(&self) {
        self.shared.apply(Event::Cancel);
        self.shared.settle();
    }

    pub fn state(&self) -> RecorderState {
        self.shared.lock_following().status()
    }
    /// Whether audio is being captured now (`Recording`, not paused).
    pub fn is_recording(&self) -> bool {
        self.shared.lock_following().status() == RecorderState::Recording
    }
    /// Seconds recorded, pauses excluded: 0 until recording starts, held while paused and
    /// after stopping. Live from the clock in a window; the virtual clock headless.
    pub fn elapsed(&self) -> f64 {
        self.shared.lock_following().elapsed
    }
    /// The smoothed input level, silent unless recording. Takes no lock in a window and never
    /// allocates, so it's fine to read every frame.
    pub fn level(&self) -> Level {
        if self.shared.faked.load(Ordering::Relaxed) {
            self.shared.follow();
        }
        unpack(self.shared.level.load(Ordering::Acquire))
    }
    /// [`level().meter()`](Level::meter) as a [`Live`] value, for `live_scale`, `live_rect`
    /// or `live_opacity`: a bound meter redraws at the meter rate while recording without
    /// re-running the view, and draws nothing while idle or paused.
    pub fn live_level(&self) -> Live<f32> {
        self.shared.live.clone()
    }
    /// Called, from any thread, when the state changes: started, paused, resumed, stopped,
    /// interrupted or failed. Hand it the app's `setup` waker so `poll` hears a permission
    /// answer or an unplugged microphone. Nothing calls it for elapsed time or levels.
    pub fn set_waker(&self, waker: crate::Waker) {
        self.shared.waker.set(waker);
        self.shared.settle();
    }
}

const DENIED: &str = "microphone access is denied: allow it in System Settings > Privacy & Security > Microphone";
const RESTRICTED: &str = "microphone access is restricted on this device";

fn granted(answer: MicPermission) -> Result<(), String> {
    match answer {
        MicPermission::Granted => Ok(()),
        MicPermission::Restricted => Err(RESTRICTED.into()),
        MicPermission::Denied | MicPermission::Undetermined => Err(DENIED.into()),
    }
}

impl Control {
    /// The clock elapsed time follows.
    fn now(&self) -> Option<f64> {
        if self.virtual_clock { headless_clock() } else { Some(wall_now()) }
    }
}

/// An effect that runs with the control unlocked, as it blocks, re-enters or does I/O.
enum Unlocked {
    Ask(Attempt),
    OpenFake { attempt: Attempt, mic: FakeMicrophone, path: PathBuf },
    StopDevice { attempt: Attempt, session: backend::Session },
    CancelDevice(backend::Session),
    RemoveFile(PathBuf),
}

impl Shared {
    /// The fake microphone a start would use: the recorder's own, or, headless, the thread's.
    fn fake_mic(&self) -> Option<FakeMicrophone> {
        let own = self.opts.lock().unwrap().fake.clone();
        own.or_else(|| (!devices()).then(FakeMicrophone::installed))
    }

    fn resolve_fake_permission(&self) -> MicPermission {
        let mut opts = self.opts.lock().unwrap();
        match &mut opts.fake {
            Some(mic) => mic.resolve(),
            None => FakeMicrophone::request_installed(),
        }
    }

    /// Steps the machine with `event` and carries out what it asks, feeding the answers back
    /// in, until nothing is left. Returns what `stop` replies, if it was asked. Opening a
    /// device and its answer happen under one lock, so no other event lands between them.
    /// Call with no locks held; the caller settles.
    fn apply(self: &Arc<Self>, event: Event) -> Option<Result<Recording, String>> {
        let mut events = vec![event];
        let mut reply = None;
        while !events.is_empty() {
            let mut unlocked = Vec::new();
            {
                let mut c = self.ctl.lock().unwrap();
                while !events.is_empty() {
                    let event = events.remove(0);
                    let now = c.now();
                    for effect in c.step(event, now) {
                        match effect {
                            Effect::Silence => self.publish(Level::SILENT),
                            Effect::Publish(level) => self.publish(level),
                            Effect::Ask { attempt } => unlocked.push(Unlocked::Ask(attempt)),
                            Effect::Open { attempt } => match self.fake_mic() {
                                Some(mic) => unlocked.push(Unlocked::OpenFake { attempt, mic, path: c.path.clone() }),
                                None => {
                                    let opts = self.opts.lock().unwrap().clone();
                                    events.push(Event::DeviceOpened { attempt, opened: backend::start(self, attempt, &c.path, &opts) });
                                }
                            },
                            Effect::FollowHeadless => self.follow_headless(),
                            Effect::PauseDevice => c.device().into_iter().for_each(backend::pause),
                            Effect::ResumeDevice => c.device().into_iter().for_each(backend::resume),
                            Effect::StopDevice { attempt, session } => unlocked.push(Unlocked::StopDevice { attempt, session }),
                            Effect::CancelDevice(session) => unlocked.push(Unlocked::CancelDevice(session)),
                            Effect::RemoveFile(path) => unlocked.push(Unlocked::RemoveFile(path)),
                            Effect::Reply(result) => reply = Some(result),
                        }
                    }
                }
                self.faked.store(c.is_fake(), Ordering::Relaxed);
            }
            events.extend(unlocked.into_iter().filter_map(|job| self.run(job)));
        }
        reply
    }

    fn run(self: &Arc<Self>, job: Unlocked) -> Option<Event> {
        match job {
            Unlocked::Ask(attempt) => self.ask(attempt),
            Unlocked::OpenFake { attempt, mic, path } => {
                let opened = mic.open().and_then(|input| {
                    // The file exists from the start, as a device recording's does.
                    fake::write_wav(&path, &input, 0)?;
                    Ok(FakeSession::new(input, path, mic.unplug))
                });
                Some(Event::FakeOpened { attempt, opened })
            }
            Unlocked::StopDevice { attempt, session } => Some(Event::Stopped { attempt, result: backend::stop(session) }),
            Unlocked::CancelDevice(session) => {
                backend::cancel(session);
                None
            }
            Unlocked::RemoveFile(path) => {
                let _ = std::fs::remove_file(path);
                None
            }
        }
    }

    /// Microphone access for `attempt`: the answer now, or later from the system's prompt.
    fn ask(self: &Arc<Self>, attempt: Attempt) -> Option<Event> {
        let fake = self.fake_mic();
        if fake.is_none()
            && let Err(e) = backend::preflight()
        {
            return Some(Event::Permission { attempt, granted: Err(e) });
        }
        let answer = match (fake.as_ref().map_or_else(backend::permission, |mic| mic.permission), fake) {
            (MicPermission::Undetermined, Some(_)) => self.resolve_fake_permission(),
            (MicPermission::Undetermined, None) => {
                let weak = Arc::downgrade(self);
                backend::request_permission(Box::new(move |answer| {
                    let Some(shared) = weak.upgrade() else { return };
                    shared.apply(Event::Permission { attempt, granted: granted(answer) });
                    shared.settle();
                }));
                return None;
            }
            (answer, _) => answer,
        };
        Some(Event::Permission { attempt, granted: granted(answer) })
    }

    /// Follow this recorder on every headless frame of this thread, once.
    fn follow_headless(self: &Arc<Self>) {
        HEADLESS.with_borrow_mut(|h| {
            h.retain(|w| w.strong_count() > 0);
            if !h.iter().any(|w| std::ptr::eq(w.as_ptr(), Arc::as_ptr(self))) {
                h.push(Arc::downgrade(self));
            }
        });
    }

    /// The control, caught up to the clock.
    fn lock_following(self: &Arc<Self>) -> std::sync::MutexGuard<'_, Control> {
        self.follow();
        self.ctl.lock().unwrap()
    }

    /// A recording catches up to the clock: elapsed, a fake's levels, and an unplugged input.
    fn follow(self: &Arc<Self>) {
        self.apply(Event::Tick);
        self.settle();
    }

    fn publish(&self, level: Level) {
        self.level.store(pack(level), Ordering::Release);
        self.live.set(level.meter());
    }

    /// Tells the app's waker that the state changed. Call with no locks held.
    fn settle(&self) {
        let generation = self.ctl.lock().unwrap().generation;
        self.waker.tell(generation);
    }

    // Reports from a device backend's thread.

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn began(self: &Arc<Self>, attempt: Attempt) {
        self.apply(Event::Began { attempt });
        self.settle();
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn failed(self: &Arc<Self>, attempt: Attempt, error: String) {
        self.apply(Event::Failed { attempt, error });
        self.settle();
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn interrupted(self: &Arc<Self>, attempt: Attempt, reason: String, duration: f64) {
        self.apply(Event::Interrupted { attempt, reason, duration });
        self.settle();
    }

    /// A level from the device, published unless the recording moved on.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn metered(self: &Arc<Self>, attempt: Attempt, level: Level) {
        self.apply(Event::Metered { attempt, level });
    }
}

/// The last handle is gone: a recording in progress stops and keeps its file. This never
/// locks (it has the only reference), so it can't deadlock wherever the last handle drops.
impl Drop for Shared {
    fn drop(&mut self) {
        let c = self.ctl.get_mut().unwrap_or_else(PoisonError::into_inner);
        let now = c.now();
        let source = match std::mem::replace(&mut c.phase, Phase::Idle) {
            Phase::Recording { source, mut anchor, .. } => {
                control::advance(&mut c.elapsed, &mut anchor, now);
                source
            }
            Phase::Paused { source, .. } => source,
            Phase::Opening { session, .. } => Source::Device(session),
            _ => return,
        };
        match source {
            Source::Fake(fake) => {
                let _ = fake.write(c.elapsed);
            }
            Source::Device(session) => backend::finish(session),
        }
    }
}

/// `path` with an extension that holds `format`, which AVAudioRecorder picks the container by.
fn with_format_extension(path: &Path, format: RecordFormat) -> PathBuf {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    let (keep, want): (&[&str], &str) = match format {
        RecordFormat::Aac => (&["m4a", "mp4"], "m4a"),
        RecordFormat::Wav => (&["wav"], "wav"),
    };
    if ext.as_deref().is_some_and(|e| keep.contains(&e)) { path.to_path_buf() } else { path.with_extension(want) }
}

fn pack(level: Level) -> u64 {
    (u64::from(level.peak_db().to_bits()) << 32) | u64::from(level.rms_db().to_bits())
}

fn unpack(bits: u64) -> Level {
    Level::new(f32::from_bits((bits >> 32) as u32), f32::from_bits(bits as u32))
}

#[cfg(test)]
mod tests;
