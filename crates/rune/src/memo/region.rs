//! [`region`] and [`region_with`]: subtrees keyed by their [`Tracked`](crate::Tracked) and `Cx`
//! reads, with debug stale-region checking.

#[cfg(debug_assertions)]
use std::cell::RefCell;
use std::hash::Hash;

use crate::El;

use super::{hash_of, memo_inner};

/// The version every [`region`] passes: a region's reads are its version.
const REGION: u64 = 0x7265_6769_6f6e_0001;

/// The subtree `build` returns, keyed `id`, built again only when something it read changed:
/// a [`Tracked`](crate::Tracked) value it read was written, a `Cx` layout or caret read it made
/// moved, or the window's environment changed. Otherwise `build` isn't called and the previous
/// subtree is reused whole, as a [`memo`](super::memo)'s is, with the same node ids and state.
///
/// A region must read the state it shows through `Tracked` values: anything else it reads (a
/// plain field, a global, a `Cell`) can change without it noticing. Debug builds build every
/// reused region again and print the first element that differs, naming the region.
#[inline]
#[cfg_attr(debug_assertions, track_caller)]
pub fn region(id: impl Into<String>, build: impl FnOnce() -> El) -> El {
    region_with(id, (), build)
}

/// [`region`] that also rebuilds when `inputs` change: values the closure uses that aren't
/// [`Tracked`](crate::Tracked) or read inside it, such as a palette or a size computed by the
/// caller. Debug builds check its reuse as `region`'s.
#[inline]
#[cfg_attr(debug_assertions, track_caller)]
pub fn region_with(id: impl Into<String>, inputs: impl Hash, build: impl FnOnce() -> El) -> El {
    let mut build = Some(build);
    region_inner(id.into(), hash_of(&(REGION, inputs)), &mut || build.take().expect("a region builds once")())
}

#[cfg_attr(debug_assertions, track_caller)]
fn region_inner(key: String, version: u64, build: &mut dyn FnMut() -> El) -> El {
    #[cfg(not(debug_assertions))]
    {
        memo_inner(key, version, true, build)
    }
    #[cfg(debug_assertions)]
    {
        let check = key.clone();
        let mut called = false;
        let el = memo_inner(key, version, true, &mut || {
            called = true;
            build()
        });
        // Reused: debug builds build it again to show it was right to.
        if !called {
            let fresh = build();
            let (mut a, mut b) = (Vec::new(), Vec::new());
            print_el(&el, "", &mut a);
            print_el(&fresh, "", &mut b);
            if let Some(i) = (0..a.len().max(b.len())).find(|&i| a.get(i) != b.get(i)) {
                REGION_WARNINGS.with_borrow_mut(|w| {
                    if w.iter().any(|x: &String| x.starts_with(&format!("rune: region {check:?} "))) { return; }
                    let warning = format!(
                        "rune: region {check:?} was reused, but building it again gives something else at element {i}: \
                         reused {:?}, built {:?}. It reads state that isn't Tracked.",
                        a.get(i).map_or("(none)", String::as_str), b.get(i).map_or("(none)", String::as_str));
                    eprintln!("{warning}");
                    w.push(warning);
                });
            }
        }
        el
    }
}

#[cfg(debug_assertions)]
thread_local! {
    static REGION_WARNINGS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// The stale-region warnings debug builds printed (`region`); always empty in release builds.
pub fn region_warnings() -> Vec<String> {
    #[cfg(debug_assertions)]
    { REGION_WARNINGS.with_borrow(Clone::clone) }
    #[cfg(not(debug_assertions))]
    { Vec::new() }
}

/// One line per element of `el`'s subtree, in order: its place, role, style, visuals and text,
/// without its handlers. What a region's debug check compares.
#[cfg(debug_assertions)]
fn print_el(el: &El, parent: &str, out: &mut Vec<String>) {
    let path = format!("{parent}/{}", el.key.as_deref().unwrap_or("#"));
    let text = el.text.as_ref().map(|t| {
        let content = if t.redacted { "[redacted]" } else { t.content.as_str() };
        format!("{content:?} {:?} {}/{} {} spans", t.color, t.style.size, t.style.weight, t.spans.len())
    });
    out.push(format!("{path} {} {:?} {:?} {:?}", el.role_name(), el.style, el.vis, text));
    for child in el.all_children() {
        print_el(child, &path, out);
    }
}
