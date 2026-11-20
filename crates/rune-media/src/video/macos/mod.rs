//! AVFoundation: AVPlayer + AVPlayerItemVideoOutput for windows, AVAssetImageGenerator for
//! posters and headless frames, and a bare AVPlayer for audio.

mod asset;
mod park;
mod port;

#[cfg(test)]
pub(super) use asset::extension;
#[cfg(test)]
pub(crate) use asset::test_frames;
pub(super) use asset::{decode, probe};
#[cfg(test)]
pub(super) use park::{held_capacity, on_kept, parked_free, parked_with};

use super::phases::{Effect, Event, step};
use super::turn::{self, Turn};
use super::{Control, Kind, Meta, Shared, playback_enabled};
use crate::native_surface::{ByteFrame, NativeSurface, Orient};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_av_foundation::{AVAssetImageGenerator, AVPlayer, AVPlayerItem, AVPlayerItemVideoOutput};
use port::Av;
use rune_core::machine::{Attempt, Effects};
use rune_core::main_queue::on_main;
use rune_core::time::Instant;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

/// Posters never need more than this many pixels per side; headless frames keep full size.
const POSTER_MAX: f64 = 1920.0;
/// How often playing audio checks for its end, loops and loading.
const AUDIO_TICK: Duration = Duration::from_millis(50);

#[derive(Default)]
pub(crate) struct Native {
    /// Headless decodes reuse one generator.
    pub(super) generator: Option<Retained<AVAssetImageGenerator>>,
    pub(super) player: Option<Player>,
    pub(super) posters: Arc<Mutex<Posters>>,
    pub(super) decoded: Decoded,
}

/// Audio AVFoundation can't read, decoded once on the load worker (or a headless probe) to a WAV
/// it can, which every player for the clip then opens.
#[cfg(feature = "ogg-audio")]
pub(super) type Decoded = Arc<std::sync::OnceLock<Result<Option<super::foreign::Wav>, String>>>;
#[cfg(not(feature = "ogg-audio"))]
pub(super) type Decoded = ();
// SAFETY: the generator is only used under the Shared mutex. Player objects are created, used and
// released on the main thread: pulls and audio ticks run there and `Player`'s drop hops to it.
#[allow(unsafe_code)]
unsafe impl Send for Native {}

#[derive(Default)]
pub(super) struct Posters {
    want: Option<(f64, Attempt)>,
    busy: bool,
    /// The asking attempt, and a video poster and its facts, or (no frame) an audio clip's facts.
    pub(super) done: Option<(Attempt, Result<(Option<ByteFrame>, Meta), String>)>,
}

pub(super) struct Player {
    // Some until dropped; `Drop` moves the objects to the main thread.
    pub(super) objects: Option<Objects>,
    /// Video: how its buffers turn upright (they ignore the track's `preferredTransform`).
    pub(super) orient: Orient,
}
pub(super) struct Objects {
    pub(super) player: Retained<AVPlayer>,
    pub(super) item: Retained<AVPlayerItem>,
    /// None for audio.
    pub(super) output: Option<Retained<AVPlayerItemVideoOutput>>,
}

impl Player {
    pub(super) fn new(objects: Objects) -> Self {
        Self { objects: Some(objects), orient: Orient::UP }
    }
}

impl Drop for Player {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        let Some(objects) = self.objects.take() else { return };
        if MainThreadMarker::new().is_some() {
            // SAFETY: on the main thread (checked above) and `objects.player` is a live `AVPlayer`.
            unsafe { objects.player.pause() };
            return;
        }
        // The last handle went away on another thread: release on the main queue.
        struct Release(Objects);
        // SAFETY: `Release` only transfers `Objects` into `on_main` so `AVPlayer` is paused and dropped on the main thread.
        unsafe impl Send for Release {}
        let release = Release(objects);
        on_main(move || {
            let release = release; // Whole, so the closure is Send.
            // SAFETY: dispatched onto the main queue where `AVPlayer` is owned, and `release.0.player` is live.
            unsafe { release.0.player.pause() };
        });
    }
}

/// Every live audio player, for the tick.
static AUDIO: Mutex<Vec<Weak<Shared>>> = Mutex::new(Vec::new());
/// Some audio clip wants ticking. Written on the main thread.
static AUDIO_BUSY: AtomicBool = AtomicBool::new(false);
/// The tick thread is running.
static TICKING: AtomicBool = AtomicBool::new(false);

pub(super) fn created(shared: &Arc<Shared>) {
    if let Some(e) = asset::unsupported_format(&shared.source, shared.kind) {
        shared.ctl.lock().unwrap().fail(e);
        return;
    }
    if shared.kind == Kind::Audio {
        AUDIO.lock().unwrap().push(Arc::downgrade(shared));
        if playback_enabled() {
            load_audio(shared);
        }
    }
}

/// The window loop is starting: load audio clips made before it.
pub(super) fn enable() {
    let clips: Vec<Arc<Shared>> = AUDIO.lock().unwrap().iter().filter_map(Weak::upgrade).collect();
    for clip in clips {
        load_audio(&clip);
        on_main(move || drive(&clip));
    }
}

/// Read an audio clip's duration on a worker, so tiles know it before they play.
fn load_audio(shared: &Arc<Shared>) {
    let mut native = shared.native.lock().unwrap();
    let mut c = shared.ctl.lock().unwrap();
    let effects = step(&mut c, Event::LoadAudio);
    drop(c);
    ask_posters(shared, &mut native, effects);
}

/// Whether a poster decode has landed, and with a picture.
#[cfg(test)]
pub(crate) fn test_poster(shared: &Shared) -> Option<bool> {
    let native = shared.native.lock().unwrap();
    let posters = native.posters.lock().unwrap();
    posters.done.as_ref().map(|(_, d)| matches!(d, Ok((Some(_), _))))
}

/// An app warming the page it shows next: decode the clip's first frame now, so the first frame
/// that draws it already has it. Nothing once a poster or player has started, or for audio.
pub(super) fn preload(shared: &Arc<Shared>) {
    let mut native = shared.native.lock().unwrap();
    let mut c = shared.ctl.lock().unwrap();
    let effects = step(&mut c, Event::Preload);
    drop(c);
    ask_posters(shared, &mut native, effects);
}

/// Starts the worker for a load `step` asked for. Call without the control lock.
fn ask_posters(shared: &Arc<Shared>, native: &mut Native, effects: Effects<Effect>) {
    for effect in effects {
        if let Effect::AskPoster(at, attempt) = effect {
            request_poster(shared, native, at, attempt);
        }
    }
}

/// One worker per clip opens it and decodes the newest requested poster (none for audio),
/// then wakes its windows, or for audio drives it on the main queue. Called only while no
/// player runs, so a video's poster may land.
fn request_poster(shared: &Arc<Shared>, native: &mut Native, at: f64, attempt: Attempt) {
    let mut slot = native.posters.lock().unwrap();
    slot.want = Some((at, attempt));
    if slot.busy {
        return;
    }
    slot.busy = true;
    let posters = native.posters.clone();
    let clip = Arc::downgrade(shared);
    let (source, headers, kind, decoded) = (shared.source.clone(), shared.headers.clone(), shared.kind, native.decoded.clone());
    std::thread::Builder::new()
        .name("rune-media-poster".into())
        .spawn(move || {
            let mut opened: Option<(Option<Retained<AVAssetImageGenerator>>, Meta)> = None;
            loop {
                let Some((at, attempt)) = ({
                    let mut slot = posters.lock().unwrap();
                    let want = slot.want.take();
                    if want.is_none() {
                        slot.busy = false;
                    }
                    want
                }) else {
                    return;
                };
                if clip.strong_count() == 0 {
                    return;
                }
                // AVFoundation autoreleases; this thread has no pool of its own.
                let result = objc2::rc::autoreleasepool(|_| {
                    if opened.is_none() {
                        let (asset, meta) = asset::open(&source, &headers, kind, &decoded)?;
                        opened = Some(((kind == Kind::Video).then(|| asset::generator(&asset, Some(POSTER_MAX))), meta));
                    }
                    let (generator, meta) = opened.as_ref().unwrap();
                    let frame = match generator {
                        Some(g) => Some(asset::image(g, meta.frame_time(meta.frame_index(at)))?),
                        None => None,
                    };
                    Ok((frame, *meta))
                });
                posters.lock().unwrap().done = Some((attempt, result));
                let Some(clip) = clip.upgrade() else { return };
                match kind {
                    Kind::Video => clip.wake_windows(),
                    Kind::Audio => on_main(move || drive(&clip)),
                }
            }
        })
        .expect("spawn media poster worker");
}

/// Applies a handle edit at once on the main thread, so a paused, undrawn video stops (and goes
/// quiet) without waiting for a frame that draws it. Edits from other threads hop to it.
pub(super) fn apply(shared: &Arc<Shared>) {
    if !playback_enabled() || shared.ctl.lock().unwrap().failed() {
        return;
    }
    let Some(mtm) = MainThreadMarker::new() else {
        let clip = Arc::downgrade(shared);
        on_main(move || {
            if let Some(clip) = clip.upgrade() {
                apply(&clip);
            }
        });
        return;
    };
    if shared.kind == Kind::Audio {
        drive(shared);
        return;
    }
    // Already being pulled further up this stack; that pull reconciles.
    let Ok(mut native) = shared.native.try_lock() else { return };
    if native.player.is_none() {
        return;
    }
    let playing = {
        let mut c = shared.ctl.lock().unwrap();
        turn::reconcile(&mut c, &mut Av::new(shared, &mut native, Some(mtm), None), rune_core::time::now());
        c.playing()
    };
    drop(native);
    // A parked player that plays again must not be released.
    if playing {
        park::unpark(shared);
    }
}

/// Audio, on the main thread: what a live player (`Control::live`) reports now, while it plays.
/// `live`: the player's own position is the one to read (`Control::live`).
#[allow(unsafe_code)]
pub(super) fn live_time(shared: &Shared, live: bool) -> Option<f64> {
    if !live || shared.kind != Kind::Audio || MainThreadMarker::new().is_none() {
        return None;
    }
    let native = shared.native.try_lock().ok()?;
    let p = native.player.as_ref()?;
    // SAFETY: on the main thread (checked above) under `shared.native` lock; `player` is a live `AVPlayer`.
    let time = asset::seconds(unsafe { p.objects.as_ref()?.player.currentTime() });
    let c = shared.ctl.lock().unwrap();
    c.playing().then(|| c.duration.map_or(time, |d| time.clamp(0.0, d)))
}

pub(super) fn pull(shared: &Arc<Shared>, surface: &NativeSurface, target: Instant) -> bool {
    let now = rune_core::time::now();
    let mut native = shared.native.lock().unwrap();
    let mut c = shared.ctl.lock().unwrap();
    let mut av = Av::new(shared, &mut native, None, Some((surface, target)));
    let turn = turn::turn(&mut c, &mut av, now);
    let asked = av.asked;
    finish(shared, native, c, turn, asked)
}

/// The display link draws a playing clip every refresh.
pub(super) fn due(_: &Shared) -> Option<Instant> {
    None
}

/// Poster requests and parking, shared by video pulls and audio drives. Returns whether the
/// clip is playing, seeking or loading (a seek waits for it).
fn finish(shared: &Arc<Shared>, mut native: MutexGuard<'_, Native>, c: MutexGuard<'_, Control>, turn: Turn, asked: Option<(f64, Attempt)>) -> bool {
    match turn {
        Turn::Idle => {
            drop(c);
            if let Some((at, attempt)) = asked {
                request_poster(shared, &mut native, at, attempt);
            }
            false
        }
        Turn::Loading => true,
        Turn::Finished => {
            let (active, should_park) = (c.active(), c.parkable());
            drop(c);
            drop(native);
            if should_park { park::park(shared) } else { park::unpark(shared) }
            active
        }
    }
}

/// Audio, on the main thread: take a finished load, make or update the player, follow it to
/// the end, tell the app about changes, and keep the tick going while it plays or seeks.
pub(super) fn drive(shared: &Arc<Shared>) {
    if step_audio(shared) {
        AUDIO_BUSY.store(true, Ordering::SeqCst);
        start_ticking();
    }
    shared.settle(true);
}

/// Returns whether the clip needs ticking.
fn step_audio(shared: &Arc<Shared>) -> bool {
    let Some(mtm) = MainThreadMarker::new() else { return false };
    let now = rune_core::time::now();
    // Already being driven further up this stack (a waker that edits the clip).
    let Ok(mut native) = shared.native.try_lock() else { return true };
    let mut c = shared.ctl.lock().unwrap();
    let mut av = Av::new(shared, &mut native, Some(mtm), None);
    let turn = turn::turn(&mut c, &mut av, now);
    let asked = av.asked;
    finish(shared, native, c, turn, asked)
}

/// Main thread: drive every audio clip that might be playing.
fn tick() {
    let clips: Vec<Arc<Shared>> = {
        let mut audio = AUDIO.lock().unwrap();
        audio.retain(|w| w.strong_count() > 0);
        audio.iter().filter_map(Weak::upgrade).collect()
    };
    let mut busy = false;
    for clip in &clips {
        busy |= step_audio(clip);
        clip.settle(true);
    }
    AUDIO_BUSY.store(busy, Ordering::SeqCst);
}

/// A thread that posts `tick` to the main queue every `AUDIO_TICK` while audio is busy.
fn start_ticking() {
    // SeqCst throughout: each side stores one flag, then loads the other.
    if TICKING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::Builder::new()
        .name("rune-audio-tick".into())
        .spawn(|| {
            loop {
                std::thread::sleep(AUDIO_TICK);
                if !AUDIO_BUSY.load(Ordering::SeqCst) {
                    TICKING.store(false, Ordering::SeqCst);
                    // Something started between the check and the store: keep going if no
                    // other ticker took over.
                    if !AUDIO_BUSY.load(Ordering::SeqCst) || TICKING.swap(true, Ordering::SeqCst) {
                        return;
                    }
                }
                on_main(tick);
            }
        })
        .expect("spawn audio tick");
}
