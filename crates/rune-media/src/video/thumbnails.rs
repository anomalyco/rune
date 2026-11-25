//! Off-main-thread still-frame extraction for video thumbnails and timeline filmstrips.
//!
//! Build a batch with [`Thumbnails::request`], [`ThumbnailRequest::new`], or
//! [`MediaRequest::thumbnails`](super::MediaRequest::thumbnails). Frames decode on a shared
//! process-wide FIFO queue capped at four concurrent OS decoders (with zero idle threads and no
//! hidden global pixel cache). Each frame writes straight into its [`Thumbnail::pixels`] handle so
//! Rune wakes any window drawing it as soon as it lands.

use super::{MediaSource, WakerSlot};
use rune_image::Pixels;
#[cfg(any(target_os = "macos", target_os = "windows", all(test, not(target_arch = "wasm32"))))]
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Condvar, Mutex};
use std::sync::{Arc, OnceLock};

/// Default longest-side thumbnail cap in pixels.
const DEFAULT_MAX_SIZE: u32 = 320;
/// Hard ceiling on thumbnail dimensions, matching `rune_image::MAX_SIDE`.
const MAX_THUMBNAIL_SIDE: u32 = 8192;

#[cfg(any(target_os = "macos", target_os = "windows", all(test, not(target_arch = "wasm32"))))]
const MAX_WORKERS: usize = 4;

const PHASE_QUEUED: u8 = 0;
#[cfg(any(target_os = "macos", target_os = "windows", all(test, not(target_arch = "wasm32"))))]
const PHASE_RUNNING: u8 = 1;
const PHASE_DONE: u8 = 2;

#[cfg(any(target_os = "macos", target_os = "windows", all(test, not(target_arch = "wasm32"))))]
const INVALID_TIMESTAMP: &str = "invalid timestamp";
const CANCELLED: &str = "cancelled";
#[cfg(any(not(any(target_os = "macos", target_os = "windows")), test))]
const UNSUPPORTED: &str = "video thumbnails aren't supported on this platform yet";

/// How accurately a thumbnail timestamp is decoded.
///
/// ```
/// use rune_media::ThumbnailPrecision;
///
/// assert_eq!(ThumbnailPrecision::default(), ThumbnailPrecision::Exact);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum ThumbnailPrecision {
    /// Decode the frame showing at the requested timestamp (slower on long-GOP clips).
    #[default]
    Exact,
    /// Decode the nearest keyframe (faster for timeline filmstrips and clip grids).
    NearestKeyframe,
}

/// Builder for an off-main-thread batch of video thumbnails.
///
/// Build with [`Thumbnails::request`], [`ThumbnailRequest::new`], or
/// [`MediaRequest::thumbnails`](super::MediaRequest::thumbnails), add timestamps with
/// [`at`](Self::at) or [`times`](Self::times), and kick off decoding with [`start`](Self::start).
/// Header values never appear in `Debug` output.
///
/// ```no_run
/// use rune_media::{ThumbnailPrecision, Thumbnails};
///
/// let strip = Thumbnails::request("clip.mp4")
///     .times([0.0, 2.0, 4.0, 6.0])
///     .max_size(320)
///     .precision(ThumbnailPrecision::NearestKeyframe)
///     .start();
/// assert_eq!(strip.len(), 4);
/// ```
#[derive(Clone)]
pub struct ThumbnailRequest {
    source: MediaSource,
    headers: Vec<(String, String)>,
    times: Vec<f64>,
    max_size: u32,
    precision: ThumbnailPrecision,
    waker: Option<rune_core::Waker>,
}

impl std::fmt::Debug for ThumbnailRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let header_names: Vec<&str> = self.headers.iter().map(|(name, _)| name.as_str()).collect();
        f.debug_struct("ThumbnailRequest")
            .field("source", &self.source)
            .field("headers", &header_names)
            .field("times", &self.times)
            .field("max_size", &self.max_size)
            .field("precision", &self.precision)
            .field("waker", &self.waker.is_some())
            .finish()
    }
}

impl ThumbnailRequest {
    /// Starts a thumbnail request for `source` with default longest side (`320` px) and
    /// [`ThumbnailPrecision::Exact`].
    ///
    /// ```
    /// use rune_media::ThumbnailRequest;
    ///
    /// let strip = ThumbnailRequest::new("clip.mp4").start();
    /// assert!(strip.is_empty());
    /// assert!(strip.is_done());
    /// ```
    pub fn new(source: impl Into<MediaSource>) -> Self {
        Self {
            source: source.into(),
            headers: Vec::new(),
            times: Vec::new(),
            max_size: DEFAULT_MAX_SIZE,
            precision: ThumbnailPrecision::Exact,
            waker: None,
        }
    }

    pub(super) fn with_headers(source: MediaSource, headers: Vec<(String, String)>) -> Self {
        Self { headers, ..Self::new(source) }
    }

    /// Appends timestamps (in seconds) to decode. Duplicate timestamps are decoded once; valid
    /// timestamps decode in ascending order while results map back to the original slot order.
    pub fn times(mut self, times: impl IntoIterator<Item = f64>) -> Self {
        self.times.extend(times);
        self
    }

    /// Appends one timestamp (in seconds) to decode.
    pub fn at(mut self, seconds: f64) -> Self {
        self.times.push(seconds);
        self
    }

    /// Sets the maximum longest side in pixels (clamped to `1..=8192`, default `320`). Frames
    /// smaller than `px` are never upscaled.
    pub fn max_size(mut self, px: u32) -> Self {
        self.max_size = px.clamp(1, MAX_THUMBNAIL_SIDE);
        self
    }

    /// Chooses exact or nearest-keyframe decoding.
    pub fn precision(mut self, precision: ThumbnailPrecision) -> Self {
        self.precision = precision;
        self
    }

    /// Adds an HTTP request header sent when opening a URL source. A repeated name replaces the
    /// earlier value. Values are redacted from `Debug` output.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        let (name, value) = (name.into(), value.into());
        self.headers.retain(|(n, _)| !n.eq_ignore_ascii_case(&name));
        self.headers.push((name, value));
        self
    }

    /// Installs a waker called (from any thread) as frames land or fail, and when the batch
    /// finishes.
    pub fn waker(mut self, waker: rune_core::Waker) -> Self {
        self.waker = Some(waker);
        self
    }

    /// Starts decoding the requested thumbnails off the UI thread and returns the batch handle.
    ///
    /// Empty requests settle immediately without spawning work. On platforms without a video
    /// thumbnail backend yet, every slot settles immediately with an error and any waker fires
    /// once without spawning threads.
    pub fn start(self) -> Thumbnails {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            self.start_inner(None)
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            self.start_unsupported()
        }
    }

    #[cfg(any(not(any(target_os = "macos", target_os = "windows")), test))]
    fn start_unsupported(self) -> Thumbnails {
        let frames = make_frames(&self.times);
        let state = BatchState::new_settled(
            self.source,
            self.headers,
            self.max_size,
            self.precision,
            frames.clone(),
            self.waker,
        );
        if !frames.is_empty() {
            state.fail_remaining(UNSUPPORTED);
        } else {
            state.finish_done();
        }
        Thumbnails(Arc::new(Handle { frames, state }))
    }

    #[cfg(any(target_os = "macos", target_os = "windows", all(test, not(target_arch = "wasm32"))))]
    fn start_inner(
        self,
        #[cfg(test)] test_opener: Option<TestOpener>,
        #[cfg(not(test))] _unused: Option<()>,
    ) -> Thumbnails {
        let frames = make_frames(&self.times);
        let (plan, invalid) = build_plan(&self.times);
        let state = Arc::new(BatchState {
            source: self.source,
            headers: self.headers,
            max_size: self.max_size,
            precision: self.precision,
            frames: frames.clone(),
            plan,
            cancelled: AtomicBool::new(false),
            phase: AtomicU8::new(PHASE_QUEUED),
            completed: AtomicUsize::new(0),
            generation: AtomicU64::new(0),
            waker: WakerSlot::default(),
            done_lock: Mutex::new(false),
            done_cv: Condvar::new(),
            #[cfg(test)]
            test_opener,
        });
        if let Some(waker) = self.waker {
            state.waker.set(waker);
        }
        for idx in invalid {
            let _ = state.settle_error(idx, INVALID_TIMESTAMP.to_string());
        }
        if state.plan.is_empty() {
            state.finish_done();
        } else {
            let generation = state.generation.load(Ordering::Acquire);
            state.waker.tell(generation);
            if !enqueue(state.clone()) {
                state.fail_remaining("couldn't start a thumbnail decoder worker");
            }
        }
        Thumbnails(Arc::new(Handle { frames, state }))
    }

    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(super) fn start_with_decoder(self, opener: TestOpener) -> Thumbnails {
        self.start_inner(Some(opener))
    }

    #[cfg(test)]
    pub(super) fn max_size_value(&self) -> u32 {
        self.max_size
    }

    #[cfg(test)]
    pub(super) fn source_value(&self) -> &MediaSource {
        &self.source
    }

    #[cfg(test)]
    pub(super) fn headers_value(&self) -> &[(String, String)] {
        &self.headers
    }

    #[cfg(test)]
    pub(super) fn start_as_unsupported(self) -> Thumbnails {
        self.start_unsupported()
    }
}

/// A running or completed batch of video thumbnails.
///
/// Cheap to clone; all clones share one batch. Dropping the last `Thumbnails` clone (or calling
/// [`cancel`](Self::cancel)) cancels any frames not yet decoded and releases the underlying
/// decoder as soon as the current frame finishes. Cloned [`Thumbnail`] handles hold only their own
/// [`Pixels`] and outcome, so they stay readable after the parent batch is dropped.
///
/// ```no_run
/// use rune_media::{ThumbnailPrecision, Thumbnails};
///
/// let strip = Thumbnails::request("clip.mp4")
///     .times([0.0, 1.0, 2.0])
///     .max_size(160)
///     .precision(ThumbnailPrecision::NearestKeyframe)
///     .start();
/// strip.wait();
/// assert!(strip.is_done());
/// ```
#[derive(Clone)]
pub struct Thumbnails(Arc<Handle>);

impl std::fmt::Debug for Thumbnails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Thumbnails")
            .field("len", &self.len())
            .field("completed", &self.completed())
            .field("is_done", &self.is_done())
            .finish()
    }
}

impl Thumbnails {
    /// Starts building a thumbnail request for `source`.
    pub fn request(source: impl Into<MediaSource>) -> ThumbnailRequest {
        ThumbnailRequest::new(source)
    }

    /// Every requested thumbnail slot, in the order timestamps were added.
    pub fn frames(&self) -> &[Thumbnail] {
        &self.0.frames
    }

    /// The thumbnail slot at `index`, if in range.
    pub fn frame(&self, index: usize) -> Option<&Thumbnail> {
        self.0.frames.get(index)
    }

    /// Number of requested thumbnail slots.
    pub fn len(&self) -> usize {
        self.0.frames.len()
    }

    /// Whether the batch has zero requested timestamps.
    pub fn is_empty(&self) -> bool {
        self.0.frames.is_empty()
    }

    /// Number of slots that have settled (either ready with pixels or failed with an error).
    pub fn completed(&self) -> usize {
        self.0.state.completed.load(Ordering::Acquire)
    }

    /// Whether the batch has finished (every slot settled, or cancelled and cleaned up).
    pub fn is_done(&self) -> bool {
        self.0.state.phase.load(Ordering::Acquire) == PHASE_DONE
    }

    /// Installs or replaces the waker notified as slots settle. If progress has already happened,
    /// the new waker is called once right away to catch up.
    pub fn set_waker(&self, waker: rune_core::Waker) {
        self.0.state.waker.set(waker);
        let generation = self.0.state.generation.load(Ordering::Acquire);
        self.0.state.waker.tell(generation);
    }

    /// Cancels any frames not yet decoded. Remaining unsettled slots fail with `"cancelled"` and
    /// already-landed frames stay readable.
    pub fn cancel(&self) {
        self.0.state.cancel();
    }

    /// Blocks the calling thread until every slot has settled (or the batch is cancelled).
    /// Non-blocking on `wasm32` (where batches settle immediately in `start()`).
    pub fn wait(&self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut done = self.0.state.done_lock.lock().unwrap_or_else(|e| e.into_inner());
            while !*done {
                done = self.0.state.done_cv.wait(done).unwrap_or_else(|e| e.into_inner());
            }
        }
    }
}

/// One still frame from a [`Thumbnails`] batch.
///
/// Pass [`pixels`](Self::pixels) straight to `El::pixels` or `El::pixels_fit`; Rune wakes any
/// window showing it as soon as the frame lands. Cloning a `Thumbnail` shares its [`Pixels`] and
/// outcome without keeping the parent batch alive.
///
/// ```
/// use rune_media::Thumbnails;
///
/// let strip = Thumbnails::request("clip.mp4").at(f64::NAN).start();
/// let frame = strip.frame(0).unwrap();
/// assert!(!frame.is_ready());
/// assert!(frame.error().is_some());
/// ```
#[derive(Clone, Debug)]
pub struct Thumbnail {
    requested_time: f64,
    pixels: Pixels,
    outcome: Arc<OnceLock<Result<f64, String>>>,
}

impl Thumbnail {
    /// The shared pixel handle populated as soon as this frame lands.
    pub fn pixels(&self) -> &Pixels {
        &self.pixels
    }

    /// The timestamp (in seconds) originally requested for this slot.
    pub fn requested_time(&self) -> f64 {
        self.requested_time
    }

    /// The actual presentation timestamp (in seconds) of the decoded frame, once ready.
    pub fn actual_time(&self) -> Option<f64> {
        self.outcome.get().and_then(|res| res.as_ref().ok().copied())
    }

    /// Decoded pixel dimensions `(width, height)`, once ready.
    pub fn size(&self) -> Option<(u32, u32)> {
        self.is_ready().then(|| self.pixels.size())
    }

    /// Whether this frame has decoded into [`pixels`](Self::pixels).
    pub fn is_ready(&self) -> bool {
        matches!(self.outcome.get(), Some(Ok(_)))
    }

    /// Why this frame failed (invalid timestamp, unsupported platform, open/decode failure, or
    /// cancelled), if it failed.
    pub fn error(&self) -> Option<&str> {
        self.outcome.get().and_then(|res| res.as_ref().err().map(String::as_str))
    }
}

struct Handle {
    frames: Vec<Thumbnail>,
    state: Arc<BatchState>,
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.state.cancel();
    }
}

/// Decoded RGBA8 still frame returned by a platform backend or test decoder.
#[cfg_attr(not(any(target_os = "macos", target_os = "windows", test)), allow(dead_code))]
pub(crate) struct DecodedFrame {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rgba: Vec<u8>,
    pub(crate) actual_time: f64,
}

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(super) trait TestDecoder {
    fn decode(&mut self, at: f64, max_size: u32, precision: ThumbnailPrecision) -> Result<DecodedFrame, String>;
}

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(super) type TestOpener = Arc<
    dyn Fn(&MediaSource, &[(String, String)], u32, ThumbnailPrecision) -> Result<Box<dyn TestDecoder>, String>
        + Send
        + Sync,
>;

#[cfg_attr(not(any(target_os = "macos", target_os = "windows", test)), allow(dead_code))]
struct DecodeStep {
    at: f64,
    slots: Vec<usize>,
}

#[cfg_attr(not(any(target_os = "macos", target_os = "windows", test)), allow(dead_code))]
struct BatchState {
    source: MediaSource,
    headers: Vec<(String, String)>,
    max_size: u32,
    precision: ThumbnailPrecision,
    frames: Vec<Thumbnail>,
    plan: Vec<DecodeStep>,
    cancelled: AtomicBool,
    phase: AtomicU8,
    completed: AtomicUsize,
    generation: AtomicU64,
    waker: WakerSlot,
    #[cfg(not(target_arch = "wasm32"))]
    done_lock: Mutex<bool>,
    #[cfg(not(target_arch = "wasm32"))]
    done_cv: Condvar,
    #[cfg(all(test, not(target_arch = "wasm32")))]
    test_opener: Option<TestOpener>,
}

impl BatchState {
    #[cfg(any(not(any(target_os = "macos", target_os = "windows")), test))]
    fn new_settled(
        source: MediaSource,
        headers: Vec<(String, String)>,
        max_size: u32,
        precision: ThumbnailPrecision,
        frames: Vec<Thumbnail>,
        waker: Option<rune_core::Waker>,
    ) -> Arc<Self> {
        let state = Arc::new(Self {
            source,
            headers,
            max_size,
            precision,
            frames,
            plan: Vec::new(),
            cancelled: AtomicBool::new(false),
            phase: AtomicU8::new(PHASE_DONE),
            completed: AtomicUsize::new(0),
            generation: AtomicU64::new(0),
            waker: WakerSlot::default(),
            #[cfg(not(target_arch = "wasm32"))]
            done_lock: Mutex::new(false),
            #[cfg(not(target_arch = "wasm32"))]
            done_cv: Condvar::new(),
            #[cfg(all(test, not(target_arch = "wasm32")))]
            test_opener: None,
        });
        if let Some(waker) = waker {
            state.waker.set(waker);
        }
        state
    }

    #[cfg(any(target_os = "macos", target_os = "windows", all(test, not(target_arch = "wasm32"))))]
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        if self.phase.compare_exchange(PHASE_QUEUED, PHASE_DONE, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            self.fail_remaining(CANCELLED);
        }
    }

    #[cfg(any(target_os = "macos", target_os = "windows", all(test, not(target_arch = "wasm32"))))]
    fn settle_ok(&self, slots: &[usize], frame: DecodedFrame) {
        let DecodedFrame { width, height, rgba, actual_time } = frame;
        let last = slots.len().saturating_sub(1);
        let mut rgba = Some(rgba);
        let mut any = false;
        for (pos, &idx) in slots.iter().enumerate() {
            let slot = &self.frames[idx];
            if slot.outcome.get().is_some() {
                continue;
            }
            let buf = if pos == last {
                rgba.take().unwrap_or_default()
            } else {
                rgba.as_ref().cloned().unwrap_or_default()
            };
            // Store pixels before publishing the outcome so any observer seeing `is_ready()`
            // already observes the populated pixel buffer and dimensions.
            slot.pixels.set(width, height, buf);
            if slot.outcome.set(Ok(actual_time)).is_ok() {
                self.completed.fetch_add(1, Ordering::AcqRel);
                any = true;
            }
        }
        if any {
            let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
            self.waker.tell(generation);
        }
    }

    fn settle_error(&self, idx: usize, message: String) -> bool {
        if self.frames[idx].outcome.set(Err(message)).is_ok() {
            self.completed.fetch_add(1, Ordering::AcqRel);
            self.generation.fetch_add(1, Ordering::AcqRel);
            true
        } else {
            false
        }
    }

    fn fail_remaining(&self, message: &str) {
        for idx in 0..self.frames.len() {
            if self.frames[idx].outcome.get().is_none() {
                let _ = self.settle_error(idx, message.to_string());
            }
        }
        self.finish_done();
    }

    fn finish_done(&self) {
        self.phase.store(PHASE_DONE, Ordering::Release);
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut done = self.done_lock.lock().unwrap_or_else(|e| e.into_inner());
            *done = true;
            self.done_cv.notify_all();
        }
        self.waker.tell(generation);
    }

    #[cfg(any(target_os = "macos", target_os = "windows", all(test, not(target_arch = "wasm32"))))]
    fn drive_decoder<D>(&self, mut decoder: D, mut decode: impl FnMut(&mut D, f64, u32, ThumbnailPrecision) -> Result<DecodedFrame, String>) {
        for step in &self.plan {
            if self.is_cancelled() {
                drop(decoder);
                self.fail_remaining(CANCELLED);
                return;
            }
            let decoded = decode(&mut decoder, step.at, self.max_size, self.precision);
            if self.is_cancelled() {
                drop(decoder);
                self.fail_remaining(CANCELLED);
                return;
            }
            match decoded {
                Ok(frame) => self.settle_ok(&step.slots, frame),
                Err(err) => {
                    let mut any = false;
                    for &idx in &step.slots {
                        any |= self.settle_error(idx, err.clone());
                    }
                    if any {
                        let generation = self.generation.load(Ordering::Acquire);
                        self.waker.tell(generation);
                    }
                }
            }
        }
        drop(decoder);
        self.finish_done();
    }

    #[cfg(any(target_os = "macos", target_os = "windows", all(test, not(target_arch = "wasm32"))))]
    fn run(&self) {
        if self.phase.compare_exchange(PHASE_QUEUED, PHASE_RUNNING, Ordering::AcqRel, Ordering::Acquire).is_err() {
            return;
        }
        if self.is_cancelled() {
            self.fail_remaining(CANCELLED);
            return;
        }
        #[cfg(test)]
        if let Some(opener) = &self.test_opener {
            match opener(&self.source, &self.headers, self.max_size, self.precision) {
                Ok(decoder) => self.drive_decoder(decoder, |d, at, max, prec| d.decode(at, max, prec)),
                Err(err) => self.fail_remaining(&err),
            }
            return;
        }
        #[cfg(target_os = "macos")]
        {
            match super::macos::open_thumbnails(&self.source, &self.headers, self.max_size, self.precision) {
                Ok(decoder) => self.drive_decoder(decoder, |d, at, max, prec| d.decode(at, max, prec)),
                Err(err) => self.fail_remaining(&err),
            }
        }
        #[cfg(target_os = "windows")]
        {
            match super::windows::still::open_thumbnails(&self.source, &self.headers) {
                Ok(decoder) => self.drive_decoder(decoder, |d, at, max, prec| d.decode(at, max, prec)),
                Err(err) => self.fail_remaining(&err),
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            self.fail_remaining(UNSUPPORTED);
        }
    }
}

fn make_frames(times: &[f64]) -> Vec<Thumbnail> {
    times
        .iter()
        .map(|&requested_time| Thumbnail {
            requested_time,
            pixels: Pixels::new(),
            outcome: Arc::new(OnceLock::new()),
        })
        .collect()
}

/// Groups finite timestamps (negative clamped to `0.0`) in ascending order, deduplicating equal
/// clamped timestamps while recording every original slot index, and collects non-finite indices.
#[cfg(any(target_os = "macos", target_os = "windows", all(test, not(target_arch = "wasm32"))))]
fn build_plan(times: &[f64]) -> (Vec<DecodeStep>, Vec<usize>) {
    let mut invalid = Vec::new();
    // All finite values >= 0.0 share the same total order under `to_bits()` once `-0.0` is
    // normalized to `+0.0`.
    let mut by_time: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    for (idx, &t) in times.iter().enumerate() {
        if !t.is_finite() {
            invalid.push(idx);
            continue;
        }
        let clamped = if t <= 0.0 { 0.0 } else { t };
        by_time.entry(clamped.to_bits()).or_default().push(idx);
    }
    let plan = by_time
        .into_iter()
        .map(|(bits, slots)| DecodeStep { at: f64::from_bits(bits), slots })
        .collect();
    (plan, invalid)
}

#[cfg(any(target_os = "macos", target_os = "windows", all(test, not(target_arch = "wasm32"))))]
struct Queue {
    jobs: VecDeque<Arc<BatchState>>,
    workers: usize,
}

#[cfg(any(target_os = "macos", target_os = "windows", all(test, not(target_arch = "wasm32"))))]
static QUEUE: Mutex<Queue> = Mutex::new(Queue { jobs: VecDeque::new(), workers: 0 });

#[cfg(any(target_os = "macos", target_os = "windows", all(test, not(target_arch = "wasm32"))))]
fn enqueue(state: Arc<BatchState>) -> bool {
    let mut q = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    q.jobs.push_back(state);
    let most = MAX_WORKERS.min(std::thread::available_parallelism().map_or(1, |n| n.get()));
    if q.workers >= most {
        return true;
    }
    q.workers += 1;
    drop(q);
    if rune_core::task::spawn("rune-thumbnails", worker_loop).is_ok() {
        return true;
    }
    let mut q = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    q.workers -= 1;
    if q.workers > 0 {
        return true;
    }
    let _ = q.jobs.pop_back();
    false
}

#[cfg(any(target_os = "macos", target_os = "windows", all(test, not(target_arch = "wasm32"))))]
fn worker_loop() {
    loop {
        let job = {
            let mut q = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
            let Some(job) = q.jobs.pop_front() else {
                q.workers -= 1;
                return;
            };
            job
        };
        job.run();
    }
}
