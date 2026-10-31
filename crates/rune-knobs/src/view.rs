//! The layer ⌘⇧K opens: region picking over the window, and the panel, drawn to match the knobs
//! library's main look (flat accent-tinted slider fills with a hairline handle, label left and
//! readout right on one row, rows sharing one card, text tabs, a preset tray).

use rune::{Color, El, Icon, TextStyle, Tx, col, el, icon, row, spacer, text};

use crate::rows::{CHIP, Ctx, LABEL, PAD, ROW, TITLE, VALUE, card, presets};
use crate::{Action, Group, Knobs, Style, icons};

const W: f32 = 300.0;
const CARD_W: f32 = W - 2.0 * PAD;
/// Where an inline panel's first frame starts, before its card's laid-out width is known: the
/// playground's side column.
const INLINE_CARD_W: f32 = 220.0;

const TAG: TextStyle = TextStyle::new(11.0, 550.0).lh(14.0);

/// The smallest region under the point.
fn region_at(rects: &[(&'static str, [f32; 4])], x: f32, y: f32) -> Option<&'static str> {
    rects
        .iter()
        .filter(|(_, r)| x >= r[0] && x < r[0] + r[2] && y >= r[1] && y < r[1] + r[3])
        .min_by(|a, b| (a.1[2] * a.1[3]).total_cmp(&(b.1[2] * b.1[3])))
        .map(|(n, _)| *n)
}

/// The whole layer, to add over the app's root (it draws nothing while closed). `lens` finds
/// the app's `Knobs` in its state. Region rects are read in window points, so put it where
/// its top-left is the window's and nothing scales it.
pub fn layer<S: 'static>(k: &Knobs, groups: &'static [Group], st: Style, lens: fn(&mut S) -> &mut Knobs) -> El {
    let l = overlay(k, groups, st, lens, "knobs-layer");
    if !k.open {
        return l;
    }
    l.child(row().key("knobs-dock").absolute(0.0, 0.0).w_full().pt(12.0).pr(12.0).child(spacer()).child(panel(k, groups, st, lens, None)))
}

/// Region picking and the outline around a group's region, over the element keyed `key`
/// (which this is): it reads that element's place to put window rects inside it.
pub(crate) fn overlay<S: 'static>(k: &Knobs, groups: &'static [Group], st: Style, lens: fn(&mut S) -> &mut Knobs, key: &'static str) -> El {
    let mut l = el().key(key).absolute(0.0, 0.0).w_full().h_full();
    if !k.open {
        return l;
    }
    let (ox, oy) = rune::memo::window_layout(key).map_or((0.0, 0.0), |r| (r[0], r[1]));
    let rects: Vec<(&'static str, [f32; 4])> = groups.iter().filter_map(|g| rune::memo::window_layout(g.region).map(|r| (g.name, r))).collect();
    let picking = k.picking;
    if picking {
        let at = rects.clone();
        l = l.child(
            el().key("knobs-pick")
                .absolute(0.0, 0.0)
                .w_full()
                .h_full()
                .on_pointer_move_local(move |s: &mut S, e, tx: &mut Tx| {
                    tx.instant();
                    let k = lens(s);
                    let h = region_at(&at, e.window_x, e.window_y);
                    if k.hover.as_deref() != h {
                        k.hover = h.map(str::to_owned);
                    }
                })
                .on_hover_change(move |s: &mut S, inside, tx: &mut Tx| {
                    if !inside {
                        tx.instant();
                        lens(s).hover = None;
                    }
                })
                .on_press_tx(move |s: &mut S, tx: &mut Tx| {
                    tx.instant();
                    let k = lens(s);
                    if let Some(h) = k.hover.clone() {
                        k.pick(&h);
                        if let Some(i) = groups.iter().position(|g| g.name == h) {
                            tx.scroll_into_view(crate::side::section_key(i));
                        }
                    }
                }),
        );
    }
    let outlined = k.hover.clone();
    if let Some((name, r)) = outlined.and_then(|n| rects.iter().find(|(g, _)| *g == n).copied()) {
        let mut o = el().key("knobs-outline").absolute(r[0] - ox, r[1] - oy).w(r[2]).h(r[3]).radius(6.0).border(1.0, st.outline);
        if picking {
            o = o.child(row().key("knobs-tag").absolute(6.0, 6.0).h(20.0).px(7.0).center_y().radius(5.0).fill(st.bg).child(text(name).font(TAG).color(st.fg)));
        }
        l = l.child(o);
    }
    l
}

/// The panel alone, for an app's own layout (a storybook's right-hand column): it doesn't
/// float, pick regions or take keys itself, and a single group shows no tabs. The app sends it
/// ↑ ↓ ← → with [`Knobs::panel_key`]; `active` marks the knob those keys move. It fills its
/// parent's width, and its rows lay out from the width they get (a slider's track grows with
/// it), so a 240 pt column and a 300 pt one both read.
pub fn inline<S: 'static>(k: &Knobs, groups: &'static [Group], st: Style, lens: fn(&mut S) -> &mut Knobs, active: bool) -> El {
    panel(k, groups, st, lens, Some(active))
}

/// A card of buttons that each do one thing, with the key that does it on a cap: a story's
/// triggers ("R  Replay", "F  Finish a task"). The app binds the keys. It fills its parent's
/// width up to 300 pt.
pub fn actions<S: 'static>(st: Style, items: &[Action<S>]) -> El {
    let mut card = col().key("knobs-actions").w_full().max_w(W).padding(4.0).gap(2.0).radius(12.0).fill(st.bg).border_gradient(0.5, st.rim, st.border);
    for (i, a) in items.iter().enumerate() {
        let run = a.run;
        card = card.child(
            row()
                .key(format!("knobs-action-{i}"))
                .role("button")
                .label(a.label)
                .w_full()
                .h(ROW)
                .px(6.0)
                .gap(10.0)
                .center_y()
                .radius(8.0)
                .hover(st.fill_hover)
                .on_press_tx(move |s: &mut S, tx: &mut Tx| run(s, tx))
                .child(row().key("cap").h(20.0).min_w(22.0).px(6.0).radius(5.0).fill(st.fill_selected).center().child(text(a.cap).font(VALUE).color(st.fg)))
                .child(text(a.label).font(LABEL).color(st.fg2)),
        );
    }
    card
}

pub(crate) fn tool<S: 'static>(st: Style, key: &str, glyph: &'static Icon, label: &str, enabled: bool, lens: fn(&mut S) -> &mut Knobs, f: impl Fn(&mut Knobs, &mut Tx) + 'static) -> El {
    let b = row()
        .key(key)
        .role("button")
        .label(label)
        .help(label)
        .w(22.0)
        .h(22.0)
        .radius(6.0)
        .center()
        .child(icon(glyph, 14.0).color(st.fg2));
    if !enabled {
        return b.opacity(0.3);
    }
    b.hover(st.fill_hover).on_press_tx(move |s: &mut S, tx: &mut Tx| {
        tx.instant();
        f(lens(s), tx);
    })
}

/// The panel's title and its tools: pick a part (with `pick`), Random and Reset for `g`, Undo,
/// Copy values.
pub(crate) fn head<S: 'static>(k: &Knobs, groups: &'static [Group], st: Style, lens: fn(&mut S) -> &mut Knobs, g: Option<Group>, title: &str, pick: bool) -> El {
    let mut head = row().key("knobs-head").w_full().h(22.0).gap(2.0).center_y().child(text(title.to_owned()).font(TITLE).color(st.fg).truncate_fade().shrink()).child(spacer());
    if pick {
        head = head.child(tool(st, "knobs-repick", &icons::TARGET, "Pick a part", true, lens, |k, _| k.pick_region()));
    }
    if let Some(g) = g {
        head = head
            .child(tool(st, "knobs-random", &icons::RANDOM, "Random", true, lens, move |k, _| k.random(&g)))
            .child(tool(st, "knobs-reset", &icons::RESET, "Reset", k.can_reset(&g), lens, move |k, _| k.reset(&g)));
    }
    head.child(tool(st, "knobs-undo", &icons::UNDO, "Undo", k.can_undo(), lens, |k, _| {
        k.undo();
    })).child(tool(st, "knobs-copy", &icons::COPY, "Copy values", true, lens, move |k, tx| {
        let all: Vec<Group> = match k.current(groups) {
            Some(g) => vec![g],
            None => groups.to_vec(),
        };
        tx.copy(all.iter().map(|g| k.code(g)).collect::<Vec<_>>().join("\n"));
    }))
}

/// `inline` is `Some(active)` for [`inline`].
fn panel<S: 'static>(k: &Knobs, groups: &'static [Group], st: Style, lens: fn(&mut S) -> &mut Knobs, inline: Option<bool>) -> El {
    let g = k.current(groups);
    let title = match (inline, groups) {
        (Some(_), [one]) => one.name,
        _ => "Knobs",
    };
    let head = head(k, groups, st, lens, g, title, inline.is_none());

    let mut tabs = row().key("knobs-tabs").w_full().gap(16.0).px(PAD);
    for (i, t) in groups.iter().enumerate() {
        let on = g.is_some_and(|g| g.name == t.name);
        let lit = on || k.hover.as_deref() == Some(t.name);
        let name = t.name;
        tabs = tabs.child(
            col()
                .key(format!("knobs-tab-{i}"))
                .role("tab")
                .label(name)
                .h(28.0)
                .on_hover_change(move |s: &mut S, inside, tx: &mut Tx| {
                    tx.instant();
                    let k = lens(s);
                    if inside {
                        k.hover = Some(name.to_owned());
                    } else if k.hover.as_deref() == Some(name) {
                        k.hover = None;
                    }
                })
                .on_press_tx(move |s: &mut S, tx: &mut Tx| {
                    tx.instant();
                    lens(s).pick(name);
                })
                .child(spacer())
                .child(text(name).font(CHIP).color(if lit { st.fg } else { st.fg2 }))
                .child(spacer())
                .child(el().key("bar").w_full().h(1.5).radius(1.0).fill(if on { st.handle } else { Color::TRANSPARENT })),
        );
    }

    let p = col().key("knobs-panel");
    let p = if inline.is_some() { p.w_full() } else { p.w(W) };
    let p = p.padding(PAD).radius(12.0).fill(st.bg).border_gradient(0.5, st.rim, st.border);
    if let Some(active) = inline {
        let p = p.child(head.pl(PAD));
        let p = if groups.len() > 1 { p.child(tabs.mt(4.0)).child(el().key("knobs-rule").w_full().h(0.5).fill(st.border)) } else { p };
        return match g {
            Some(g) => body(true, p, k, groups, &g, st, lens, active),
            None => p,
        };
    }
    let p = p
        .shadow(8.0, 24.0, st.shadow)
        .shadow(1.0, 3.0, st.shadow)
        .on_hover_change(move |s: &mut S, inside, tx: &mut Tx| {
            tx.instant();
            let k = lens(s);
            if inside && k.picking {
                k.hover = None;
            }
        })
        .focusable(true)
        .no_focus_ring()
        .on_key(move |s: &mut S, ev, tx: &mut Tx| {
            tx.instant();
            lens(s).panel_key(groups, ev)
        })
        .on_press_tx(move |s: &mut S, tx: &mut Tx| {
            tx.instant();
            lens(s).kbd = true;
        })
        .child(head.pl(PAD))
        .child(tabs.mt(4.0))
        .child(el().key("knobs-rule").w_full().h(0.5).fill(st.border));
    let Some(g) = g else {
        return p.child(text("Click a part of the window to tune it.").key("knobs-hint").font(LABEL).color(st.fg3).px(PAD).mt(12.0).mb(4.0));
    };
    body(false, p, k, groups, &g, st, lens, k.kbd)
}

/// The presets and the knobs' card, under the panel's head; `active` marks the keyboard's knob.
fn body<S: 'static>(inline: bool, p: El, k: &Knobs, groups: &[Group], g: &Group, st: Style, lens: fn(&mut S) -> &mut Knobs, active: bool) -> El {
    let cx = Ctx { k, st, lens, w: if inline { INLINE_CARD_W } else { CARD_W } };
    let gi = groups.iter().position(|x| x.name == g.name).unwrap_or(0);
    let all: Vec<usize> = (0..g.knobs.len()).collect();
    let mark = (active && !all.is_empty()).then(|| k.cursor.min(all.len() - 1));
    p.child(presets(cx, g).mt(PAD)).child(card(cx, g, gi, &all, mark).mt(PAD))
}

#[cfg(test)]
#[path = "view_tests.rs"]
mod tests;
