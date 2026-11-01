//! Built scenes, kept by source, style and hover, so a diagram rebuilt with nothing new costs a
//! lookup, and a diagram streaming in builds each line held to the one before (see [`steady`]).

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use rune::Color;

use crate::canvas::Scene;
use crate::{DiagramStyle, Hover, scene, steady};

const CACHE_LEN: usize = 16;

#[derive(Default)]
pub(crate) struct Cache {
    map: HashMap<u64, Built>,
    order: VecDeque<u64>,
}

thread_local! {
    pub(crate) static SCENES: RefCell<Cache> = RefCell::new(Cache::default());
}

/// A scene and, for one built growing, what its layouts leave for the next build to hold to.
pub(crate) type Built = (Option<Rc<Scene>>, Option<Rc<steady::Memory>>);

/// `src`'s scene, from the cache or built and kept; held to `prev` when it is `Some`.
pub(crate) fn built(src: &str, style: &DiagramStyle, hover: Option<(Hover, Color)>, prev: Option<Option<Rc<steady::Memory>>>) -> Built {
    let key = scene_key(src, style, hover, prev.is_some());
    if let Some(b) = SCENES.with_borrow(|c| c.map.get(&key).cloned()) {
        return b;
    }
    let b = build(src, style, hover, prev);
    SCENES.with_borrow_mut(|c| {
        if c.order.len() >= CACHE_LEN
            && let Some(old) = c.order.pop_front()
        {
            c.map.remove(&old);
        }
        c.order.push_back(key);
        c.map.insert(key, b.clone());
    });
    b
}

fn build(src: &str, style: &DiagramStyle, hover: Option<(Hover, Color)>, prev: Option<Option<Rc<steady::Memory>>>) -> Built {
    // Defensive: a bug in layout must cost a diagram, not the app.
    let safe = || std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| scene(src, style, hover))).ok().flatten();
    match prev {
        None => (safe().map(Rc::new), None),
        Some(prev) => {
            let held = prev.as_ref().map_or([0.0; 2], |m| m.size);
            let (s, mut memory) = steady::build(prev, safe);
            // Nor does the card shrink, even as the diagram shrinks to fit a wider layout.
            let s = s.map(|mut s| {
                s.size = [s.size[0].max(held[0]), s.size[1].max(held[1])];
                s
            });
            memory.size = s.as_ref().map_or(held, |s| s.size);
            (s.map(Rc::new), Some(Rc::new(memory)))
        }
    }
}

fn scene_key(src: &str, style: &DiagramStyle, hover: Option<(Hover, Color)>, growing: bool) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    src.hash(&mut h);
    style.hash(&mut h);
    hover.hash(&mut h);
    growing.hash(&mut h);
    // Layout measures labels in the app's fonts.
    rune::text::font_epoch().hash(&mut h);
    h.finish()
}

/// How many lines back a growing build looks for one it can hold to before it starts afresh.
const MAX_HELD_LINES: usize = 64;

/// `src` laid out held to the same source a line shorter, laid out held to the one a line
/// shorter again, and so on, so it lays out the same however many lines each build added.
pub(crate) fn growing(src: &str, style: &DiagramStyle, hover: Option<(Hover, Color)>) -> Option<Rc<Scene>> {
    let mut chain = vec![src.trim_end()];
    let mut prev = None;
    if let Some(b) = SCENES.with_borrow(|c| c.map.get(&scene_key(chain[0], style, hover, true)).cloned()) {
        return b.0;
    }
    while let Some(shorter) = chain.last().and_then(|s| s.rfind('\n').map(|i| s[..i].trim_end())) {
        if let Some(b) = SCENES.with_borrow(|c| c.map.get(&scene_key(shorter, style, hover, true)).cloned()) {
            prev = b.1;
            break;
        }
        if chain.len() >= MAX_HELD_LINES {
            break;
        }
        chain.push(shorter);
    }
    // Only the source asked for is kept: the shorter ones were built for it alone.
    let last = chain.remove(0);
    for s in chain.into_iter().rev() {
        prev = build(s, style, hover, Some(prev)).1;
    }
    built(last, style, hover, Some(prev)).0
}
