//! One tab of a [`TabStrip`](super::TabStrip): its pill, avatar, title, status mark and close
//! button, with the press, drag, hover and middle-click handlers on its slot.

use rune::{Anim, Beat, Color, Drag, Easing, El, MotionIntent, Phase, Sequence, SpringPair, Transition, Tx, col, el, icon, row, text};

use super::drag::{Lane, Release};
use super::{Strip, TabDensity, TabItem, TabItemStatus, TabReorder, TabTearMove, TabTearOut};
use crate::{ControlSize, IconButton, IconButtonVariant, Kit};

/// The pill's corner radius (`rounded-[6px]`).
const RADIUS: f32 = 6.0;
/// The status and close slot (`tab-close`, 20 points, 4 in from the tab's end).
const SLOT: f32 = 20.0;
const SLOT_END: f32 = 4.0;
/// Status dots: 7 points in the slot, 6 on an icon-only tab's avatar corner.
const DOT: f32 = 7.0;
const BADGE: f32 = 6.0;
/// The title fades out over its last 16 points (`tab-nav.css`'s mask).
const FADE: f32 = 16.0;

/// A prompt arriving in a background tab rings it once: in over 180 ms, out by 900 ms.
static PULSE: [Beat; 3] = [
    Beat::new(0.0).opacity(0.0),
    Beat::new(0.18).opacity(1.0).easing(Easing::EASE_OUT),
    Beat::new(0.9).opacity(0.0).easing(Easing::EASE_OUT),
];

/// Where tab `index` sits and how it reads, worked out by the strip.
pub(super) struct Place {
    pub index: usize,
    pub lane: Lane,
    /// Its width across a horizontal strip; `None` fills a vertical one.
    pub width: Option<f32>,
    pub density: TabDensity,
    pub active: bool,
    /// A separator stands before it: neither it nor the tab before it is active. Either one's
    /// hover hides it, by Rune's hover groups, so a hover commits nothing.
    pub separator: bool,
}

/// The hover group a tab shares with the separator after it (`Place::separator` of the next).
fn hover_key(list_key: &str, id: &str) -> String {
    format!("{list_key}-hover-{id}")
}

/// The tab, and while it's torn out of a layered strip, its copy floating over the window: a tab
/// inside the strip stays inside the strip's clip. `prev`: the id of the tab before it.
///
/// What its hover changes (the wash, the title's fade, the close button, the slot's ground and
/// the separators either side) is drawn by Rune's hover groups, not from state, so a hover runs
/// no view.
pub(super) fn build<S: 'static>(item: TabItem<'_, S>, at: Place, prev: Option<&str>, st: &Strip<S>, k: &Kit) -> (El, Option<El>) {
    let c = &k.c;
    let id = item.id.into_owned();
    let vertical = at.width.is_none();
    let lifted = st.drag.dragging() == Some(at.index);
    let torn = lifted && st.drag.torn_out();
    let editing = item.editor.is_some();
    let status = item.status;
    // Emphasized whatever the pointer does; a hover emphasizes the rest (`title`, `status_slot`).
    let emphasized = at.active || status.is_some() || editing;
    let icon_only = at.density == TabDensity::IconOnly;
    let dotted = matches!(status, Some(TabItemStatus::Attention | TabItemStatus::Unread | TabItemStatus::Failed));
    let busy = status == Some(TabItemStatus::Busy);
    let closable = item.closable && !item.pinned && st.handlers.close.is_some() && !editing;
    let two_line = vertical && item.subtitle.is_some();

    // The tab paints the window's deep background under an overlay: pressed while it's the
    // active or dragged one, the hover wash under the pointer.
    let overlay = if at.active || lifted || editing {
        c.pressed
    } else if item.marked {
        c.hover
    } else {
        Color::TRANSPARENT
    };
    // The tab's own colour, opaque: the strip's ground under the overlay, layered as the GPU paints the pill.
    let composite = [st.base, overlay];
    let h = if two_line { st.two_line_height } else { st.row_height };

    let mut pill = row().key("pill").w_full().h(h).radius(RADIUS).center_y();
    pill = if lifted { grounded(pill, composite, RADIUS) } else { pill.fill(if at.active || editing || item.marked { overlay } else { Color::TRANSPARENT }) };
    // The hover wash, instant as the web's `:hover` is: the slot's own on a horizontal tab; a
    // vertical slot reaches into the gaps, so its pill carries the wash as a layer.
    let washes = !at.active && !lifted && !editing && !item.marked;
    if vertical {
        // Mounted whatever the state, so a press that lifts the tab doesn't cut it away.
        pill = pill.child(hidden(el().key("hover").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).radius(RADIUS).fill(c.hover).group_opacity(0.0, if washes { 1.0 } else { 0.0 })));
    }
    pill = match (vertical, icon_only) {
        (true, _) => pill.px(st.link_inset).gap(if two_line { 6.0 } else { 8.0 }),
        (false, true) => pill.justify_center(),
        (false, false) => pill.px(6.0).gap(6.0),
    };

    // A busy icon-only tab spins in its avatar's place.
    let lead = match icon_only && busy {
        true => row().key("avatar").size(16.0, 16.0),
        false => lead(item.avatar, item.icon, at.active, icon_only && dotted, status, k),
    };
    let title = (!icon_only).then(|| title(&item.title, item.editor, &at, vertical, emphasized, k));
    pill = match (item.subtitle, title) {
        (Some(sub), Some(t)) if vertical => {
            let sub = text(sub.into_owned()).key("subtitle").font(k.ty.ui_compact).color(c.text_muted).truncate_fade_width(FADE / k.ty.ui_compact.size).a11y_hidden();
            let first = row().key("line1").w_full().gap(6.0).center_y().child(lead).child(t);
            pill.child(col().key("lines").grow(1.0).shrink().min_w(0.0).child(first).child(row().key("line2").w_full().pl(16.0 + 6.0).pr(SLOT_END + SLOT).child(sub)))
        }
        (_, Some(t)) => pill.child(lead).child(t),
        (_, None) => pill.child(lead),
    };
    // What the pointer lays over the slot's ground where the tab has no overlay of its own.
    let hover_overlay = (overlay == Color::TRANSPARENT).then_some(c.hover);
    let look = SlotLook { status, active: at.active, vertical, icon_only, hover_overlay, closable, composite, h, width: at.width };
    if let Some(slot) = status_slot(look, st, &id, k) {
        pill = pill.child(slot);
    }
    if item.pulse_seq > 0 && !at.active {
        pill = pill.child(
            el().key(format!("pulse-{}", item.pulse_seq))
                .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .radius(RADIUS)
                .border(1.0, c.border_strong)
                .sequence(Sequence::new(&PULSE))
                .motion_intent(MotionIntent::PULSE)
                .a11y_hidden(),
        );
    }

    // Vertical rows reach half the gap above and below, so no press between them misses.
    let (before, after) = match vertical {
        true => (if at.index > 0 { st.gap / 2.0 } else { 0.0 }, if at.index + 1 < st.count { st.gap / 2.0 } else { 0.0 }),
        false => (0.0, 0.0),
    };
    let shift = st.drag.shift(at.index, vertical);
    let reordering = st.drag.gap_open() || matches!(st.drag.phase(), super::TabDragPhase::Pressed { .. } | super::TabDragPhase::Dragging(super::TabDragMode::Reorder { .. }));
    let mut slot = el()
        .key(format!("t-{id}"))
        .label(item.title.into_owned())
        .description(status_words(status))
        // `:is(:hover, :has(:focus-visible))`: Tab reaching it shows its close button and wash.
        .hover_group_focus()
        // The separator after it hides with it.
        .hover_group_shared(hover_key(&st.list_key, &id))
        .hover_motion(Anim::INSTANT, Anim::INSTANT)
        .position_in_set(at.index + 1, st.count)
        .focusable(!editing)
        .focus_selected(at.active)
        // Tabs step aside for a drag on the reorder spring; everything else (a drop, a tab opening,
        // closing or moving because another view reordered them) reflows.
        .motion(if reordering { st.spring } else { rune_motion::motion::REFLOW })
        .offset(shift[0], shift[1])
        .transition(Transition::scale(0.95).springs(SpringPair::new(rune_motion::motion::SNAP, rune_motion::motion::SNAP_EXIT)).hold_layout());
    slot = match at.width {
        Some(w) if washes => slot.w(w).h(h).radius(RADIUS).hover(c.hover),
        Some(w) => slot.w(w).h(h),
        None => slot.w_full().h(before + h + after).pt(before).pb(after),
    };
    if at.separator {
        // It comes and goes at once, as `display: none` does, while the pointer crosses tabs:
        // this tab's hover hides the empty element around it, and the tab before it hides the
        // line, through the hover group that element shares with it.
        let line = hidden(el().key("line").absolute(0.0, 0.0).size(1.5, 12.0).radius(0.75).fill(st.separator).group_opacity(1.0, 0.0));
        let mut around = el().key("separator").absolute(-st.gap / 2.0 - 0.75, (h - 12.0) / 2.0).size(0.0, 0.0).a11y_hidden()
            .hover_motion(Anim::INSTANT, Anim::INSTANT).group_opacity(1.0, 0.0);
        if let Some(prev) = prev {
            around = around.hover_group_shared(hover_key(&st.list_key, prev));
        }
        slot = slot.child(hidden(around.child(line)));
    }
    // Lifted off the strip: a touch larger, floating. The one bounce in the strip.
    let lift = |e: El| k.elevate(e.radius(RADIUS).scale(1.03).scale_motion(rune_motion::motion::MORPH).motion_intent(MotionIntent::OVERSHOOT), &k.elevation.floating);
    let ghost = (torn && st.layered && !st.drag.handed_off()).then(|| {
        let w = at.width.or(st.drag.lifted_rect().map(|r| r[2])).unwrap_or(224.0);
        let float = el().key(format!("{}-ghost", st.list_key)).size(w, h).child(pill.clone()).a11y_hidden();
        // Over the tab's own place, then wherever the pointer takes it.
        let float = float.anchored(format!("{}-anchor", st.list_key), rune::Side::Bottom, rune::Align::Start, -h);
        lift(float.offset(shift[0], shift[1]).offset_motion(Anim::INSTANT))
    });
    // While its copy flies, the tab keeps only its place.
    slot = slot.child(if ghost.is_some() { el().key("pill").w_full().h(h).motion(rune::Spring::NONE) } else { pill });
    if let Some(access) = st.access.filter(|_| torn) {
        // The pill's place, under a key no other strip uses: the floating copy's anchor, and,
        // mounted as the tab tears out, where it reports its width for the copy.
        let place = el().key(format!("{}-anchor", st.list_key)).pin(Some(0.0), Some(before), Some(0.0), None).h(h).a11y_hidden();
        slot = slot.child(
            place
                .on_layout_tx(move |s: &mut S, r, tx: &mut Tx| {
                    access(s).report_rect(r);
                    tx.live_only();
                }),
        );
    }
    if let Some(prefix) = &st.matched {
        slot = slot.matched(format!("{prefix}-{id}")).matched_crop();
    }
    if lifted {
        // It follows the pointer exactly, over its neighbours.
        slot = slot.raise().offset_motion(Anim::INSTANT);
    }
    if torn && st.drag.handed_off() {
        // Its own window shows it now, under the pointer: the strip only keeps the gesture.
        slot = slot.opacity(0.0).opacity_motion(rune::Spring::NONE).motion_intent(MotionIntent::CUT);
    } else if ghost.is_some() {
        // The copy flies; the tab itself waits unseen in its place.
        slot = slot.opacity(0.0).opacity_motion(rune::Spring::NONE).offset(0.0, 0.0).motion_intent(MotionIntent::CUT);
    } else if torn {
        slot = lift(slot);
    }
    if !editing {
        slot = handlers(slot, &id, &at, closable, st);
    }
    if let Some(f) = item.wrap {
        slot = f(slot, at.density);
    }
    // After the handlers, which would make it a button.
    (slot.role("tab"), ghost)
}

/// The 16 point avatar, icon or empty mark, with an icon-only tab's status badge on its corner.
fn lead(avatar: Option<El>, glyph: Option<&'static rune::Icon>, active: bool, badge: bool, status: Option<TabItemStatus>, k: &Kit) -> El {
    let c = &k.c;
    let mark = match (avatar, glyph) {
        (Some(a), _) => a,
        (None, Some(i)) => icon(i, 16.0).key("icon").color(if active { c.icon_base } else { c.icon_muted }),
        (None, None) => el().size(16.0, 16.0).radius(3.0).border(1.0, c.border_muted),
    };
    if !badge {
        return row().key("avatar").size(16.0, 16.0).center().child(mark).a11y_hidden();
    }
    // The dot sits on the avatar's corner, which is cut clear 4.5 around it (`tab-nav.css`:
    // `radial-gradient(circle 4.5px at 100% 0, transparent 4px, black 5px)`).
    let mark = row().key("mark").size(16.0, 16.0).center().cutout_circle([16.0, 0.0], 4.0, 5.0).child(mark);
    row().key("avatar").size(16.0, 16.0).center().child(mark).a11y_hidden().child(
        el().key("badge").absolute(16.0 - BADGE / 2.0, -BADGE / 2.0).size(BADGE, BADGE).radius(BADGE / 2.0).fill(dot_color(status, k)),
    )
}

/// The title: clipped, never ellipsised, fading out before the status and close slot.
fn title(title: &str, editor: Option<El>, at: &Place, vertical: bool, emphasized: bool, k: &Kit) -> El {
    let c = &k.c;
    // `text-[13px] font-medium` with no tracking, where the kit's compact UI type has -0.04.
    let font = rune::TextStyle { weight: k.ty.ui_medium.weight, tracking: 0.0, ..k.ty.ui_compact };
    let ink = if at.active || editor.is_some() { c.text_base } else { c.text_faint };
    if let Some(field) = editor {
        return row().key("title").grow(1.0).shrink().min_w(0.0).mr(SLOT_END).child(field);
    }
    // A title runs in its own direction (`tab-nav.tsx`'s `dir="auto"`).
    let label = text(title.to_owned()).key("label").font(font).color(ink).grow(1.0).shrink().a11y_hidden().dir(rune::Direction::Auto);
    // A compact tab hides its title while the status or close slot would cover it.
    let compact = at.density == TabDensity::Compact;
    if compact && emphasized {
        // Hidden in place and in the rest colour, so nothing about it animates unseen.
        let label = text(title.to_owned()).key("label").font(font).color(c.text_faint).grow(1.0).shrink().a11y_hidden().dir(rune::Direction::Auto);
        return row().key("title").grow(1.0).shrink().min_w(0.0).child(hidden(label.truncate_fade_width(0.0).opacity(0.0)).instant_changes());
    }
    let wrap = row().key("title").grow(1.0).shrink().min_w(0.0);
    // The fade ends 24 points before the link's end, clear of the slot, at the end of the title's
    // own words: a Latin title in a right-to-left tab starts at its left and fades at its right,
    // never hiding its first word under the slot (as Desktop's `flex-1` title does).
    let faded = |label: El| label.truncate_fade_width(FADE / font.size);
    if emphasized {
        return wrap.mr(24.0).child(faded(label));
    }
    // At rest it fades only an overflowing title, 4 points from a horizontal tab's end. A compact
    // tab's hover hides it; a wider tab's hover shows it faded clear of the slot instead, in the
    // same place.
    let end = if vertical { 0.0 } else { 4.0 };
    let rest = label.clone().truncate_fade_width(FADE / font.size);
    if compact {
        return wrap.mr(end).child(hidden(rest.group_opacity(1.0, 0.0)));
    }
    let rest = row().key("rest").grow(1.0).shrink().min_w(0.0).mr(end).child(hidden(rest.group_opacity(1.0, 0.0)));
    let hovered = hidden(row().key("hovered").pin(Some(0.0), Some(0.0), Some(24.0), Some(0.0))
        .group_opacity(0.0, 1.0).child(faded(label)));
    wrap.child(rest).child(hovered)
}

/// What the status slot needs to know about its tab.
struct SlotLook {
    status: Option<TabItemStatus>,
    active: bool,
    vertical: bool,
    icon_only: bool,
    /// The overlay the slot's ground takes under the pointer, when the tab has none at rest.
    hover_overlay: Option<Color>,
    closable: bool,
    /// The tab's own colour: the strip's ground and the overlay on it.
    composite: [Color; 2],
    h: f32,
    width: Option<f32>,
}

/// The status mark and close button, which share one slot: the close button shows in the
/// mark's place while the pointer is on the tab, at once.
fn status_slot<S: 'static>(look: SlotLook, st: &Strip<S>, id: &str, k: &Kit) -> Option<El> {
    let SlotLook { status, active, vertical, icon_only, hover_overlay, closable, composite, h, width } = look;
    let busy = status == Some(TabItemStatus::Busy);
    // An icon-only tab carries its dots on the avatar; only a busy ring takes the slot.
    let mark = match status {
        Some(TabItemStatus::Busy) => Some((st.busy)(k)),
        Some(s) if !icon_only => Some(el().size(DOT, DOT).radius(DOT / 2.0).fill(dot_color(Some(s), k))),
        _ => None,
    };
    // A busy icon-only tab that isn't active keeps spinning over its avatar instead of closing.
    let close = closable && !(icon_only && busy && !active);
    if mark.is_none() && !close {
        return None;
    }
    // The slot paints the tab under it so a long title never shows through.
    // A vertical tab's slot is clear unless the pointer or a status is on it.
    // Under the pointer it's always painted, in the hover's overlay where the tab has none.
    let painted = (status.is_some() || (active && !vertical)) && !(icon_only && !busy);
    let top = (h - SLOT) / 2.0;
    let mut slot = row().key("status").size(SLOT, SLOT).center();
    let ground = |key: &'static str, fill: Color| hidden(el().key(key).pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).fill(fill).group_opacity(0.0, 1.0));
    if painted {
        slot = grounded(slot, composite, 0.0);
    } else {
        slot = slot.child(ground("ground-base", composite[0]));
        if hover_overlay.is_none() {
            slot = slot.child(ground("ground-rest", composite[1]));
        }
    }
    if let Some(over) = hover_overlay {
        slot = slot.child(ground("ground-hover", over));
    }
    slot = match (icon_only, width) {
        (true, Some(w)) => slot.absolute((w - SLOT) / 2.0, top),
        _ => slot.pin(None, Some(top), Some(SLOT_END), None),
    };
    // An icon-only tab's slot shows only under the pointer, or while it spins.
    if icon_only && !busy {
        slot = hidden(slot.group_opacity(0.0, 1.0));
    }
    if let Some(m) = mark {
        let keep = icon_only && busy && !active;
        let m = row().key("status-mark").size(SLOT, SLOT).absolute(0.0, 0.0).center().child(m).a11y_hidden();
        slot = slot.child(if close && !keep { hidden(m.group_opacity(1.0, 0.0)) } else { m });
    }
    if close {
        let lock = width.filter(|_| st.horizontal_lock).unwrap_or(0.0);
        let (access, on_close, id) = (st.access, st.handlers.close.clone(), id.to_owned());
        let button = IconButton::new(st.close_icon.unwrap_or(k.glyph.close))
            .control_size(ControlSize::Small)
            .variant(IconButtonVariant::GhostMuted)
            .label("Close tab")
            .on_press_tx(move |s: &mut S, tx: &mut Tx| {
                if let Some(a) = access.filter(|_| lock > 0.0) {
                    a(s).width_lock = Some(lock);
                }
                if let Some(f) = &on_close {
                    f(s, &id, tx);
                }
            })
            .build(k);
        // Pointer only unless the strip makes it a Tab stop: the strip's arrows move between tabs,
        // and keyboard focus on the tab shows it without a ring of its own.
        let button = if st.close_tab_stop { button } else { button.focusable(false).no_focus_ring() };
        // Its glyph enters with it, unseen until the pointer comes, so the wrapper lands it at once.
        slot = slot.child(hidden(row().key("close").absolute(0.0, 0.0).group_opacity(0.0, 1.0).instant_changes().child(button)));
    }
    Some(slot)
}

fn handlers<S: 'static>(mut slot: El, id: &str, at: &Place, closable: bool, st: &Strip<S>) -> El {
    let h = &st.handlers;
    let (access, lane, vertical) = (st.access, at.lane, at.width.is_none());
    let lock = at.width.filter(|_| st.horizontal_lock).unwrap_or(0.0);
    let select_early = st.select_on_drag_start;
    let reveal = (st.scroll_key.clone(), format!("t-{id}"));
    if let Some(close) = h.close.clone().filter(|_| closable) {
        let id = id.to_owned();
        slot = slot.on_middle_press(move |s: &mut S, tx: &mut Tx| {
            if let Some(a) = access.filter(|_| lock > 0.0) {
                a(s).width_lock = Some(lock);
            }
            close(s, &id, tx);
        });
    }
    {
        let (id, select, reveal) = (id.to_owned(), h.select.clone(), reveal.clone());
        slot = slot.on_press_tx(move |s: &mut S, tx: &mut Tx| {
            if let Some(a) = access
                && a(s).take_swallowed_click()
            {
                return;
            }
            if let Some(f) = &select {
                f(s, &id, tx);
                tx.scroll_into_view_in(reveal.0.clone(), reveal.1.clone());
            }
        });
    }
    if let Some(f) = h.double.clone() {
        let id = id.to_owned();
        slot = slot.on_double_press_tx(move |s: &mut S, tx: &mut Tx| f(s, &id, tx));
    }
    if access.is_some() || h.hover.is_some() {
        let (id, hover) = (id.to_owned(), h.hover.clone());
        slot = slot.on_hover_change(move |s: &mut S, inside, tx: &mut Tx| {
            if let Some(a) = access {
                a(s).set_hovered(&id, inside);
            }
            // The strip draws nothing from it (its hover groups do): only the app's handler can
            // change what the view draws, and it says so.
            match &hover {
                Some(f) => f(s, &id, inside, tx),
                None => tx.live_only(),
            }
        });
    }
    let Some(access) = access else { return slot };

    let (id, select, reorder, tear, tear_move) = (id.to_owned(), h.select.clone(), h.reorder.clone(), h.tear_out.clone(), h.tear_move.clone());
    let (scroll, auto) = (st.scroll_key.clone(), st.auto.clone());
    slot.on_drag(move |s: &mut S, d: &mut Drag, tx: &mut Tx| {
        match d.phase {
            Phase::Down => {
                access(s).press(&lane, [d.x, d.y], super::auto_scroll::offset(&auto, vertical));
                if select_early && let Some(f) = &select {
                    f(s, &id, tx);
                    tx.scroll_into_view_in(reveal.0.clone(), reveal.1.clone());
                }
            }
            Phase::Move => {
                // Near an end the strip scrolls under the dragged tab, which keeps to the pointer.
                if access(s).reordering() {
                    super::auto_scroll::step(&auto, &scroll, vertical, d, tx);
                }
                let scrolled = access(s).scrolled_since(super::auto_scroll::offset(&auto, vertical));
                let (dx, dy) = (d.dx, d.dy);
                let (along, across) = if vertical { (dy + scrolled, dx) } else { (dx + scrolled, dy) };
                let was_torn = access(s).torn_out();
                if access(s).follow(&lane, along, across, [dx, dy]) {
                    // The tabs it passes spring aside; it follows the pointer exactly.
                    tx.animated();
                }
                let torn = access(s).torn_out();
                if let Some(f) = tear_move.as_ref().filter(|_| torn || was_torn) {
                    let grab = access(s).grab();
                    f(s, TabTearMove { id: id.clone(), index: lane.index, torn, grab, window_point: d.window_point(), screen_point: d.screen_point() }, tx);
                }
            }
            Phase::Up => {
                tx.animated();
                let on_tab = (0.0..=d.w).contains(&d.x) && (0.0..=d.h).contains(&d.y);
                // A press already selected the tab; its click needn't again.
                match access(s).release(d.cancelled(), on_tab, select_early && on_tab) {
                    Release::Click => {}
                    Release::Reorder { from, to } => {
                        if let Some(f) = &reorder {
                            f(s, TabReorder { id: id.clone(), from, to }, tx);
                        }
                        access(s).dropped();
                    }
                    Release::TearOut { from, offset } => {
                        if let Some(f) = &tear {
                            f(s, TabTearOut { id: id.clone(), index: from, offset, window_point: Some(d.window_point()), screen_point: d.screen_point() }, tx);
                        }
                    }
                    Release::None => {}
                }
            }
        }
    })
}

/// An element the tab's hover shows or hides at once (the web's `transition: none`): no fade in
/// or out with the tab, which would run unseen while it's hidden.
fn hidden(e: El) -> El {
    e.transition(Transition::none()).motion_intent(MotionIntent::CUT)
}

/// Paints `e` opaque in the tab's colour: `ground[0]` with the overlay `ground[1]` as a layer
/// under its children, so it matches a translucent pill over the same ground exactly.
fn grounded(e: El, ground: [Color; 2], radius: f32) -> El {
    e.fill(ground[0]).child(el().key("ground").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).radius(radius).fill(ground[1]))
}

fn dot_color(status: Option<TabItemStatus>, k: &Kit) -> Color {
    match status {
        Some(TabItemStatus::Attention) => k.c.warning.fg,
        Some(TabItemStatus::Failed) => k.c.danger.fg,
        _ => k.c.bg_accent,
    }
}

fn status_words(status: Option<TabItemStatus>) -> &'static str {
    match status {
        Some(TabItemStatus::Busy) => "Working",
        Some(TabItemStatus::Attention) => "Needs attention",
        Some(TabItemStatus::Unread) => "Unread",
        Some(TabItemStatus::Failed) => "Failed",
        None => "",
    }
}
