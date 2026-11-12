//! Launch and quit marks: when the process reached each step Rune takes on the way to a
//! window's first frame and out of the last one, for an app that writes a launch trace or
//! guards its launch time. Each name keeps the first time it was reached: the event loop
//! starting, the first window made, the GPU's adapter, device and surface, the renderer's
//! pipelines, the text system's fonts, the first frame built, encoded, presented and shown, then
//! the quit starting, its hooks, the windows closing and hiding, the GPU going with them and the
//! loop ending. Beside them, a few totals for everything before the first frame presents:
//! glyphs rasterised and the time it took ([`metrics`]), and which uber shader was built
//! ([`notes`]).
//!
//! Recording costs a lock the first time a name is reached and nothing after; the totals stop
//! once the first frame presents.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use crate::time::Instant;

#[derive(Default)]
struct Book {
    marks: Vec<(&'static str, Instant)>,
    metrics: Vec<(&'static str, u64)>,
    notes: Vec<(&'static str, &'static str)>,
}

static BOOK: Mutex<Book> = Mutex::new(Book { marks: Vec::new(), metrics: Vec::new(), notes: Vec::new() });
/// Totals stop at the first presented frame.
static TOTALS_DONE: AtomicBool = AtomicBool::new(false);

fn book() -> std::sync::MutexGuard<'static, Book> {
    BOOK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Records `name` at now, unless it was reached before. An app marks its own steps here too.
pub fn mark(name: &'static str) {
    let now = Instant::now();
    let mut book = book();
    if !book.marks.iter().any(|(n, _)| *n == name) {
        book.marks.push((name, now));
        // A page's marks are the browser's too: its performance timeline and DevTools show them.
        #[cfg(target_arch = "wasm32")]
        page_mark(name);
    }
}

#[cfg(target_arch = "wasm32")]
fn page_mark(name: &str) {
    use wasm_bindgen::JsCast;
    let Ok(performance) = js_sys::Reflect::get(&js_sys::global(), &"performance".into()) else { return };
    let Ok(mark) = js_sys::Reflect::get(&performance, &"mark".into()) else { return };
    if let Some(mark) = mark.dyn_ref::<js_sys::Function>() { let _ = mark.call1(&performance, &name.into()); }
}

/// [`mark`] behind a flag of the call site's own, for a step that runs every frame.
pub(crate) fn mark_once(flag: &AtomicBool, name: &'static str) {
    if !flag.swap(true, Ordering::Relaxed) {
        mark(name);
    }
}

/// Every mark so far, in the order they were first reached.
pub fn marks() -> Vec<(&'static str, Instant)> {
    book().marks.clone()
}

/// Totals for the steps before the first presented frame: `text.glyphs` (glyphs rasterised or
/// queued for the GPU's fill), `text.gpu_fills` (of those, queued for the GPU's fill),
/// `text.raster_ns` (the UI thread's time rasterising them, in nanoseconds),
/// `text.outline_ns` (building the outline records Windfoil's masks are filled from),
/// `text.shaped` (texts shaped and broken into lines: layout cache misses) and `text.shape_ns`
/// (the UI thread's time doing it).
pub fn metrics() -> Vec<(&'static str, u64)> {
    book().metrics.clone()
}

/// What Rune chose on the way: `uber` names the uber shader the first renderer drew with.
pub fn notes() -> Vec<(&'static str, &'static str)> {
    book().notes.clone()
}

/// Whether totals are still being kept (the first frame hasn't presented).
pub(crate) fn counting() -> bool {
    !TOTALS_DONE.load(Ordering::Relaxed)
}

/// Adds `amount` to total `name` while totals are kept.
pub(crate) fn add(name: &'static str, amount: u64) {
    if !counting() {
        return;
    }
    let mut book = book();
    match book.metrics.iter_mut().find(|(n, _)| *n == name) {
        Some((_, total)) => *total += amount,
        None => book.metrics.push((name, amount)),
    }
}

/// Adds the time since `started` to total `name` (in nanoseconds) when it drops.
pub(crate) struct Timed(Option<(&'static str, Instant)>);

impl Timed {
    /// Times until it drops, while totals are kept and `when` holds.
    pub(crate) fn start(name: &'static str, when: bool) -> Timed {
        Timed((when && counting()).then(|| (name, Instant::now())))
    }
}

impl Drop for Timed {
    fn drop(&mut self) {
        if let Some((name, started)) = self.0 {
            add(name, u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX));
        }
    }
}

/// Keeps `value` for `key`, the first one given.
pub(crate) fn note(key: &'static str, value: &'static str) {
    let mut book = book();
    if !book.notes.iter().any(|(k, _)| *k == key) {
        book.notes.push((key, value));
    }
}

/// A glyph this frame draws found no room in the atlas, so the frame lacks it.
pub(crate) fn text_missed() {
    MISSED.store(true, Ordering::Relaxed);
}

static MISSED: AtomicBool = AtomicBool::new(false);
static TEXT_COMPLETE: AtomicBool = AtomicBool::new(false);
static INPUT_READY: AtomicBool = AtomicBool::new(false);

/// A frame presented: the first one marks `frame.first.presented` and stops the totals; the first
/// that drew every glyph it laid out marks `text.complete`.
pub(crate) fn presented() {
    if !TOTALS_DONE.swap(true, Ordering::Relaxed) {
        mark("frame.first.presented");
    }
    if !MISSED.swap(false, Ordering::Relaxed) {
        mark_once(&TEXT_COMPLETE, "text.complete");
    }
}

/// The event loop is about to wait for input: after the first present, `input.ready`.
pub(crate) fn loop_waits() {
    if !counting() {
        mark_once(&INPUT_READY, "input.ready");
    }
}

/// Whether `key` has a note.
pub(crate) fn noted(key: &'static str) -> bool {
    book().notes.iter().any(|(k, _)| *k == key)
}
