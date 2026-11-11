//! AVFoundation: AVPlayer + AVPlayerItemVideoOutput for windows, AVAssetImageGenerator for
//! posters and headless frames, and a bare AVPlayer for audio.
use super::*;
use crate::platform::main_queue::on_main;
use crate::native_surface::Orient;
use crate::native_surface::macos::PixelBuffer;
use objc2_av_foundation::{
    AVAsset, AVAssetImageGenerator, AVMediaTypeAudio, AVMediaTypeVideo, AVPlayer, AVPlayerItem, AVPlayerItemStatus, AVPlayerItemVideoOutput,
    AVURLAsset,
};
use objc2_core_foundation::{CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo, kCGColorSpaceSRGB};
use objc2_core_media::{CMTime, CMTimeFlags};
use objc2_core_video::{
    CVGetCurrentHostTime, CVGetHostClockFrequency, kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey,
    kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
};
use objc2_foundation_video::{NSDictionary, NSString, NSURL};
use objc2_video::rc::Retained;
use objc2_video::runtime::AnyObject;
use objc2_video::{AnyThread, MainThreadMarker};
use super::phases::Item;
use crate::machine::{Attempt, Effects};
use super::turn::{self, Port, Turn};
use std::time::Duration;

/// Paused players that keep a native player (and its decoder) for a quick resume. Older paused
/// players are released; a video's last frame stays on screen and resuming recreates it.
const PARKED_PLAYERS: usize = 4;
/// Posters never need more than this many pixels per side; headless frames keep full size.
const POSTER_MAX: f64 = 1920.0;
/// How often playing audio checks for its end, loops and loading.
const AUDIO_TICK: Duration = Duration::from_millis(50);

#[derive(Default)]
pub(crate) struct Native {
    /// Headless decodes reuse one generator.
    generator: Option<Retained<AVAssetImageGenerator>>,
    player: Option<Player>,
    posters: Arc<Mutex<Posters>>,
    decoded: Decoded,
}

/// Audio AVFoundation can't read, decoded once on the load worker (or a headless probe) to a WAV
/// it can, which every player for the clip then opens.
#[cfg(feature = "ogg-audio")]
type Decoded = Arc<std::sync::OnceLock<Result<Option<foreign::Wav>, String>>>;
#[cfg(not(feature = "ogg-audio"))]
type Decoded = ();
// The generator is only used under the Shared mutex. Player objects are created, used and
// released on the main thread: pulls and audio ticks run there and `Player`'s drop hops to it.
unsafe impl Send for Native {}

#[derive(Default)]
struct Posters {
    want: Option<(f64, Attempt)>,
    busy: bool,
    /// The asking attempt, and a video poster and its facts, or (no frame) an audio clip's facts.
    done: Option<(Attempt, Result<(Option<ByteFrame>, Meta), String>)>,
}

struct Player {
    // Some until dropped; `Drop` moves the objects to the main thread.
    objects: Option<Objects>,
    /// Video: how its buffers turn upright (they ignore the track's `preferredTransform`).
    orient: Orient,
}
struct Objects {
    player: Retained<AVPlayer>,
    item: Retained<AVPlayerItem>,
    /// None for audio.
    output: Option<Retained<AVPlayerItemVideoOutput>>,
}

impl Player {
    fn new(objects: Objects) -> Self {
        Self { objects: Some(objects), orient: Orient::UP }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let Some(objects) = self.objects.take() else { return };
        if MainThreadMarker::new().is_some() {
            unsafe { objects.player.pause() };
            return;
        }
        // The last handle went away on another thread: release on the main queue.
        struct Release(Objects);
        unsafe impl Send for Release {}
        let release = Release(objects);
        on_main(move || {
            let release = release; // Whole, so the closure is Send.
            unsafe { release.0.player.pause() };
        });
    }
}

/// Paused players holding a native player, oldest first.
static PARKED: Mutex<Vec<Weak<Shared>>> = Mutex::new(Vec::new());
/// Every live audio player, for the tick.
static AUDIO: Mutex<Vec<Weak<Shared>>> = Mutex::new(Vec::new());
/// Some audio clip wants ticking. Written on the main thread.
static AUDIO_BUSY: AtomicBool = AtomicBool::new(false);
/// The tick thread is running.
static TICKING: AtomicBool = AtomicBool::new(false);

/// The source's lowercase extension, if any.
pub(super) fn extension(source: &MediaSource) -> Option<String> {
    let name = match source {
        MediaSource::File(path) => path.file_name()?.to_string_lossy().into_owned(),
        MediaSource::Url(url) => {
            let path = url.split(['?', '#']).next().unwrap_or_default();
            // The last segment of the path, never the host.
            let path = path.split_once("://").map_or(path, |(_, rest)| rest.split_once('/').map_or("", |(_, path)| path));
            path.rsplit('/').next().unwrap_or_default().to_string()
        }
    };
    Some(name.rsplit_once('.')?.1.to_ascii_lowercase()).filter(|ext| !ext.is_empty())
}

/// Audio in a container AVFoundation can't read, which `ogg-audio` decodes instead.
fn foreign_audio(source: &MediaSource, kind: Kind) -> bool {
    cfg!(feature = "ogg-audio")
        && kind == Kind::Audio
        && extension(source).is_some_and(|ext| matches!(ext.as_str(), "ogg" | "oga" | "opus" | "webm" | "mka" | "mkv"))
}

/// AVFoundation has no Ogg, WebM or Matroska demuxer; fail at once rather than wait on a load
/// that can only fail.
fn unsupported_format(source: &MediaSource, kind: Kind) -> Option<String> {
    let ext = extension(source)?;
    (!foreign_audio(source, kind) && matches!(ext.as_str(), "ogg" | "oga" | "ogv" | "opus" | "spx" | "webm" | "mkv" | "mka"))
        .then(|| format!(".{ext} isn't supported on macOS (AVFoundation can't read it); use mp3, m4a, wav, mp4 or mov"))
}

pub(super) fn created(shared: &Arc<Shared>) {
    if let Some(e) = unsupported_format(&shared.source, shared.kind) {
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

fn url(source: &MediaSource) -> Result<Retained<NSURL>, String> {
    match source {
        MediaSource::File(path) => {
            if !path.exists() {
                return Err(format!("no such file: {}", path.display()));
            }
            let path = std::path::absolute(path).unwrap_or_else(|_| path.clone());
            Ok(NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy())))
        }
        MediaSource::Url(url) => NSURL::URLWithString(&NSString::from_str(url)).ok_or_else(|| format!("invalid URL: {url}")),
    }
}

fn asset(source: &MediaSource, headers: &[(String, String)]) -> Result<Retained<AVURLAsset>, String> {
    let url = url(source)?;
    if headers.is_empty() {
        return Ok(unsafe { AVURLAsset::URLAssetWithURL_options(&url, None) });
    }
    let names: Vec<_> = headers.iter().map(|(name, _)| NSString::from_str(name)).collect();
    let values: Vec<_> = headers.iter().map(|(_, value)| NSString::from_str(value)).collect();
    let fields = NSDictionary::from_slices(&names.iter().map(|n| &**n).collect::<Vec<_>>(), &values.iter().map(|v| &**v).collect::<Vec<_>>());
    // Long-standing and widely used, though missing from AVFoundation's headers.
    let key = NSString::from_str("AVURLAssetHTTPHeaderFieldsKey");
    let fields: &AnyObject = &fields;
    let options = NSDictionary::<NSString, AnyObject>::from_slices(&[&*key], &[fields]);
    Ok(unsafe { AVURLAsset::URLAssetWithURL_options(&url, Some(&options)) })
}

/// Opens a clip and reads its facts, blocking. Audio AVFoundation can't read plays from a WAV
/// decoded from it: up front for a foreign extension, or when its own asset has no audio track
/// and its bytes are Ogg or WebM.
fn open(source: &MediaSource, headers: &[(String, String)], kind: Kind, decoded: &Decoded) -> Result<(Retained<AVURLAsset>, Meta), String> {
    let opened = |asset: Retained<AVURLAsset>| meta(&asset, kind).map(|meta| (asset, meta));
    #[cfg(feature = "ogg-audio")]
    {
        let wav = |wav: &foreign::Wav| opened(asset(&MediaSource::File(wav.path().into()), &[])?);
        if foreign_audio(source, kind) {
            return match decoded.get_or_init(|| decode_foreign(source, headers, false)) {
                Ok(Some(decoded)) => wav(decoded),
                Ok(None) => Err("no Ogg or WebM audio".into()),
                Err(e) => Err(e.clone()),
            };
        }
        let own = opened(asset(source, headers)?);
        if own.is_err() && kind == Kind::Audio {
            match decoded.get_or_init(|| decode_foreign(source, headers, true)) {
                Ok(Some(decoded)) => return wav(decoded),
                // Ogg or WebM that didn't decode: say why, not that AVFoundation found nothing.
                Err(e) => return Err(e.clone()),
                Ok(None) => {}
            }
        }
        own
    }
    #[cfg(not(feature = "ogg-audio"))]
    {
        let () = decoded;
        opened(asset(source, headers)?)
    }
}

/// The asset a player opens: the decoded WAV once there is one.
fn player_asset(source: &MediaSource, headers: &[(String, String)], decoded: &Decoded) -> Result<Retained<AVURLAsset>, String> {
    #[cfg(feature = "ogg-audio")]
    if let Some(Ok(Some(wav))) = decoded.get() {
        return asset(&MediaSource::File(wav.path().into()), &[]);
    }
    #[cfg(not(feature = "ogg-audio"))]
    let () = decoded;
    asset(source, headers)
}

/// Fetches a clip and decodes it to a WAV. When sniffing, bytes that aren't Ogg or WebM give
/// `None`, and a fetch that fails gives `None` too, leaving AVFoundation's error to report.
#[cfg(feature = "ogg-audio")]
fn decode_foreign(source: &MediaSource, headers: &[(String, String)], sniff: bool) -> Result<Option<foreign::Wav>, String> {
    let bytes = match fetch(source, headers) {
        Ok(bytes) => bytes,
        Err(_) if sniff => return Ok(None),
        Err(e) => return Err(e),
    };
    if sniff && !foreign::sniff(&bytes) {
        return Ok(None);
    }
    foreign::Wav::decode(bytes).map(Some)
}

/// A clip's bytes: a file read, or a blocking fetch with its request headers.
#[cfg(feature = "ogg-audio")]
#[allow(deprecated)] // NSURLConnection's synchronous load is what a worker or headless run wants.
fn fetch(source: &MediaSource, headers: &[(String, String)]) -> Result<Vec<u8>, String> {
    use objc2_foundation_video::{NSHTTPURLResponse, NSMutableURLRequest, NSURLConnection};
    if let MediaSource::File(path) = source {
        return std::fs::read(path).map_err(|e| format!("can't read {}: {e}", path.display()));
    }
    let request = NSMutableURLRequest::requestWithURL(&*url(source)?);
    for (name, value) in headers {
        request.setValue_forHTTPHeaderField(Some(&NSString::from_str(value)), &NSString::from_str(name));
    }
    let mut response = None;
    let data = NSURLConnection::sendSynchronousRequest_returningResponse_error(&request, Some(&mut response))
        .map_err(|e| e.localizedDescription().to_string())?;
    let status = response.and_then(|r| r.downcast::<NSHTTPURLResponse>().ok()).map(|r| r.statusCode());
    if let Some(status) = status.filter(|s| !(200..300).contains(s)) {
        return Err(format!("HTTP {status}"));
    }
    Ok(data.to_vec())
}

fn seconds(t: CMTime) -> f64 {
    let s = unsafe { t.seconds() };
    if s.is_finite() { s } else { 0.0 }
}
fn cm(seconds: f64) -> CMTime {
    unsafe { CMTime::with_seconds(seconds, 600_000) }
}
fn zero() -> CMTime {
    CMTime { value: 0, timescale: 1, flags: CMTimeFlags::Valid, epoch: 0 }
}

/// Blocks until the asset's tracks load (a network fetch for URLs).
#[allow(deprecated)] // The synchronous loaders are what a worker or headless run wants.
fn meta(asset: &AVURLAsset, kind: Kind) -> Result<Meta, String> {
    let duration = || seconds(unsafe { asset.duration() });
    if kind == Kind::Audio {
        let audio = unsafe { AVMediaTypeAudio }.ok_or("AVMediaTypeAudio unavailable")?;
        if unsafe { asset.tracksWithMediaType(audio) }.firstObject().is_none() {
            return Err("no playable audio track (unreachable, or a format AVFoundation can't read)".into());
        }
        return Ok(Meta { duration: duration(), size: None, frame: 1.0 / 30.0 });
    }
    let video = unsafe { AVMediaTypeVideo }.ok_or("AVMediaTypeVideo unavailable")?;
    let tracks = unsafe { asset.tracksWithMediaType(video) };
    let Some(track) = tracks.firstObject() else {
        return Err("no playable video track".into());
    };
    let size = unsafe { track.naturalSize() };
    let t = unsafe { track.preferredTransform() };
    let w = (size.width * t.a + size.height * t.c).abs().round();
    let h = (size.width * t.b + size.height * t.d).abs().round();
    let min = seconds(unsafe { track.minFrameDuration() });
    let nominal = unsafe { track.nominalFrameRate() } as f64;
    let frame = if min > 0.0 { min } else if nominal > 0.0 { 1.0 / nominal } else { 1.0 / 30.0 };
    if w < 1.0 || h < 1.0 {
        return Err("video track has no size".into());
    }
    Ok(Meta { duration: duration(), size: Some((w as u32, h as u32)), frame })
}

/// How the first video track's buffers turn upright. Blocks until tracks load; free once an
/// item using the asset is ready to play.
#[allow(deprecated)]
fn orient(asset: &AVAsset) -> Orient {
    let Some(video) = (unsafe { AVMediaTypeVideo }) else { return Orient::UP };
    let Some(track) = unsafe { asset.tracksWithMediaType(video) }.firstObject() else { return Orient::UP };
    let t = unsafe { track.preferredTransform() };
    Orient::from_transform(t.a, t.b, t.c, t.d)
}

fn generator(asset: &AVURLAsset, max: Option<f64>) -> Retained<AVAssetImageGenerator> {
    let g = unsafe { AVAssetImageGenerator::assetImageGeneratorWithAsset(asset) };
    unsafe {
        g.setAppliesPreferredTrackTransform(true);
        g.setRequestedTimeToleranceBefore(zero());
        g.setRequestedTimeToleranceAfter(zero());
        if let Some(max) = max {
            g.setMaximumSize(CGSize::new(max, max));
        }
    }
    g
}

#[allow(deprecated)] // copyCGImageAtTime is the synchronous form.
fn image(generator: &AVAssetImageGenerator, at: f64) -> Result<ByteFrame, String> {
    let image = unsafe { generator.copyCGImageAtTime_actualTime_error(cm(at), std::ptr::null_mut()) }
        .map_err(|e| e.localizedDescription().to_string())?;
    bgra(&image)
}

/// Draws a decoded image into sRGB BGRA bytes (opaque, so premultiplied equals straight).
fn bgra(image: &CGImage) -> Result<ByteFrame, String> {
    let (w, h) = (CGImage::width(Some(image)), CGImage::height(Some(image)));
    if w == 0 || h == 0 {
        return Err("decoded an empty frame".into());
    }
    let mut bytes = vec![0u8; w * h * 4];
    let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }));
    let info = CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0;
    let context = unsafe { CGBitmapContextCreate(bytes.as_mut_ptr().cast(), w, h, 8, w * 4, space.as_deref(), info) }
        .ok_or("CGBitmapContextCreate failed")?;
    CGContext::draw_image(Some(&context), CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(w as f64, h as f64)), Some(image));
    drop(context);
    Ok(ByteFrame {
        width: w as u32,
        height: h as u32,
        format: Format::Bgra,
        color: ColorInfo { matrix: crate::native_surface::Matrix::Bt709, transfer: crate::native_surface::Transfer::Srgb },
        planes: [bytes, Vec::new()],
    })
}

pub(super) fn probe(native: &mut Native, source: &MediaSource, headers: &[(String, String)], kind: Kind) -> Result<Meta, String> {
    let (asset, meta) = open(source, headers, kind, &native.decoded)?;
    if kind == Kind::Video {
        native.generator = Some(generator(&asset, None));
    }
    Ok(meta)
}

pub(super) fn decode(native: &mut Native, source: &MediaSource, headers: &[(String, String)], at: f64) -> Result<ByteFrame, String> {
    if native.generator.is_none() {
        let asset = asset(source, headers)?;
        native.generator = Some(generator(&asset, None));
    }
    image(native.generator.as_ref().unwrap(), at)
}

/// A clip's frame at `at` upright (as posters and headless runs show it), and as its buffer
/// stores it (as the player hands it out) with how that turns upright.
#[cfg(test)]
pub(crate) fn test_frames(source: &MediaSource, at: f64) -> Result<(ByteFrame, ByteFrame, Orient), String> {
    let asset = asset(source, &[])?;
    let raw = generator(&asset, None);
    unsafe { raw.setAppliesPreferredTrackTransform(false) };
    Ok((image(&generator(&asset, None), at)?, image(&raw, at)?, orient(&asset)))
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
                let result = objc2_video::rc::autoreleasepool(|_| {
                    if opened.is_none() {
                        let (asset, meta) = open(&source, &headers, kind, &decoded)?;
                        opened = Some(((kind == Kind::Video).then(|| generator(&asset, Some(POSTER_MAX))), meta));
                    }
                    let (generator, meta) = opened.as_ref().unwrap();
                    let frame = match generator {
                        Some(g) => Some(image(g, meta.frame_time(meta.frame_index(at)))?),
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

fn create_player(source: &MediaSource, headers: &[(String, String)], kind: Kind, decoded: &Decoded, mtm: MainThreadMarker) -> Result<Objects, String> {
    let asset = player_asset(source, headers, decoded)?;
    let item = unsafe { AVPlayerItem::playerItemWithAsset(&asset, mtm) };
    let output = (kind == Kind::Video).then(|| {
        let empty = CFDictionary::<CFString, CFType>::from_slices(&[], &[]);
        let format = CFNumber::new_i32(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange as i32);
        let attributes = unsafe {
            CFDictionary::<CFString, CFType>::from_slices(
                &[kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey],
                &[format.as_ref(), AsRef::<CFType>::as_ref(&*empty), CFBoolean::new(true).as_ref()],
            )
        };
        // CFDictionary and NSDictionary are toll-free bridged.
        let attributes = unsafe { &*(CFRetained::as_ptr(&attributes).as_ptr() as *const NSDictionary<NSString, AnyObject>) };
        unsafe {
            let output = AVPlayerItemVideoOutput::initWithPixelBufferAttributes(AVPlayerItemVideoOutput::alloc(), Some(attributes));
            output.setSuppressesPlayerRendering(true);
            item.addOutput(&output);
            output
        }
    });
    let player = unsafe { AVPlayer::playerWithPlayerItem(Some(&item), mtm) };
    Ok(Objects { player, item, output })
}

/// Seconds on the host clock (`CACurrentMediaTime`'s) for an `Instant`.
fn host_seconds(at: Instant) -> f64 {
    let now = Instant::now();
    let host = CVGetCurrentHostTime() as f64 / CVGetHostClockFrequency();
    if at >= now { host + (at - now).as_secs_f64() } else { host - (now - at).as_secs_f64() }
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
        turn::reconcile(&mut c, &mut Av::new(shared, &mut native, Some(mtm), None), Instant::now());
        c.playing()
    };
    drop(native);
    // A parked player that plays again must not be released.
    if playing {
        unpark(shared);
    }
}

/// Audio, on the main thread: what a live player (`Control::live`) reports now, while it plays.
/// `live`: the player's own position is the one to read (`Control::live`).
pub(super) fn live_time(shared: &Shared, live: bool) -> Option<f64> {
    if !live || shared.kind != Kind::Audio || MainThreadMarker::new().is_none() {
        return None;
    }
    let native = shared.native.try_lock().ok()?;
    let p = native.player.as_ref()?;
    let time = seconds(unsafe { p.objects.as_ref()?.player.currentTime() });
    let c = shared.ctl.lock().unwrap();
    c.playing().then(|| c.duration.map_or(time, |d| time.clamp(0.0, d)))
}

/// A clip's native side for one turn, with its locks held. Reads and does what it's asked.
struct Av<'a> {
    shared: &'a Arc<Shared>,
    native: &'a mut Native,
    /// Filled by the caller that already checked, or by the turn when it first needs to know.
    mtm: Option<MainThreadMarker>,
    /// Video, during a pull: the surface and the presentation time its frame is for.
    target: Option<(&'a NativeSurface, Instant)>,
    /// A finished poster load's frame.
    poster: Option<ByteFrame>,
    /// A poster the turn asked for, started once the control lock is released.
    asked: Option<(f64, Attempt)>,
}

impl<'a> Av<'a> {
    fn new(shared: &'a Arc<Shared>, native: &'a mut Native, mtm: Option<MainThreadMarker>, target: Option<(&'a NativeSurface, Instant)>) -> Self {
        Self { shared, native, mtm, target, poster: None, asked: None }
    }

    fn player(&mut self) -> &mut Player {
        self.native.player.as_mut().unwrap()
    }

    fn objects(&self) -> &Objects {
        self.native.player.as_ref().unwrap().objects.as_ref().unwrap()
    }
}

impl Port for Av<'_> {
    fn on_main(&mut self) -> bool {
        self.mtm = self.mtm.or_else(MainThreadMarker::new);
        self.mtm.is_some()
    }

    fn take_poster(&mut self) -> Option<(Attempt, Result<Meta, String>)> {
        let (attempt, done) = self.native.posters.lock().unwrap().done.take()?;
        Some((attempt, done.map(|(bytes, meta)| {
            self.poster = bytes;
            meta
        })))
    }

    fn show_poster(&mut self, next_id: &mut dyn FnMut() -> u64) -> Result<(), String> {
        let (Some(bytes), Some((surface, _))) = (self.poster.take(), self.target) else { return Ok(()) };
        surface.push_bytes(bytes, next_id())
    }

    fn ask_poster(&mut self, at: f64, attempt: Attempt) {
        self.asked = Some((at, attempt));
    }

    fn make_player(&mut self) -> Result<Instant, String> {
        let shared = self.shared;
        let objects = create_player(&shared.source, &shared.headers, shared.kind, &self.native.decoded, self.mtm.unwrap())?;
        self.native.player = Some(Player::new(objects));
        Ok(Instant::now())
    }

    fn drop_player(&mut self) {
        self.native.player = None;
    }

    fn unpark(&mut self) {
        unpark(self.shared);
    }

    fn item(&self) -> Item {
        let item = &self.objects().item;
        let status = unsafe { item.status() };
        if status == AVPlayerItemStatus::Failed {
            Item::Failed(unsafe { item.error() }.map_or_else(|| "playback failed".into(), |e| e.localizedDescription().to_string()))
        } else if status == AVPlayerItemStatus::ReadyToPlay {
            Item::Ready
        } else {
            Item::Loading
        }
    }

    fn orient(&mut self) {
        let o = self.objects();
        if o.output.is_some() {
            let asset = unsafe { o.item.asset() };
            let orient = orient(&asset);
            self.player().orient = orient;
        }
    }

    fn item_facts(&self) -> Meta {
        let item = &self.objects().item;
        let size = unsafe { item.presentationSize() };
        let duration = seconds(unsafe { item.duration() });
        let size = (size.width >= 1.0 && size.height >= 1.0).then(|| (size.width.round() as u32, size.height.round() as u32));
        Meta { duration, size, frame: 1.0 / 30.0 }
    }

    fn alive(&self) -> bool {
        self.native.player.as_ref().is_some_and(|p| p.objects.is_some())
    }

    fn set_muted(&mut self, muted: bool) {
        unsafe { self.objects().player.setMuted(muted) };
    }

    fn set_volume(&mut self, volume: f32) {
        unsafe { self.objects().player.setVolume(volume) };
    }

    fn seek_to(&mut self, at: f64) {
        unsafe { self.objects().player.seekToTime_toleranceBefore_toleranceAfter(cm(at), zero(), zero()) };
    }

    fn set_rate(&mut self, rate: f32) {
        let player = &self.objects().player;
        unsafe { if rate > 0.0 { player.setRate(rate) } else { player.pause() } };
    }

    fn restart(&mut self, rate: f32) {
        let player = &self.objects().player;
        unsafe {
            player.seekToTime_toleranceBefore_toleranceAfter(zero(), zero(), zero());
            player.setRate(rate);
        }
    }

    fn frame(&mut self, next_id: &mut dyn FnMut() -> u64) -> (f64, Result<Option<f64>, String>) {
        let (surface, target) = self.target.unwrap();
        let p = self.native.player.as_ref().unwrap();
        let output = p.objects.as_ref().unwrap().output.as_ref().unwrap();
        let item_time = unsafe { output.itemTimeForHostTime(host_seconds(target)) };
        let mut shown = item_time;
        let mut landed = Ok(None);
        if unsafe { output.hasNewPixelBufferForItemTime(item_time) }
            && let Some(buffer) = unsafe { output.copyPixelBufferForItemTime_itemTimeForDisplay(item_time, &mut shown) }
        {
            let id = next_id();
            // SAFETY: AVPlayerItemVideoOutput hands out finished buffers it no longer writes.
            let pushed = unsafe { PixelBuffer::from_raw(Retained::as_ptr(&buffer) as *mut _) }
                .and_then(|b| surface.push_oriented(b, id, p.orient));
            landed = pushed.map(|_| Some(seconds(shown)));
        }
        (seconds(item_time), landed)
    }

    fn current(&self) -> f64 {
        seconds(unsafe { self.objects().player.currentTime() })
    }

    fn stopped(&mut self) -> bool {
        unsafe { self.objects().player.rate() == 0.0 }
    }
}

pub(super) fn pull(shared: &Arc<Shared>, surface: &NativeSurface, target: Instant) -> bool {
    let now = Instant::now();
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
            let (active, park) = (c.active(), c.parkable());
            drop(c);
            drop(native);
            if park { self::park(shared) } else { unpark(shared) }
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
    let now = Instant::now();
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

fn unpark(shared: &Arc<Shared>) {
    PARKED.lock().unwrap().retain(|w| w.strong_count() > 0 && !std::ptr::eq(w.as_ptr(), Arc::as_ptr(shared)));
}

/// Keep at most `PARKED_PLAYERS` paused players; release the oldest. Main thread only.
fn park(shared: &Arc<Shared>) {
    for clip in parked_with(shared) {
        // A video's last frame stays on its surface; resuming recreates the player.
        let player = {
            let mut native = clip.native.lock().unwrap();
            step(&mut clip.ctl.lock().unwrap(), Event::Parked);
            native.player.take()
        };
        drop(player);
    }
}

/// Parks `shared` and returns the clips to release: the oldest past `PARKED_PLAYERS`. A clip
/// that plays again (before its next pull unparks it) leaves the list and is never released.
pub(super) fn parked_with(shared: &Arc<Shared>) -> Vec<Arc<Shared>> {
    // A clip being edited on another thread right now counts as playing.
    let playing = |clip: &Shared| clip.ctl.try_lock().map_or(true, |c| c.playing());
    let mut parked = PARKED.lock().unwrap();
    parked.retain(|w| w.upgrade().is_some_and(|clip| Arc::ptr_eq(&clip, shared) || !playing(&clip)));
    if !parked.iter().any(|w| std::ptr::eq(w.as_ptr(), Arc::as_ptr(shared))) {
        parked.push(Arc::downgrade(shared));
    }
    let excess = parked.len().saturating_sub(PARKED_PLAYERS);
    parked.drain(..excess).filter_map(|w| w.upgrade()).collect()
}
