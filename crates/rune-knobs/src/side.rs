//! ⌘⇧K as a side panel down the window's right edge: it slides in on [`Knobs::slide`] and pushes the
//! page over, its left edge drags to resize it, and its groups are listed in one scroll (see
//! [`crate::tree`] for what's listed and folded, [`crate::side_list`] for how). The group last
//! used is the panel's: its name heads the panel, over its presets, and Random and Reset act
//! on it. Under the list, a line says what the hovered or keyboard's knob does.

use std::collections::BTreeSet;

use rune::{Cursor, Drag, El, Phase, ScrollSpec, Spring, SpringPair, TextStyle, Transition, Tx, col, edit_access, el, row, spacer, text, text_input};

use crate::rows::{Ctx, LABEL, PAD, VALUE, presets};
use crate::side_list::{list, tabs};
use crate::tree::{Did, Node, Plan, plan};
use crate::view::{head, overlay};
use crate::{Group, Knob, Knobs, SIDE_MAX, SIDE_MIN, Style};

/// The panel's inset from its edges.
pub(crate) const INSET: f32 = 12.0;
const ABOUT: TextStyle = TextStyle::new(12.0, 450.0).lh(16.0);
const ABOUT_TITLE: TextStyle = TextStyle::new(12.0, 600.0).lh(16.0);

pub(crate) fn section_key(i: usize) -> String {
    format!("knobs-sec-{i}")
}

/// Where the keyboard's row is, to scroll it into view.
fn node_key(n: Node, groups: &[Group]) -> String {
    match n {
        Node::Page(p) => format!("knobs-page-{}", groups.iter().position(|g| g.page() == p).unwrap_or(0)),
        Node::Group(g) => format!("{}/head", section_key(g)),
        Node::Knob(g, i) => format!("{}/body/knobs-card/knobs-knob-{i}", section_key(g)),
    }
}

/// `page` with the knobs beside it: put it where the page would go, filling the window. Read
/// [`Knobs::side_room`] for the width the page loses while it's open.
pub fn side<S: 'static>(page: El, k: &Knobs, groups: &'static [Group], st: Style, lens: fn(&mut S) -> &mut Knobs) -> El {
    let w = k.side_w();
    let host = col().key("knobs-page").grow(1.0).shrink().min_w(0.0).h_full().min_h(0.0).child(page).child(overlay(k, groups, st, lens, "knobs-over"));
    // The panel keeps its width and is anchored to the box's left edge, so as the box opens the
    // panel slides in from the right with it; closed, it waits off the window's edge.
    // Shut, the panel isn't built: it was a few hundred elements every commit of every app.
    // Its removal holds it through the slide out.
    let slide = k.slide();
    let panel = k.open.then(|| {
        let out = Transition::identity().hold_layout().springs(SpringPair::new(slide, Spring::new(slide.duration * 1.4, 0.0)));
        panel(k, groups, st, lens, w).transition(out)
    });
    let edge = el()
        .key("knobs-edge")
        .absolute(0.0, 0.0)
        .w(6.0)
        .h_full()
        .cursor(Cursor::ResizeColumn)
        .help("Drag to resize; double-click for the default width")
        .on_drag(move |s: &mut S, g: &mut Drag, tx: &mut Tx| {
            tx.instant();
            let k = lens(s);
            match g.phase {
                Phase::Down => g.data = k.side_w(),
                Phase::Move => k.width = (g.data - g.dx).clamp(SIDE_MIN, SIDE_MAX).round(),
                Phase::Up if g.cancelled() => k.width = g.data,
                Phase::Up => {}
            }
        })
        .on_double_press(move |s: &mut S| lens(s).width = 0.0);
    let mut bx = el().key("knobs-side").w(if k.open { w } else { 0.0 }).h_full().clip_soft(0.0).motion(slide).children(panel);
    if k.open {
        bx = bx.child(edge);
    }
    row().key("knobs-frame").w_full().h_full().min_h(0.0).child(host).child(bx)
}

/// The groups whose part of the window shows now: their region is laid out and overlaps the
/// page beside the panel.
fn seen(k: &Knobs, groups: &[Group]) -> BTreeSet<usize> {
    if !k.open {
        return BTreeSet::new();
    }
    let Some(page) = rune::memo::window_layout("knobs-page") else { return BTreeSet::new() };
    let overlaps = |r: [f32; 4]| r[2] > 0.0 && r[3] > 0.0 && r[0] < page[0] + page[2] && r[0] + r[2] > page[0] && r[1] < page[1] + page[3] && r[1] + r[3] > page[1];
    (0..groups.len()).filter(|&g| !groups[g].region.is_empty() && rune::memo::window_layout(groups[g].region).is_some_and(overlaps)).collect()
}

fn panel<S: 'static>(k: &Knobs, groups: &'static [Group], st: Style, lens: fn(&mut S) -> &mut Knobs, w: f32) -> El {
    let g = k.current(groups);
    let title = match g {
        Some(g) => g.name,
        None => "Pick a part",
    };
    let cx = Ctx { k, st, lens, w: w - 2.0 * INSET };
    let plan = plan(k, groups, seen(k, groups));
    let mut top = col().key("knobs-top").w_full().px(INSET).pt(INSET).pb(8.0).gap(PAD).child(head(k, groups, st, lens, g, title, true).pl(PAD));
    top = match g {
        Some(g) => top.child(presets(cx, &g)),
        None => top.child(text("Click a part of the window to tune it.").key("knobs-hint").font(LABEL).color(st.fg3).px(PAD).h(28.0)),
    };
    top = top.child(filter(k, groups, st, lens, &plan));
    if !plan.tabs.is_empty() && k.query.is_empty() {
        top = top.child(tabs(st, lens, &plan));
    }
    let scroll = col().key("knobs-scroll").w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).scroll_edge_fade(24.0).child(list(cx, groups, &plan));
    let keys = plan.clone();
    col()
        .key("knobs-panel")
        .absolute(0.0, 0.0)
        .w(w)
        .h_full()
        .min_h(0.0)
        .fill(st.bg)
        .focusable(true)
        .no_focus_ring()
        .on_key(move |s: &mut S, ev, tx: &mut Tx| {
            let k = lens(s);
            if !k.open {
                return false;
            }
            let did = k.tree_key(groups, &keys, ev);
            took(k, did, groups, tx)
        })
        // The pointer moving takes the highlight from the keyboard's row, so only one row is lit.
        .on_pointer_move_local(move |s: &mut S, _, tx: &mut Tx| {
            let k = lens(s);
            if k.kbd && k.drag.is_none() {
                k.kbd = false;
                tx.instant();
            } else {
                tx.live_only();
            }
        })
        .child(el().key("knobs-rim").absolute(0.0, 0.0).w(0.5).h_full().fill(st.border))
        .child(top)
        .child(scroll)
        .child(about(k, groups, st))
}

/// What a panel key did, shown: the motion it asks for and the keyboard's row in view. True if
/// the key was taken.
fn took(k: &mut Knobs, did: Did, groups: &[Group], tx: &mut Tx) -> bool {
    match did {
        Did::Not => return false,
        Did::Now if k.open => tx.instant(),
        Did::Now => tx.motion(k.slide()),
        Did::Fold => tx.motion(k.fold_spring()),
    }
    if let Some(n) = k.at() {
        tx.scroll_into_view(node_key(n, groups));
    }
    true
}

/// How to start typing, or the field: a single-line text field while it edits, with the
/// keys the panel keeps (↑ ↓ Tab, Return, Esc) handled before it, and the matches counted on
/// the right.
fn filter<S: 'static>(k: &Knobs, groups: &'static [Group], st: Style, lens: fn(&mut S) -> &mut Knobs, plan: &Plan) -> El {
    let typed = !k.query.is_empty();
    let mut r = row().key("knobs-filter").w_full().h(28.0).px(PAD).gap(8.0).center_y().radius(7.0).fill(st.inset);
    if k.filtering {
        let keys = plan.clone();
        let mut edit = k.edit().clone();
        if edit.text != k.query {
            edit.replace_text(k.query.clone());
        }
        let field = text_input(&edit, LABEL)
            .key("knobs-filter-input")
            .single_line()
            .color(st.fg)
            .caret(st.fg2, st.fg.alpha(0.22))
            .grow(1.0)
            .min_w(0.0)
            .on_edit(edit_access(move |s: &mut S| lens(s).edit_mut()))
            .on_input_key(move |s: &mut S, ev, tx: &mut Tx| {
                let k = lens(s);
                k.sync_edit();
                let did = k.filter_key(groups, &keys, ev);
                took(k, did, groups, tx)
            });
        let keys = plan.clone();
        let field = field.on_input_change(move |s: &mut S, tx: &mut Tx| {
            tx.instant();
            let k = lens(s);
            k.edited();
            k.kbd = true;
            k.refilter(groups, &keys);
        });
        // Taking the field out gives focus back to the panel.
        r = r.child(el().key("knobs-filter-field").grow(1.0).min_w(0.0).h_full().center_y().focus_return("knobs-panel").child(field));
    } else {
        r = r
            .on_press_tx(move |s: &mut S, tx: &mut Tx| {
                tx.instant();
                let k = lens(s);
                k.kbd = true;
                k.start_filter();
            })
            .child(text(if typed { k.query.clone() } else { "Press / to filter".to_owned() }).font(LABEL).color(if typed { st.fg } else { st.fg3 }).truncate_fade().shrink());
    }
    if typed {
        let n: usize = plan.sections.iter().flat_map(|s| &s.subs).map(|s| s.knobs.len()).sum();
        if !k.filtering {
            r = r.child(spacer());
        }
        r = r.child(text(if n == 1 { "1 knob".to_owned() } else { format!("{n} knobs") }).font(VALUE).color(st.fg3));
    }
    r
}

/// The knob the pointer is over, or else the keyboard's.
fn described<'a>(k: &Knobs, groups: &'a [Group]) -> Option<&'a Knob> {
    if let Some(h) = k.hot.as_deref() {
        return groups.iter().flat_map(|g| g.knobs).find(|d| d.name == h);
    }
    match k.at() {
        Some(Node::Knob(g, i)) if k.kbd => groups.get(g).and_then(|g| g.knobs.get(i)),
        _ => None,
    }
}

/// The line under the list: what the knob in hand does, and the option under the pointer
/// (or the one picked), or the keys.
fn about(k: &Knobs, groups: &[Group], st: Style) -> El {
    let c = col().key("knobs-about").w_full().h(64.0).px(INSET + PAD).pt(8.0).gap(2.0);
    match described(k, groups) {
        Some(d) => {
            let c = c.child(text(d.title()).font(ABOUT_TITLE).color(st.fg2).truncate_fade());
            let c = if d.describe().is_empty() { c } else { c.child(text(d.describe()).key("knobs-about-line").font(ABOUT).color(st.fg3).wrap()) };
            let opt = match &k.hot_opt {
                Some((n, j)) if *n == d.name => Some(*j),
                _ if !d.labels.is_empty() => Some(k.get(d).round() as usize),
                _ => None,
            };
            match opt.and_then(|j| Some((d.labels.get(j)?, d.hint(j)))) {
                Some((label, hint)) if !hint.is_empty() => c.child(text(format!("{label}: {hint}")).key("knobs-about-option").font(ABOUT).color(st.fg2).truncate_fade()),
                _ => c,
            }
        }
        None => c.child(text("↑↓ move   ←→ fold, open or change   1–9 pick   / filter").font(ABOUT).color(st.fg3).wrap()),
    }
}

#[cfg(test)]
#[path = "side_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "filter_tests.rs"]
mod filter_tests;
