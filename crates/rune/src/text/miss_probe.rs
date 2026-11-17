//! TEXT-2 measuring probe (not for `dev`): with `RUNE_TEXT2_MISS=1`, each layout-cache miss, re-break
//! and eviction goes to stderr with what keyed it, and each frame that shaped or rasterised anything
//! prints its split: shaping, re-breaking, glyph rasterising, outline building, and the frame's
//! update, upload/prepare, encode, submit and present times.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::time::Instant;

thread_local! {
    static SEEN: RefCell<(HashSet<u64>, HashSet<u64>)> = RefCell::new((HashSet::new(), HashSet::new()));
    static NS: Cell<[u64; 4]> = const { Cell::new([0; 4]) };
    static COUNT: Cell<[u32; 5]> = const { Cell::new([0; 5]) };
}

#[derive(Clone, Copy)]
pub(crate) enum Slot { Shape = 0, Rebreak = 1, Raster = 2, Outline = 3 }

/// Counted things: shaped texts, re-broken texts, glyph misses, GPU fills queued, outlines built.
#[derive(Clone, Copy)]
pub(crate) enum Count { Shaped = 0, Rebroken = 1, Glyphs = 2, GpuFills = 3, Outlines = 4 }

pub(crate) fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("RUNE_TEXT2_MISS").is_ok_and(|v| v == "1" || v == "2"))
}

pub(crate) fn count(c: Count) {
    if on() { COUNT.with(|x| { let mut v = x.get(); v[c as usize] += 1; x.set(v) }); }
}

pub(crate) struct Timer(Slot, Option<Instant>);
impl Timer {
    pub(crate) fn new(slot: Slot, when: bool) -> Timer { Timer(slot, (when && on()).then(Instant::now)) }
}
impl Drop for Timer {
    fn drop(&mut self) {
        if let Some(t) = self.1 { NS.with(|x| { let mut v = x.get(); v[self.0 as usize] += t.elapsed().as_nanos() as u64; x.set(v) }); }
    }
}

fn ms() -> f64 {
    static T0: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    T0.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

/// One frame's line, from the window's stamps (µs): update, upload and prepare, encode, submit,
/// present. Printed when the frame shaped or rasterised anything.
pub(crate) fn frame(update: f64, prepare: f64, encode: f64, submit: f64, present: f64) {
    if !on() { return; }
    let ns = NS.with(|x| x.replace([0; 4]));
    let c = COUNT.with(|x| x.replace([0; 5]));
    if c[0] + c[1] + c[2] + c[3] + c[4] == 0 { return; }
    let us = |v: u64| v as f64 / 1000.0;
    eprintln!("TEXT2F t={:.1} update_us={update:.0} prepare_us={prepare:.0} encode_us={encode:.0} submit_us={submit:.0} present_us={present:.0} shaped={} shape_us={:.0} rebroken={} rebreak_us={:.0} glyphs={} raster_us={:.0} outlines={} outline_us={:.0} gpu_fills={}",
        ms(), c[0], us(ns[0]), c[1], us(ns[1]), c[2], us(ns[2]), c[4], us(ns[3]), c[3]);
}

fn lines() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("RUNE_TEXT2_MISS").is_ok_and(|v| v == "1"))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn shaped(kind: &str, g: u32, shape: u64, key: u64, scale: f32, w: Option<f32>, style: &crate::tok::TextStyle, text: &str, layouts: usize) {
    if !lines() { return; }
    let (shape_seen, key_seen) = SEEN.with_borrow_mut(|(s, k)| (!s.insert(shape), !k.insert(key)));
    let head: String = text.chars().take(28).map(|c| if c.is_control() { ' ' } else { c }).collect();
    eprintln!("TEXT2 {kind} t={:.1} g={g} shape={shape:016x} seen={} key={key:016x} kseen={} scale={scale} w={w:?} size={} wt={:?} fam={:?} n={layouts} len={} {head:?}",
        ms(), shape_seen as u8, key_seen as u8, style.size, style.weight, style.family, text.len());
}

pub(crate) fn event(what: &str, g: u32, before: usize, after: usize) {
    if lines() && before != after { eprintln!("TEXT2 {what} t={:.1} g={g} layouts {before}->{after}", ms()); }
}

/// A free-form probe line (`RUNE_TEXT2_MISS` set).
pub(crate) fn note(line: std::fmt::Arguments<'_>) {
    if on() { eprintln!("TEXT2N t={:.1} {line}", ms()); }
}
