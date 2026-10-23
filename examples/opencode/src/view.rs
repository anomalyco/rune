//! Views: the shell (sidebar + panel), the home page and the session page. Values follow
//! OpenCode Desktop's v2 styles (`packages/app`, `packages/session-ui`, `packages/ui` tokens).

use std::sync::OnceLock;

use rune::{
    Anim, AnimExt, Color, Curve, Cursor, Cx, Cycle, Drag, Easing, El, GrowthSpec, Icon, Phase, ScrollSpec,
    Spring, TextStyle, Tx, col, edge, el, icon, row, spacer, text, text_input,
};

use rune_kit::{Kit, Markdown, MarkdownStyle};

use crate::icons as ic;
use crate::model::{self, Message, Part, SessionInfo, ToolStatus, Transcript};
use crate::palette::{Pal, pal};
use crate::{App, Route, TabStatus};
use crate::pickers::Kind;

// Type: Inter variable (weights 440/530/670 are real instances) and IBM Plex Mono.
const UI: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
const UI_MED: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0).tracking(-0.04);
const UI_SMALL: TextStyle = TextStyle::new(12.0, 440.0).lh(16.0);
const AVATAR: TextStyle = TextStyle::new(11.0, 530.0).lh(16.0);
const MD: TextStyle = TextStyle::new(14.0, 400.0).lh(22.4);
const BUBBLE: TextStyle = TextStyle::new(14.0, 400.0).lh(21.0);
const H1: TextStyle = TextStyle::new(17.0, 600.0).lh(24.0);
const H2: TextStyle = TextStyle::new(15.0, 600.0).lh(24.0);
const H3: TextStyle = TextStyle::new(13.0, 600.0).lh(20.0);
const CODE: TextStyle = TextStyle::new(13.0, 400.0).mono().lh(20.0);
const PROMPT: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0);

/// `--container-session`: 744 including 24 px padding on each side.
const CONTENT_W: f32 = 696.0;

pub fn app(s: &App, cx: &mut Cx<App>) -> El {
    let p = pal(cx.system_dark());
    cx.reduced_motion(s.reduced_motion);
    let w = s.sidebar_w;
    let open = s.sidebar_open;
    let mut root = row()
        .key("window")
        .role("window")
        .fill(p.bg_deep)
        .w_full()
        .h_full()
        .child(sidebar(s, p))
        .child(
            col()
                .key("panel")
                .grow(1.0)
                .shrink()
                .ml(8.0)
                .mt(8.0)
                .mr(8.0)
                .mb(8.0)
                .radius(10.0)
                .fill(p.bg_base)
                .border(0.5, p.ring)
                .shadow(2.0, 4.0, p.shadow)
                .shadow(1.0, 2.0, p.shadow)
                .clip_soft(0.0)
                .child(match &s.route {
                    Route::Home => home(s, p),
                    Route::Session(id) => session_page(s, p, id),
                    Route::Draft => draft_page(s, p),
                }),
        )
        // The empty inset above the panels moves the window, as the titlebar does.
        .child(el().key("drag-inset").absolute(0.0, 0.0).w_full().h(8.0).window_drag());
    if open {
        root = root.child(resize_handle(s, p, w));
    }
    root = root.child(sidebar_toggle(p, open, w));
    if s.pickers.open.is_some() {
        root = root.child(el().key("picker-dismiss").absolute(0.0, 0.0).w_full().h_full()
            .on_press(|s: &mut App| s.close_picker(false)))
            .child(picker(s, p));
    }
    root
}

// ---------------------------------------------------------------------------------------------
// Pieces

fn avatar(p: &Pal, name: &str) -> El {
    let k = name.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32)) as usize % 8;
    let (bg, border) = p.avatars[k];
    let letter = name.chars().find(|c| c.is_alphanumeric()).map_or("?".into(), |c| c.to_uppercase().to_string());
    el().size(16.0, 16.0)
        .radius(4.0)
        .fill(bg)
        .border(0.5, border)
        .center()
        .child(text(letter).font(AVATAR).color(p.avatar_fg))
}

fn roots(s: &App) -> impl Iterator<Item = &SessionInfo> {
    s.sessions.iter().filter(|x| x.parent.is_none())
}

// ---------------------------------------------------------------------------------------------
// Sidebar: Desktop's vertical tabs (`shell/titlebar/titlebar.tsx`, `tab-nav.tsx`).

/// Default, minimum and maximum widths (`shell.tsx`'s ResizeHandle).
pub const SIDEBAR_W: f32 = 260.0;
pub const SIDEBAR_MIN: f32 = 120.0;
pub const SIDEBAR_MAX: f32 = 640.0;
/// The sidebars' shared spring, "smooth" (`panel-motion.ts`).
const SIDEBAR_SPRING: Spring = Spring::new(0.3, 0.0);
/// Where the zoom button ends with the lights on the header's centre line (`macVerticalTrafficLights`).
const TRAFFIC_LIGHTS_END: f32 = 85.0;
/// The collapsed toggle's box: from the shell inset plus header padding (18) to past the lights.
const COLLAPSED_SLOT: f32 = TRAFFIC_LIGHTS_END + 16.0 - 18.0 + 32.0;
/// How far a closed panel's contents dim at the end of the flight.
const FADE_FLOOR: f32 = 0.5;

const INSTANT: Anim = Anim::INSTANT;

/// Home, Search sessions and New session (`data-titlebar-tab-action`).
fn action_row(p: &Pal, key: &str, glyph: &'static Icon, label: &str, hint: &str, pressed: bool) -> El {
    let ink = if pressed { p.text } else { p.faint };
    let mut r = row()
        .key(key)
        .role("button")
        .label(label)
        .h(28.0)
        .w_full()
        .pl(6.0)
        .pr(8.0)
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .child(icon(glyph, 16.0).color(ink).group_color(p.text))
        .child(text(label).font(UI).color(ink).group_color(p.text).truncate());
    if pressed {
        r = r.fill(p.pressed);
    }
    if !hint.is_empty() {
        r = r.child(spacer()).child(text(hint).font(UI).color(p.faint).group_opacity(0.0, 1.0));
    }
    r
}

fn sidebar(s: &App, p: &Pal) -> El {
    let current = match &s.route {
        Route::Session(id) => Some(s.root_of(id)),
        _ => None,
    };
    let mut list = col().key("tabs").w_full().gap(4.0).pb(16.0);
    for t in &s.tabs {
        list = list.child(tab_row(s, p, t, current == Some(t.id.as_str())));
    }

    let w = s.sidebar_w;
    let inner = col()
        .key("sidebar-inner")
        // Pinned to the aside's end edge: it slides out as the aside narrows.
        .absolute(if s.sidebar_open { 0.0 } else { -w }, 0.0)
        .w(w)
        .h_full()
        .pt(8.0)
        .pl(10.0)
        .pr(2.0)
        .pb(10.0)
        .opacity(if s.sidebar_open { 1.0 } else { FADE_FLOOR })
        .opacity_motion(SIDEBAR_SPRING)
        .child(
            // Shares the session header's 48px centre line with the traffic lights; drags the window.
            row()
                .key("sidebar-header")
                .w_full()
                .h(48.0)
                .mb(8.0)
                .gap(8.0)
                .center_y()
                .window_drag()
                .child(el().key("lights").w(TRAFFIC_LIGHTS_END - 10.0).h(28.0))
                .child(spacer())
                // The toggle floats above (see `sidebar_toggle`); its slot keeps the row's shape.
                .child(el().key("toggle-slot").size(28.0, 28.0)),
        )
        .child(
            col()
                .key("actions")
                .w_full()
                .gap(4.0)
                .child(action_row(p, "home", &ic::GRID_PLUS, "Home", "⌘B", s.route == Route::Home).on_press_tx(
                    |s: &mut App, tx: &mut Tx| {
                        s.route = Route::Home;
                        tx.settle();
                    },
                ))
                .child(action_row(p, "search", &ic::MAGNIFYING_GLASS, "Search sessions", "", false).on_press_tx(
                    |s: &mut App, tx: &mut Tx| {
                        s.route = Route::Home;
                        s.search.focused = true;
                        tx.settle();
                    },
                ))
                .child(action_row(p, "new", &ic::EDIT, "New session", "⌘T", false).on_press_tx(
                    |s: &mut App, tx: &mut Tx| {
                        s.route = Route::Draft;
                        tx.settle();
                    },
                )),
        )
        .child(el().key("tabs-gap").w_full().h(16.0))
        .child(
            col()
                .key("tabs-scroll")
                .w_full()
                .grow(1.0)
                .shrink()
                .scroll(ScrollSpec { stick_bottom: false })
                .clip_fade(0.0, 0.0, 0.0, 16.0)
                .child(list),
        );

    // The aside's width moves the panel's edge; its contents keep their width, pinned to that edge.
    el().key("sidebar")
        .role("navigation")
        .w(if s.sidebar_open { w } else { 0.0 })
        .h_full()
        
        .clip_soft(0.0)
        .motion(SIDEBAR_SPRING)
        .child(inner)
}

fn tab_row(s: &App, p: &Pal, t: &crate::tabs::Tab, active: bool) -> El {
    let info = s.session(&t.id);
    let title = info.map(title_of).unwrap_or_else(|| if t.title.is_empty() { "Untitled session".into() } else { t.title.clone() });
    let dir = info.map_or(t.directory.as_str(), |i| i.directory.as_str());
    let proj = info
        .and_then(|i| s.project(&i.project))
        .map(|x| x.name.clone())
        .unwrap_or_else(|| dir.rsplit('/').find(|x| !x.is_empty()).unwrap_or("").to_owned());
    let status = s.tab_status(&t.id);
    // The title fades out before the status/close slot (`--tab-title-fade-offset`).
    let fade_offset = if active || status.is_some() { 24.0 } else { 4.0 };
    // 4px from the row's end; the row spans the sidebar less its 10 + 2 padding.
    let slot_x = s.sidebar_w - 12.0 - 24.0;
    let id = t.id.clone();
    let close_id = t.id.clone();
    let mut r = row()
        .key(format!("t-{}", t.id))
        .role("button")
        .label(title.clone())
        .w_full()
        .h(28.0)
        
        .radius(6.0)
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| s.open(&id, tx))
        .child(
            row()
                .key("link")
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .h_full()
                .pl(6.0)
                .mr(fade_offset + 6.0)
                .gap(6.0)
                .center_y()
                .clip_fade(0.0, 0.0, 16.0, 0.0)
                .child(avatar(p, &proj))
                .child(text(title).font(UI_MED).color(if active { p.text } else { p.faint })),
        )
        .child(
            // Status and close share the slot; hovering the row swaps one for the other.
            el().key("status")
                .absolute(slot_x, 4.0)
                .size(20.0, 20.0)
                .center()
                .group_opacity(1.0, 0.0)
                .child(match status {
                    Some(TabStatus::Busy) => activity_ring(p),
                    Some(TabStatus::Attention) => el().size(7.0, 7.0).radius(3.5).fill(p.warning),
                    Some(TabStatus::Unread) => el().size(7.0, 7.0).radius(3.5).fill(p.bg_accent),
                    None => el(),
                }),
        )
        .child(
            el().key("close")
                .role("button")
                .label("Close tab")
                .absolute(slot_x, 4.0)
                .size(20.0, 20.0)
                .radius(4.0)
                .center()
                .fill(p.bg_deep)
                .hover(p.hover)
                .hover_motion(INSTANT, INSTANT)
                .group_opacity(0.0, 1.0)
                .on_press_tx(move |s: &mut App, tx: &mut Tx| s.close_tab(&close_id, tx))
                .child(
                    el().key("close-ink")
                        .size(20.0, 20.0)
                        .radius(4.0)
                        .center()
                        .fill(if active { p.pressed } else { p.hover })
                        .child(icon(&ic::XMARK_SMALL, 16.0).color(p.icon_muted)),
                ),
        );
    if active {
        r = r.fill(p.pressed);
    }
    r
}

/// The sidebar edge's resize handle: an 8px hit area across the gap, with a pill that waits a
/// moment before showing on hover (`resize-handle.css`).
fn resize_handle(s: &App, p: &Pal, w: f32) -> El {
    let enter = Curve::tween(0.15, Easing::cubic(0.2, 0.8, 0.2, 1.0)).delay(0.2);
    let exit = Curve::tween(0.11, Easing::cubic(0.4, 0.0, 1.0, 1.0));
    let dragging = s.sidebar_drag.is_some();
    el().key("sidebar-resize")
        .absolute(w, 0.0)
        .w(8.0)
        .h_full()
        .center()
        .cursor(Cursor::ResizeColumn)
        .hover_group()
        .hover_motion(enter, exit)
        .on_drag(|s: &mut App, d: &mut Drag, tx: &mut Tx| {
            tx.instant();
            match d.phase {
                Phase::Down => s.sidebar_drag = Some(s.sidebar_w),
                Phase::Move => {
                    let from = s.sidebar_drag.unwrap_or(s.sidebar_w);
                    s.sidebar_w = (from + d.dx).clamp(SIDEBAR_MIN, SIDEBAR_MAX);
                }
                Phase::Up => s.sidebar_drag = None,
            }
        })
        .child(
            el().key("pill")
                .size(4.0, 40.0)
                .radius(2.0)
                .fill(if dragging { p.icon } else { p.icon_muted })
                .group_opacity(if dragging { 1.0 } else { 0.0 }, 1.0),
        )
}

/// One toggle that reads as the header's and the collapsed one: it rides the sidebar's edge
/// to the collapsed spot past the traffic lights while its glyph turns (`sidebar-toggle.tsx`).
fn sidebar_toggle(p: &Pal, open: bool, w: f32) -> El {
    let x = if open { w - 2.0 - 28.0 } else { 18.0 + COLLAPSED_SLOT - 4.0 - 28.0 };
    el().key("sidebar-toggle")
        .role("button")
        .label("Toggle sidebar")
        .absolute(x, 18.0)
        .size(28.0, 28.0)
        .radius(6.0)
        .center()
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .motion(SIDEBAR_SPRING)
        .on_press(|s: &mut App| s.sidebar_open = !s.sidebar_open)
        .child(icon(&ic::LAYOUT_LEFT_FULL, 20.0).color(p.icon_muted).absolute(4.0, 4.0).opacity(if open { 1.0 } else { 0.0 }).opacity_motion(SIDEBAR_SPRING))
        .child(icon(&ic::LAYOUT_LEFT, 20.0).color(p.icon_muted).absolute(4.0, 4.0).opacity(if open { 0.0 } else { 1.0 }).opacity_motion(SIDEBAR_SPRING))
}

/// Room the session header keeps for the collapsed toggle.
fn collapsed_slot(s: &App) -> El {
    // Beta measures from the header's 10px padding; this header pads 14.
    el().key("toggle-space").w(if s.sidebar_open { 0.0 } else { COLLAPSED_SLOT - 4.0 }).h(28.0).motion(SIDEBAR_SPRING)
}

/// Desktop's activity spinner, "Soft Radar": a 5×5 dot matrix whose sweep head steps clockwise
/// around each square ring, leaving a fading trail (`spinner-variants.ts`), 14px, icon-muted.
fn activity_ring(p: &Pal) -> El {
    let mut r = el().key("ring").size(14.0, 14.0);
    for (k, d) in radar().iter().enumerate() {
        let mut dot = el()
            .key(format!("d{k}"))
            .absolute(d.x, d.y)
            .size(d.size, d.size)
            .radius(d.size * 0.24)
            .fill(p.icon_muted);
        dot = match d.cycle {
            Some(c) => dot.cycle(c),
            None => dot.opacity(d.still),
        };
        r = r.child(dot);
    }
    r
}

struct RadarDot {
    x: f32,
    y: f32,
    size: f32,
    still: f32,
    cycle: Option<Cycle>,
}

fn radar() -> &'static [RadarDot] {
    static DOTS: OnceLock<Vec<RadarDot>> = OnceLock::new();
    DOTS.get_or_init(|| {
        const SPAN: i32 = 5;
        const LOOP: f32 = 1.8;
        const DIM: f32 = 0.2;
        const EDGE: f32 = 1.5;
        let r3 = |v: f32| (v * 1000.0).round() / 1000.0;
        // Soft Radar's trail from 0.55, sampled every 4% over 48% of the loop.
        let fade = [1.0, 0.987, 0.934, 0.812, 0.599, 0.384, 0.238, 0.143, 0.081, 0.041, 0.017, 0.004, 0.0];
        let mut trail: Vec<(f32, f32)> = fade.iter().enumerate().map(|(i, l)| (i as f32 * 4.0, r3(0.55 * l))).collect();
        trail.push((100.0, 0.0));
        let level_at = |stops: &[(f32, f32)], pc: f32| -> f32 {
            let Some(k) = stops.iter().position(|s| s.0 >= pc) else { return stops.last().map_or(0.0, |s| s.1) };
            if k == 0 {
                return stops[0].1;
            }
            let ((p0, l0), (p1, l1)) = (stops[k - 1], stops[k]);
            l0 + (l1 - l0) * (pc - p0) / if p1 - p0 == 0.0 { 1.0 } else { p1 - p0 }
        };
        let frac = |v: f32| v - v.floor();
        let cells: Vec<(i32, i32)> = (0..SPAN * SPAN).map(|i| (i % SPAN - SPAN / 2, i / SPAN - SPAN / 2)).collect();
        let angle = |(gx, gy): (i32, i32)| frac((gx as f32).atan2(-(gy as f32)) / std::f32::consts::TAU);
        let ring_of = |(gx, gy): (i32, i32)| gx.abs().max(gy.abs());
        let scale = 14.0 / 16.0;
        cells
            .iter()
            .map(|&c| {
                let (x, y, size) = ((8.0 + c.0 as f32 * 3.0 - 1.0) * scale, (8.0 + c.1 as f32 * 3.0 - 1.0) * scale, 2.0 * scale);
                if c == (0, 0) {
                    // The steady centre glows like the fresh trail.
                    return RadarDot { x, y, size, still: DIM + (1.0 - DIM) * trail[0].1, cycle: None };
                }
                let at = r3(angle(c));
                let others = cells.iter().filter(|&&o| o != c && o != (0, 0) && ring_of(o) == ring_of(c));
                let behind = others.clone().map(|&o| r3(frac(at - r3(angle(o))))).fold(1.0f32, f32::min) * 100.0;
                let ahead = others.map(|&o| r3(frac(r3(angle(o)) - at))).fold(1.0f32, f32::min) * 100.0;
                // "step": lit while it's the dot nearest the head, then the trail.
                let off = ahead / 2.0 + EDGE;
                let on = 100.0 - behind / 2.0;
                let mut shape = vec![(0.0, 1.0), (ahead / 2.0, 1.0), (off, level_at(&trail, off))];
                shape.extend(trail.iter().copied().filter(|&(pc, _)| pc > off && pc < 100.0 && pc < on - EDGE));
                shape.extend([(on - EDGE, 0.0), (on, 1.0), (100.0, 1.0)]);
                let still = DIM + (1.0 - DIM) * level_at(&shape, frac(0.22 - at) * 100.0);
                let stops: Vec<(f32, f32)> = shape.iter().map(|&(pc, l)| (pc / 100.0, DIM + (1.0 - DIM) * l)).collect();
                let stops: &'static [(f32, f32)] = Box::leak(stops.into_boxed_slice());
                RadarDot { x, y, size, still, cycle: Some(Cycle { period: LOOP, phase: at, stops, still, fps: 30.0 }) }
            })
            .collect()
    })
}

fn title_of(s: &SessionInfo) -> String {
    if s.title.is_empty() { "Untitled session".into() } else { s.title.clone() }
}

// ---------------------------------------------------------------------------------------------
// Home: search and recent sessions by day (Desktop's home sessions column).

fn home(s: &App, p: &Pal) -> El {
    let q = s.search.text.to_lowercase();
    let mut groups: Vec<(&str, Vec<&SessionInfo>)> = vec![("Today", vec![]), ("Yesterday", vec![]), ("Older", vec![])];
    let day = |ms: i64| ms.div_euclid(86_400_000);
    for ses in roots(s).filter(|x| q.is_empty() || x.title.to_lowercase().contains(&q)).take(60) {
        let d = day(s.now_ms) - day(ses.updated);
        groups[if d <= 0 { 0 } else if d == 1 { 1 } else { 2 }].1.push(ses);
    }
    let mut list = col().key("groups").w_full().pr(12.0).pb(64.0);
    let mut first = true;
    for (title, items) in groups.into_iter().filter(|g| !g.1.is_empty()) {
        let mut header = row().key(format!("h-{title}")).h(28.0).pl(12.0).center_y().child(text(title).font(UI).color(p.muted));
        if first {
            header = header.child(spacer()).child(
                row()
                    .key("new-session")
                    .role("button")
                    .h(28.0)
                    .px(8.0)
                    .gap(6.0)
                    .center_y()
                    .radius(6.0)
                    .hover(p.hover)
                    .on_press_tx(|s: &mut App, tx: &mut Tx| {
                        s.route = Route::Draft;
                        tx.settle();
                    })
                    .child(icon(&ic::EDIT, 16.0).color(p.icon_muted))
                    .child(text("New session").font(UI_MED).color(p.muted)),
            );
        }
        let mut rows = col().key(format!("g-{title}")).gap(1.0).pt(16.0).mb(24.0);
        for ses in items {
            let proj = s.project(&ses.project).map_or("", |x| x.name.as_str());
            let id = ses.id.clone();
            rows = rows.child(
                row()
                    .key(format!("r-{}", ses.id))
                    .role("button")
                    .label(ses.title.clone())
                    .h(40.0)
                    .pl(12.0)
                    .pr(16.0)
                    .gap(6.0)
                    .center_y()
                    .radius(6.0)
                    .hover(p.hover)
                    .press(p.pressed)
                    .on_press_tx(move |s: &mut App, tx: &mut Tx| s.open(&id, tx))
                    .child(avatar(p, proj))
                    .child(text(title_of(ses)).font(UI_MED).color(p.text).truncate())
                    .child(text(proj.to_owned()).font(UI).color(p.faint).truncate())
                    .child(spacer())
                    .child(text(model::ago(s.now_ms, ses.updated)).font(UI).color(p.faint)),
            );
        }
        list = list.child(header).child(rows);
        first = false;
    }
    if !q.is_empty() && first {
        list = list.child(
            row().key("no-results").h(40.0).pl(12.0).center_y().child(text(format!("No sessions found for {}", s.search.text)).font(UI).color(p.muted)),
        );
    }

    let search = row()
        .key("search")
        .h(36.0)
        .pl(12.0)
        .pr(8.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .fill(p.search_bg)
        .hover(p.layer2)
        .child(icon(&ic::MAGNIFYING_GLASS, 16.0).color(p.icon_muted))
        .child(
            text_input(&s.search, UI.lh(20.0))
                .key("search-input")
                .grow(1.0)
                .shrink()
                .h(20.0)
                .placeholder("Search sessions", p.faint)
                .caret(p.text, p.selection)
                .on_edit(|s: &mut App| &mut s.search),
        );

    col()
        .key("home")
        .grow(1.0)
        .w_full()
        .scroll(ScrollSpec { stick_bottom: false })
        .clip_soft(0.0)
        .child(
            col()
                .key("home-content")
                .w_full()
                .center()
                .px(24.0)
                .pt(48.0)
                .child(col().key("home-column").w_full().max_w(720.0).child(search).child(list.mt(12.0))),
        )
}

// ---------------------------------------------------------------------------------------------
// Session

fn draft_page(s: &App, p: &Pal) -> El {
    col()
        .key("draft")
        .grow(1.0)
        .w_full()
        .child(
            row()
                .key("header")
                .h(48.0)
                .pl(14.0)
                .center_y()
                .window_drag()
                .child(collapsed_slot(s))
                .child(text("New session").font(UI_MED).color(p.text)),
        )
        .child(
            col()
                .key("draft-hero")
                .grow(1.0)
                .center()
                .gap(6.0)
                .child(text("What should we build?").font(TextStyle::new(20.0, 530.0).lh(28.0)).color(p.text))
                .child(
                    text("Sessions are read-only here: this build never creates one on the server.")
                        .font(UI)
                        .color(p.faint),
                ),
        )
        .child(composer_dock(s, p))
        .child(status_bar(s, p, None))
}

fn session_page(s: &App, p: &Pal, id: &str) -> El {
    let info = s.session(id);
    let t = s.transcripts.get(id);
    col()
        .key("session")
        .grow(1.0)
        .w_full()
        .child(header(s, p, info, t))
        .child(timeline(s, p, id, t))
        .child(composer_dock(s, p))
        .child(status_bar(s, p, info))
}

fn header(s: &App, p: &Pal, info: Option<&SessionInfo>, t: Option<&Transcript>) -> El {
    let mut crumbs = row().key("crumbs").gap(0.0).center_y().shrink();
    if let Some(info) = info {
        let proj = s.project(&info.project).map_or("", |x| x.name.as_str());
        crumbs = crumbs
            .child(avatar(p, proj).mr(8.0))
            .child(text(proj.to_owned()).font(UI_MED).color(p.faint))
            .child(text("/").font(TextStyle::new(11.0, 500.0)).color(p.faint).pl(8.0).pr(4.0));
        if let Some(parent) = info.parent.as_deref().and_then(|pid| s.session(pid)) {
            crumbs = crumbs
                .child(text(title_of(parent)).font(UI_MED).color(p.faint).truncate())
                .child(text("/").font(TextStyle::new(11.0, 500.0)).color(p.faint).pl(8.0).pr(4.0));
        }
        crumbs = crumbs.child(text(title_of(info)).font(UI_MED).color(p.text).truncate().pl(4.0));
    }
    // Context and spend: the last finished step's context window, the session's cost.
    let last = t.and_then(|t| {
        t.messages.iter().rev().find_map(|m| match m {
            Message::Assistant(a) if a.tokens.context() > 0 => Some(a.tokens.context()),
            _ => None,
        })
    });
    let ctx = last.unwrap_or(0);
    // The session's own total (it includes subagents); the transcript's sum when it's ahead.
    let summed = t.map_or(0.0, |t| t.messages.iter().map(|m| if let Message::Assistant(a) = m { a.cost } else { 0.0 }).sum::<f64>());
    let cost = info.map_or(0.0, |i| i.cost).max(summed);
    let limit = 200_000.0;
    let usage = row()
        .key("usage")
        .gap(6.0)
        .center_y()
        .child(ring(p, (ctx as f32 / limit).min(1.0)))
        .child(text(model::tokens_short(ctx)).font(UI_SMALL).color(p.muted))
        .child(text(format!("${cost:.2}")).font(UI_SMALL).color(p.faint));
    row()
        .key("header")
        .h(48.0)
        .pl(14.0)
        .pr(14.0)
        .gap(8.0)
        .center_y()
        .window_drag()
        .child(collapsed_slot(s))
        .child(crumbs)
        .child(spacer())
        .child(usage)
}

/// Context ring: a track circle and an arc, both icons built once per 1/32 step.
fn ring(p: &Pal, frac: f32) -> El {
    static ARCS: OnceLock<Vec<Icon>> = OnceLock::new();
    static TRACK: Icon = Icon { name: "ring-track", view: 16.0, body: r#"<circle cx="8" cy="8" r="6" stroke="currentColor" stroke-width="2"/>"# };
    let arcs = ARCS.get_or_init(|| {
        (0..=32)
            .map(|k| {
                let f = k as f32 / 32.0;
                let body = if k == 0 {
                    String::new()
                } else if k == 32 {
                    r#"<circle cx="8" cy="8" r="6" stroke="currentColor" stroke-width="2"/>"#.to_owned()
                } else {
                    let a = f * std::f32::consts::TAU;
                    let (x, y) = (8.0 + 6.0 * a.sin(), 8.0 - 6.0 * a.cos());
                    format!(
                        r#"<path d="M8 2A6 6 0 {} 1 {x:.3} {y:.3}" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>"#,
                        u8::from(f > 0.5)
                    )
                };
                Icon { name: "ring-arc", view: 16.0, body: Box::leak(body.into_boxed_str()) }
            })
            .collect()
    });
    let k = (frac * 32.0).round().clamp(0.0, 32.0) as usize;
    el().key("ring")
        .size(16.0, 16.0)
        .child(icon(&TRACK, 16.0).color(p.layer3).absolute(0.0, 0.0))
        .child(icon(&arcs[k], 16.0).color(p.icon).absolute(0.0, 0.0))
}

/// One timeline row's content, before it becomes a growth-stack child.
struct RowSpec {
    key: String,
    el: El,
    streaming: bool,
}

fn timeline(s: &App, p: &Pal, id: &str, t: Option<&Transcript>) -> El {
    let e = edge::ALL[s.edge];
    let rows = t.map(|t| rows(s, p, t, e.word_fade)).unwrap_or_default();
    let mut stack = col()
        .key("timeline")
        .w_full()
        .center()
        .pt(8.0)
        .pb(24.0)
        .virtualize(80.0)
        .growth(GrowthSpec { edge: e, ..Default::default() });
    for r in rows {
        stack = stack.child(col().key(r.key).w_full().max_w(CONTENT_W).streaming(r.streaming).child(r.el));
    }
    if t.is_none() {
        stack = stack.child(
            col().key("loading").w_full().max_w(CONTENT_W).h(40.0).center().child(text("Loading…").font(UI).color(p.faint)),
        );
    }
    col()
        .key(format!("scroll-{id}"))
        .grow(1.0)
        .shrink()
        .w_full()
        .scroll(ScrollSpec { stick_bottom: true })
        .clip_fade(0.0, 16.0, 0.0, 8.0)
        .child(stack)
}

fn rows(s: &App, p: &Pal, t: &Transcript, fade: f32) -> Vec<RowSpec> {
    let mut out = Vec::new();
    let busy = t.busy();
    let last_assistant = t.messages.iter().rposition(|m| matches!(m, Message::Assistant(_)));
    // Consecutive tool calls (across steps) collapse into one "Used N tools" row.
    let mut group: Vec<(&str, &str, &str, &ToolStatus)> = Vec::new();
    let mut group_key = String::new();
    let flush = |out: &mut Vec<RowSpec>, group: &mut Vec<(&str, &str, &str, &ToolStatus)>, key: &str| {
        if group.is_empty() {
            return;
        }
        let k = format!("g-{key}");
        let open = s.expanded.contains(&k);
        out.push(RowSpec { el: tool_group(p, &k, group, open), key: k, streaming: false });
        group.clear();
    };
    for (mi, m) in t.messages.iter().enumerate() {
        match m {
            Message::User { id, text: body, .. } => {
                flush(&mut out, &mut group, &group_key);
                out.push(RowSpec { key: format!("u-{id}"), el: user_bubble(p, id, body, out.is_empty()), streaming: false });
            }
            Message::Note { id, text: body } => {
                flush(&mut out, &mut group, &group_key);
                out.push(RowSpec { key: format!("n-{id}"), el: row().pb(12.0).child(text(body.clone()).font(UI).color(p.faint)), streaming: false });
            }
            Message::Assistant(a) => {
                let live_msg = Some(mi) == last_assistant && a.completed.is_none() && a.error.is_none();
                for (pi, part) in a.parts.iter().enumerate() {
                    let is_last = live_msg && pi + 1 == a.parts.len();
                    match part {
                        Part::Tool { id, name, title, status, input, .. } if name == "task" || name == "subagent" => {
                            flush(&mut out, &mut group, &group_key);
                            let agent = input["subagent_type"].as_str().unwrap_or("general");
                            out.push(RowSpec { key: format!("a-{id}"), el: subagent_chip(p, agent, title, status), streaming: false });
                        }
                        Part::Tool { id, name, title, status, .. } => {
                            if group.is_empty() {
                                group_key = id.clone();
                            }
                            group.push((id, name, title, status));
                        }
                        Part::Text { text: body } => {
                            flush(&mut out, &mut group, &group_key);
                            if body.trim().is_empty() && !is_last {
                                continue;
                            }
                            out.push(RowSpec {
                                key: format!("t-{}-{pi}", a.id),
                                el: markdown_view(p, body, fade).pb(12.0),
                                streaming: is_last,
                            });
                        }
                        Part::Reasoning { text: body, started, completed } => {
                            flush(&mut out, &mut group, &group_key);
                            let k = format!("r-{}-{pi}", a.id);
                            let open = s.expanded.contains(&k);
                            let secs = completed.map(|c| ((c - started) as f64 / 1000.0).round().max(1.0) as i64);
                            out.push(RowSpec { el: reasoning(p, &k, body, secs, open, fade), key: k, streaming: is_last && open });
                        }
                    }
                }
                if let Some(err) = &a.error {
                    flush(&mut out, &mut group, &group_key);
                    out.push(RowSpec { key: format!("e-{}", a.id), el: row().pb(12.0).child(text(err.clone()).font(UI).color(p.danger).wrap().w_full()), streaming: false });
                }
            }
        }
    }
    flush(&mut out, &mut group, &group_key);
    if let Some(last) = last_assistant {
        out.push(RowSpec { key: "turn-actions".into(), el: turn_actions(p, busy, &t.session, &t.messages[last]), streaming: false });
    }
    out
}

fn user_bubble(p: &Pal, id: &str, body: &str, first: bool) -> El {
    row()
        .w_full()
        .justify_end()
        .pt(if first { 8.0 } else { 20.0 })
        .pb(16.0)
        .child(
            col()
                .max_w(460.0)
                .shrink()
                .px(12.0)
                .py(8.0)
                .radius(10.0)
                .fill(p.layer2)
                .child(text(body.to_owned()).key(format!("text-{id}")).font(BUBBLE).color(p.text).wrap()),
        )
}

fn chevron(open: bool) -> &'static Icon {
    if open { &ic::CHEVRON_DOWN } else { &ic::CHEVRON_RIGHT }
}

fn tool_icon(name: &str) -> &'static Icon {
    match name {
        "shell" | "bash" => &ic::CONSOLE,
        "grep" | "glob" => &ic::MAGNIFYING_GLASS,
        "read" | "edit" | "write" => &ic::CODE_LINES,
        "todowrite" | "todoread" => &ic::CHECKLIST,
        _ => &ic::FILE_TREE,
    }
}

fn tool_group(p: &Pal, key: &str, tools: &[(&str, &str, &str, &ToolStatus)], open: bool) -> El {
    let n = tools.len();
    let running = tools.iter().any(|t| matches!(t.3, ToolStatus::Running | ToolStatus::Pending));
    let k = key.to_owned();
    let trigger = row()
        .key("trigger")
        .role("button")
        .h(28.0)
        .center_y()
        .on_press(move |s: &mut App| {
            if !s.expanded.remove(&k) {
                s.expanded.insert(k.clone());
            }
        })
        .child(text(if running { "Using " } else { "Used " }).font(UI).color(p.muted))
        .child(text(n.to_string()).font(UI).color(p.text))
        .child(text(if n == 1 { " tool" } else { " tools" }).font(UI).color(p.muted))
        .child(icon(chevron(open), 14.0).color(p.icon_faint).ml(2.0));
    let mut c = col().w_full().pb(12.0).child(trigger);
    if open {
        let mut list = col().key("tools").gap(0.0).pl(4.0);
        for (id, name, title, status) in tools {
            let color = match status {
                ToolStatus::Error => p.danger,
                _ => p.muted,
            };
            list = list.child(
                row()
                    .key(format!("tool-{id}"))
                    .h(28.0)
                    .gap(8.0)
                    .center_y()
                    .child(icon(tool_icon(name), 14.0).color(p.icon_muted))
                    .child(text(name.to_string()).font(UI_MED).color(color))
                    .child(text(title.to_string()).font(UI).color(p.faint).truncate()),
            );
        }
        c = c.child(list);
    }
    c
}

fn reasoning(p: &Pal, key: &str, body: &str, secs: Option<i64>, open: bool, fade: f32) -> El {
    let k = key.to_owned();
    let label = match secs {
        Some(s) => format!("Thought for {s}s"),
        None => "Thinking".into(),
    };
    let mut c = col().w_full().pb(12.0).child(
        row()
            .key("trigger")
            .role("button")
            .h(28.0)
            .gap(6.0)
            .center_y()
            .on_press(move |s: &mut App| {
                if !s.expanded.remove(&k) {
                    s.expanded.insert(k.clone());
                }
            })
            .child(icon(&ic::BRAIN, 16.0).color(p.icon_muted))
            .child(text(label).font(UI).color(p.muted))
            .child(icon(chevron(open), 14.0).color(p.icon_faint)),
    );
    if open {
        c = c.child(
            row().key("thought").pl(22.0).pt(4.0).child(text(body.to_owned()).font(UI.lh(20.0)).color(p.muted).wrap().reveal(fade).w_full()),
        );
    }
    c
}

fn subagent_chip(p: &Pal, agent: &str, title: &str, status: &ToolStatus) -> El {
    let done = *status == ToolStatus::Completed;
    row().w_full().pb(12.0).child(
        row()
            .key("chip")
            .role("button")
            .h(32.0)
            .px(10.0)
            .gap(8.0)
            .center_y()
            .shrink()
            .radius(8.0)
            .fill(p.layer1)
            .border(0.5, p.border)
            .hover(p.layer2)
            .child(icon(&ic::SUBAGENT, 16.0).color(p.icon))
            .child(text(model::capitalize(agent)).font(UI_MED).color(p.text))
            .child(text(title.to_owned()).font(UI).color(p.muted).truncate())
            .child(text(if done { "Done" } else { "Running" }).font(UI_SMALL).color(if done { p.faint } else { p.accent })),
    )
}

fn working(p: &Pal) -> El {
    row().h(28.0).gap(8.0).center_y().pb(12.0).child(el().size(6.0, 6.0).radius(3.0).fill(p.accent)).child(text("Working").font(UI).color(p.muted))
}

fn turn_actions(p: &Pal, busy: bool, session: &str, message: &Message) -> El {
    use rune::AnimExt;
    // These share a footprint: let the outgoing label/icon finish before the other arrives.
    // Otherwise the copy icon's outline reads as a missing glyph over the W in Working.
    let fade = |show| rune::Curve::ease_in_out(0.18).delay(if show { 0.18 } else { 0.0 });
    let has_text = matches!(message, Message::Assistant(a) if a.parts.iter().any(|part| matches!(part, Part::Text { text } if !text.trim().is_empty())));
    let mut button = row().key("copy").role("button").label("Copy response")
        .size(28.0, 28.0).pb(12.0).center().radius(4.0).hover(p.hover).press(p.pressed)
        .opacity(if busy || !has_text { 0.0 } else { 1.0 })
        .opacity_motion(fade(!busy && has_text))
        .child(icon(&ic::OUTLINE_COPY, 16.0).color(p.muted));
    if !busy && has_text {
        let session = session.to_owned();
        let id = message.id().to_owned();
        button = button.on_press_tx(move |s: &mut App, tx: &mut Tx| {
            if let Some(Message::Assistant(a)) = s.transcripts.get(&session).and_then(|t| t.messages.iter().find(|m| m.id() == id)) {
                tx.copy(a.parts.iter().filter_map(|part| match part {
                    Part::Text { text } => Some(text.as_str()),
                    _ => None,
                }).collect::<Vec<_>>().join("\n\n"));
            }
        });
    }
    // One reserved tail: completion crossfades in place rather than first removing Working's
    // height and then adding actions. The scroll endpoint cannot reverse during this handoff.
    el().w_full().h(28.0).motion(rune::Spring::new(0.34, 0.0))
        .child(working(p).key("working").absolute(0.0, 0.0).opacity(if busy { 1.0 } else { 0.0 }).opacity_motion(fade(busy)))
        .child(button.absolute(0.0, 0.0))
}

// ---------------------------------------------------------------------------------------------
// Markdown

fn markdown_view(p: &Pal, src: &str, fade: f32) -> El {
    let kit = Kit::neutral(p.dark);
    let style = MarkdownStyle::from_kit(&kit)
        .text(p.text)
        .muted(p.muted)
        .link(p.accent)
        .inline_code_bg(p.inline_code_bg)
        .code_block(p.layer1, p.border_muted)
        .quote_rule(p.layer3)
        .rule(p.border)
        .body_font(MD)
        .heading_font(1, H1)
        .heading_font(2, H2)
        .heading_font(3, H3)
        .code_block_font(CODE);
    Markdown::new(src).style(style).reveal(fade).build(&kit)
}

// ---------------------------------------------------------------------------------------------
// Composer and status bar

fn ghost(p: &Pal, key: &str, label: &str, chevron: bool) -> El {
    let mut r = row()
        .key(key)
        .role("button")
        .h(28.0)
        .px(8.0)
        .gap(2.0)
        .center_y()
        .radius(6.0)
        .hover(p.hover)
        .press(p.pressed)
        .child(text(label.to_owned()).font(UI).color(p.muted));
    if chevron {
        r = r.child(icon(&ic::CHEVRON_DOWN, 16.0).color(p.icon_faint));
    }
    r
}

fn composer_dock(s: &App, p: &Pal) -> El {
    let selected = s.selection();
    let model = selected.model;
    let effort = if model.variant.is_empty() { "Default".into() } else { model::capitalize(&model.variant) };
    let ready = !s.composer.text.trim().is_empty();
    let submit = el()
        .key("submit")
        .role("button")
        .label("Send")
        .size(28.0, 28.0)
        .radius(6.0)
        .center()
        .fill(if ready { p.contrast } else { p.layer2 })
        .hover(p.hover)
        .on_press_tx(|s: &mut App, tx: &mut Tx| s.send(tx))
        .child(icon(&ic::ARROW_UP, 16.0).color(if ready { p.text } else { p.icon_faint }));
    let composer = col()
        .key("composer")
        .w_full()
        .max_w(CONTENT_W)
        .min_h(96.0)
        .radius(12.0)
        .fill(if p.dark { p.layer1 } else { p.bg_base })
        .border(0.5, p.ring)
        .shadow(1.0, 2.0, p.shadow)
        .child(
            col().key("editor").w_full().px(16.0).pt(16.0).pb(8.0).child(
                text_input(&s.composer, PROMPT)
                    .key("prompt")
                    .w_full()
                    .min_h(40.0)
                    .placeholder("Ask anything… \"Fix a TODO in the codebase\"", p.faint)
                    .caret(p.text, p.selection)
                    .on_edit(|s: &mut App| &mut s.composer)
                    .on_input_key(|s: &mut App, ev, _| s.picker_key(ev))
                    .on_submit(|s: &mut App, tx: &mut Tx| s.send(tx)),
            ),
        )
        .child(
            row()
                .key("toolbar")
                .h(44.0)
                .px(8.0)
                .gap(4.0)
                .center_y()
                .child(picker_trigger(s, p, Kind::Agent, &model::capitalize(&selected.agent)))
                .child(picker_trigger(s, p, Kind::Model, &s.pickers.models.iter()
                    .find(|m| m.id == model.id && m.provider == model.provider).map_or_else(|| model.display(), |m| m.name.clone())))
                .child(picker_trigger(s, p, Kind::Effort, &effort))
                .child(spacer())
                .child(submit),
        );
    col().key("dock").w_full().center().px(24.0).pb(10.0).child(composer)
}

fn picker_trigger(s: &App, p: &Pal, kind: Kind, label: &str) -> El {
    let mut button = ghost(p, kind.key(), label, true).label(format!("Choose {}", kind.key()));
    if !s.composer.focused && s.pickers.toolbar_focus == Some(kind) {
        button = button.fill(p.hover);
    }
    if !s.live() {
        button = button.on_press(move |s: &mut App| s.open_picker(kind));
    }
    button
}

fn picker(s: &App, p: &Pal) -> El {
    let kind = s.pickers.open.unwrap();
    let active = s.picker_active();
    let selected = s.picker_current(kind);
    let items = s.picker_items();
    let transition = rune::Transition::scale(0.96).springs(rune::SpringPair {
        enter: rune::Spring::new(0.25, 0.0), exit: rune::Spring::new(0.20, 0.0),
    });
    let mut menu = col().key(format!("picker-{}", kind.key())).role("menu").label(format!("Choose {}", kind.key()))
        .above(kind.key(), 6.0).w(if kind == Kind::Model { 284.0 } else { 180.0 })
        .padding(2.0).radius(6.0).fill(p.layer1).border(0.5, p.border_muted)
        .shadow(4.0, 12.0, p.shadow).motion(rune::Spring::new(0.25, 0.0)).transition(transition)
        // Empty space in the menu consumes pointer presses instead of dismissing it.
        .on_press(|_: &mut App| {});
    if kind == Kind::Model {
        menu = menu.child(row().key("picker-search-row").h(28.0).pl(10.0).pr(4.0).gap(8.0).center_y()
            .child(icon(&ic::MAGNIFYING_GLASS, 16.0).color(p.icon_muted))
            .child(text_input(&s.pickers.search, UI).key("picker-search").grow(1.0).shrink().h(28.0).animate_edit_layout()
                .placeholder("Search models", p.faint).caret(p.text, p.selection)
                .on_edit(|s: &mut App| &mut s.pickers.search)
                .on_input_key(|s: &mut App, ev, _| s.picker_key(ev))))
            .child(el().h(0.5).w_full().fill(p.border_muted));
    }
    let mut list = col().key("picker-options").w_full();
    // Retain identities while filtering. Row slots and their neighbours share one layout
    // spring; each slot clips its own sharp text while its opacity leaves more quickly.
    let all_items = if kind == Kind::Model {
        s.pickers.models.iter().map(|m| crate::pickers::Item { key: m.key.clone(), label: m.name.clone(), group: m.group.clone() }).collect()
    } else { items.clone() };
    let mut group = "";
    let mut height = 0.0_f32;
    for item in &all_items {
        let shown = items.iter().any(|i| i.key == item.key);
        if !item.group.is_empty() && item.group != group {
            let shown = items.iter().any(|i| i.group == item.group);
            list = list.child(col().key(format!("group-{}", item.group)).h(if shown { 24.0 } else { 0.0 })
                .opacity(if shown { 1.0 } else { 0.0 }).opacity_motion(rune::Curve::ease_in_out(0.15))
                .clip_fade(0.0, 6.0, 0.0, 6.0)
                .child(text(item.group.clone()).font(UI_SMALL).color(p.faint).h(24.0).px(10.0).pt(4.0)));
            if shown { height += 24.0; }
            group = &item.group;
        }
        let key = item.key.clone();
        let hover_key = key.clone();
        let mut option = row().key(format!("option-{key}")).role("menuitemradio").label(item.label.clone())
            .h(if shown { 28.0 } else { 0.0 }).w_full().px(10.0).gap(8.0).center_y().radius(4.0)
            .opacity(if shown { 1.0 } else { 0.0 }).opacity_motion(rune::Curve::ease_in_out(0.15))
            .clip_fade(0.0, 6.0, 0.0, 6.0)
            .fill(if active.as_deref() == Some(&key) { p.hover } else { Color::TRANSPARENT })
            .hover(p.hover).press(p.pressed)
            .child(text(item.label.clone()).font(UI).color(p.text)).child(spacer());
        if shown {
            option = option.on_hover(move |s: &mut App| s.pickers.activate(hover_key.clone()))
                .on_press(move |s: &mut App| s.choose_picker(&key));
        }
        if item.key == selected { option = option.child(icon(&ic::CHECK_SMALL, 16.0).color(p.icon)); }
        list = list.child(option);
        if shown { height += 28.0; }
    }
    if items.is_empty() {
        list = list.child(text("No models found").font(UI).color(p.faint).h(48.0).px(10.0).pt(16.0));
        height = 48.0;
    }
    menu.child(col().key("picker-scroll").w_full().h(height.min(220.0))
        .scroll(ScrollSpec { stick_bottom: false }).clip_fade(0.0, 8.0, 0.0, 8.0).child(list))
}

fn status_bar(s: &App, p: &Pal, info: Option<&SessionInfo>) -> El {
    let dir = info.map_or(String::new(), |i| i.directory.replace("/Users/demo", "~").replace(&home_dir(), "~"));
    let e = edge::ALL[s.edge];
    let live = s.live();
    row()
        .key("status")
        .h(28.0)
        .px(16.0)
        .gap(12.0)
        .center_y()
        .child(icon(&ic::FOLDER, 14.0).color(p.icon_faint))
        .child(text(dir).font(UI_SMALL).color(p.faint).truncate())
        .child(spacer())
        .child(
            row()
                .key("edge")
                .role("button")
                .label("Streaming edge")
                .h(22.0)
                .px(6.0)
                .center_y()
                .radius(4.0)
                .hover(p.hover)
                .on_press(|s: &mut App| s.edge = (s.edge + 1) % edge::ALL.len())
                .child(text(format!("Edge: {}", e.name)).font(UI_SMALL).color(p.faint)),
        )
        .child(el().size(6.0, 6.0).radius(3.0).fill(if live { p.success } else { p.icon_faint }))
        .child(text(s.status.clone()).font(UI_SMALL).color(p.faint))
}

fn home_dir() -> String {
    static H: OnceLock<String> = OnceLock::new();
    H.get_or_init(|| std::env::var("HOME").unwrap_or_else(|_| "\u{0}".into())).clone()
}
