//! Media playback: a cheap, cloneable [`Player`] handle for video drawn into a
//! [`NativeSurface`], or for audio with no element at all.
//!
//! The handle only records intent (play, pause, seek, rate, looping, muted).
//!
//! Video: each frame that draws the element reconciles that intent with the platform player on
//! the UI thread and pulls the frame for the frame's presentation time, so a paused or hidden
//! video costs nothing per frame. On macOS, AVPlayer + AVPlayerItemVideoOutput pull
//! IOSurface-backed `CVPixelBuffer`s for the display link's target time, zero-copy. Until a
//! video first plays, and after its player is parked, it shows a poster frame decoded on a
//! worker thread, with no player or decoder session held.
//!
//! Audio ([`Player::audio`]): no surface and no frames. In a window, edits apply on the main
//! thread at once, and a 20 Hz main-queue tick runs only while some clip plays, loads to play
//! or seeks, to notice the end, loop and loading. Nothing redraws for it: apps read
//! [`Player::time`] on their own frames (see [`Player::set_waker`]).
//!
//! Headless runs (virtual clock) never create a player and never make sound. Video decodes the
//! frame at its virtual time synchronously, so `--snap` and `--frames` are deterministic: time
//! starts on the first frame that draws the element after `play`, and advances by the runtime
//! clock times `rate`. Audio opens synchronously on the first read of its state, duration or
//! time (or on a frame once it plays), and its time follows the clock of the runtime stepping
//! on the thread that made it, from the first frame or edit after `play`: pausing, seeking,
//! rate, looping and the end (which wakes the app's waker) all as in a window, silently.
//!
//! On Windows, Media Foundation decodes on a thread per clip, on the GPU where it can, into
//! shared NV12 textures Rune samples in place; WASAPI plays the sound (`windows`).
//!
//! Other platforms report [`PlayerState::Failed`] and draw a neutral placeholder for video.

#[cfg(all(target_os = "macos", feature = "ogg-audio"))]
mod foreign;
#[cfg(target_os = "macos")]
mod macos;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod phases;
#[cfg(target_os = "macos")]
use macos as backend;
#[cfg(all(test, target_os = "macos"))]
pub(crate) use macos::test_frames;
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod turn;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod unsupported;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use unsupported as backend;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
use self::windows as backend;
#[cfg(target_os = "windows")]
#[doc(hidden)]
pub use self::windows::{LayerStatus, LayerSurface};
#[cfg(target_os = "windows")]
pub(crate) use self::windows::release_removed_device;
mod waker;

use phases::{Effect, Event, Run, Stage, step};
pub(crate) use waker::WakerSlot;

use crate::native_surface::{ByteFrame, ColorInfo, Format, NativeSurface};
use rune_core::machine::Seq;
use rune_core::time::Instant;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

/// Fallback seconds per frame for internal snapping when a stream reports no positive rate.
pub(crate) const DEFAULT_FRAME: f64 = 1.0 / 30.0;

/// A playing clip whose next picture is due sooner than this (seconds) draws every refresh;
/// later, the window sleeps until then.
#[doc(hidden)]
pub const WAKE_MIN: f64 = 0.002;

/// The platform player's entry points that a window reaches through a clip it draws, called through
/// the clip so that only an app that makes a [Player] links them (the decoder, the audio
/// stream, Media Foundation): a window links this module's code whether or not it ever draws video.
pub(crate) struct Backend {
    probe: fn(&mut backend::Native, &MediaSource, &[(String, String)], Kind) -> Result<Meta, String>,
    decode: fn(&mut backend::Native, &MediaSource, &[(String, String)], f64) -> Result<(ByteFrame, f64), String>,
    drive: fn(&Arc<Shared>),
    pull: fn(&Arc<Shared>, &NativeSurface, Instant) -> bool,
    due: fn(&Shared) -> Option<Instant>,
    #[cfg(target_os = "windows")]
    layer: fn(&Arc<Shared>, Option<(u32, u32)>) -> LayerStatus,
    #[cfg(target_os = "windows")]
    layer_showing: fn(&Arc<Shared>, (u32, u32)) -> bool,
    #[cfg(target_os = "windows")]
    unlayer: fn(&Arc<Shared>, &NativeSurface),
}

static BACKEND: Backend = Backend {
    probe: backend::probe,
    decode: backend::decode,
    drive: backend::drive,
    pull: backend::pull,
    due: backend::due,
    #[cfg(target_os = "windows")]
    layer: backend::layer,
    #[cfg(target_os = "windows")]
    layer_showing: backend::layer_showing,
    #[cfg(target_os = "windows")]
    unlayer: backend::unlayer,
};

/// Windows: shows shared's video in a compositor layer, or (None) asks for none.
#[cfg(target_os = "windows")]
#[doc(hidden)]
pub fn layer(shared: &Arc<Shared>, ask: Option<(u32, u32)>) -> LayerStatus { (shared.backend.layer)(shared, ask) }

/// Windows: read-only check of whether shared's video is still running and showing on its
/// compositor layer at `size`, without changing the decoder's ask or layer state.
#[cfg(target_os = "windows")]
#[doc(hidden)]
pub fn layer_showing(shared: &Arc<Shared>, size: (u32, u32)) -> bool { (shared.backend.layer_showing)(shared, size) }

/// Windows: takes shared's video out of its compositor layer, back to surface.
#[cfg(target_os = "windows")]
#[doc(hidden)]
pub fn unlayer(shared: &Arc<Shared>, surface: &NativeSurface) { (shared.backend.unlayer)(shared, surface) }

/// Set once a window event loop runs. Until then (headless runs, tests) no player is created.
static PLAYBACK: AtomicBool = AtomicBool::new(false);

/// Called by the windowed shell before its event loop starts.
#[doc(hidden)]
pub fn enable_playback() {
    if !PLAYBACK.swap(true, Ordering::AcqRel) {
        backend::enable();
    }
}

pub(crate) fn playback_enabled() -> bool {
    PLAYBACK.load(Ordering::Acquire)
}

/// Headless: the clock of the runtime stepping on this thread, and the audio clips made here.
#[derive(Default)]
struct Headless {
    clock: Option<f64>,
    audio: Vec<Weak<Shared>>,
}

thread_local! {
    static HEADLESS: std::cell::RefCell<Headless> = std::cell::RefCell::default();
}

/// A headless runtime starts a frame at `clock`: audio clips made on this thread follow it,
/// and the app's waker hears one end.
#[doc(hidden)]
pub fn headless_frame(clock: f64) {
    let clips: Vec<Arc<Shared>> = HEADLESS.with_borrow_mut(|h| {
        h.clock = Some(clock);
        h.audio.retain(|w| w.strong_count() > 0);
        h.audio.iter().filter_map(Weak::upgrade).collect()
    });
    for clip in clips {
        clip.follow_clock();
    }
}

/// A new runtime on this thread starts its clock at 0, replacing an earlier runtime's.
#[doc(hidden)]
pub fn headless_reset() {
    HEADLESS.with_borrow_mut(|h| h.clock = Some(0.0));
}

/// Never panics, so destructors can ask (`None` while the thread's storage is torn down).
pub(crate) fn headless_clock() -> Option<f64> {
    HEADLESS.try_with(|h| h.try_borrow().ok().and_then(|h| h.clock)).ok().flatten()
}

/// Where media comes from. Strings containing `://` are URLs; anything else is a file path.
///
/// Local file sources ([`MediaSource::File`] and `file:` URLs) open strictly as self-contained
/// local media files: macOS forbids external references (`AVAssetReferenceRestrictions::ForbidAll`),
/// and Windows requires a plain local disk path with a direct media container extension and
/// resolves it from a local byte stream without wildcard handler fallback. Network `http(s)://`
/// URLs keep URL resolution unchanged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaSource {
    File(PathBuf),
    Url(String),
}

impl From<&str> for MediaSource {
    fn from(s: &str) -> Self {
        if s.contains("://") { Self::Url(s.into()) } else { Self::File(s.into()) }
    }
}
impl From<String> for MediaSource {
    fn from(s: String) -> Self {
        if s.contains("://") { Self::Url(s) } else { Self::File(s.into()) }
    }
}
impl From<PathBuf> for MediaSource {
    fn from(path: PathBuf) -> Self {
        Self::File(path)
    }
}
impl From<&Path> for MediaSource {
    fn from(path: &Path) -> Self {
        Self::File(path.into())
    }
}

/// A media URL with HTTP request headers, for clips behind authentication. Build it with
/// [`Player::request`], then make the player with [`audio`](Self::audio) or
/// [`video`](Self::video). Every request the player makes for the clip sends the headers.
/// Header values never appear in `Debug` output or logs.
///
/// ```no_run
/// let token = "…";
/// let clip = rune_media::Player::request("https://example.com/media/clip.m4a")
///     .header("Authorization", format!("Bearer {token}"))
///     .audio();
/// clip.play();
/// ```
#[derive(Clone)]
pub struct MediaRequest {
    url: String,
    headers: Vec<(String, String)>,
}

impl std::fmt::Debug for MediaRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.headers.iter().map(|(name, _)| name.as_str()).collect();
        f.debug_struct("MediaRequest").field("url", &self.url).field("headers", &names).finish()
    }
}

impl MediaRequest {
    /// Adds a header sent with every request for the clip. A repeated name replaces the earlier
    /// value.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        let (name, value) = (name.into(), value.into());
        self.headers.retain(|(n, _)| !n.eq_ignore_ascii_case(&name));
        self.headers.push((name, value));
        self
    }
    /// An audio clip, as [`Player::audio`].
    pub fn audio(self) -> Player {
        Player::create(MediaSource::Url(self.url), Kind::Audio, self.headers)
    }
    /// A video, muted, as [`Player::url`].
    pub fn video(self) -> Player {
        Player::create(MediaSource::Url(self.url), Kind::Video, self.headers)
    }
}

/// Playback state. Video reports it as of the last frame that drew it; audio as of its last
/// tick or edit.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PlayerState {
    /// Opening the source (and, for video, decoding its first frame).
    Loading,
    /// Loaded and paused.
    Ready,
    Playing,
    /// Reached the end without looping. `play` starts again from the beginning.
    Ended,
    Failed(String),
}

#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Video,
    Audio,
}

/// A video or audio clip. Clones share one player; dropping the last clone (and, for video,
/// every element showing it) stops and frees it. Video starts muted; audio starts unmuted.
///
/// ```ignore
/// let clip = rune::Player::file("capture.mov");
/// clip.set_looping(true);
/// clip.play();
/// // In a view: size the element to the clip's aspect ratio once it is known.
/// let (w, h) = clip.natural_size().unwrap_or((16, 9));
/// let player = rune::el().w(480.0).h(480.0 * h as f32 / w as f32)
///     .video(&clip, rune::SurfaceFit::Fit);
///
/// let tap = rune::Player::audio("sounds/tap.m4a");
/// tap.play(); // audible: audio isn't muted
/// let progress = tap.time() / tap.duration().unwrap_or(1.0);
/// ```
#[derive(Clone)]
pub struct Player {
    surface: NativeSurface,
    shared: Arc<Shared>,
}

/// [`Player`], named for the element builder it pairs with (`El::video`).
pub type Video = Player;

impl std::fmt::Debug for Player {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let c = self.shared.ctl.lock().unwrap();
        f.debug_struct("Player")
            .field("kind", &self.shared.kind)
            .field("source", &self.shared.source)
            .field("state", &c.state())
            .field("time", &c.time)
            .finish_non_exhaustive()
    }
}

impl Player {
    /// A video, muted.
    pub fn new(source: MediaSource) -> Self {
        Self::create(source, Kind::Video, Vec::new())
    }
    /// A local movie file (mp4, mov; H.264 or HEVC), muted.
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self::new(MediaSource::File(path.into()))
    }
    /// A video at an `http(s)://` or `file://` URL, muted.
    pub fn url(url: impl Into<String>) -> Self {
        Self::new(MediaSource::Url(url.into()))
    }
    /// An audio clip (mp3, m4a/AAC, wav, aiff, flac; Ogg Vorbis, Ogg Opus and WebM Opus with
    /// the default `ogg-audio` feature) from a path or URL, unmuted. It plays without any
    /// element, draws nothing and pulls no frames.
    pub fn audio(source: impl Into<MediaSource>) -> Self {
        Self::create(source.into(), Kind::Audio, Vec::new())
    }
    /// A URL to open with request headers: `Player::request(url).header(name, value).audio()`
    /// (or `.video()`). See [`MediaRequest`].
    pub fn request(url: impl Into<String>) -> MediaRequest {
        MediaRequest { url: url.into(), headers: Vec::new() }
    }

    fn create(source: MediaSource, kind: Kind, headers: Vec<(String, String)>) -> Self {
        let ctl = Control { muted: kind == Kind::Video, kind, ..Control::default() };
        let shared = Arc::new(Shared {
            source,
            headers,
            kind,
            ctl: Mutex::new(ctl),
            wakes: Mutex::new(Vec::new()),
            waker: WakerSlot::default(),
            notified: AtomicU64::new(0),
            native: Mutex::new(backend::Native::default()),
            backend: &BACKEND,
        });
        backend::created(&shared);
        if kind == Kind::Audio && !playback_enabled() {
            HEADLESS.with_borrow_mut(|h| h.audio.push(Arc::downgrade(&shared)));
        }
        Self { surface: NativeSurface::with_video(shared.clone()), shared }
    }

    pub fn source(&self) -> &MediaSource {
        &self.shared.source
    }
    /// Whether this is an audio-only player from [`Player::audio`].
    pub fn is_audio(&self) -> bool {
        self.shared.kind == Kind::Audio
    }

    /// Decode the first frame now, off the main thread, so the first frame that draws this clip
    /// shows it rather than an empty box: for the page an app expects to show next. Headless
    /// runs decode frames as they draw, so there it does nothing.
    pub fn preload(&self) {
        if playback_enabled() {
            backend::preload(&self.shared);
        }
    }

    /// Start or resume playback. After [`PlayerState::Ended`] it starts from the beginning.
    pub fn play(&self) {
        self.send(Event::Play);
    }
    pub fn pause(&self) {
        self.send(Event::Pause);
    }
    /// Jump to `seconds` (clamped to the clip). Keeps playing if it was playing.
    pub fn seek(&self, seconds: f64) {
        self.send(Event::Seek { to: seconds, exact: true });
    }
    /// Jump to the nearest keyframe at or before `seconds` (clamped to the clip), skipping the
    /// decode of intermediate frames up to the target. Use while dragging a scrub bar, then call
    /// [`seek`](Self::seek) on release for the exact frame.
    ///
    /// On Windows (Media Foundation) this keeps the first keyframe returned by the source reader
    /// and reports that keyframe's timestamp through [`time`](Self::time) once it lands; on macOS
    /// (`AVPlayer`) it seeks with infinite tolerances (`kCMTimePositiveInfinity`). Headless runs
    /// treat it as [`seek`](Self::seek); on platforms without playback support, clips remain in
    /// [`PlayerState::Failed`] with [`is_seeking`](Self::is_seeking) returning `false`. Keeps
    /// playing if it was playing.
    ///
    /// ```no_run
    /// let clip = rune_media::Player::file("capture.mp4");
    /// clip.seek_fast(12.5);
    /// clip.seek(12.5);
    /// ```
    pub fn seek_fast(&self, seconds: f64) {
        self.send(Event::Seek { to: seconds, exact: false });
    }
    /// Pause playback and step by `frames` video pictures (positive moves forward, negative moves
    /// backward), clamped to the clip's first and last frames.
    ///
    /// Stepping by `0` pauses without seeking. Stepping past either end keeps the current boundary
    /// frame without starting a seek. No-op for [`Player::audio`], failed clips, and platforms
    /// without playback support (where clips start in [`PlayerState::Failed`]).
    ///
    /// While the target picture is being decoded, [`is_seeking`](Self::is_seeking) returns `true`
    /// and [`presented_time`](Self::presented_time) continues to report the picture still on screen
    /// until the new one lands. On macOS, a ready `AVPlayer` uses `AVPlayerItem.step(byCount:)`
    /// when the item supports it and falls back to an exact seek otherwise; Windows and headless
    /// runs perform an exact seek to the neighbouring frame.
    ///
    /// ```no_run
    /// let clip = rune_media::Player::file("capture.mp4");
    /// clip.step(1);
    /// clip.step(-1);
    /// ```
    pub fn step(&self, frames: i32) {
        if self.shared.kind == Kind::Audio {
            return;
        }
        self.send(Event::Step(frames));
    }
    /// Playback speed while playing; 1 is normal. Negative and non-finite rates are ignored.
    pub fn set_rate(&self, rate: f32) {
        if rate.is_finite() && rate > 0.0 {
            self.send(Event::Rate(rate));
        }
    }
    pub fn set_looping(&self, looping: bool) {
        self.edit(|c| c.looping = looping);
    }
    /// Sound plays only while unmuted. Videos start muted, audio clips unmuted.
    pub fn set_muted(&self, muted: bool) {
        self.edit(|c| c.muted = muted);
    }
    /// The sound's gain while unmuted, from 0 (silent) to 1 (as recorded, the default).
    /// Non-finite values are ignored.
    pub fn set_volume(&self, volume: f32) {
        if volume.is_finite() {
            self.edit(|c| c.volume = volume.clamp(0.0, 1.0));
        }
    }
    /// Routes sound to the platform audio output device UID (macOS: CoreAudio's
    /// `kAudioDevicePropertyDeviceUID`, via `AVPlayer.audioOutputDeviceUniqueID`). `None`
    /// (default) or an empty string follows the system output; other platforms record the UID.
    /// ```no_run
    /// let clip = rune_media::Player::audio("track.m4a");
    /// clip.set_output_device(Some("BuiltInSpeakerDevice"));
    /// ```
    pub fn set_output_device(&self, uid: Option<&str>) {
        let uid = uid.filter(|s| !s.is_empty());
        if self.shared.ctl.lock().unwrap().output_device.as_deref() == uid {
            return;
        }
        self.edit(|c| c.set_output_device(uid));
    }
    /// Called, from any thread, when the state changes: loaded, ended, failed, or started
    /// again by a loop. Hand it the app's `setup` waker so `poll` sees an audio clip end
    /// without ticking. Nothing calls it for progress. If the state already changed (loaded, or
    /// failed at once on an unsupported format), it's called once right away.
    pub fn set_waker(&self, waker: rune_core::Waker) {
        self.shared.waker.set(waker);
        self.shared.settle(false);
    }

    pub fn state(&self) -> PlayerState {
        self.read().state()
    }
    /// Whether `play` is in effect (also true while loading after `play`).
    pub fn is_playing(&self) -> bool {
        self.shared.ctl.lock().unwrap().playing()
    }
    /// Whether a [`seek`](Self::seek), [`seek_fast`](Self::seek_fast) or [`step`](Self::step) is
    /// still waiting for its picture (or position, for audio) to land.
    ///
    /// Returns `false` once the target picture has been shown, if the clip has failed, and in
    /// headless runs or on unsupported backends.
    ///
    /// ```no_run
    /// let clip = rune_media::Player::file("capture.mp4");
    /// clip.seek_fast(12.5);
    /// if !clip.is_seeking() {
    ///     clip.seek_fast(13.0);
    /// }
    /// ```
    pub fn is_seeking(&self) -> bool {
        backend::is_seeking(&self.shared)
    }
    /// Playback position in seconds. Video: as of the last drawn frame (or the last `seek`).
    /// Audio: read from the player when called on the main thread while it plays.
    pub fn time(&self) -> f64 {
        let (time, live) = {
            let c = self.read();
            (c.time, c.live())
        };
        backend::live_time(&self.shared, live).unwrap_or(time)
    }
    /// Media presentation timestamp (in seconds) of the video picture currently shown on
    /// [`surface`](Self::surface) (or the platform video layer).
    ///
    /// Distinct from [`time`](Self::time): while a seek or [`step`](Self::step) is in flight,
    /// `presented_time` continues to report the previous picture's timestamp, and once paused on
    /// a landed frame it reports that frame's own timestamp rather than the requested seek target.
    /// Returns `None` before any picture has landed, for [`Player::audio`], on failed clips, and
    /// on unsupported backends.
    ///
    /// ```no_run
    /// let clip = rune_media::Player::file("capture.mp4");
    /// if let (Some(pts), Some(fd)) = (clip.presented_time(), clip.frame_duration()) {
    ///     let _frame = (pts / fd).round() as u64;
    /// }
    /// ```
    pub fn presented_time(&self) -> Option<f64> {
        backend::presented_time(&self.shared)
    }
    /// Length in seconds, once loaded.
    pub fn duration(&self) -> Option<f64> {
        self.read().duration
    }
    /// Nominal duration of one video frame in seconds, once loaded.
    ///
    /// Returns `None` before the clip loads, for [`Player::audio`], on unsupported backends, or
    /// when the video stream reports no valid positive frame rate.
    ///
    /// ```no_run
    /// let clip = rune_media::Player::file("capture.mp4");
    /// if let Some(fd) = clip.frame_duration() {
    ///     let _fps = 1.0 / fd;
    /// }
    /// ```
    pub fn frame_duration(&self) -> Option<f64> {
        if self.shared.kind == Kind::Audio {
            return None;
        }
        self.shared.ctl.lock().unwrap().meta.and_then(|m| m.frame).filter(|f| f.is_finite() && *f > 0.0)
    }
    /// Display size in pixels (rotation applied), once a video loads. None for audio.
    /// Whether the clip has a sound track it can play (a page's media controls grey the mute
    /// button for one without); None until the clip's facts are read.
    pub fn has_sound(&self) -> Option<bool> {
        self.shared.ctl.lock().unwrap().meta.map(|m| m.sound)
    }
    pub fn natural_size(&self) -> Option<(u32, u32)> {
        self.shared.ctl.lock().unwrap().size
    }
    pub fn rate(&self) -> f32 {
        self.shared.ctl.lock().unwrap().rate
    }
    pub fn looping(&self) -> bool {
        self.shared.ctl.lock().unwrap().looping
    }
    pub fn muted(&self) -> bool {
        self.shared.ctl.lock().unwrap().muted
    }
    pub fn volume(&self) -> f32 {
        self.shared.ctl.lock().unwrap().volume
    }
    /// The audio output device UID last set, or `None` when following the system output.
    /// ```no_run
    /// let clip = rune_media::Player::audio("track.m4a");
    /// assert_eq!(clip.output_device(), None);
    /// ```
    pub fn output_device(&self) -> Option<String> {
        self.shared.ctl.lock().unwrap().output_device.clone()
    }
    /// The surface video frames land in: for frame observers and draw counts.
    pub fn surface(&self) -> &NativeSurface {
        &self.surface
    }

    fn read(&self) -> MutexGuard<'_, Control> {
        if self.shared.kind == Kind::Audio && !playback_enabled() {
            self.shared.open_now();
            self.shared.follow_clock();
        }
        self.shared.ctl.lock().unwrap()
    }

    /// A handle edit that changes the clip's phase. Handle edits ask for no effects.
    fn send(&self, event: Event) {
        self.edit(|c| {
            step(c, event);
        });
    }

    fn edit(&self, f: impl FnOnce(&mut Control)) {
        // Headless audio: fold in the time played up to now at the old rate, then restart the
        // clock from the edit.
        let clock = (self.shared.kind == Kind::Audio && !playback_enabled()).then(headless_clock).flatten();
        if clock.is_some() {
            self.shared.open_now();
        }
        {
            let mut c = self.shared.ctl.lock().unwrap();
            if let Some(now) = clock {
                c.advance_virtual(now);
            }
            f(&mut c);
            if let Some(now) = clock {
                c.advance_virtual(now);
            }
        }
        if clock.is_some() {
            self.shared.settle(false);
        }
        backend::apply(&self.shared);
        self.shared.wake_windows();
    }
}

/// The clock a pull samples: a window's next presentation, or a headless run's virtual time.
#[doc(hidden)]
#[derive(Clone, Copy, Debug)]
pub enum Clock {
    Display(Instant),
    Virtual(f64),
}

/// What one pull tells the runtime.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pulled {
    /// Keep drawing frames: playing, or waiting on a seek.
    pub active: bool,
    /// Changes when state, duration or size change, so views reading them re-run.
    pub generation: u64,
    /// While active: no frame is needed before this, when the next picture is due. None asks
    /// for every refresh.
    pub due: Option<Instant>,
}

/// Folds one drawn video's `generation` into a frame's signature, which changes, re-running
/// views, only when some video's state does. Order-free, so paint order doesn't count.
#[doc(hidden)]
pub fn fold(signature: u64, generation: u64, key: usize) -> u64 {
    signature.wrapping_add((generation ^ (key as u64).rotate_left(32)).wrapping_mul(0x9e37_79b9_7f4a_7c15).rotate_left(31))
}

/// [`fold`] for an element that isn't pulled this frame (out of sight, transparent): what it
/// last heard, without pulling a frame. Nothing for a surface with no video.
#[doc(hidden)]
pub fn fold_unpulled(signature: u64, surface: &NativeSurface) -> u64 {
    match surface.video() {
        Some(video) => fold(signature, video.ctl.lock().unwrap().generation, surface.key()),
        None => signature,
    }
}

/// What `--tree` reports for a media element.
#[doc(hidden)]
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub kind: Kind,
    pub state: PlayerState,
    pub time: f64,
    pub duration: Option<f64>,
    pub size: Option<(u32, u32)>,
    pub rate: f32,
    pub looping: bool,
    pub muted: bool,
}

/// Clip facts read once when it opens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Meta {
    pub duration: f64,
    /// None for audio.
    pub size: Option<(u32, u32)>,
    /// Nominal seconds per frame when the video stream reports a positive rate; `None` for audio
    /// or streams without a reported rate.
    pub frame: Option<f64>,
    /// The clip has a sound track the player can play.
    pub sound: bool,
}

#[doc(hidden)]
pub struct Shared {
    source: MediaSource,
    /// HTTP request headers for a URL source. Never logged.
    headers: Vec<(String, String)>,
    kind: Kind,
    ctl: Mutex<Control>,
    wakes: Mutex<Vec<Weak<rune_core::live::Wake>>>,
    waker: WakerSlot,
    /// The generation windows last heard about.
    notified: AtomicU64,
    native: Mutex<backend::Native>,
    backend: &'static Backend,
}

impl Shared {
    #[doc(hidden)]
    pub fn snapshot(&self) -> Snapshot {
        let c = self.ctl.lock().unwrap();
        Snapshot {
            kind: self.kind,
            state: c.state(),
            time: c.time,
            duration: c.duration,
            size: c.size,
            rate: c.rate,
            looping: c.looping,
            muted: c.muted,
        }
    }

    /// Redraw every window that has drawn this player. Safe from any thread.
    fn wake_windows(&self) {
        let wakes: Vec<_> = self.wakes.lock().unwrap().iter().filter_map(Weak::upgrade).collect();
        for wake in wakes {
            wake.notify();
        }
    }

    /// Tell the app's waker (and, outside a frame, the windows) that the state changed.
    /// Call with no locks held.
    fn settle(&self, windows: bool) {
        let generation = self.ctl.lock().unwrap().generation;
        self.waker.tell(generation);
        if self.notified.swap(generation, Ordering::AcqRel) != generation && windows {
            self.wake_windows();
        }
    }

    fn register(&self, wake: &Arc<rune_core::live::Wake>) {
        let mut wakes = self.wakes.lock().unwrap();
        wakes.retain(|w| w.strong_count() > 0);
        if !wakes.iter().any(|w| std::ptr::eq(w.as_ptr(), Arc::as_ptr(wake))) {
            wakes.push(Arc::downgrade(wake));
        }
    }

    /// Headless: open the clip synchronously, once.
    fn open_now(&self) {
        let mut native = self.native.lock().unwrap();
        if !self.ctl.lock().unwrap().opened() {
            let meta = (self.backend.probe)(&mut native, &self.source, &self.headers, self.kind);
            self.ctl.lock().unwrap().open(meta);
            drop(native);
            self.settle(false);
        }
    }

    /// Headless audio: playback time follows the runtime clock of this thread, if one steps.
    fn follow_clock(&self) {
        if self.kind != Kind::Audio || playback_enabled() {
            return;
        }
        let Some(now) = headless_clock() else { return };
        if self.ctl.lock().unwrap().playing() {
            self.open_now();
        }
        self.ctl.lock().unwrap().advance_virtual(now);
        self.settle(false);
    }

    /// Runs on the UI thread for each element that draws this player, once per frame.
    #[doc(hidden)]
    pub fn pull(self: &Arc<Self>, surface: &NativeSurface, clock: Clock, wake: &Arc<rune_core::live::Wake>) -> Pulled {
        self.register(wake);
        if self.kind == Kind::Audio {
            // Nothing to draw: the element only re-runs its view when the state changes.
            match clock {
                Clock::Virtual(_) => self.open_now(),
                Clock::Display(_) => (self.backend.drive)(self),
            }
            return Pulled { active: false, generation: self.ctl.lock().unwrap().generation, due: None };
        }
        let (active, due) = match clock {
            Clock::Virtual(now) => (self.pull_virtual(surface, now), None),
            Clock::Display(target) => ((self.backend.pull)(self, surface, target), (self.backend.due)(self)),
        };
        let pulled = {
            let mut c = self.ctl.lock().unwrap();
            for effect in step(&mut c, Event::Drawn) {
                if effect == Effect::Placeholder {
                    push_placeholder(surface);
                }
            }
            Pulled { active: active && !c.failed(), generation: c.generation, due }
        };
        self.settle(false);
        pulled
    }

    /// Headless: decode the frame at the virtual playback time, synchronously.
    fn pull_virtual(&self, surface: &NativeSurface, now: f64) -> bool {
        self.open_now();
        let mut native = self.native.lock().unwrap();
        let (meta, frame, shown) = {
            let mut c = self.ctl.lock().unwrap();
            let Some(meta) = c.meta.filter(|_| !c.failed()) else { return false };
            c.advance_virtual(now);
            (meta, meta.frame_index(c.time), c.shown)
        };
        if shown != Some(frame) {
            let decoded = (self.backend.decode)(&mut native, &self.source, &self.headers, meta.frame_time(frame));
            let mut c = self.ctl.lock().unwrap();
            c.shown = Some(frame);
            let id = c.next_frame_id();
            match decoded.and_then(|(bytes, pts)| surface.push_bytes(bytes, id).map(|()| pts)) {
                Ok(pts) => c.presented = Some(c.clamp_time(pts)),
                Err(e) => c.fail(e),
            }
        }
        self.ctl.lock().unwrap().playing()
    }
}

impl Meta {
    /// Nominal seconds per frame, falling back to 30 fps for internal snapping when unknown.
    pub(crate) fn frame_step(&self) -> f64 {
        self.frame.filter(|f| f.is_finite() && *f > 0.0).unwrap_or(DEFAULT_FRAME)
    }
    pub(crate) fn frames(&self) -> i64 {
        ((self.duration / self.frame_step()).round() as i64).max(1)
    }
    pub(crate) fn frame_index(&self, t: f64) -> i64 {
        let t = if t.is_finite() { t.max(0.0) } else { 0.0 };
        ((t / self.frame_step() + 1e-6).floor() as i64).clamp(0, self.frames() - 1)
    }
    /// The middle of frame `i`, so decoders never land on a neighbour's boundary.
    pub(crate) fn frame_time(&self, i: i64) -> f64 {
        ((i as f64 + 0.5) * self.frame_step()).min(self.duration)
    }
    /// The start timestamp of frame `i`, centred in Windows `Landing::keeps`'s window.
    #[cfg(target_os = "windows")]
    pub(crate) fn frame_start(&self, i: i64) -> f64 {
        (i as f64 * self.frame_step()).min(self.duration)
    }
    /// Target frame index and clamped step delta from `base`, or `None` when `frames == 0` or
    /// already at the clamped clip end.
    pub(crate) fn step_from(&self, base: f64, frames: i32) -> Option<(i64, i32)> {
        if frames == 0 {
            return None;
        }
        let cur = self.frame_index(base);
        let last = self.frames() - 1;
        let next = cur.saturating_add(i64::from(frames)).clamp(0, last);
        if next == cur {
            return None;
        }
        let delta = (next - cur).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        Some((next, delta))
    }
}

/// A seek recorded on the handle, waiting to be carried out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Seek {
    pub at: f64,
    pub exact: bool,
}

impl Seek {
    pub(super) const fn exact(at: f64) -> Self {
        Self { at, exact: true }
    }
}

/// A position change recorded on the handle, waiting to be carried out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Pending {
    Seek(Seek),
    /// Step by `frames` video pictures (non-zero), pausing playback.
    Step(i32),
}

/// A clip's state, stepped by [`phases::step`].
#[derive(Clone, Debug)]
pub(crate) struct Control {
    kind: Kind,
    // Intent, from the handle.
    pub rate: f32,
    pub looping: bool,
    pub muted: bool,
    pub volume: f32,
    pub output_device: Option<String>,
    pub(super) output_generation: u64,
    /// A seek or frame step not yet carried out, by a player, a poster, a decoder or the headless
    /// clock.
    pending: Option<Pending>,
    // Reported by pulls and ticks.
    pub time: f64,
    /// Media presentation timestamp of the video picture currently on the surface (posters,
    /// player frames, parked frames, headless frames). `None` before any picture lands, for audio,
    /// or after failure.
    pub(super) presented: Option<f64>,
    pub duration: Option<f64>,
    pub size: Option<(u32, u32)>,
    pub meta: Option<Meta>,
    run: Run,
    stage: Stage,
    /// Fences poster requests: a request for a new time or a native player taking over moves
    /// it on.
    posters: Seq,
    /// The time the current poster attempt asked for; None once a player took over.
    poster_at: Option<f64>,
    /// The poster attempt started for a paused seek, until its frame lands or is overtaken.
    poster_seeking: Option<rune_core::machine::Attempt>,
    generation: u64,
    frame_id: u64,
    /// Headless: the frame index on the surface.
    shown: Option<i64>,
}

impl Default for Control {
    fn default() -> Self {
        Self {
            kind: Kind::Video,
            rate: 1.0,
            looping: false,
            muted: true,
            volume: 1.0,
            output_device: None,
            output_generation: 0,
            pending: None,
            time: 0.0,
            presented: None,
            duration: None,
            size: None,
            meta: None,
            run: Run::default(),
            stage: Stage::default(),
            posters: Seq::default(),
            poster_at: None,
            poster_seeking: None,
            generation: 0,
            frame_id: 0,
            shown: None,
        }
    }
}

/// A 16:9 mid-grey frame for videos that can't play.
fn push_placeholder(surface: &NativeSurface) {
    let (w, h) = (32, 18);
    let grey = [0x46u8, 0x46, 0x46, 0xff];
    let rgba: Vec<u8> = grey.iter().copied().cycle().take(w * h * 4).collect();
    let frame = ByteFrame {
        width: w as u32,
        height: h as u32,
        format: Format::Bgra,
        color: ColorInfo { matrix: crate::native_surface::Matrix::Bt709, transfer: crate::native_surface::Transfer::Srgb },
        planes: [rgba, Vec::new()],
    };
    surface.push_bytes(frame, u64::MAX).ok();
}

#[cfg(test)]
mod tests;
