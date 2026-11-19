//! Debug-only layout warnings for mistakes that lay out without error but look broken.
use std::collections::BTreeSet;
use std::sync::Mutex;

use rune_taffy::prelude::*;

use super::Node;

/// A scroll viewport that resolved to 0 pt inside a `.grow()` container while its content has
/// height: `.grow()` sets flex-basis 0, so with no bounded ancestor there is nothing to grow
/// into. Warns once per `.scroll(..)` call site.
static WARNED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

#[cfg(test)]
thread_local! {
    static SCOPED_WARNED: std::cell::RefCell<Option<BTreeSet<String>>> = const { std::cell::RefCell::new(None) };
}

fn mark_warned(key: String) -> bool {
    #[cfg(test)]
    if let Some(inserted) = SCOPED_WARNED.with(|slot| slot.borrow_mut().as_mut().map(|set| set.insert(key.clone()))) {
        return inserted;
    }
    WARNED.lock().unwrap().insert(key)
}

pub(in crate::runtime) fn collapsed_scrolls(taffy: &LayoutEngine, nodes: &[Node]) {
    let height = |i: usize| taffy.box_layout(nodes[i].taffy).h;
    let bottom = |i: usize| {
        let l = taffy.box_layout(nodes[i].taffy);
        l.y + l.h
    };
    let grown = |i: usize| {
        let s = taffy.style(nodes[i].taffy);
        s.flex_grow() > 0.0 && s.flex_basis() == length(0.0)
    };
    for (i, n) in nodes.iter().enumerate() {
        let s = taffy.style(n.taffy);
        if s.whole_rows() && !s.whole_rows_column() && mark_warned(format!("whole-rows-dir:{}", n.path)) {
            eprintln!(
                "rune: El::whole_rows on {:?} is ignored because its container is not a \
                 non-wrapping flex column (Display::Flex, FlexDirection::Column, FlexWrap::NoWrap). (debug builds only)",
                n.path,
            );
        }
        if n.scroll.is_none() { continue; }
        let direct = || (i + 1..nodes.len()).take_while(|&j| nodes[j].parent >= Some(i)).filter(|&j| nodes[j].parent == Some(i));
        if n.growth.is_some() {
            let site = n.scroll_source.map_or_else(|| n.path.clone(), |s| s.to_string());
            if mark_warned(format!("growth:{site}")) {
                eprintln!(
                    "rune: scroll viewport {:?} (.scroll at {site}) also sets .growth(..); \
                     put .growth(..) on a child column inside .scroll(..) so the viewport box \
                     does not take the stack's presented height. (debug builds only)",
                    n.path,
                );
            }
        }
        let mut kids = direct();
        if let Some(g) = kids.find(|&j| nodes[j].growth.is_some())
            && kids.next().is_some()
        {
            let site = n.scroll_source.map_or_else(|| n.path.clone(), |s| s.to_string());
            if mark_warned(format!("growth-sibling:{site}")) {
                eprintln!(
                    "rune: growth stack {:?} inside scroll viewport {:?} (.scroll at {site}) \
                     has sibling elements after it; a growth stack must be the last (or only) \
                     child of its scroll viewport so nothing below it overlaps its growing rows. (debug builds only)",
                    nodes[g].path, n.path,
                );
            }
        }
        if !n.scroll_axes.y() || height(i) >= 0.5 { continue; }
        let content = direct().map(bottom).fold(0.0_f32, f32::max);
        if content < 0.5 { continue; }
        let mut up = Some(i);
        let mut collapsed = false;
        while let Some(j) = up {
            if grown(j) && height(j) < 0.5 { collapsed = true; break; }
            up = nodes[j].parent;
        }
        if !collapsed { continue; }
        let site = n.scroll_source.map_or_else(|| n.path.clone(), |s| s.to_string());
        if !mark_warned(site.clone()) { continue; }
        eprintln!(
            "rune: scroll viewport {:?} (.scroll at {site}) is 0 pt tall inside a .grow() container \
             while its content is {content} pt. .grow() sets flex-basis 0, so it only fills space a \
             bounded parent leaves: bound an ancestor's height (.h(..), .h_full()) or set .min_h(..), \
             and put .min_h(0.0) on nested flex columns. (debug builds only)",
            n.path,
        );
    }
}

/// Isolates layout warn-once tracking on the current thread for the duration of a test so
/// warnings emitted by earlier or concurrent tests do not leak into assertions.
#[cfg(test)]
pub(in crate::runtime) struct WarningScope(Option<BTreeSet<String>>);

#[cfg(test)]
impl Drop for WarningScope {
    fn drop(&mut self) {
        SCOPED_WARNED.with(|slot| *slot.borrow_mut() = self.0.take());
    }
}

#[cfg(test)]
pub(in crate::runtime) fn scope() -> WarningScope {
    WarningScope(SCOPED_WARNED.with(|slot| slot.borrow_mut().replace(BTreeSet::new())))
}

#[cfg(test)]
pub(in crate::runtime) fn warned_at(needle: &str) -> bool {
    if let Some(found) = SCOPED_WARNED.with(|slot| slot.borrow().as_ref().map(|set| set.iter().any(|site| site.contains(needle)))) {
        return found;
    }
    WARNED.lock().unwrap().iter().any(|site| site.contains(needle))
}

#[cfg(test)]
mod tests {
    use crate::{Env, GrowthSpec, Runtime, ScrollSpec, col, el};

    fn view(bounded: &bool, _: &mut crate::Cx<bool>) -> crate::El {
        let list = col().key("list").scroll(ScrollSpec { stick_bottom: false }).grow(1.0)
            .child(col().key("rows").child(el().h(400.0)));
        let page = col().key("page").child(list);
        col().child(if *bounded { page.h_full() } else { page })
    }

    #[test]
    fn a_grown_scroll_in_an_auto_height_column_warns_and_keeps_its_layout() {
        let _warnings = super::scope();
        for bounded in [true, false] {
            let mut rt = Runtime::new(bounded, view, (200.0, 200.0), Env::new(true, 1.0));
            rt.frame(0.0);
            let list = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("list")).unwrap();
            assert_eq!(list.rect[3], if bounded { 200.0 } else { 0.0 }, "layout is unchanged");
            assert_eq!(super::warned_at("layout_diagnostics.rs"), !bounded);
        }
    }

    #[test]
    fn warning_scope_isolates_tests_from_prior_process_warnings() {
        fn bad_growth_sibling(_: &(), _: &mut crate::Cx<()>) -> crate::El {
            col().key("scroll").size(200.0, 200.0).scroll(ScrollSpec { stick_bottom: false })
                .child(col().key("stack").w_full().growth(GrowthSpec::default()).child(el().h(40.0)))
                .child(el().key("after").w_full().h(40.0))
        }
        // Pollute the process-global set outside any test scope.
        let mut prior = Runtime::new((), bad_growth_sibling, (200.0, 200.0), Env::new(true, 1.0));
        prior.frame(0.0);
        assert!(super::warned_at("growth-sibling:"));

        {
            let _warnings = super::scope();
            assert!(!super::warned_at("growth-sibling:"));
            let mut rt = Runtime::new((), bad_growth_sibling, (200.0, 200.0), Env::new(true, 1.0));
            rt.frame(0.0);
            assert!(super::warned_at("growth-sibling:"));
        }
        {
            let _warnings = super::scope();
            assert!(!super::warned_at("growth-sibling:"));
        }
    }
}
