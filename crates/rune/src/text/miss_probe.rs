//! TEXT-2 measuring probe (not for `dev`): with `RUNE_TEXT2_MISS=1`, each layout-cache miss, re-break
//! and eviction goes to stderr with what keyed it.

use std::cell::RefCell;
use std::collections::HashSet;

thread_local! {
    static SEEN: RefCell<(HashSet<u64>, HashSet<u64>)> = RefCell::new((HashSet::new(), HashSet::new()));
}

pub(crate) fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("RUNE_TEXT2_MISS").is_ok_and(|v| v == "1"))
}

fn ms() -> f64 {
    static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    T0.get_or_init(std::time::Instant::now).elapsed().as_secs_f64() * 1000.0
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn shaped(kind: &str, g: u32, shape: u64, key: u64, scale: f32, w: Option<f32>, style: &crate::tok::TextStyle, text: &str, layouts: usize) {
    if !on() { return; }
    let (shape_seen, key_seen) = SEEN.with_borrow_mut(|(s, k)| (!s.insert(shape), !k.insert(key)));
    let head: String = text.chars().take(28).map(|c| if c.is_control() { ' ' } else { c }).collect();
    eprintln!("TEXT2 {kind} t={:.1} g={g} shape={shape:016x} seen={} key={key:016x} kseen={} scale={scale} w={w:?} size={} wt={:?} fam={:?} n={layouts} len={} {head:?}",
        ms(), shape_seen as u8, key_seen as u8, style.size, style.weight, style.family, text.len());
}

pub(crate) fn event(what: &str, g: u32, before: usize, after: usize) {
    if on() && before != after { eprintln!("TEXT2 {what} t={:.1} g={g} layouts {before}->{after}", ms()); }
}
