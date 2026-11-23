//! When each traced frame reached the screen, next to when it was meant to.
//!
//! A native `--trace` stamps frames at CPU submission (`t`) and, on macOS, at the display
//! link's target (`target_ms`). Neither shows whether frames reach the screen evenly: two
//! frames can present a fraction of a millisecond apart and leave a gap after them, which
//! reads as a hitch even when every frame's motion is right. On macOS the traced window's
//! layer reports each drawable's Metal `presentedTime`, and the trace writes it as
//! `present_ms`, with `present_dt_ms` from the previous presented frame. Other platforms
//! have no reliable source and write neither.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;
use crate::machine::Effects;
use crate::time::Instant;

/// A drawable's sequence number and when it reached the screen, or `None` when the system
/// skipped it.
pub(crate) type Presented = (u64, Option<Instant>);

/// Where present times come from. Implementations are filled from another thread.
pub(crate) trait Source: Send + Sync {
    fn take(&self, out: &mut Vec<Presented>);
}

/// A frame whose present time hasn't arrived after this many later frames is written
/// without one. Presentation runs two or three frames behind submission.
const WAIT_FRAMES: u32 = 32;

/// Trace lines held until their frame's present time is known, so each frame's line carries
/// it. Lines stay in submission order.
pub(crate) struct Presents {
    source: Arc<dyn Source>,
    start: Instant,
    pending: VecDeque<Pending>,
    cadence: Cadence,
    scratch: Vec<Presented>,
}

struct Pending {
    /// The line without its closing brace.
    line: String,
    present: Present,
    /// The previous traced frame asked for this one right away, so the gap between their
    /// presents is cadence, not idle time.
    continuous: bool,
    age: u32,
}

/// Where a queued frame stands with its present time.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Present {
    /// The frame presented no drawable the source tracks.
    NoDrawable,
    /// Drawable `seq` hasn't reported yet.
    Waiting { seq: u64 },
    /// Drawable `seq` reached the screen `at` ms after the trace's start.
    Landed { seq: u64, at: f64 },
    /// Metal skipped drawable `seq`.
    Skipped { seq: u64 },
}

impl Present {
    fn new(seq: Option<u64>) -> Present {
        seq.map_or(Present::NoDrawable, |seq| Present::Waiting { seq })
    }

    fn seq(self) -> Option<u64> {
        match self {
            Present::NoDrawable => None,
            Present::Waiting { seq } | Present::Landed { seq, .. } | Present::Skipped { seq } => Some(seq),
        }
    }
}

/// Whether the next written line's present starts an interval.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Cadence {
    /// The last written frame didn't reach the screen, or none was written.
    Broken,
    /// The last written frame reached the screen `at` ms in.
    After(f64),
}

/// A line leaves the queue: its frame's present, and whether it was drawn right after the frame
/// before.
#[derive(Clone, Copy, Debug)]
struct Written {
    present: Present,
    continuous: bool,
}

/// What a written line carries.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Field {
    /// `present_ms`, and `present_dt_ms` when the frame before was presented back to back.
    At { at: f64, dt: Option<f64> },
    /// `present_ms: null`: Metal skipped the drawable.
    Null,
}

/// An interval spans only frames drawn back to back that both reached the screen.
fn step(cadence: &mut Cadence, event: Written) -> Effects<Field> {
    let last = match *cadence {
        Cadence::After(last) if event.continuous => Some(last),
        _ => None,
    };
    match event.present {
        Present::Landed { at, .. } => {
            *cadence = Cadence::After(at);
            Effects::one(Field::At { at, dt: last.map(|last| r3(at - last)) })
        }
        Present::Skipped { .. } => {
            *cadence = Cadence::Broken;
            Effects::one(Field::Null)
        }
        Present::NoDrawable | Present::Waiting { .. } => {
            *cadence = Cadence::Broken;
            Effects::none()
        }
    }
}

/// Milliseconds from `start`, rounded like the trace's `t`, negative before it.
pub(crate) fn ms_since(start: Instant, at: Instant) -> f64 {
    let ms = match at.checked_duration_since(start) {
        Some(d) => d.as_secs_f64() * 1000.0,
        None => -start.duration_since(at).as_secs_f64() * 1000.0,
    };
    r3(ms)
}

fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

impl Presents {
    pub(crate) fn new(source: Arc<dyn Source>, start: Instant) -> Self {
        Self { source, start, pending: VecDeque::new(), cadence: Cadence::Broken, scratch: Vec::new() }
    }

    /// Queues a frame's line (without its closing brace) and writes every line now complete.
    /// `seq`: the drawable the frame presented, if the source tracks it.
    pub(crate) fn push(&mut self, line: String, seq: Option<u64>, continuous: bool, out: &mut impl std::io::Write) {
        for p in &mut self.pending { p.age += 1; }
        self.pending.push_back(Pending { line, present: Present::new(seq), continuous, age: 0 });
        self.poll();
        self.write_ready(out, false);
    }

    /// Whether any line still waits for its present time.
    pub(crate) fn waiting(&mut self) -> bool {
        self.poll();
        self.pending.iter().any(|p| matches!(p.present, Present::Waiting { .. }))
    }

    /// Writes every queued line, with or without its present time.
    pub(crate) fn flush(&mut self, out: &mut impl std::io::Write) {
        self.poll();
        self.write_ready(out, true);
    }

    fn poll(&mut self) {
        self.source.take(&mut self.scratch);
        for (seq, at) in self.scratch.drain(..) {
            if let Some(p) = self.pending.iter_mut().find(|p| p.present.seq() == Some(seq)) {
                p.present = match at {
                    Some(at) => Present::Landed { seq, at: ms_since(self.start, at) },
                    None => Present::Skipped { seq },
                };
            }
        }
    }

    fn write_ready(&mut self, out: &mut impl std::io::Write, all: bool) {
        while let Some(p) = self.pending.front() {
            if !(all || !matches!(p.present, Present::Waiting { .. }) || p.age >= WAIT_FRAMES) { break; }
            let p = self.pending.pop_front().unwrap();
            let line = self.finish(p);
            writeln!(out, "{line}").ok();
        }
    }

    fn finish(&mut self, p: Pending) -> String {
        let mut line = p.line;
        for field in step(&mut self.cadence, Written { present: p.present, continuous: p.continuous }) {
            match field {
                Field::At { at, dt } => {
                    write!(line, r#","present_ms":{at}"#).ok();
                    if let Some(dt) = dt { write!(line, r#","present_dt_ms":{dt}"#).ok(); }
                }
                Field::Null => line.push_str(r#","present_ms":null"#),
            }
        }
        line.push('}');
        line
    }
}

/// Percentiles of a sample, nearest rank.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Spread {
    pub p50: f64,
    pub p95: f64,
    pub max: f64,
}

impl Spread {
    fn of(mut values: Vec<f64>) -> Option<Spread> {
        if values.is_empty() { return None; }
        values.sort_by(f64::total_cmp);
        let rank = |p: f64| values[((p * values.len() as f64).ceil() as usize).clamp(1, values.len()) - 1];
        Some(Spread { p50: rank(0.5), p95: rank(0.95), max: values[values.len() - 1] })
    }
}

/// Present cadence over a trace.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Summary {
    pub hz: f64,
    pub frames: usize,
    /// Frames with a present time.
    pub presented: usize,
    /// Frames the system skipped (`present_ms: null`).
    pub skipped: usize,
    /// Present-to-present intervals between frames drawn back to back.
    pub interval: Option<Spread>,
    /// Intervals under [`BUNCHED_MS`]: frames that reached the screen together.
    pub bunched: usize,
    /// Intervals over 1.5 refresh periods: a refresh showed nothing new.
    pub missed: usize,
    /// Present time minus the display link's target.
    pub lag: Option<Spread>,
}

pub(crate) const BUNCHED_MS: f64 = 2.0;

impl Summary {
    /// Summarizes a trace's text. `None` for traces without present times, including every
    /// trace written before they existed.
    pub(crate) fn of_trace(text: &str) -> Option<Summary> {
        let mut lines = text.lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok());
        let header = lines.next()?;
        let mut hz = header["hz"].as_f64().filter(|hz| hz.is_finite() && *hz > 0.0).unwrap_or(60.0);
        let (mut frames, mut presented, mut skipped, mut timed) = (0, 0, 0, false);
        let (mut intervals, mut lags) = (Vec::new(), Vec::new());
        for frame in lines.filter(|l| l.get("t").is_some()) {
            frames += 1;
            if let Some(rate) = frame["hz"].as_f64().filter(|hz| hz.is_finite() && *hz > 0.0) {
                hz = rate;
            }
            let Some(present) = frame.get("present_ms") else { continue };
            timed = true;
            let Some(present) = present.as_f64() else { skipped += 1; continue };
            presented += 1;
            if let Some(dt) = frame["present_dt_ms"].as_f64() { intervals.push(dt); }
            if let Some(target) = frame["target_ms"].as_f64() { lags.push(present - target); }
        }
        if !timed { return None; }
        let missed_ms = 1.5 * 1000.0 / hz;
        Some(Summary {
            hz, frames, presented, skipped,
            bunched: intervals.iter().filter(|dt| **dt < BUNCHED_MS).count(),
            missed: intervals.iter().filter(|dt| **dt > missed_ms).count(),
            interval: Spread::of(intervals),
            lag: Spread::of(lags),
        })
    }
}

impl std::fmt::Display for Summary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let spread = |s: Option<Spread>| s.map_or("none".into(), |s| format!("p50 {:.2} p95 {:.2} max {:.2} ms", s.p50, s.p95, s.max));
        write!(f, "present: {} frames, {} presented, {} skipped at {:.0} Hz; interval {}; {} bunched (<{} ms), {} missed (>{:.2} ms); lag {}",
            self.frames, self.presented, self.skipped, self.hz, spread(self.interval), self.bunched, BUNCHED_MS,
            self.missed, 1.5 * 1000.0 / self.hz, spread(self.lag))
    }
}

/// How long a finished trace waits for its last frames' present times.
pub(crate) const SETTLE: Duration = Duration::from_millis(150);

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub(crate) mod metal {
    //! Present times from a CAMetalLayer. wgpu acquires and presents the drawable itself and
    //! exposes neither, so the traced window's layer becomes an instance of a subclass of its
    //! own class whose `nextDrawable` adds a Metal presented handler to each drawable it
    //! returns. Only that layer changes, only while tracing or while a transparent window with
    //! a shadow needs to know when its frames land (`macos_window_shape`), and it gets its
    //! class back when the hook drops.

    use super::{Presented, Source};
    use objc2_metal::MTLDrawable;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject, ClassBuilder, ProtocolObject, Sel};
    use objc2::{Message, msg_send, sel};
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::ffi::CString;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    #[link(name = "QuartzCore", kind = "framework")]
    unsafe extern "C" { fn CACurrentMediaTime() -> f64; }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRunLoopGetMain() -> *mut std::ffi::c_void;
        fn CFRunLoopWakeUp(run_loop: *mut std::ffi::c_void);
    }

    /// Wakes the main run loop for another turn. Callable from any thread.
    pub(crate) fn wake_main() {
        // SAFETY: CFRunLoopWakeUp is thread-safe; the main run loop lives as long as the process.
        unsafe { CFRunLoopWakeUp(CFRunLoopGetMain()) }
    }

    /// Drawables acquired and presented on a hooked layer.
    #[derive(Default)]
    pub(crate) struct Log {
        next: AtomicU64,
        acquired: AtomicU64,
        /// `(seq, presentedTime)`: host seconds, 0 for a skipped drawable. Kept only while tracing.
        presented: Mutex<Vec<(u64, f64)>>,
        untraced: bool,
        /// The newest drawable that reached the screen.
        on_screen: AtomicU64,
        /// Wake the main run loop when a drawable reaches the screen.
        watch: AtomicBool,
    }

    impl Log {
        /// A log that only tracks which drawable is on screen, for a window that isn't traced.
        pub(crate) fn untraced() -> Self { Log { untraced: true, ..Default::default() } }

        /// The drawable the last `nextDrawable` returned, once.
        pub(crate) fn acquired(&self) -> Option<u64> {
            Some(self.acquired.swap(0, Ordering::AcqRel)).filter(|seq| *seq != 0)
        }

        /// The drawable the last `nextDrawable` returned (0 before the first).
        pub(crate) fn last_acquired(&self) -> u64 { self.next.load(Ordering::Acquire) }

        /// The newest drawable Metal reported on screen (0 before the first).
        pub(crate) fn on_screen(&self) -> u64 { self.on_screen.load(Ordering::Acquire) }

        /// While on, each drawable that reaches the screen wakes the main run loop.
        pub(crate) fn watch(&self, on: bool) { self.watch.store(on, Ordering::Release); }
    }

    impl Source for Log {
        fn take(&self, out: &mut Vec<Presented>) {
            let mut presented = self.presented.lock().unwrap();
            if presented.is_empty() { return; }
            // SAFETY: CACurrentMediaTime has no preconditions.
            let (now, media_now) = (crate::time::now(), unsafe { CACurrentMediaTime() });
            out.extend(presented.drain(..).map(|(seq, at)| (seq, (at > 0.0).then(|| instant_at(now, media_now, at)))));
        }
    }

    /// The instant of host time `media` (CACurrentMediaTime seconds), given both clocks now.
    pub(crate) fn instant_at(now: Instant, media_now: f64, media: f64) -> Instant {
        let delta = Duration::from_secs_f64((media - media_now).abs());
        if media >= media_now { now + delta } else { now.checked_sub(delta).unwrap_or(now) }
    }

    /// A hooked layer: its address, original class and log.
    struct Hooked { addr: usize, base: &'static AnyClass, log: Arc<Log> }

    static LAYERS: Mutex<Vec<Hooked>> = Mutex::new(Vec::new());

    unsafe extern "C-unwind" fn next_drawable(this: &AnyObject, _: Sel) -> *mut AnyObject {
        let addr = this as *const AnyObject as usize;
        let hooked = LAYERS.lock().unwrap().iter().find(|h| h.addr == addr).map(|h| (h.base, h.log.clone()));
        let base = match hooked {
            Some((base, _)) => base,
            // The hook dropped while another subclass (key-value observing) sat on top of ours,
            // so the class stayed. Skip every hooked class, or `super` lands back here.
            None => original_class(this.class()),
        };
        // SAFETY: `base` is the layer's original CAMetalLayer class; the result is +0.
        let drawable: *mut AnyObject = unsafe { msg_send![super(this, base), nextDrawable] };
        if let (Some((_, log)), Some(raw)) = (hooked, NonNull::new(drawable)) {
            let seq = log.next.fetch_add(1, Ordering::Relaxed) + 1;
            log.acquired.store(seq, Ordering::Release);
            let handler = block2::RcBlock::new(move |d: NonNull<ProtocolObject<dyn MTLDrawable>>| {
                // SAFETY: Metal passes the presented drawable for the handler's duration.
                let at = unsafe { d.as_ref() }.presentedTime();
                if !log.untraced { log.presented.lock().unwrap().push((seq, at)); }
                if at > 0.0 {
                    log.on_screen.fetch_max(seq, Ordering::AcqRel);
                    if log.watch.load(Ordering::Acquire) { wake_main(); }
                }
            });
            // SAFETY: nextDrawable returns a CAMetalDrawable, which conforms to MTLDrawable;
            // Metal copies the block.
            unsafe {
                let drawable = raw.cast::<ProtocolObject<dyn MTLDrawable>>().as_ref();
                drawable.addPresentedHandler(block2::RcBlock::as_ptr(&handler));
            }
        }
        drawable
    }

    const HOOKED_PREFIX: &str = "RunePresentTimed_";

    /// The superclass of the hooked class nearest the root in `class`'s chain.
    fn original_class(class: &'static AnyClass) -> &'static AnyClass {
        let mut hooked = None;
        let mut next = Some(class);
        while let Some(class) = next {
            if class.name().to_bytes().starts_with(HOOKED_PREFIX.as_bytes()) { hooked = Some(class); }
            next = class.superclass();
        }
        hooked.and_then(AnyClass::superclass).expect("next_drawable only runs on a hooked class")
    }

    /// The hooked subclass of `base`, made once per class.
    fn hooked_class(base: &AnyClass) -> Option<&'static AnyClass> {
        static CLASSES: Mutex<Vec<(usize, &'static AnyClass)>> = Mutex::new(Vec::new());
        let mut classes = CLASSES.lock().unwrap();
        let key = base as *const AnyClass as usize;
        if let Some((_, class)) = classes.iter().find(|(k, _)| *k == key) { return Some(class); }
        let name = CString::new(format!("{HOOKED_PREFIX}{}", base.name().to_str().ok()?)).ok()?;
        let class = match ClassBuilder::new(&name, base) {
            Some(mut builder) => {
                // SAFETY: the signature matches -[CAMetalLayer nextDrawable].
                unsafe { builder.add_method(sel!(nextDrawable), next_drawable as unsafe extern "C-unwind" fn(_, _) -> _); }
                builder.register()
            }
            None => AnyClass::get(&name)?,
        };
        classes.push((key, class));
        Some(class)
    }

    /// A surface's layer, reporting present times to a [`Log`] until dropped.
    pub(crate) struct LayerHook {
        layer: Retained<AnyObject>,
        base: &'static AnyClass,
        class: &'static AnyClass,
    }

    impl LayerHook {
        /// `None` when the surface isn't a plain CAMetalLayer on Metal.
        pub(crate) fn attach(surface: &wgpu::Surface<'_>, log: Arc<Log>) -> Option<Self> {
            // SAFETY: the layer is only retained and has its class swapped for a subclass of its
            // class without state; wgpu keeps using it as a CAMetalLayer.
            // Under wgpu's lock until the class is swapped, so wgpu doesn't use the layer in between.
            crate::gpu::with_layer(surface, |layer| Self::hook(layer.retain(), log))?
        }

        fn hook(layer: Retained<AnyObject>, log: Arc<Log>) -> Option<Self> {
            let metal = AnyClass::get(c"CAMetalLayer")?;
            // SAFETY: isKindOfClass: takes any class.
            let is_metal: bool = unsafe { msg_send![&*layer, isKindOfClass: metal] };
            if !is_metal { return None; }
            Self::hook_instance(layer, log)
        }

        pub(super) fn hook_instance(layer: Retained<AnyObject>, log: Arc<Log>) -> Option<Self> {
            // SAFETY: `layer` is a live ObjC object; never place a subclass above KVO's.
            if unsafe { crate::platform::macos::kvo_observed(&layer) } { return None; }
            let (base, class) = if crate::platform::macos::has_subclass(&layer, HOOKED_PREFIX.as_bytes()) {
                let top = layer.class();
                (original_class(top), top)
            } else {
                let base = layer.class();
                let class = hooked_class(base)?;
                (base, class)
            };
            LAYERS.lock().unwrap().push(Hooked { addr: Retained::as_ptr(&layer) as usize, base, log });
            if !std::ptr::eq(layer.class(), class) {
                // SAFETY: the subclass adds no ivars, so the instance layout is unchanged.
                unsafe { AnyObject::set_class(&layer, class) };
            }
            Some(LayerHook { layer, base, class })
        }
    }

    impl Drop for LayerHook {
        fn drop(&mut self) {
            // Leave a class someone else swapped in since (key-value observing) alone.
            if std::ptr::eq(self.layer.class(), self.class) {
                // SAFETY: restores the original class; layouts are identical.
                unsafe { AnyObject::set_class(&self.layer, self.base) };
            }
            let addr = Retained::as_ptr(&self.layer) as usize;
            LAYERS.lock().unwrap().retain(|h| h.addr != addr);
        }
    }

    #[cfg(test)]
    #[test]
    fn host_time_maps_onto_instants() {
        let now = crate::time::now();
        assert_eq!(instant_at(now, 100.0, 100.008), now + Duration::from_millis(8));
        assert_eq!(instant_at(now, 100.0, 99.992), now - Duration::from_millis(8));
    }
}

#[cfg(test)]
#[path = "present_time_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "present_time_walk_tests.rs"]
mod walk_tests;
