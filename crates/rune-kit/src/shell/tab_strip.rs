//! A strip of tabs across a titlebar or down a sidebar: keyed tabs that open and close on
//! springs, shrink then scroll when they overflow, reorder by drag, tear out, and close with a
//! middle click.

use std::borrow::Cow;
use std::rc::Rc;

use rune::{Color, Drag, El, Icon, Phase, Spring, Tx};

use crate::{IconButton, Kit};

mod auto_scroll;
mod drag;
mod layout;
mod tab;
#[cfg(test)]
mod tests;

pub use drag::{TabDragMode, TabDragPhase, TabDragState, TearOutSpec};

use drag::Release;
use layout::{TabBind, TabBindCtx, TabItemCore, TabStripCore};

/// Which way a [`TabStrip`] runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TabOrientation {
    /// Across a titlebar, browser style.
    #[default]
    Horizontal,
    /// Down a sidebar, one tab per row.
    Vertical,
}

/// How much a horizontal tab shows, from its width ([`TabOverflow::density_for`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TabDensity {
    /// Over 64 points: avatar, title fading at its end, and the status or close slot.
    Full,
    /// 44 to 64 points: the title hides while the tab is active, hovered or shows a status, so
    /// the slot never covers the avatar.
    Compact,
    /// 44 points or less: the avatar alone, centred, with a status dot on its corner; the close
    /// button centres over the tab under the pointer, and a busy tab spins in the avatar's place.
    IconOnly,
}

/// What a strip does when its tabs don't fit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TabOverflow {
    /// Horizontal tabs share the strip equally, from `max_width` down to `min_width`, then the
    /// strip scrolls sideways with a `fade_width` linear fade at each edge while anything is
    /// past it (Desktop's `titlebar-tabs-fade-*`). Pinned tabs keep `pinned_width`. When
    /// `min_width` exceeds `max_width` (or `max_width` is NaN), `min_width` wins; a NaN
    /// `min_width` leaves the lower bound open while still capping at `max_width`.
    ShrinkThenScroll { min_width: f32, max_width: f32, pinned_width: f32, fade_width: f32 },
    /// Full-width rows that scroll, the top and bottom feathering by as much as is left to scroll
    /// past them, up to `fade_width` (Desktop's `--vertical-tabs-fade-*`).
    Scroll { fade_width: f32 },
}

impl TabOverflow {
    /// OpenCode's titlebar tabs: 28 to 224 points, pinned 36, 24 point fades.
    pub const HORIZONTAL_DEFAULT: Self = Self::ShrinkThenScroll { min_width: 28.0, max_width: 224.0, pinned_width: 36.0, fade_width: 24.0 };
    /// OpenCode's side tabs: 16 point fades.
    pub const VERTICAL_DEFAULT: Self = Self::Scroll { fade_width: 16.0 };

    pub fn density_for(width: f32) -> TabDensity {
        if width <= 44.0 {
            TabDensity::IconOnly
        } else if width <= 64.0 {
            TabDensity::Compact
        } else {
            TabDensity::Full
        }
    }

    /// The width of each unpinned tab when `unpinned` of them and `pinned` pinned ones share
    /// `available` points with `gap` between each: equal shares between the policy's bounds.
    /// Past the minimum the tabs overflow and the strip scrolls. When `min_width` exceeds
    /// `max_width` (or `max_width` is NaN), `min_width` wins; a NaN `min_width` leaves the lower
    /// bound open. `f32::INFINITY` for `Scroll`.
    pub fn tab_width(&self, available: f32, unpinned: usize, pinned: usize, gap: f32) -> f32 {
        let Self::ShrinkThenScroll { min_width, max_width, pinned_width, .. } = *self else { return f32::INFINITY };
        let hi = max_width.max(min_width);
        if unpinned == 0 {
            return hi;
        }
        let gaps = (unpinned + pinned).saturating_sub(1) as f32 * gap;
        ((available - pinned as f32 * pinned_width - gaps) / unpinned as f32).max(min_width).min(hi)
    }

    pub(super) fn fade(&self) -> f32 {
        match *self {
            Self::ShrinkThenScroll { fade_width, .. } | Self::Scroll { fade_width } => fade_width,
        }
    }
}

/// A tab's status mark, in the slot its close button takes under the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TabItemStatus {
    /// Working: the busy indicator ([`TabStrip::busy_indicator`]), shown on the active tab too.
    Busy,
    /// Waiting on the user: a warning dot.
    Attention,
    /// Something new: an accent dot.
    Unread,
    /// Something new that failed: a danger dot.
    Failed,
}

/// A drop that moved tab `id` from index `from` to `to`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabReorder {
    pub id: String,
    pub from: usize,
    pub to: usize,
}

/// A tab dragged off the strip and released: the app opens it in a window of its own.
#[derive(Clone, Debug, PartialEq)]
pub struct TabTearOut {
    pub id: String,
    pub index: usize,
    /// Pointer travel since the press, in points.
    pub offset: [f32; 2],
    /// Where it was released, in window points.
    pub window_point: Option<[f32; 2]>,
    /// The same point on the desktop in physical pixels (`Drag::screen_point`), for placing the
    /// new window under the pointer. `None` headless.
    pub screen_point: Option<(i32, i32)>,
}

/// The pointer moving a torn-out tab, or bringing it back to the strip (`torn` false), while
/// the button is still down: the app can open the tab's window now and move it with the
/// pointer, so the drag carries on in the new window ([`TabDragState::hand_off`]).
#[derive(Clone, Debug, PartialEq)]
pub struct TabTearMove {
    pub id: String,
    pub index: usize,
    /// Off the strip; false once the tab came back within the reattach distance.
    pub torn: bool,
    /// Where the press landed in the tab, in points from its top-left: where the pointer sits
    /// in the tab the new window shows.
    pub grab: [f32; 2],
    /// The pointer in window points.
    pub window_point: [f32; 2],
    /// The pointer on the desktop in physical pixels. `None` headless.
    pub screen_point: Option<(i32, i32)>,
}

/// What a strip knows of a tab it may not build ([`TabStrip::lazy_tabs`]): its id, and what
/// sets its length along the strip and keeps it built.
#[derive(Clone, Debug)]
pub struct TabSlot<'a> {
    pub id: Cow<'a, str>,
    pub pinned: bool,
    /// It has a subtitle ([`TabItem::subtitle`]): a vertical row of 45 points.
    pub two_line: bool,
    /// Built wherever the strip has scrolled: its menu is open, or it's being renamed.
    pub keep: bool,
}

/// A tab strip's words for assistive technology ([`TabStrip::words`]): the close button's name
/// and each status a tab's mark shows, said after the tab's title.
#[derive(Clone, Copy, Debug)]
pub struct TabWords {
    pub close: &'static str,
    pub busy: &'static str,
    pub attention: &'static str,
    pub unread: &'static str,
    pub failed: &'static str,
}

impl Default for TabWords {
    fn default() -> Self {
        TabWords { close: "Close tab", busy: "Working", attention: "Needs attention", unread: "Unread", failed: "Failed" }
    }
}

/// Past this many tabs a strip builds only those near what it shows ([`TabStrip::lazy_tabs`]).
pub const BAND_PAST: usize = 40;

type Each<S> = Rc<dyn Fn(&mut S, &str, &mut Tx)>;
type Hover<S> = Rc<dyn Fn(&mut S, &str, bool, &mut Tx)>;
type Commit<S, T> = Rc<dyn Fn(&mut S, T, &mut Tx)>;
type Access<S> = fn(&mut S) -> &mut TabDragState;

/// One tab: `TabItem::new(id, title).avatar(..).status(..)`.
pub struct TabItem<'a, S: 'static> {
    core: TabItemCore<'a>,
    _s: std::marker::PhantomData<fn(&mut S)>,
}

impl<'a, S: 'static> TabItem<'a, S> {
    pub fn new(id: impl Into<Cow<'a, str>>, title: impl Into<Cow<'a, str>>) -> Self {
        TabItem { core: TabItemCore::new(id.into(), title.into()), _s: std::marker::PhantomData }
    }
    /// A second, muted line under the title in a vertical strip (a project's name); the row
    /// grows to 45 points.
    pub fn subtitle(mut self, subtitle: impl Into<Cow<'a, str>>) -> Self { self.core.subtitle = Some(subtitle.into()); self }
    /// A 16 point leading element: a project's avatar.
    pub fn avatar(mut self, avatar: El) -> Self { self.core.avatar = Some(avatar); self }
    /// A 16 point leading icon in place of an avatar (a draft's pencil), muted until active.
    pub fn icon(mut self, icon: &'static Icon) -> Self { self.core.icon = Some(icon); self }
    /// Its status mark. The strip shows what it's given; OpenCode leaves the dots off the
    /// active tab and keeps the busy ring.
    pub fn status(mut self, status: Option<TabItemStatus>) -> Self { self.core.status = status; self }
    /// Pinned tabs come first in the strip, keep [`TabOverflow`]'s `pinned_width` as icons,
    /// have no close button and reorder only among themselves.
    pub fn pinned(mut self, pinned: bool) -> Self { self.core.pinned = pinned; self }
    /// Whether the close button and a middle click close it (default true).
    pub fn closable(mut self, closable: bool) -> Self { self.core.closable = closable; self }
    /// Highlights the tab while its context menu is open.
    pub fn marked(mut self, marked: bool) -> Self { self.core.marked = marked; self }
    /// Raise it to ring a background tab once (900 ms), as a prompt arriving does.
    pub fn pulse(mut self, seq: u32) -> Self { self.core.pulse_seq = seq; self }
    /// A field in place of the title while renaming in place. The tab doesn't drag meanwhile.
    pub fn editor(mut self, field: El) -> Self { self.core.editor = Some(field); self }
    /// Wraps the built tab: a context menu handler, a hover card's anchor.
    pub fn decorate(mut self, f: impl FnOnce(El, TabDensity) -> El + 'a) -> Self { self.core.wrap = Some(Box::new(f)); self }
}

struct TabHandlers<S> {
    access: Option<Access<S>>,
    select: Option<Each<S>>,
    close: Option<Each<S>>,
    double: Option<Each<S>>,
    hover: Option<Hover<S>>,
    reorder: Option<Commit<S, TabReorder>>,
    tear_out: Option<Commit<S, TabTearOut>>,
    tear_move: Option<Commit<S, TabTearMove>>,
}

impl<S: 'static> TabBind for TabHandlers<S> {
    fn has_close(&self) -> bool { self.close.is_some() }
    fn has_access(&self) -> bool { self.access.is_some() }

    fn bind_root_hover(&self, root: El) -> El {
        let Some(access) = self.access else { return root };
        root.on_hover_change(move |s: &mut S, inside, tx: &mut Tx| {
            // Only letting go of the width lock a close left changes what the strip draws.
            if inside || !access(s).leave() {
                tx.live_only();
            }
        })
    }

    fn bind_torn_anchor(&self, place: El) -> El {
        let Some(access) = self.access else { return place };
        place.on_layout_tx(move |s: &mut S, r, tx: &mut Tx| {
            access(s).report_rect(r);
            tx.live_only();
        })
    }

    fn bind_close_button(&self, button: IconButton, id: String, lock: f32) -> IconButton {
        let (access, on_close) = (self.access, self.close.clone());
        button.on_press_tx(move |s: &mut S, tx: &mut Tx| {
            if let Some(a) = access.filter(|_| lock > 0.0) {
                a(s).width_lock = Some(lock);
            }
            if let Some(f) = &on_close {
                f(s, &id, tx);
            }
        })
    }

    fn bind_tab(&self, mut slot: El, ctx: TabBindCtx<'_>) -> El {
        let (access, lane, vertical, lock) = (self.access, ctx.lane, ctx.vertical, ctx.lock);
        let (select_early, active) = (ctx.select_early, ctx.active);
        let reveal = (ctx.scroll_key.to_owned(), format!("t-{}", ctx.id));
        if let Some(close) = self.close.clone().filter(|_| ctx.closable) {
            let id = ctx.id.to_owned();
            slot = slot.on_middle_press(move |s: &mut S, tx: &mut Tx| {
                if let Some(a) = access.filter(|_| lock > 0.0) {
                    a(s).width_lock = Some(lock);
                }
                close(s, &id, tx);
            });
        }
        {
            let (id, select, reveal) = (ctx.id.to_owned(), self.select.clone(), reveal.clone());
            slot = slot.on_press_tx(move |s: &mut S, tx: &mut Tx| {
                if let Some(a) = access && a(s).take_swallowed_click() {
                    tx.live_only();
                    return;
                }
                if let Some(f) = &select {
                    f(s, &id, tx);
                    tx.scroll_into_view_in(reveal.0.clone(), reveal.1.clone());
                }
            });
        }
        if let Some(f) = self.double.clone() {
            let id = ctx.id.to_owned();
            slot = slot.on_double_press_tx(move |s: &mut S, tx: &mut Tx| f(s, &id, tx));
        }
        if access.is_some() || self.hover.is_some() {
            let (id, hover) = (ctx.id.to_owned(), self.hover.clone());
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

        let (id, select, reorder, tear, tear_move) = (ctx.id.to_owned(), self.select.clone(), self.reorder.clone(), self.tear_out.clone(), self.tear_move.clone());
        let target = tab::DragTarget { lane, vertical, select_early, active, scroll: ctx.scroll_key.to_owned(), auto: ctx.auto.clone() };
        slot.on_drag(move |s: &mut S, d: &mut Drag, tx: &mut Tx| match d.phase {
            Phase::Down => {
                if tab::drag_down(access(s), &target, d, select.is_some(), tx)
                    && let Some(f) = &select
                {
                    f(s, &id, tx);
                    tx.scroll_into_view_in(reveal.0.clone(), reveal.1.clone());
                }
            }
            Phase::Move => {
                if let Some(mv) = tab::drag_move(access(s), &target, d, &id, reorder.is_some(), tear_move.is_some(), tx)
                    && let Some(f) = &tear_move
                {
                    f(s, mv, tx);
                }
            }
            Phase::Up => match tab::drag_up(access(s), d, target.select_early, tx) {
                Release::Click | Release::None => {}
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
            },
        })
    }
}

/// A keyed strip of tabs: `TabStrip::new("tabs", TabOrientation::Horizontal).tabs(..)
/// .active(..).drag_state(&s.drag, |s| &mut s.drag).on_select(..).build(&kit)`.
///
/// Horizontal tabs are 28 points tall with 6 between them, share the strip from 224 points
/// down to 28 and then scroll; a 1.5 point separator stands between neighbours that aren't
/// active or hovered. In a `.window_drag()` titlebar the gaps between tabs, the bands above and
/// below them and the space after the trailing action move the window, as only the tabs and
/// buttons take presses. Vertical tabs fill the row with 3 points between them, each reaching
/// across half the gap so no press misses. Each tab is keyed `t-{id}`; a vertical tab's row as
/// drawn, without that reach, is keyed `t-{id}-pill`, for overlays anchored beside it.
///
/// A press selects a tab as the button goes down, so a drag carries the selected tab and the
/// release draws nothing ([`Self::select_on_drag_start`]); a press on the selected tab and a
/// press on a close button select nothing. Dragging 4 points along
/// the strip reorders, Escape puts the tab back, and with [`Self::tear_out`] dragging it off
/// the strip tears it out. A middle click closes a tab.
pub struct TabStrip<'a, S: 'static> {
    core: TabStripCore<'a>,
    handlers: TabHandlers<S>,
}

impl<'a, S: 'static> TabStrip<'a, S> {
    pub fn new(key: impl Into<String>, orientation: TabOrientation) -> Self {
        TabStrip {
            core: TabStripCore::new(key.into(), orientation),
            handlers: TabHandlers { access: None, select: None, close: None, double: None, hover: None, reorder: None, tear_out: None, tear_move: None },
        }
    }
    pub fn tabs(mut self, tabs: impl IntoIterator<Item = TabItem<'a, S>>) -> Self {
        self.core.items.extend(tabs.into_iter().map(|t| t.core));
        self
    }
    /// The tabs, of which the strip builds only those it shows (`build(index)`), when there are
    /// more than [`BAND_PAST`]: the rest stand as empty space their length, so a strip of
    /// hundreds of tabs costs a frame what a screenful does. In place of [`Self::tabs`].
    pub fn lazy_tabs(mut self, slots: Vec<TabSlot<'a>>, mut build: impl FnMut(usize) -> TabItem<'a, S> + 'a) -> Self {
        self.core.lazy = Some((slots, Box::new(move |i| build(i).core)));
        self
    }
    /// The selected tab, by id.
    pub fn active(mut self, active_id: Option<&'a str>) -> Self { self.core.active = active_id; self }
    /// The points a horizontal strip has, trailing action included: `cx.layout_size(key)`'s
    /// width (the strip's root grows into the space it's given). It sets the tabs' shared
    /// width and density; without it they take their widest.
    pub fn viewport_width(mut self, width: Option<f32>) -> Self { self.core.viewport = width; self }
    /// The overflow policy (default [`TabOverflow::HORIZONTAL_DEFAULT`] or
    /// [`TabOverflow::VERTICAL_DEFAULT`]).
    pub fn overflow(mut self, policy: TabOverflow) -> Self { self.core.overflow = Some(policy); self }
    /// Points between tabs (default 6 horizontal, 3 vertical).
    pub fn gap(mut self, points: f32) -> Self { self.core.gap = Some(points.max(0.0)); self }
    /// A tab's height (default 28).
    pub fn row_height(mut self, points: f32) -> Self { self.core.row_height = points.max(1.0); self }
    /// Whether a press selects the tab as the button goes down, so a drag selects it too (default
    /// true, as tab-nav.tsx's onMouseDown does in either orientation), or the release does and
    /// a drag leaves the selection (for touch, where a long press opens a menu).
    pub fn select_on_drag_start(mut self, on: bool) -> Self { self.core.select_on_drag_start = Some(on); self }
    /// Lets a drag across the strip tear a tab out ([`Self::on_tear_out`]). Off by default, and
    /// never for the only tab unless [`TearOutSpec::lone`].
    pub fn tear_out(mut self, spec: Option<TearOutSpec>) -> Self { self.core.tear = spec; self }
    /// Gives each tab `El::matched("{prefix}-{id}")`, so a tab flies between strips when the
    /// layout swaps one for another.
    pub fn matched_prefix(mut self, prefix: Option<&'a str>) -> Self { self.core.matched = prefix; self }
    /// The spring tabs step aside on during a drag (default `SNAP`, 0.25 s). Drops, tabs opening and
    /// closing, and widths changing reflow on `REFLOW`.
    pub fn reorder_spring(mut self, spring: Spring) -> Self { self.core.spring = spring; self }
    /// An element right after the last horizontal tab (the new-tab button), outside the
    /// scrolling, with the space past it left bare.
    pub fn trailing_action(mut self, action: El) -> Self { self.core.trailing = Some(action); self }
    /// The trailing action's width, which the tabs leave it (default 28, a large icon button).
    pub fn trailing_width(mut self, points: f32) -> Self { self.core.trailing_width = points.max(0.0); self }
    /// The strip's gestures, kept in the app's state; `access` finds them there. Without it the
    /// tabs select and close but don't drag, and closing doesn't lock their width.
    pub fn drag_state(mut self, state: &TabDragState, access: fn(&mut S) -> &mut TabDragState) -> Self {
        self.core.drag = Some(state.clone());
        self.handlers.access = Some(access);
        self
    }
    /// The colour under the strip (default `bg_deep`), which a dragged tab and the close slot
    /// paint opaque.
    pub fn fill(mut self, color: Color) -> Self { self.core.fill = Some(color); self }
    /// The close button's glyph (default the kit's close glyph).
    pub fn close_icon(mut self, icon: &'static Icon) -> Self { self.core.close_icon = Some(icon); self }
    /// Each tab's close button is a Tab stop after its tab with the kit's focus ring, as a web
    /// page's `<button>` is. Off, it's pointer only: the strip's arrows move between tabs.
    /// A primary pointer that can't hover (`@media (hover: none)`, `tab-nav.css`): every tab shows
    /// its close button on its painted slot, its status mark 16 points before it, and its title
    /// fades clear of both, as a hover shows them.
    pub fn hover_none(mut self, on: bool) -> Self { self.core.hover_none = on; self }
    pub fn close_tab_stop(mut self, on: bool) -> Self { self.core.close_tab_stop = on; self }
    /// What a busy tab shows (default the kit's spinner at 14 points).
    pub fn busy_indicator(mut self, f: fn(&Kit) -> El) -> Self { self.core.busy = f; self }
    /// The tab list's accessible name (default "Tabs").
    /// What a screen reader hears of each tab beyond its title, in the app's language: its
    /// close button's name and its status. English by default.
    pub fn words(mut self, words: TabWords) -> Self { self.core.words = words; self }
    pub fn label(mut self, label: impl Into<String>) -> Self { self.core.label = label.into(); self }
    pub fn on_select(mut self, f: impl Fn(&mut S, &str, &mut Tx) + 'static) -> Self { self.handlers.select = Some(Rc::new(f)); self }
    /// The close button, a middle click.
    pub fn on_close(mut self, f: impl Fn(&mut S, &str, &mut Tx) + 'static) -> Self { self.handlers.close = Some(Rc::new(f)); self }
    pub fn on_double_click(mut self, f: impl Fn(&mut S, &str, &mut Tx) + 'static) -> Self { self.handlers.double = Some(Rc::new(f)); self }
    /// The pointer entering (`true`) and leaving a tab. The strip draws its hover by hover
    /// groups, so a handler that changes nothing the view draws says `tx.live_only()`, and the
    /// hover runs no view; without a handler it never does.
    pub fn on_hover(mut self, f: impl Fn(&mut S, &str, bool, &mut Tx) + 'static) -> Self { self.handlers.hover = Some(Rc::new(f)); self }
    pub fn on_reorder(mut self, f: impl Fn(&mut S, TabReorder, &mut Tx) + 'static) -> Self { self.handlers.reorder = Some(Rc::new(f)); self }
    pub fn on_tear_out(mut self, f: impl Fn(&mut S, TabTearOut, &mut Tx) + 'static) -> Self { self.handlers.tear_out = Some(Rc::new(f)); self }
    /// Each move of a torn-out tab, and its return to the strip, while the button is down.
    pub fn on_tear_move(mut self, f: impl Fn(&mut S, TabTearMove, &mut Tx) + 'static) -> Self { self.handlers.tear_move = Some(Rc::new(f)); self }

    /// The strip. A torn-out tab stays inside the strip's clip; [`Self::build_layered`] lifts it out.
    pub fn build(self, k: &Kit) -> El {
        self.core.assemble(k, None, &self.handlers)
    }

    /// The strip, with a torn-out tab floating over the window in `layer`, which the view mounts
    /// at its root with [`crate::root`] as it does popovers and menus.
    pub fn build_layered(self, k: &Kit, layer: &mut Vec<El>) -> El {
        self.core.assemble(k, Some(layer), &self.handlers)
    }
}
