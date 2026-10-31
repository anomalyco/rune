//! How the side panel draws its [`Plan`]: pages with their groups under them, captions, tabs.
//! A header press folds or opens its section on [`Knobs::fold_spring`]; a folded section keeps
//! its rows under a clip that closes to nothing, so they roll up rather than vanish.

use rune::{Color, El, TextStyle, Tx, col, el, row, spacer, text};

use crate::rows::{CHIP, Ctx, PAD, VALUE, card};
use crate::side::{INSET, section_key};
use crate::tree::{Head, Node, Plan, Section, Sub};
use crate::view::tool;
use crate::icons;
use crate::{Group, Knobs, Style};

const PAGE_H: f32 = 34.0;
const GROUP_H: f32 = 30.0;
/// How far a page's groups sit in from its header.
const NEST: f32 = 12.0;
const PAGE: TextStyle = TextStyle::new(13.0, 600.0).lh(18.0);
const GROUP: TextStyle = TextStyle::new(12.0, 550.0).lh(16.0);
const CAPTION: TextStyle = TextStyle::new(11.0, 600.0).lh(14.0).tracking(0.3);

pub(crate) fn list<S: 'static>(cx: Ctx<'_, S>, groups: &'static [Group], plan: &Plan) -> El {
    let mut l = col().key("knobs-sections").w_full().px(INSET).pb(INSET);
    for s in &plan.sections {
        l = l.child(section(cx, groups, s));
    }
    if plan.sections.iter().all(|s| s.subs.is_empty()) {
        l = l.child(text("Nothing matches").key("knobs-none").font(GROUP).color(cx.st.fg3).px(PAD).h(GROUP_H));
    }
    l
}

/// Rows that fold: open, they take their own height; folded, none, on the fold spring.
fn body(open: bool) -> El {
    let b = col().key("body").w_full().clip_soft(0.0);
    if open { b } else { b.h(0.0) }
}

fn section<S: 'static>(cx: Ctx<'_, S>, groups: &'static [Group], s: &Section) -> El {
    let (k, st, lens) = (cx.k, cx.st, cx.lens);
    let subs = |mut c: El, sticky: f32| {
        for sub in &s.subs {
            c = c.child(group(cx, groups, sub, sticky));
        }
        c
    };
    match s.head {
        Head::Bare => subs(col().key("knobs-bare").w_full(), 0.0),
        Head::Caption(c) => subs(col().key(format!("knobs-cap-{c}")).w_full().child(row().key("cap").w_full().h(28.0).px(PAD).items_end().pb(6.0).child(text(c.to_uppercase()).font(CAPTION).color(st.fg3))), 0.0),
        Head::Page(p) => {
            let i = groups.iter().position(|g| g.page() == p).unwrap_or(0);
            let on = k.kbd && k.at() == Some(Node::Page(p));
            let n: usize = s.subs.iter().map(|x| groups[x.g].knobs.len()).sum();
            let open = s.open;
            let header = row()
                .key("head")
                .role("button")
                .label(p)
                .w_full()
                .h(PAGE_H)
                .px(PAD)
                .gap(7.0)
                .center_y()
                .radius(7.0)
                .fill(if on { st.fill_hover } else { st.bg })
                .sticky_top(0.0)
                .on_press_tx(move |s: &mut S, tx: &mut Tx| {
                    let k = lens(s);
                    tx.motion(k.fold_spring());
                    k.set_folded(p, open);
                    k.at = Some(Node::Page(p));
                    k.kbd = true;
                })
                .child(text(p).font(PAGE).color(if s.seen { st.fg } else { st.fg2 }).truncate_fade().shrink())
                .children(s.seen.then(|| dot(st)))
                .child(spacer())
                .child(text(n.to_string()).font(VALUE).color(if s.open { st.fg3 } else { st.fg2 }));
            col().key(format!("knobs-page-{i}")).w_full().child(header).child(subs(body(s.open), PAGE_H))
        }
    }
}

/// The on-screen mark beside a name.
fn dot(st: Style) -> El {
    el().key("seen").w(5.0).h(5.0).radius(2.5).fill(st.fg3)
}

/// A group under its page's header: `sticky` is where its header sticks, below the page's.
fn group<S: 'static>(cx: Ctx<'_, S>, groups: &'static [Group], sub: &Sub, sticky: f32) -> El {
    let nested = sticky > 0.0;
    let (k, st, lens) = (cx.k, cx.st, cx.lens);
    let g = &groups[sub.g];
    let name = g.name;
    let gi = sub.g;
    let current = k.group.as_deref() == Some(name);
    let lit = current || k.hover.as_deref() == Some(name);
    let on = k.kbd && k.at() == Some(Node::Group(gi));
    let title = if sub.full && !g.under.is_empty() { format!("{} · {}", g.under, name) } else { name.to_owned() };
    let open = sub.open;
    let header = row()
        .key("head")
        .role("button")
        .label(name)
        .w_full()
        .h(GROUP_H)
        .pl(if nested { PAD + NEST } else { PAD })
        .pr(PAD)
        .gap(7.0)
        .center_y()
        .radius(7.0)
        .fill(if on { st.fill_hover } else { st.bg })
        .sticky_top(sticky)
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
            let k = lens(s);
            tx.motion(k.fold_spring());
            k.set_folded(name, open);
            k.at = Some(Node::Group(gi));
            k.kbd = true;
            if !open {
                k.group = Some(name.to_owned());
            }
        })
        .on_secondary_press(move |s: &mut S, _, tx: &mut Tx| {
            tx.instant();
            lens(s).reset(g);
        })
        .child(text(title).font(GROUP).color(if lit { st.fg } else { st.fg2 }).truncate_fade().shrink())
        .children((sub.seen && !sub.full).then(|| dot(st)))
        .child(spacer());
    // Hovered, the header offers Random and Reset for its group in place of its count.
    let header = if k.hover.as_deref() == Some(name) {
        header
            .gap(2.0)
            .pr(PAD - 6.0)
            .child(tool(st, "random", &icons::RANDOM, "Random", true, lens, move |k, _| k.random(g)))
            .child(tool(st, "reset", &icons::RESET, "Reset", k.can_reset(g), lens, move |k, _| k.reset(g)))
    } else {
        header.child(text(sub.knobs.len().to_string()).font(VALUE).color(if open { st.fg3 } else { st.fg2 }))
    };
    let mark = match k.at() {
        Some(Node::Knob(g, i)) if k.kbd && g == gi => Some(i),
        _ => None,
    };
    col().key(section_key(gi)).w_full().child(header).child(body(open).child(card(cx, g, gi, &sub.knobs, mark).mb(INSET)))
}

/// The Tabs layout's pages: the one showing underlined, pages with nothing on screen dimmer.
pub(crate) fn tabs<S: 'static>(st: Style, lens: fn(&mut S) -> &mut Knobs, plan: &Plan) -> El {
    let mut r = row().key("knobs-tabs").w_full().flex_wrap().gap(14.0).px(PAD);
    for (i, (p, seen)) in plan.tabs.iter().enumerate() {
        let on = plan.tab == Some(*p);
        let p = *p;
        r = r.child(
            col()
                .key(format!("knobs-tab-{i}"))
                .role("tab")
                .label(p)
                .h(26.0)
                .on_press_tx(move |s: &mut S, tx: &mut Tx| {
                    tx.instant();
                    let k = lens(s);
                    k.tab = Some(p.to_owned());
                    k.at = None;
                })
                .child(spacer())
                .child(text(p).font(CHIP).color(if on { st.fg } else if *seen { st.fg2 } else { st.fg3 }))
                .child(spacer())
                .child(el().key("bar").w_full().h(1.5).radius(1.0).fill(if on { st.handle } else { Color::TRANSPARENT })),
        );
    }
    r
}
